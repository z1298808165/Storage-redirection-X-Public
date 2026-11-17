/// 读取 Linux `/proc/<pid>/stat` 的进程启动时钟值（字段 22）。
///
/// PID 会被复用；需要跨异步清理周期识别同一个进程实例时，必须同时比较该值。
pub fn process_start_time_ticks(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let after_name = stat.rsplit_once(')')?.1.trim_start();
    after_name.split_whitespace().nth(19)?.parse().ok()
}

// 请求任务的凭据与包身份；执行者边界先于包名、进程组和 UID 回退判定。

/// 停止向发起应用追溯的原因；普通应用切换进程名或非特权域不会触发边界。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionBoundary {
    #[default]
    None,
    Root,
    Shell,
    Privileged,
    Unverified,
}

impl AttributionBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Root => "root",
            Self::Shell => "shell",
            Self::Privileged => "privileged",
            Self::Unverified => "unverified",
        }
    }
}

/// 同一任务实例的身份快照；`uid` 保留请求 UID，包名与共享组标签分别存储。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestIdentity {
    pub uid: i32,
    pub pid: i32,
    pub start_time: u64,
    pub native: String,
    pub selinux_context: String,
    pub boundary: AttributionBoundary,
    pub package: String,
    pub shared_user_id: String,
}

impl RequestIdentity {
    pub fn same_task(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.uid == other.uid
            && self.start_time == other.start_time
            && self.native == other.native
            && self.selinux_context == other.selinux_context
            && self.boundary == other.boundary
    }
    pub fn is_terminal(&self) -> bool {
        self.boundary != AttributionBoundary::None
    }

    /// 跨 UID 代写能力只授予已核验的系统提供器；旧进程名还须匹配 PM 的 UID 组。
    pub fn is_system_writer(&self) -> bool {
        !self.is_terminal()
            && (crate::redirect::policy::is_system_writer_package(&self.package)
                || crate::redirect::policy::is_legacy_writer_process_name(&self.native)
                    && crate::redirect::policy::get_packages_for_uid(self.uid)
                        .iter()
                        .any(|package| crate::redirect::policy::is_system_writer_package(package)))
    }

    /// 监视标签、来源与可信度；核验失败保留未知，共享组只作回退显示。
    pub fn monitor_identity(&self) -> (&str, &'static str, &'static str) {
        match self.boundary {
            AttributionBoundary::Unverified => ("-", "unknown", "none"),
            AttributionBoundary::Root
            | AttributionBoundary::Shell
            | AttributionBoundary::Privileged => (&self.native, "executor", "high"),
            AttributionBoundary::None if !self.package.is_empty() => {
                (&self.package, "caller", "high")
            }
            AttributionBoundary::None if !self.shared_user_id.is_empty() => {
                (&self.shared_user_id, "shared_uid", "fallback")
            }
            AttributionBoundary::None => ("-", "unknown", "none"),
        }
    }

    /// 无法读取任务时仍保留内核请求身份；未知凭据不得作为 App 来源使用。
    pub fn unresolved(uid: i32, pid: i32) -> Self {
        Self {
            uid,
            pid,
            native: match uid {
                0 => "root",
                2000 => "shell",
                _ => "",
            }
            .to_owned(),
            boundary: match uid {
                0 => AttributionBoundary::Root,
                2000 => AttributionBoundary::Shell,
                _ => AttributionBoundary::Unverified,
            },
            ..Self::default()
        }
    }

    /// 任务已失效时保留原始快照，只关闭包身份与后续归因回退。
    pub fn invalidate(&mut self) {
        self.boundary = AttributionBoundary::Unverified;
        self.package.clear();
        self.shared_user_id.clear();
    }

    /// 异步恢复只接受同一任务；原请求的终止边界不随任务退出或凭据变化撤销。
    pub fn revalidate(&mut self, current: Option<&Self>) {
        if !self.is_terminal() && current.is_none_or(|current| !self.same_task(current)) {
            self.invalidate();
        }
    }

    /// 原始任务字段用于核查执行者，不把共享组名或 native 名称当作包名。
    pub fn log_fields(&self) -> String {
        format!(
            "|caller_uid={}|caller_pid={}|caller_start_time={}|caller_context={}|caller_boundary={}|native={}",
            self.uid,
            self.pid,
            self.start_time,
            crate::platform::log_fields::encode(&self.selinux_context),
            self.boundary.as_str(),
            crate::platform::log_fields::encode(&self.native)
        )
    }

    /// 替换附加字段中的任务快照，避免解析器对重复键取值不一致。
    pub fn with_log_fields(&self, extra: &str) -> String {
        let mut fields = extra
            .split('|')
            .filter(|field| {
                !matches!(
                    field.split_once('=').map(|(key, _)| key),
                    Some(
                        "caller_uid"
                            | "caller_pid"
                            | "caller_start_time"
                            | "caller_context"
                            | "caller_boundary"
                            | "native"
                    )
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        fields.push_str(&self.log_fields());
        fields.trim_start_matches('|').to_owned()
    }
}

pub(crate) fn attribution_boundary(uids: &[i32; 4], context: &str) -> AttributionBoundary {
    if uids.contains(&0) {
        AttributionBoundary::Root
    } else if uids.contains(&2000) || context.split(':').nth(2) == Some("shell") {
        AttributionBoundary::Shell
    } else if matches!(
        context.split(':').nth(2),
        Some("su" | "magisk" | "ksu" | "kernelsu")
    ) {
        AttributionBoundary::Privileged
    } else if context.is_empty() {
        AttributionBoundary::Unverified
    } else {
        AttributionBoundary::None
    }
}

fn native_name(cmdline: &str) -> String {
    cmdline
        .split('\0')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .char_indices()
        .take_while(|(offset, ch)| offset + ch.len_utf8() <= 255)
        .map(|(_, ch)| ch)
        .collect()
}

/// 读取真实 UID 与 argv0 名称供进程名册使用；无可读名称时返回 None。
/// 名称可以是 native basename，不代表已核验包；请求归因使用任务快照接口。
pub fn read_proc_identity(pid: i32) -> Option<(i32, String)> {
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let uid = read_proc_uid(pid)?;
    let package = resolve_identity_package(&cmdline, uid)?;
    Some((uid, package))
}

/// `/proc/<pid>/status` 里的实际 uid，读不到返回 None。
pub fn read_proc_uid(pid: i32) -> Option<i32> {
    read_proc_uids(pid).map(|uids| uids[0])
}

fn read_proc_uids(pid: i32) -> Option<[i32; 4]> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_proc_uids(&status)
}

fn parse_proc_uids(status: &str) -> Option<[i32; 4]> {
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let uids = [
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
            fields.next()?.parse().ok()?,
        ];
        return uids.iter().all(|uid| *uid >= 0).then_some(uids);
    }
    None
}

/// 查询任务对应的已安装包，保留内核为本次操作报告的 UID；任务的 fsuid 可与包 UID 不同。
#[cfg(unix)]
pub fn read_proc_identity_for_request(pid: i32, request_uid: i32) -> Option<RequestIdentity> {
    if pid <= 0 || request_uid < 0 {
        return None;
    }
    let mut identity = read_request_task(pid, request_uid)?;
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let user_id = crate::platform::user_id_from_uid(request_uid);
    if !identity.is_terminal() {
        identity.package = resolve_request_package(&cmdline, request_uid, |name| {
            crate::redirect::policy::get_uid_for_package_in_user(name, user_id)
        })
        .or_else(|| resolve_launching_app_by_process_group(pid, request_uid))
        .or_else(|| {
            crate::platform::packages::packages_for_uid(request_uid)
                .ok()
                .and_then(unique_request_package)
        })
        .unwrap_or_default();
        if identity.package.is_empty() {
            identity.shared_user_id =
                crate::platform::packages::shared_user_id_for_uid(request_uid)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
        }
    }
    read_request_task(pid, request_uid)
        .filter(|after| identity.same_task(after))
        .map(|_| identity)
}

/// 只读取原请求任务的内核身份，不查询 PackageManager；用于缓存命中前核验。
pub fn read_request_task(pid: i32, request_uid: i32) -> Option<RequestIdentity> {
    if pid <= 0 || request_uid < 0 {
        return None;
    }
    let start_before = crate::platform::process_start_time_ticks(pid)?;
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let uids = read_proc_uids(pid)?;
    if !request_uid_is_task_credential(&uids, request_uid) {
        return None;
    }
    let selinux_context = std::fs::read_to_string(format!("/proc/{pid}/attr/current"))
        .unwrap_or_default()
        .trim_matches(['\0', '\n', '\r'])
        .to_string();
    let identity = RequestIdentity {
        uid: request_uid,
        pid,
        start_time: start_before,
        native: native_name(&cmdline),
        boundary: attribution_boundary(&uids, &selinux_context),
        selinux_context,
        ..RequestIdentity::default()
    };
    (crate::platform::process_start_time_ticks(pid) == Some(start_before)).then_some(identity)
}

fn unique_request_package(packages: std::collections::HashSet<String>) -> Option<String> {
    if packages.len() != 1 {
        return None;
    }
    packages
        .into_iter()
        .next()
        .filter(|name| is_package_shape(name))
}

fn resolve_request_package(
    cmdline: &str,
    request_uid: i32,
    package_uid: impl Fn(&str) -> i32,
) -> Option<String> {
    let package = resolve_identity_package(cmdline, request_uid)?;
    (is_package_shape(&package) && package_uid(&package) == request_uid).then_some(package)
}

/// pid 表保留 argv0 的包名或原生命令名；命令行参数不作为包身份来源。
fn resolve_identity_package(cmdline: &str, uid: i32) -> Option<String> {
    let first = cmdline.split('\0').next()?.trim();
    let mut value = first.split(':').next()?.trim();
    if let Some(basename) = value.rsplit('/').next() {
        value = basename;
    }
    if is_package_shape(value) {
        if package_uid_matches(value, uid) {
            return Some(value.to_string());
        }
        if let Some(parent) = truncate_to_known_package_for_uid(value, uid) {
            return Some(parent);
        }
        return Some(value.to_string());
    }
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return None;
    }
    Some(value.to_string())
}

/// 从 cgroup v2 的 `uid_<uid>/pid_<pid>` 组首解析已安装包。
/// 组首须仍为同一任务、未触发执行者边界且包 UID 匹配；布局不符或组首退出返回 None。
pub fn resolve_launching_app_by_process_group(pid: i32, uid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let leader = parse_process_group_leader(&cgroup, uid)?;
    if leader == pid {
        return None;
    }
    let task = read_request_task(leader, uid)?;
    if task.is_terminal() {
        return None;
    }
    let (leader_uid, leader_name) = read_proc_identity(leader)?;
    if leader_uid != uid || !is_package_shape(&leader_name) {
        return None;
    }
    if !package_uid_matches(&leader_name, uid) {
        return None;
    }
    read_request_task(leader, uid)
        .filter(|after| task.same_task(after))
        .map(|_| leader_name)
}

/// 从 `/proc/<pid>/cgroup` 内容里取 cgroup v2 层级的进程组首 pid。
///
/// 只认 `0::/<任意>/uid_<uid>/pid_<pid>` 形态，且 uid 段必须与给定 uid 相同：uid 不符
/// 说明这不是本 uid 的应用派生出来的进程（例如被 root 工具移进别的组），不能采信。
fn parse_process_group_leader(cgroup: &str, uid: i32) -> Option<i32> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let mut segments = path.trim().rsplit('/');
    let leader = segments.next()?.strip_prefix("pid_")?.parse::<i32>().ok()?;
    let group_uid = segments.next()?.strip_prefix("uid_")?.parse::<i32>().ok()?;
    if group_uid != uid || leader <= 0 {
        return None;
    }
    Some(leader)
}

/// 是否具备包名形态（含点、不以点开头/结尾、无连续点）。
fn is_package_shape(value: &str) -> bool {
    value.contains('.') && !value.starts_with('.') && !value.ends_with('.') && !value.contains("..")
}

fn truncate_to_known_package_for_uid(candidate: &str, uid: i32) -> Option<String> {
    let mut remaining = candidate;
    while let Some(index) = remaining.rfind('.') {
        remaining = &remaining[..index];
        if remaining.matches('.').count() < 1 {
            break;
        }
        if package_uid_matches(remaining, uid) {
            return Some(remaining.to_string());
        }
    }
    None
}

fn package_uid_matches(package_name: &str, uid: i32) -> bool {
    crate::redirect::policy::get_uid_for_package_in_user(
        package_name,
        crate::platform::user_id_from_uid(uid),
    ) == uid
}

fn request_uid_is_task_credential(uids: &[i32; 4], request_uid: i32) -> bool {
    request_uid >= 0 && uids.contains(&request_uid)
}

#[cfg(test)]
#[path = "../../tests/native-unit/proc_identity.rs"]
mod tests;
