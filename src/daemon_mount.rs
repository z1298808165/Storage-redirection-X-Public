use crate::domain::PathMapping;
use crate::fuse_redirect::{
    FuseRedirectConfig, ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount,
    log_scoped_mount_roots, mount_blocking_with_ready,
};
use crate::fuse_supervisor::{self, EndpointHealth, RecoveryAction};
use crate::mount::MountPlanner;
use crate::mount_identity::{self, MountLedger, MountVerdict};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{fs, module_paths, mountinfo, paths};
use libc::{
    AF_UNIX, CLONE_NEWNS, MNT_DETACH, O_CLOEXEC, O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY, SIGKILL,
    SIGTERM, SO_RCVTIMEO, SOCK_DGRAM, SOL_SOCKET, WNOHANG, c_int, c_void, close, open, recv, send,
    setns, setsockopt, socketpair, umount2, waitpid,
};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const PARENT_RECV_TIMEOUT_SEC: i64 = 5;
const PARENT_RECV_GRACE_TIMEOUT_SEC: i64 = 1;
const FUSE_READY_TIMEOUT_SEC: i64 = 4;
const DAEMON_MOUNT_SLOW_MS: i64 = 20;
const MAX_UNMOUNT_PASSES_PER_TARGET: usize = 32;
/// 同一应用允许同时存在的卡死挂载子进程数上限，超过后熔断该应用的后续挂载请求。
/// 熔断按包名隔离：单个应用的一次挂载超时不应该让整机其它应用的挂载请求一起失败。
const MAX_STUCK_MOUNT_CHILDREN: usize = 2;
/// 卡死子进程计入熔断的时长窗口。
///
/// 超过该时长仍未回收的子进程通常处于内核态不可中断等待（D 状态），此时 SIGKILL
/// 不生效、`waitpid` 也只会持续返回 0，子进程会一直留在回收列表里。如果继续把它
/// 算作熔断依据，被波及的应用会一直挂载失败直到守护进程重启。因此到期后它不再
/// 计入阈值，只保留在回收列表中继续尝试回收。
const STUCK_MOUNT_CHILD_BLOCK_WINDOW_MS: i64 = 120_000;
/// 卡死子进程总数安全阀。单应用熔断只能限制单个应用的堆积速度，若多个应用同时
/// 出现无法回收的挂载子进程，仍需要一道全局上限避免整机无限 fork。
const MAX_TOTAL_STUCK_MOUNT_CHILDREN: usize = 32;
/// 卡死子进程回收列表的长度上限。D 状态进程可能长期无法回收，需要兜底避免
/// 列表无界增长；超出时丢弃最早登记的条目，保留较新的卡死记录。
const MAX_TRACKED_STUCK_MOUNT_CHILDREN: usize = 64;
const STUCK_MOUNT_SKIP_LOG_STEP: u64 = 32;

static ACTIVE_MOUNT_PIDS: Lazy<Mutex<HashSet<i32>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static LAST_SUCCESS_BY_PID: Lazy<Mutex<HashMap<i32, (u64, u64)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static STUCK_MOUNT_CHILDREN: Lazy<Mutex<Vec<StuckMountChild>>> =
    Lazy::new(|| Mutex::new(Vec::new()));
static STUCK_MOUNT_SKIP_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

/// 一个已被判定卡住、仍在等待回收的挂载子进程。
struct StuckMountChild {
    pid: i32,
    /// 登记该子进程时对应请求的包名，用于把熔断限制在同一应用内。
    package_name: String,
    /// 登记时刻的单调时钟毫秒值，用于让熔断窗口随时间失效。
    since_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountOperation {
    Reload,
    Disable,
}

pub struct MountRequest {
    pub operation: MountOperation,
    pub pid: i32,
    pub uid: i32,
    pub package_name: String,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub is_mapping_mode_only: bool,
    pub storage_backend_mode: crate::config::StorageBackendMode,
    pub is_file_monitor_enabled: bool,
    pub config_version: u64,
}

impl crate::fuse_redirect::MountRequestFields for MountRequest {
    fn package_name(&self) -> &str {
        &self.package_name
    }
    fn pid(&self) -> i32 {
        self.pid
    }
    fn uid(&self) -> i32 {
        self.uid
    }
    fn app_data_dir(&self) -> &str {
        &self.app_data_dir
    }
    fn redirect_target(&self) -> &str {
        &self.redirect_target
    }
    fn is_file_monitor_enabled(&self) -> bool {
        self.is_file_monitor_enabled
    }
    fn storage_backend_mode(&self) -> crate::config::StorageBackendMode {
        self.storage_backend_mode
    }
    fn allowed_real_paths(&self) -> &[String] {
        &self.allowed_real_paths
    }
    fn excluded_real_paths(&self) -> &[String] {
        &self.excluded_real_paths
    }
    fn sandboxed_paths(&self) -> &[String] {
        &self.sandboxed_paths
    }
    fn read_only_paths(&self) -> &[String] {
        &self.read_only_paths
    }
    fn path_mappings(&self) -> &[crate::domain::PathMapping] {
        &self.path_mappings
    }
    fn is_mapping_mode_only(&self) -> bool {
        self.is_mapping_mode_only
    }
}

pub fn has_mount_state(request: &MountRequest) -> bool {
    has_mount_state_internal(request, false)
}

/// 周期 reconcile 使用更严格的状态判定，额外确认记录的目标仍存在于应用 namespace。
pub fn has_healthy_mount_state(request: &MountRequest) -> bool {
    has_mount_state_internal(request, true)
}

/// 判定「该应用的挂载已按**当前配置**建立，重挂只会叠加挂载层」。
///
/// 这是重挂的幂等判据：状态健康（目标仍在应用 namespace 内、FUSE 子进程存活）**且**记录下来的
/// 配置指纹与当前配置一致。
///
/// 为什么需要它：重挂不会先摘除旧挂载栈，而对同一个进程重复挂载会在其命名空间里叠出多层。
/// 应用自己 specialize 时挂一次，守护进程启动轮次里 Prewarm 与随后两轮 Full 又会各挂一次，
/// 于是一个进程的顶层目录上会压着 2~3 层模块挂载。叠加后每层的 `root` 解析基准不同，应用
/// 读到的目录内容随层数变化（虚增或丢失），层数也没有上限——真机上表现为微信
/// `Android/media/com.tencent.mm/Lumenchat/plugins` 时而读到真实目录、时而读到空壳。
///
/// 为什么用指纹而不是 `version=`：`config_version` 是进程内自增计数器，应用侧 payload 与
/// 守护进程各自维护一份，写进同一个状态文件时会互相跳变，据此比对必然误判。
pub fn has_current_mount_state(request: &MountRequest) -> bool {
    if request.operation != MountOperation::Reload {
        return false;
    }
    if !has_mount_state_internal(request, true) {
        return false;
    }
    let current = crate::config::SettingsHub::instance().config_fingerprint();
    match mount_state_fingerprint(request) {
        Some(recorded) => recorded == current,
        // 旧版本模块写下的状态文件没有指纹字段：无法证明它对应当前配置，按需要重挂处理。
        None => false,
    }
}

/// 读取挂载状态文件里记录的配置指纹。
fn mount_state_fingerprint(request: &MountRequest) -> Option<u64> {
    let content = std::fs::read_to_string(state_file_path(request)).ok()?;
    state_value(&content, "fingerprint=").and_then(|value| value.parse::<u64>().ok())
}

fn has_mount_state_internal(request: &MountRequest, check_mount_targets: bool) -> bool {
    let state_path = state_file_path(request);
    if std::fs::metadata(&state_path).is_err() {
        return false;
    }
    // Disable 的目标是清理残留，不能因为 FUSE 子进程或 mountinfo 已经不健康
    // 就把状态视为不存在，否则坏挂载会永久跳过卸载。
    if request.operation == MountOperation::Disable {
        return true;
    }
    // 记录过 FUSE 服务却已经死掉时，挂载点会留在目标 namespace 里变成 ENOTCONN 死挂载，
    // 应用访问会直接失败。此时把挂载状态视为无效，让周期 reconcile 重新执行挂载；
    // 若 FUSE 再次启动失败，启动阶段的 mount namespace 降级会接管。
    if has_dead_fuse_child(&state_path, request) {
        return false;
    }

    // 状态文件只代表 daemon 曾经成功执行过挂载，进程的 mount namespace 可能随后被
    // 系统回收或替换。周期 reconcile 需要把这种状态视为缺失，否则应用会一直保留
    // 一份看似有效、实际访问已经失败的重定向。
    let targets = read_mount_targets(&state_path);
    if check_mount_targets {
        if !targets.is_empty() && !mount_targets_present(request.pid, &targets, request) {
            return false;
        }
        if !backend_mount_targets_responsive(request) {
            return false;
        }
    }
    true
}

/// 在目标进程的挂载命名空间内核对本轮记录的目标是否可解析、是否真的挂上了。
///
/// 调用点位于已经 `setns` 到应用命名空间的挂载子进程里，因此 `metadata` 与
/// `/proc/self/mountinfo` 反映的都是**应用自己的视图**，而不是守护进程的视图。热重载类
/// 问题需要这条记录才能把两种情况分开：绑定根本没在应用视图里生效，还是生效之后又被
/// 后续请求摘掉——后者会在下一次清理里留下 `daemon unmount ok` 记录，两条对照即可定位。
fn log_mounted_target_view(targets: &[String], request: &MountRequest) {
    if targets.is_empty() {
        return;
    }
    // 诊断：热重载后出现「子进程 mount 返回 0，但同 ns 的 /proc/<pid>/mountinfo 与
    // 事后采集都看不到新挂载」的矛盾。这里一次性自证三件事——本进程当前 ns 的
    // inode、目标应用进程 ns 的 inode、以及本进程 mountinfo 里映射目标的原始行，
    // 用于判定 setns 目标错误、ns 内被二次摘除、还是字符串形态不匹配。
    let self_ns = std::fs::read_link("/proc/self/ns/mnt")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unreadable".to_string());
    let app_ns = std::fs::read_link(format!("/proc/{}/ns/mnt", request.pid))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unreadable".to_string());
    let diag_lines = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|content| {
            let matched: Vec<&str> = content
                .lines()
                .filter(|line| line.contains("SrtProbe"))
                .collect();
            if matched.is_empty() {
                "<none>".to_string()
            } else {
                matched.join(" | ")
            }
        })
        .unwrap_or_else(|_| "<read failed>".to_string());
    log::warn!(
        "mount view diag self_ns={} app_ns={} app_pid={} srtprobe_lines={}",
        self_ns,
        app_ns,
        request.pid,
        diag_lines
    );
    let mut unreadable = 0usize;
    let mut not_mounted = 0usize;
    for target in targets {
        if std::fs::metadata(target).is_err() {
            unreadable += 1;
            log::warn!("daemon target view unreadable target={}", target);
            continue;
        }
        if current_mount_target_count(target) == 0 {
            not_mounted += 1;
            log::warn!("daemon target view not mounted target={}", target);
        }
    }
    log::info!(
        "daemon target view checked={} unreadable={} not_mounted={}",
        targets.len(),
        unreadable,
        not_mounted
    );
}

fn mount_targets_present(pid: i32, targets: &[String], request: &MountRequest) -> bool {
    let path = format!("/proc/{}/mountinfo", pid);
    let Ok(content) = std::fs::read_to_string(&path) else {
        log::warn!(
            "daemon mountinfo unavailable pid={} pkg={} path={}, remount pending",
            pid,
            request.package_name,
            path
        );
        return false;
    };
    mount_targets_present_with(&content, targets, request)
}

/// 在给定 mountinfo 文本上判定状态文件记录的目标是否都还在场。
///
/// 拆出纯判定便于离线验证：守护进程实际调用走 [`mount_targets_present`]，它只负责读取
/// `/proc/<pid>/mountinfo` 后转交到这里。
fn mount_targets_present_with(content: &str, targets: &[String], request: &MountRequest) -> bool {
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let alias_roots = paths::storage_alias_roots_for_user(user_id);
    // 同一存储路径存在多个内核视图（`/storage/emulated/<user>`、`/data/media/<user>`、
    // `/mnt/*/<user>/emulated/<user>`、`/storage/self/primary`、`/sdcard`）。它们是同一份
    // 挂载在不同 namespace 里的别名，状态文件记录哪个别名取决于挂载当时的遍历路径，因此
    // 必须按"别名组"判定：组内任一别名在场即视为该逻辑目标仍挂载。
    //
    // 若改成每个别名各自成组，就会把正常的视图差异判成整份挂载失效，每约三秒触发一次
    // `remount pending`，反复打断正在进行的文件写入。
    let expected_groups = targets
        .iter()
        .map(|target| canonical_health_target(target, &storage_root, &alias_roots))
        .collect::<HashSet<_>>();
    let present_groups = targets
        .iter()
        .filter(|target| {
            mount_target_count_from_mountinfo(content, target, &storage_root, &alias_roots) > 0
        })
        .map(|target| canonical_health_target(target, &storage_root, &alias_roots))
        .collect::<HashSet<_>>();
    let missing = expected_groups.difference(&present_groups).count();
    if missing == 0 {
        return true;
    }

    log::warn!(
        "daemon mount target group missing pid={} pkg={} missing={} total={}, remount pending",
        request.pid,
        request.package_name,
        missing,
        expected_groups.len()
    );
    false
}

/// 把一条挂载目标折算到它所属的"存储别名组"代表路径。
///
/// 代表路径统一取 `/storage/emulated/<user>` 视图；`/data/media/<user>`、`/sdcard`、
/// `/storage/self/primary`、`/storage/emulated/legacy` 以及各 `/mnt/<view>/.../emulated/<user>`
/// 都经 [`paths::storage_alias_roots_for_user`] 折算到同一代表，`/data/data` 按历史别名折算到
/// `/data/user/0`。
///
/// 只在"组内任一别名在场即健康"的口径下使用：别名之间的差异属于正常 namespace 视图差异，
/// 不能当作挂载失效。这与 [`mount_target_count_from_mountinfo`] 共用同一折算，保证
/// "期望分组"与"在场分组"口径一致。
fn canonical_health_target(target: &str, storage_root: &str, alias_roots: &[String]) -> String {
    let target = paths::normalize_syntax(target);
    let target = if target == "/data/data" {
        "/data/user/0".to_string()
    } else if let Some(rest) = target.strip_prefix("/data/data/") {
        format!("/data/user/0/{}", rest)
    } else {
        target
    };

    // `/sdcard` 是对 `/storage/emulated/<user>` 的对称链接（`paths::normalize` 同样会折叠它），
    // 但别名根清单里没有这一项：这里显式折算，避免记录到的 `/sdcard/...` 目标自成一组。
    if target == "/sdcard" {
        return storage_root.to_string();
    }
    if let Some(suffix) = target.strip_prefix("/sdcard/") {
        return format!("{}/{}", storage_root, suffix);
    }

    for alias_root in alias_roots {
        if target == *alias_root {
            return storage_root.to_string();
        }
        let Some(suffix) = target.strip_prefix(alias_root.as_str()) else {
            continue;
        };
        if suffix.starts_with('/') {
            return format!("{}{}", storage_root, suffix);
        }
    }
    target
}

/// 检查应用 namespace 内真实存储后端是否仍能响应目录访问。
///
/// `/proc/<pid>/mountinfo` 只能证明挂载记录还在，MediaProvider 的 FUSE 服务退出后，
/// 记录仍可能保留但访问返回 ENOTCONN。端点探测统一由 [`fuse_supervisor::probe_endpoint`]
/// 提供：它沿用目标进程的 mount namespace 解析路径，不需要切换 daemon 自身的 namespace，
/// 也不会修改目录元数据。
fn backend_mount_targets_responsive(request: &MountRequest) -> bool {
    for target in allowed_real_backend_targets(request) {
        let health = fuse_supervisor::probe_endpoint(request.pid, &target);
        // 只有断连类 errno 才说明挂载不可用。探测本身失败（例如路径被 SELinux 拒绝）不构成
        // 摘除重挂的理由，否则会把一次权限波动升级成整轮重挂。
        if !health.is_dead_connection() {
            continue;
        }
        log::warn!(
            "daemon backend mount endpoint unhealthy pid={} pkg={} target={} state={} errno={} {}",
            request.pid,
            request.package_name,
            target,
            health.as_str(),
            health.error_no(),
            errno_text(health.error_no())
        );
        return false;
    }
    true
}

/// 生成本次请求对应的监督快照。
///
/// 把"账本记录的归属"与"端点实时健康"合起来看：归属给出该摘谁，健康给出该不该恢复。
/// 任何一个挂载点判定为不允许注入，整个命名空间就都不注入，避免同一轮里一部分路径被清理、
/// 一部分继续叠加。
///
/// 账本不存在时返回 None：没有身份记录说明本模块从未在这个命名空间里成功挂载过，
/// 此时不存在"自己的残留"，不需要监督介入。
pub fn supervise_mount_request(
    request: &MountRequest,
) -> Option<fuse_supervisor::NamespaceSupervision> {
    let ledger = mount_identity::load(&request.package_name, request.pid)?;
    let current_namespace = mount_identity::namespace_identity(request.pid);
    let target_is_current = ledger.target_is_current();
    let poisoned = ledger.is_poisoned();
    let mut actions = Vec::new();
    let mut worst_health = EndpointHealth::Healthy;

    for mount in &ledger.mounts {
        let live = mount_identity::topmost_live_mount(request.pid, &mount.mount_point);
        let verdict = mount_identity::classify_mount(
            &ledger,
            &mount.mount_point,
            live.as_ref(),
            current_namespace,
            target_is_current,
        );
        let health = fuse_supervisor::probe_endpoint(request.pid, &mount.mount_point);
        if health.is_dead_connection() {
            worst_health = health;
        }
        actions.push(fuse_supervisor::plan_recovery(&verdict, health, poisoned));
    }

    if actions.is_empty() {
        // 账本还没有挂载明细（首次挂载后登记失败）：只能按命名空间身份判定。
        let verdict = if target_is_current && current_namespace == Some(ledger.namespace) {
            MountVerdict::Detached
        } else {
            MountVerdict::StaleNamespace
        };
        actions.push(fuse_supervisor::plan_recovery(
            &verdict,
            EndpointHealth::Missing,
            poisoned,
        ));
    }

    let action = fuse_supervisor::aggregate_actions(actions);
    if action == RecoveryAction::DropStale {
        // 命名空间已经替换：账本记录的挂载随旧命名空间一起销毁，记录本身已经没有意义。
        // 这里顺手删除，避免后续每一轮都重复判定为过期。
        if mount_identity::remove(&request.package_name, request.pid) {
            log::info!(
                "daemon mount identity dropped stale pid={} pkg={} recorded_ns={}:{}",
                request.pid,
                request.package_name,
                ledger.namespace.dev,
                ledger.namespace.ino
            );
        }
    }
    fuse_supervisor::record_action(action, worst_health);
    Some(fuse_supervisor::NamespaceSupervision::from_ledger(
        &ledger,
        worst_health,
        action,
    ))
}

/// 输出挂载身份与监督状态的诊断报告。
///
/// 读取文件并去掉首尾空白；失败返回 None。
fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

/// 判断挂载点是否覆盖给定路径。
///
/// 刻意不复用 `paths::starts_with`：那个实现是裸字符串前缀比较，`/a/Download` 会把
/// `/a/Downloads` 也算作覆盖。诊断输出必须按路径分量判断，否则会给出错误的「已覆盖」结论。
fn mount_point_covers(mount_point: &str, path: &str) -> bool {
    let mount_point = mount_point.trim_end_matches('/');
    path == mount_point || path.starts_with(&format!("{mount_point}/"))
}

/// 列出以该包名运行（或它的子进程）的进程。
///
/// Android 上进程名等于包名，子进程写作 `包名:后缀`，因此按 `cmdline` 首段匹配即可。
fn package_processes(package_name: &str) -> Vec<(i32, String)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let child_prefix = format!("{package_name}:");
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let cmdline = raw.split(|byte| *byte == 0).next().unwrap_or_default();
        let Ok(cmdline) = std::str::from_utf8(cmdline) else {
            continue;
        };
        if cmdline == package_name || cmdline.starts_with(child_prefix.as_str()) {
            found.push((pid, cmdline.to_string()));
        }
    }
    found.sort_by_key(|(pid, _)| *pid);
    found
}

/// 该进程当前是否映射着本模块的库；`None` 表示读不到 maps。
fn module_mapped_into(pid: i32) -> Option<bool> {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).ok()?;
    Some(maps.contains("storage.redirect.x"))
}

/// 跨层身份快照：把「这是哪个应用」的五个口径并列输出。
///
/// 项目里每一层用不同口径回答同一个问题——zygisk 看进程、Java hook 看调用方 uid、native 看
/// caller uid、FUSE 会话看会话创建时绑定的应用、namespace 看 `ns(dev:ino)+start_time`。失败几乎
/// 都出在层间口径不一致处，而此前只有挂载账本可查，其余各层要逐条 adb 命令拼。把五层一次列清
/// 可以让排查从「猜」变成「读表」。
fn print_cross_layer_snapshot(package_name: &str, path: Option<&str>) {
    println!("== module ==");
    let prop = std::fs::read_to_string(format!("{}/module.prop", module_paths::MODULE_DIR))
        .unwrap_or_default();
    let version = prop
        .lines()
        .find_map(|line| line.strip_prefix("version="))
        .unwrap_or("-");
    let zygisk_lib = format!("{}/zygisk/arm64-v8a.so", module_paths::MODULE_DIR);
    println!(
        "version={} boot_ok={} runtime_disabled={} zygisk_lib_bytes={}",
        version,
        read_trimmed(&format!("{}/.boot_ok", module_paths::MODULE_DIR))
            .unwrap_or_else(|| "-".to_string()),
        if std::path::Path::new(module_paths::RUNTIME_DISABLE_FILE).exists() {
            "yes"
        } else {
            "no"
        },
        std::fs::metadata(&zygisk_lib)
            .map(|meta| meta.len())
            .unwrap_or(0)
    );

    println!("== capability ==");
    let summary = crate::fuse_redirect::config::fuse_capability_summary();
    let now_ms = monotonic_ms().max(0) as u64;
    println!(
        "state={} device_fail={} tracked_scopes={} backoff_step={} teardown_fail={} retry_at_ms={} retry_in_ms={}",
        crate::fuse_redirect::config::fuse_capability_as_str(summary.capability),
        summary.device_failures,
        summary.scope_failures.len(),
        summary.backoff_step,
        summary.teardown_failures,
        summary.retry_at_ms,
        summary.retry_at_ms.saturating_sub(now_ms)
    );
    for (scope, count) in &summary.scope_failures {
        println!("  scope_fail={scope}:{count}");
    }
    let allowed_auto = crate::fuse_redirect::config::scoped_mount_allowed_for_scope(
        package_name,
        crate::config::StorageBackendMode::Auto,
    );
    println!(
        "gate pkg={} mode=auto allowed={} note=实际模式以 app_config 的字段为准",
        package_name, allowed_auto
    );

    println!("== java_hook ==");
    println!(
        "install_state={} deferred_marker={} hot_reload_request_pending={}",
        read_trimmed(module_paths::MEDIA_HOOK_INSTALL_STATE_FILE)
            .unwrap_or_else(|| "-".to_string()),
        read_trimmed(module_paths::MEDIA_HOOK_DEFERRED_FILE).unwrap_or_else(|| "-".to_string()),
        if std::path::Path::new(module_paths::MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE).exists() {
            "yes"
        } else {
            "no"
        }
    );

    println!("== app_config ==");
    let config_path = format!("{}/apps/{}.json", module_paths::CONFIG_DIR, package_name);
    match std::fs::read_to_string(&config_path) {
        Ok(content) => println!(
            "path={} bytes={} content={}",
            config_path,
            content.len(),
            content.trim()
        ),
        Err(_) => println!("path={} present=false", config_path),
    }

    println!("== processes ==");
    let processes = package_processes(package_name);
    if processes.is_empty() {
        println!("none");
    }
    for (pid, cmdline) in &processes {
        println!(
            "pid={} cmdline={} module_mapped_now={} start_ticks={}",
            pid,
            cmdline,
            module_mapped_into(*pid)
                .map(|mapped| mapped.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            crate::platform::process_start_time_ticks(*pid).unwrap_or_default()
        );
    }
    println!(
        "note: module_mapped_now=false 是常见现象——注入完成后模块会 dlclose 自己，启动之后再查 maps 查不到不能据此判定「没被注入」；该结论要看 install_state 与下面的 ledger。"
    );

    println!("== app_runtime_state ==");
    let prefix = format!("{package_name}_");
    for dir in [
        module_paths::MOUNT_STATE_DIR,
        module_paths::MOUNT_INTENT_DIR,
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(prefix.as_str()) {
                continue;
            }
            let path = entry.path();
            println!(
                "{}={}",
                path.display(),
                read_trimmed(&path.display().to_string()).unwrap_or_default()
            );
        }
    }

    let Some(path) = path else {
        return;
    };
    println!("== path ==");
    let user_id = paths::extract_user_id_from_storage_path(path);
    println!("input={} user={}", path, user_id);
    match paths::storage_to_data_media_for_user(path, user_id) {
        Some(backend) => println!("backend={}", backend),
        None => println!("backend=- (不是 /storage/emulated/<user>/... 形态)"),
    }
    println!("note: backend 是绕过重定向层看到的真实落点；是否被重定向看下面的 ledger_mount。");
    for ledger in mount_identity::list_ledgers() {
        if ledger.package_name != package_name {
            continue;
        }
        for mount in &ledger.mounts {
            println!(
                "ledger_mount point={} mount_id={} covers_input={}",
                mount.mount_point,
                mount.mount_id,
                mount_point_covers(&mount.mount_point, path)
            );
        }
    }
}

/// 这是一个独立进程入口，不共享 daemon 进程内的监督计数，因此只报告磁盘上可观察的事实：
/// 账本记录的挂载身份、目标进程是否仍是同一实例、命名空间是否被替换、以及每个挂载点当前
/// 的端点健康。用于回答"daemon 认为它挂了什么、那些挂载现在还活着吗"。
///
/// 用法：
/// - `srx_daemon doctor`：报告挂载账本总体健康；
/// - `srx_daemon doctor <包名> [路径]`：先输出该包的跨层身份快照，再只报告该包的账本。
pub fn doctor_report(args: &[String]) -> i32 {
    let package_name = args
        .first()
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let path = args
        .get(1)
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    if let Some(package_name) = package_name {
        print_cross_layer_snapshot(package_name, path);
    }

    let ledgers: Vec<MountLedger> = match package_name {
        Some(package_name) => mount_identity::list_ledgers()
            .into_iter()
            .filter(|ledger| ledger.package_name == package_name)
            .collect(),
        None => mount_identity::list_ledgers(),
    };
    let mut unhealthy = 0usize;
    println!(
        "mount identity ledger entries={} dir={}",
        ledgers.len(),
        module_paths::MOUNT_STATE_DIR
    );
    for ledger in &ledgers {
        let current_namespace = mount_identity::namespace_identity(ledger.target_pid);
        let target_is_current = ledger.target_is_current();
        let namespace_state = match current_namespace {
            Some(namespace) if namespace == ledger.namespace => "current",
            Some(_) => "replaced",
            None => "unavailable",
        };
        println!(
            "ledger pkg={} pid={} generation={} target={} namespace={} poisoned={} detach_attempts={}",
            ledger.package_name,
            ledger.target_pid,
            ledger.generation,
            if target_is_current { "alive" } else { "gone" },
            namespace_state,
            ledger.is_poisoned(),
            ledger.detach_attempts
        );
        for mount in &ledger.mounts {
            let health = fuse_supervisor::probe_endpoint(ledger.target_pid, &mount.mount_point);
            if health.is_dead_connection() {
                unhealthy = unhealthy.saturating_add(1);
            }
            let live = mount_identity::topmost_live_mount(ledger.target_pid, &mount.mount_point);
            let verdict = mount_identity::classify_mount(
                ledger,
                &mount.mount_point,
                live.as_ref(),
                current_namespace,
                target_is_current,
            );
            println!(
                "  mount point={} recorded_id={} live_id={} health={} verdict={}",
                mount.mount_point,
                mount.mount_id,
                live.as_ref()
                    .map(|live| live.mount_id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                health.as_str(),
                match verdict {
                    MountVerdict::Owned(_) => "owned",
                    MountVerdict::Detached => "detached",
                    MountVerdict::Superseded(_) => "superseded",
                    MountVerdict::StaleNamespace => "stale_namespace",
                }
            );
        }
    }
    println!(
        "supervisor {}",
        fuse_supervisor::SupervisorSummary::snapshot().render()
    );
    if unhealthy > 0 {
        println!("result unhealthy_mounts={}", unhealthy);
        return 1;
    }
    println!("result ok");
    0
}

/// 删除已经不属于存活应用进程实例的挂载状态。
///
/// FUSE 服务会在应用退出时自行卸载；状态文件由常驻 daemon 的周期 reconcile 回收，
/// 避免按 PID 命名的记录无限累积。旧格式没有启动时间，回退比较包名与 UID，避免
/// 把已复用给其它进程的 PID 误当成原应用仍在运行。
pub fn prune_stale_mount_states() -> usize {
    let Ok(entries) = std::fs::read_dir(module_paths::MOUNT_STATE_DIR) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("state") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(package_name) = state_value(&content, "package=") else {
            continue;
        };
        let Some(pid) = state_file_pid(&path, package_name) else {
            continue;
        };
        let app_start_time =
            state_value(&content, "app_start_time=").and_then(|value| value.parse::<u64>().ok());
        let is_alive = match app_start_time {
            Some(start) => crate::platform::is_process_instance_alive(pid, start),
            None => {
                let uid = state_value(&content, "uid=").and_then(|value| value.parse().ok());
                legacy_state_owner_is_alive(pid, package_name, uid)
            }
        };
        if is_alive {
            continue;
        }
        let mut cleanup_ok = true;
        for child in &read_fuse_children(&path.to_string_lossy()) {
            if !terminate_recorded_fuse_child(child) {
                cleanup_ok = false;
            }
        }
        if cleanup_ok && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("daemon pruned stale mount states count={}", removed);
    }
    removed
}

/// 清理所有已记录的 daemon 挂载。
///
/// 由 stop 流程在停止 daemon 后调用，使存活应用的 mount namespace
/// 仍可通过状态文件解析并执行 setns/卸载；清理失败的状态文件会保留，交给后续
/// daemon 启动后的 reconcile 重试。
pub fn cleanup_all_mount_states() -> bool {
    let Ok(entries) = std::fs::read_dir(module_paths::MOUNT_STATE_DIR) else {
        return true;
    };
    let mut all_ok = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("state") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            all_ok = false;
            continue;
        };
        let Some(package_name) = state_value(&content, "package=") else {
            all_ok = false;
            continue;
        };
        let Some(pid) = state_file_pid(&path, package_name) else {
            all_ok = false;
            continue;
        };
        let uid = state_value(&content, "uid=")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(-1);
        let request = MountRequest {
            operation: MountOperation::Disable,
            pid,
            uid,
            package_name: package_name.to_string(),
            app_data_dir: String::new(),
            redirect_target: String::new(),
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            path_mappings: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            is_mapping_mode_only: false,
            storage_backend_mode: crate::config::StorageBackendMode::Namespace,
            is_file_monitor_enabled: false,
            config_version: 0,
        };
        let app_start_time =
            state_value(&content, "app_start_time=").and_then(|value| value.parse::<u64>().ok());
        let app_alive = app_start_time
            .map(|start| crate::platform::is_process_instance_alive(pid, start))
            .unwrap_or_else(|| std::fs::metadata(format!("/proc/{pid}")).is_ok());
        if app_alive {
            if !execute_mount_request(&request) {
                all_ok = false;
            }
        } else {
            let children = read_fuse_children(&path.to_string_lossy());
            let mut state_ok = true;
            for child in &children {
                if !terminate_recorded_fuse_child(child) {
                    state_ok = false;
                }
            }
            if state_ok {
                let _ = std::fs::remove_file(&path);
            } else {
                all_ok = false;
            }
        }
    }
    all_ok
}

fn state_value<'a>(content: &'a str, prefix: &str) -> Option<&'a str> {
    content.lines().find_map(|line| line.strip_prefix(prefix))
}

fn state_file_pid(path: &std::path::Path, package_name: &str) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    let prefix = format!("{}_", module_paths::sanitize_name(package_name));
    stem.strip_prefix(&prefix)?.parse().ok()
}

fn legacy_state_owner_is_alive(pid: i32, package_name: &str, expected_uid: Option<i32>) -> bool {
    let Ok(cmdline) = std::fs::read(format!("/proc/{}/cmdline", pid)) else {
        return false;
    };
    let end = cmdline
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(cmdline.len());
    if String::from_utf8_lossy(&cmdline[..end]) != package_name {
        return false;
    }
    let Some(expected_uid) = expected_uid else {
        return true;
    };
    let Ok(status) = std::fs::read_to_string(format!("/proc/{}/status", pid)) else {
        return false;
    };
    status.lines().any(|line| {
        line.strip_prefix("Uid:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<i32>().ok())
            == Some(expected_uid)
    })
}

/// 检查挂载状态中记录的 FUSE 服务进程是否已经退出。
///
/// FUSE 服务是在挂载用的 fork 子进程内启动的，因此相对本 daemon 是孙进程；挂载子进程
/// 随后立即退出，这些服务会被 init 收养。也就是说 daemon 既不能也不需要 `waitpid`
/// 回收它们，`waitpid` 只会返回 ECHILD；判定存活只能查 `/proc/<pid>`。它们不会以
/// 僵尸形式留在 daemon 名下，但 PID 仍可能被复用，因此还要比较启动时钟值。
fn has_dead_fuse_child(state_path: &str, request: &MountRequest) -> bool {
    let children = read_fuse_children(state_path);
    if children.is_empty() {
        return false;
    }

    for child in children {
        if child
            .start_time_ticks
            .is_some_and(|start| crate::platform::is_process_instance_alive(child.pid, start))
        {
            continue;
        }
        log::warn!(
            "daemon fuse child gone pid={} app_pid={} pkg={}, remount pending",
            child.pid,
            request.pid,
            request.package_name
        );
        // 服务进程消失只说明这一个应用的 scoped 会话结束：应用退出、重启或系统
        // 清理它的挂载 namespace 都会这样，不代表设备整体不支持 scoped 会话。
        // 因此这里只记录待重挂，不再改写整机 FUSE 能力快照。
        return true;
    }
    false
}

pub fn execute_mount_request(request: &MountRequest) -> bool {
    let started_ms = monotonic_ms();
    let initial_state = if request.operation == MountOperation::Disable {
        "disabled"
    } else {
        "applying"
    };
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        initial_state,
    );
    if should_skip_for_stuck_children(request) {
        crate::mount_intent::mark_state(
            &request.package_name,
            request.pid,
            request.uid,
            request.storage_backend_mode,
            request.config_version,
            "failed",
        );
        return false;
    }
    let Some(_guard) = MountPidGuard::try_acquire(request) else {
        crate::mount_intent::mark_state(
            &request.package_name,
            request.pid,
            request.uid,
            request.storage_backend_mode,
            request.config_version,
            "duplicate",
        );
        return recently_mounted(request);
    };
    let is_success = run_mount_in_forked_child(request);
    let final_state = if request.operation == MountOperation::Disable {
        "disabled"
    } else if is_success {
        "mounted"
    } else {
        "failed"
    };
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        final_state,
    );
    if is_success {
        remember_successful_mount(request);
    }
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms >= DAEMON_MOUNT_SLOW_MS || !is_success {
        log::info!(
            "daemon mount pkg={} pid={} op={:?} ok={} allow={} excl={} sandbox={} ro={} map={} map_only={} ms={}",
            request.package_name,
            request.pid,
            request.operation,
            is_success,
            request.allowed_real_paths.len(),
            request.excluded_real_paths.len(),
            request.sandboxed_paths.len(),
            request.read_only_paths.len(),
            request.path_mappings.len(),
            request.is_mapping_mode_only,
            total_ms
        );
    }
    is_success
}

struct MountPidGuard {
    pid: i32,
}

impl MountPidGuard {
    fn try_acquire(request: &MountRequest) -> Option<Self> {
        let mut active = ACTIVE_MOUNT_PIDS
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if active.insert(request.pid) {
            return Some(Self { pid: request.pid });
        }

        log::warn!(
            "daemon mount duplicate pid={} pkg={} op={:?}",
            request.pid,
            request.package_name,
            request.operation
        );
        None
    }
}

impl Drop for MountPidGuard {
    fn drop(&mut self) {
        let mut active = ACTIVE_MOUNT_PIDS
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        active.remove(&self.pid);
    }
}

fn recently_mounted(request: &MountRequest) -> bool {
    if request.operation != MountOperation::Reload {
        return false;
    }
    let now = monotonic_ms() as u64;
    let mounted_recently = LAST_SUCCESS_BY_PID
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(&request.pid)
        .copied()
        .map(|(last, version)| {
            version == request.config_version && now.saturating_sub(last) <= 5_000
        })
        .unwrap_or(false);
    if mounted_recently {
        log::info!(
            "daemon mount duplicate treated as recent success pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }
    mounted_recently
}

fn remember_successful_mount(request: &MountRequest) {
    let now = monotonic_ms() as u64;
    let mut recent = LAST_SUCCESS_BY_PID
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    recent.insert(request.pid, (now, request.config_version));
    if recent.len() > 128 {
        let cutoff = now.saturating_sub(60_000);
        recent.retain(|_, (timestamp, _)| *timestamp >= cutoff);
    }
}

/// 判定本次挂载请求是否因为卡死的挂载子进程而熔断。
///
/// 判据只统计与本次请求同一包名的卡死子进程，因此单个应用的挂载超时只会让该应用
/// 的后续请求被跳过，不会连坐整机其它应用的挂载。
fn should_skip_for_stuck_children(request: &MountRequest) -> bool {
    prune_stuck_mount_children();
    let (package_stuck, total_stuck) = stuck_mount_child_counts(&request.package_name);
    // 主判据按应用隔离；全局阈值只是极端情况下的安全阀。
    if package_stuck <= MAX_STUCK_MOUNT_CHILDREN && total_stuck <= MAX_TOTAL_STUCK_MOUNT_CHILDREN {
        return false;
    }

    let count = STUCK_MOUNT_SKIP_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 8 || count.is_multiple_of(STUCK_MOUNT_SKIP_LOG_STEP) {
        log::warn!(
            "daemon mount circuit open stuck_children={} total={} pkg={} pid={} op={:?} n={}",
            package_stuck,
            total_stuck,
            request.package_name,
            request.pid,
            request.operation,
            count
        );
    }
    true
}

/// 统计仍在熔断窗口内的卡死挂载子进程数量，返回（本次请求所属应用、全部应用）。
fn stuck_mount_child_counts(package_name: &str) -> (usize, usize) {
    let children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let now = monotonic_ms();
    let mut package_stuck = 0usize;
    let mut total_stuck = 0usize;
    for child in children.iter() {
        if now.saturating_sub(child.since_ms) >= STUCK_MOUNT_CHILD_BLOCK_WINDOW_MS {
            continue;
        }
        total_stuck += 1;
        if child.package_name == package_name {
            package_stuck += 1;
        }
    }
    (package_stuck, total_stuck)
}

/// 清理已经卡住的挂载子进程。
///
/// `waitpid` 与 `kill` 都是可能被信号打断、耗时不确定的系统调用，绝不能在持有全局
/// 挂载状态锁时执行：挂载请求线程也要拿同一把锁，一旦回收阶段变慢，所有请求都会
/// 跟着阻塞。因此这里先在锁内取走整份待清理列表，立即释放锁，在锁外完成回收，
/// 最后再把仍然存活的子进程合并回列表。
fn prune_stuck_mount_children() {
    let pending = {
        let mut children = STUCK_MOUNT_CHILDREN
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if children.is_empty() {
            return;
        }
        std::mem::take(&mut *children)
    };

    let mut alive = Vec::with_capacity(pending.len());
    for child in pending {
        let mut status = 0;
        // SAFETY: status 是栈上有效的整数，指针在调用期间保持有效；child.pid 只读自
        // 回收列表，不涉及借用。
        let ret = unsafe { waitpid(child.pid, &mut status, WNOHANG) };
        if ret == child.pid {
            log::warn!(
                "daemon stuck child finally reaped child={} pkg={} status={}",
                child.pid,
                child.package_name,
                decode_wait_status(status)
            );
            continue;
        }
        if ret < 0 {
            let errno = last_errno();
            if errno == libc::ECHILD || errno == libc::ESRCH {
                continue;
            }
            log::warn!(
                "daemon stuck child waitpid failed child={} pkg={} errno={} {}",
                child.pid,
                child.package_name,
                errno,
                errno_text(errno)
            );
            alive.push(child);
            continue;
        }
        // SAFETY: kill 只接收整型参数与信号编号，不涉及借用指针。
        let _ = unsafe { libc::kill(child.pid, SIGKILL) };
        alive.push(child);
    }

    // 回收期间其它线程可能又登记了新的卡住子进程，这里只做合并，不覆盖。
    let mut children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    for child in alive {
        if !children.iter().any(|existing| existing.pid == child.pid) {
            children.push(child);
        }
    }
    // 内核态不可中断等待的子进程可能永远回收不掉，这里兜底限制列表长度：丢弃最早
    // 登记的条目，它们已经超出熔断窗口，不再影响熔断判定，只是放弃继续回收。
    if children.len() > MAX_TRACKED_STUCK_MOUNT_CHILDREN {
        let excess = children.len() - MAX_TRACKED_STUCK_MOUNT_CHILDREN;
        children.drain(..excess);
        log::warn!(
            "daemon stuck children trimmed dropped={} remaining={}",
            excess,
            children.len()
        );
    }
}

fn remember_stuck_mount_child(child: i32, package_name: &str) {
    let mut children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let now = monotonic_ms();
    if !children.iter().any(|existing| existing.pid == child) {
        children.push(StuckMountChild {
            pid: child,
            package_name: package_name.to_string(),
            since_ms: now,
        });
    }
    let package_stuck = children
        .iter()
        .filter(|existing| {
            existing.package_name == package_name
                && now.saturating_sub(existing.since_ms) < STUCK_MOUNT_CHILD_BLOCK_WINDOW_MS
        })
        .count();
    log::warn!(
        "daemon mount child stuck child={} pkg={} stuck_children={}",
        child,
        package_name,
        package_stuck
    );
}

/// fork 之前在父进程算好的挂载计划。
///
/// 子进程只保留调用线程，不能依赖其它父线程在 fork 瞬间持有的 malloc arena 或全局锁，
/// 所以所有可以提前得到的路径字符串都放在这里，子进程直接复用已分配好的内容。
struct MountForkPlan {
    /// 需要交给 FUSE 服务接管的挂载根，父子进程共用同一份结果。
    scoped_fuse_roots: Vec<String>,
    /// 挂载状态文件路径。
    state_path: String,
    /// 挂载状态文件的临时写入路径。
    temp_state_path: String,
    /// 本次请求按规则推导出的重叠挂载点，用于清理上一轮残留。
    overlay_targets: Vec<String>,
    /// 目标进程的 mount namespace 路径，已提前转换为 C 字符串。
    mount_namespace_path: Option<CString>,
}

impl MountForkPlan {
    fn build(request: &MountRequest) -> Self {
        let state_path = state_file_path(request);
        let temp_state_path = format!("{}.tmp", state_path);
        Self {
            scoped_fuse_roots: scoped_fuse_mount_roots(request),
            state_path,
            temp_state_path,
            overlay_targets: request_overlay_targets(request),
            mount_namespace_path: CString::new(format!("/proc/{}/ns/mnt", request.pid)).ok(),
        }
    }
}

fn run_mount_in_forked_child(request: &MountRequest) -> bool {
    // fork 之后的子进程只保留调用线程。此时若再做堆分配、首次初始化或获取全局锁，
    // 可能因为其它父线程在 fork 瞬间持有 malloc arena 或全局锁而永久阻塞。
    // 因此把可以提前算出的字符串与路径列表全部在父进程算好，子进程只做 setns/mount/write。
    let plan = MountForkPlan::build(request);
    let parent_timeout_sec = PARENT_RECV_TIMEOUT_SEC
        .saturating_add(FUSE_READY_TIMEOUT_SEC.saturating_mul(plan.scoped_fuse_roots.len() as i64));
    let mut sockets = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, sockets.as_mut_ptr()) } != 0 {
        log_errno("daemon socketpair failed");
        return false;
    }

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    let child = unsafe { libc::fork() };
    if child < 0 {
        log_errno("daemon fork failed");
        unsafe {
            close(sockets[0]);
            close(sockets[1]);
        }
        return false;
    }

    if child > 0 {
        unsafe { close(sockets[1]) };
        return handle_parent_process(child, sockets[0], parent_timeout_sec, &request.package_name);
    }

    unsafe { close(sockets[0]) };
    let ok = handle_child_process(request, &plan, sockets[1]);
    unsafe { libc::_exit(if ok { 0 } else { 1 }) };
}

fn handle_child_process(request: &MountRequest, plan: &MountForkPlan, sock: c_int) -> bool {
    if !set_mount_namespace(plan.mount_namespace_path.as_deref()) {
        let _ = send_mount_result(sock, -1);
        unsafe { close(sock) };
        return false;
    }

    let cleanup = clear_previous_mounts(request, plan);
    if !cleanup.is_cleared() {
        log::warn!(
            "daemon mount cleanup incomplete pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }
    // 上一轮挂载没有被确认清除时不再继续注入。
    //
    // 继续挂载只会在同一个挂载点上再叠一层：应用最终看到的是最顶层那份，而底下的死挂载
    // 仍然占用着挂载表，后续每一轮恢复都会让栈更高。这里把"摘除未验证"累计到账本，达到
    // 预算后显式拒绝注入，把问题暴露成持续可观测的状态，而不是让它无限叠加。
    if request.operation == MountOperation::Reload
        && !cleanup.is_cleared()
        && let Some(mut ledger) = mount_identity::load(&request.package_name, request.pid)
    {
        let poisoned = ledger.record_detach_failure();
        let attempts = ledger.detach_attempts;
        let _ = mount_identity::save(&ledger);
        if poisoned {
            fuse_supervisor::record_action(
                RecoveryAction::RefusePoisoned,
                EndpointHealth::Unprobed(0),
            );
            log::error!(
                "daemon mount refused reason=detach_not_verified attempts={} pid={} pkg={}",
                attempts,
                request.pid,
                request.package_name
            );
            let _ = send_mount_result(sock, -1);
            // SAFETY: sock 来自本进程已连接的 socketpair，且此处是唯一关闭路径。
            unsafe { close(sock) };
            return false;
        }
    }
    clear_previous_allowed_real_backend_mounts(request);

    if request.operation == MountOperation::Disable {
        // 已确认清除时才丢弃账本：挂载明细已随卸载消失，保留它只会让后续监督把
        // "目标上没有本模块挂载"误判成需要重新注入的 Detached 状态。
        if cleanup.is_cleared() {
            let _ = mount_identity::remove(&request.package_name, request.pid);
        }
        let _ = send_mount_result(sock, if cleanup.is_cleared() { 0 } else { -1 });
        // SAFETY: sock 来自本进程已连接的 socketpair，且此处是唯一关闭路径。
        unsafe { close(sock) };
        return cleanup.is_cleared();
    }

    let mut planner = MountPlanner::new(
        &request.package_name,
        request.uid,
        &request.app_data_dir,
        &request.redirect_target,
        false,
    );
    planner.set_file_monitor_enabled(request.is_file_monitor_enabled);
    let scoped_fuse_roots = plan.scoped_fuse_roots.as_slice();
    let ok = if request.is_mapping_mode_only {
        planner.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            scoped_fuse_roots,
        )
    } else {
        planner.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            scoped_fuse_roots,
        )
    };
    if ok {
        let fuse_roots = scoped_fuse_roots;
        log_scoped_mount_roots(
            "daemon hybrid fuse",
            &request.package_name,
            request.pid,
            fuse_roots,
        );
        // 闸门判定与规划同源（`scoped_fuse_mount_roots_for_request` 内部用的是同一个函数）；
        // 这里取出来只为让 selection_reason 如实区分「能力未放行」与「规则本来不需要 FUSE 根」。
        let gate_allowed = crate::fuse_redirect::config::scoped_mount_allowed_for_scope(
            &request.package_name,
            request.storage_backend_mode,
        );
        let (fuse_children, attempt) = if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, fuse_roots, planner.real_storage_anchor()) {
                Some(children) => (children, ScopedMountAttempt::Ready),
                None => (Vec::new(), ScopedMountAttempt::Failed),
            }
        } else if gate_allowed {
            (Vec::new(), ScopedMountAttempt::NoRootsNeeded)
        } else {
            (Vec::new(), ScopedMountAttempt::GateBlocked)
        };
        let outcome = conclude_scoped_mount(ScopedMountReport {
            package_name: &request.package_name,
            pid: request.pid,
            requested_mode: request.storage_backend_mode,
            log_prefix: "daemon hybrid fuse",
            roots_planned: fuse_roots.len(),
            sessions: fuse_children.len(),
            attempt,
            ready_reason: "scoped_mount_ready",
            failed_reason: "scoped_mount_failed",
        });
        if outcome.needs_namespace_fallback {
            log::warn!(
                "daemon hybrid fuse no scoped service mounted, fallback to mount namespace pid={} pkg={}",
                request.pid,
                request.package_name
            );
            if !apply_mount_namespace_fallback(&mut planner, request) {
                log::warn!(
                    "daemon hybrid fuse namespace fallback failed pid={} pkg={}",
                    request.pid,
                    request.package_name
                );
            }
        }
        let mounted_targets = planner.take_mounted_targets();
        // 仍在应用命名空间内，就地核对本轮目标的可见性，给热重载类问题留下应用视角证据。
        log_mounted_target_view(&mounted_targets, request);
        if !write_mount_state(request, plan, &mounted_targets, &fuse_children) {
            log::warn!("daemon mount state save failed pid={}", request.pid);
        }
        record_mount_identity(request, &mounted_targets, &fuse_children);
        let _ = send_mount_result(sock, 0);
        unsafe { close(sock) };
        return true;
    }

    let _ = send_mount_result(sock, -1);
    unsafe { close(sock) };
    false
}

fn apply_mount_namespace_fallback(planner: &mut MountPlanner, request: &MountRequest) -> bool {
    // Scoped FUSE 是优先采用的可记录只读路径。当已挂载的真实存储 FUSE 锚点
    // 能覆盖只读映射时，保留文件监视，使 MediaProvider/FUSE 仍可生成拒绝记录。
    // 否则使用强制只读绑定，避免写入被静默放行。
    // 主方案已经装好的 bind/overlay 必须先卸载。降级路径会对同一批目标重新执行挂载，
    // 若保留旧挂载会在同一目标上再叠一层，导致挂载栈重复、卸载顺序错乱。
    // 配置热重载触发的降级会走到这里，因此这一步不能省。
    let detached = planner.unmount_recorded_targets();
    if detached > 0 {
        log::info!(
            "daemon hybrid fuse namespace fallback rollback count={} pid={} pkg={}",
            detached,
            request.pid,
            request.package_name
        );
    }
    let allowed_real_paths = crate::fuse_redirect::config::expand_namespace_fallback_rules(
        request.uid,
        &request.allowed_real_paths,
    );
    let read_only_paths = crate::fuse_redirect::config::expand_namespace_fallback_rules(
        request.uid,
        &request.read_only_paths,
    );
    let can_record_fallback = request.is_file_monitor_enabled
        && planner.can_record_read_only_mapping_denials(
            &request.path_mappings,
            &read_only_paths,
            &request.excluded_real_paths,
        );
    planner.set_file_monitor_enabled(can_record_fallback);
    log::info!(
        "daemon hybrid fuse namespace fallback file_monitor={} pid={} pkg={}",
        can_record_fallback,
        request.pid,
        request.package_name
    );
    if request.is_mapping_mode_only {
        planner.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &read_only_paths,
            &[],
        )
    } else {
        planner.apply_sdcard_redirect(
            &allowed_real_paths,
            &request.excluded_real_paths,
            &read_only_paths,
            &request.path_mappings,
            &[],
        )
    }
}

/// 按根启动 scoped FUSE 服务；单根失败只丢弃该根。
///
/// 此前任一根启动失败都会调用 [`rollback_scoped_fuse_services`] 卸载所有已成功的会话
/// 并返回 `None`，一次偶发的单根失败就让整个应用失去 FUSE 覆盖；调用方还会把这次失败
/// 计入全局能力预算，累计到上限后把所有应用一起打回 mount namespace。改为按根隔离后，
/// 失败根对应路径回落到 mount namespace 的 bind 分支，其余根继续由 FUSE 覆盖，影响面
/// 收敛到单条规则。
///
/// 只有全部根都失败时才返回 `None`，保持原有的"FUSE 整体不可用"记账语义，
/// 避免真正不可用的设备仍被反复重试。
fn start_scoped_fuse_services(
    request: &MountRequest,
    roots: &[String],
    real_root_override: Option<String>,
) -> Option<Vec<FuseMountState>> {
    if roots.is_empty() {
        return Some(Vec::new());
    }

    let mut states = Vec::with_capacity(roots.len());
    let mut failed_roots: Vec<&str> = Vec::new();
    for root in roots {
        match start_fuse_service_for_root(request, root, real_root_override.clone()) {
            Some(state) => states.push(state),
            None => failed_roots.push(root.as_str()),
        }
    }

    if !failed_roots.is_empty() {
        log::warn!(
            "daemon fuse partial scoped mount pkg={} pid={} mounted={} failed={} failed_roots={}",
            request.package_name,
            request.pid,
            states.len(),
            failed_roots.len(),
            failed_roots.join(",")
        );
        // 规划阶段已跳过这些预期 FUSE 根对应的 bind；部分失败时先收回已启动会话，
        // 再以单个存储根会话保留原始规则的动态匹配，避免失败根变成无规则覆盖。
        let user_id = crate::platform::user_id_from_uid(request.uid);
        let storage_root = paths::storage_user_root_for_user(user_id);
        rollback_scoped_fuse_services(&states);
        if let Some(state) = start_fuse_service_for_root(request, &storage_root, real_root_override)
        {
            log::warn!(
                "daemon fuse partial roots collapsed to storage root pkg={} pid={} failed={}",
                request.package_name,
                request.pid,
                failed_roots.len()
            );
            return Some(vec![state]);
        }
        log::warn!(
            "daemon fuse partial roots and storage-root retry failed pkg={} pid={}",
            request.package_name,
            request.pid
        );
        return None;
    }

    if states.is_empty() {
        return None;
    }
    Some(states)
}

/// 批量启动部分失败时回滚已成功的 FUSE 服务。
///
/// 已成功的服务此时已经完成 FUSE mount，只终止子进程会把挂载点留在目标 mount
/// namespace 里变成死挂载，后续访问返回 ENOTCONN 且没有任何路径会再清理它。
/// 因此必须按启动的逆序先卸载挂载点，再终止对应子进程。
fn rollback_scoped_fuse_services(states: &[FuseMountState]) {
    for state in states.iter().rev() {
        if let Ok(c_target) = CString::new(state.target.as_str()) {
            // SAFETY: c_target 是以 NUL 结尾的合法路径，且在本次调用期间保持存活。
            if unsafe { umount2(c_target.as_ptr(), MNT_DETACH) } != 0 {
                let errno = last_errno();
                if errno != libc::EINVAL && errno != libc::ENOENT {
                    log::warn!(
                        "daemon fuse rollback umount failed target={} errno={} {}",
                        state.target,
                        errno,
                        errno_text(errno)
                    );
                }
            }
        }
        terminate_fuse_child(
            state.child,
            (state.child_start_time_ticks != 0).then_some(state.child_start_time_ticks),
        );
    }
}

fn scoped_fuse_mount_roots(request: &MountRequest) -> Vec<String> {
    crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request)
}

fn start_fuse_service_for_root(
    request: &MountRequest,
    mount_root: &str,
    real_root_override: Option<String>,
) -> Option<FuseMountState> {
    let mut ready_sockets = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        log_errno("daemon fuse ready socketpair failed");
        return None;
    }

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    let service_child = unsafe { libc::fork() };
    if service_child < 0 {
        log_errno("daemon fuse fork failed");
        unsafe {
            close(ready_sockets[0]);
            close(ready_sockets[1]);
        }
        return None;
    }

    if service_child == 0 {
        unsafe {
            close(ready_sockets[0]);
            let name = b"srx_fuse\0";
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        }
        let ok = mount_blocking_with_ready(
            fuse_config_from_request(request, Some(mount_root.to_string()), real_root_override),
            Some(ready_sockets[1]),
        );
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    unsafe { close(ready_sockets[1]) };
    set_recv_timeout(ready_sockets[0], FUSE_READY_TIMEOUT_SEC);
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    unsafe { close(ready_sockets[0]) };
    if n != expected || ready_result != 0 {
        log::warn!(
            "daemon fuse service not ready child={} recv={} ret={} pid={} pkg={}",
            service_child,
            n,
            ready_result,
            request.pid,
            request.package_name
        );
        terminate_fuse_child(service_child, None);
        return None;
    }

    let Some(child_start_time_ticks) = crate::platform::process_start_time_ticks(service_child)
    else {
        rollback_scoped_fuse_services(&[FuseMountState {
            target: mount_root.to_string(),
            child: service_child,
            child_start_time_ticks: 0,
        }]);
        return None;
    };
    Some(FuseMountState {
        target: mount_root.to_string(),
        child: service_child,
        child_start_time_ticks,
    })
}

fn set_mount_namespace(ns_path: Option<&CStr>) -> bool {
    // 路径在 fork 之前就已经转换好，这里只做 open/setns，避免子进程再次堆分配。
    let Some(c_path) = ns_path else {
        return false;
    };
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        log_errno("daemon ns open failed");
        return false;
    }
    let file = UniqueFd::new(fd);
    if unsafe { setns(file.get(), CLONE_NEWNS) } != 0 {
        log_errno("daemon setns failed");
        return false;
    }
    true
}

fn handle_parent_process(
    child: i32,
    sock: c_int,
    primary_timeout_sec: i64,
    package_name: &str,
) -> bool {
    set_recv_timeout(sock, primary_timeout_sec);
    let mut result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let mut n = recv_result(sock, &mut result);
    let mut should_reap_nonblocking = false;
    if n != expected {
        log_child_diagnostics(child, "primary_timeout");
        let _ = unsafe { libc::kill(child, SIGTERM) };
        set_recv_timeout(sock, PARENT_RECV_GRACE_TIMEOUT_SEC);
        n = recv_result(sock, &mut result);
        if n != expected {
            log_child_diagnostics(child, "grace_timeout");
            should_reap_nonblocking = true;
            let _ = unsafe { libc::kill(child, SIGKILL) };
        }
    }
    unsafe { close(sock) };
    if !reap_child(child, should_reap_nonblocking) {
        remember_stuck_mount_child(child, package_name);
    }
    result == 0
}

fn reap_child(child: i32, nonblocking: bool) -> bool {
    let mut status = 0;
    let options = if nonblocking { WNOHANG } else { 0 };
    let attempts = if nonblocking { 20 } else { 1 };
    for attempt in 0..attempts {
        let ret = unsafe { waitpid(child, &mut status, options) };
        if ret < 0 {
            log_errno("daemon waitpid failed");
            return true;
        }
        if ret > 0 {
            return true;
        }
        if !nonblocking {
            break;
        }
        if attempt + 1 < attempts {
            unsafe { libc::usleep(10 * 1000) };
        }
    }
    log::warn!("daemon child not reaped child={}", child);
    false
}

fn log_child_diagnostics(child: i32, phase: &str) {
    let wchan = read_proc_text(&format!("/proc/{}/wchan", child))
        .unwrap_or_else(|| "<unavailable>".to_string());
    let status_summary = read_proc_status_summary(&format!("/proc/{}/status", child))
        .unwrap_or_else(|| "<unavailable>".to_string());
    let stack = read_proc_text(&format!("/proc/{}/stack", child))
        .unwrap_or_else(|| "<unavailable>".to_string());

    log::warn!(
        "daemon child stuck child={} phase={} wchan={} status={}",
        child,
        phase,
        wchan.trim(),
        status_summary
    );
    let stack_trimmed = stack.trim();
    if !stack_trimmed.is_empty() && stack_trimmed != "<unavailable>" {
        log::warn!(
            "daemon child stuck child={} phase={} stack:\n{}",
            child,
            phase,
            stack_trimmed
        );
    }
}

fn read_proc_text(path: &str) -> Option<String> {
    let Ok(c_path) = CString::new(path) else {
        return None;
    };
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let file = UniqueFd::new(fd);
    let mut text = String::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(file.get(), buf.as_mut_ptr() as *mut c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        let Ok(s) = std::str::from_utf8(&buf[..n as usize]) else {
            break;
        };
        text.push_str(s);
        if text.len() >= 8192 {
            break;
        }
    }
    Some(text)
}

fn read_proc_status_summary(path: &str) -> Option<String> {
    let raw = read_proc_text(path)?;
    let mut name = String::from("?");
    let mut state = String::from("?");
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("Name:") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("State:") {
            state = rest.trim().to_string();
        }
    }
    Some(format!("name={} state={}", name, state))
}

fn set_recv_timeout(sock: c_int, seconds: i64) {
    let tv = libc::timeval {
        tv_sec: seconds,
        tv_usec: 0,
    };
    let _ = unsafe {
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &tv as *const _ as *const c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        )
    };
}

fn recv_result(sock: c_int, result: &mut i32) -> isize {
    unsafe {
        recv(
            sock,
            result as *mut _ as *mut c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    }
}

fn send_mount_result(sock: c_int, result: i32) -> bool {
    unsafe {
        send(
            sock,
            &result as *const _ as *const c_void,
            std::mem::size_of::<i32>(),
            0,
        ) == std::mem::size_of::<i32>() as isize
    }
}

/// 上一轮挂载的清理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClearOutcome {
    /// 已确认目标上不再有本模块的挂载层，可以安全注入。
    Cleared,
    /// 存在无法确认清除的残留：可能是其它组件的挂载，或卸载被内核拒绝。
    Unverified,
}

impl ClearOutcome {
    fn is_cleared(&self) -> bool {
        matches!(self, Self::Cleared)
    }
}

/// 清理上一轮挂载。
///
/// 状态文件里的目标路径上可能压着本模块的挂载，也可能已被其它组件（例如系统
/// MediaProvider 的 FUSE）或本模块的新会话接管。这里先按账本校验归属再摘除，只有确认
/// 目标上没有本模块的挂载层时才返回 [`ClearOutcome::Cleared`]。
///
/// 状态文件删除失败只记告警、不影响结论：挂载层已经确认清除，残留的状态文件只会让下一轮
/// 多做一次已幂等的归属校验，不会造成叠加。
fn clear_previous_mounts(request: &MountRequest, plan: &MountForkPlan) -> ClearOutcome {
    let state_path = plan.state_path.as_str();
    let fuse_children = read_fuse_children(state_path);
    let mut targets = read_mount_targets(state_path);
    targets.extend(plan.overlay_targets.iter().cloned());
    let targets = module_paths::normalize_mount_targets(&targets);
    if targets.is_empty() && fuse_children.is_empty() {
        return if std::fs::remove_file(state_path).is_ok() || std::fs::metadata(state_path).is_err()
        {
            ClearOutcome::Cleared
        } else {
            ClearOutcome::Unverified
        };
    }
    let ledger = mount_identity::load(&request.package_name, request.pid);
    let mut outcome = ClearOutcome::Cleared;
    // 目标已经按深度降序排列；先摘子挂载，避免父层摘除后查询到另一个视图。
    for target in &targets {
        // 判定必须在摘除本目标之前完成：摘完之后"最上层"就只剩底层视图，
        // 再也分不出那一层是本模块的沙箱根还是平台自己挂的。
        let live = mount_identity::topmost_live_mount(0, target);
        let live_layer = live
            .as_ref()
            .map(|mount| (mount.source.as_str(), mount.root.as_str()));
        if should_keep_reload_redirect_root(target, request, &plan.scoped_fuse_roots, live_layer) {
            log::info!(
                "daemon unmount keep redirect root target={} mount_id={}",
                target,
                live.as_ref().map_or(0, |mount| mount.mount_id)
            );
            continue;
        }
        if !clear_mount_target_stack_verified(target, ledger.as_ref()) {
            outcome = ClearOutcome::Unverified;
        }
    }
    for child in &fuse_children {
        if !terminate_recorded_fuse_child(child) {
            outcome = ClearOutcome::Unverified;
        }
    }
    if outcome.is_cleared()
        && std::fs::remove_file(state_path).is_err()
        && std::fs::metadata(state_path).is_ok()
    {
        log::warn!(
            "daemon mount state file removal failed path={} errno={}",
            state_path,
            last_errno()
        );
    }
    outcome
}

/// 热重载时是否保留该入口上已有的重定向根绑定。
///
/// 存储视图根（`/storage/emulated/0`、`/mnt/user/0/emulated/0` 等）的 bind 落在系统 FUSE
/// 的挂载点上：摘掉再重挂会在同一路径上换取另一个 dentry，而重载自身产生的 mount/umount
/// 事件会让 MediaProvider 失效它缓存的那个 dentry（典型的 `mountinfo` 里挂着、应用行走却
/// 落回真实存储），于是热重载后应用读到的是公共存储，沙箱里才存在的目标路径直接 ENOENT。
///
/// 判据刻意收得很窄：只在「本次仍是重定向、后端没有换成需要 scoped FUSE 根、该入口当前
/// 最上层确是本应用沙箱根、且沙箱根与本次重定向目标一致」时才保留；其余情况（改沙箱
/// 目标、改后端、非模块层、本层不在场）一律按原逻辑摘除重建。
///
/// `live_layer` 是该入口当前最上层挂载的 `(source, root)`；`None` 表示该入口上没有活动挂载。
fn should_keep_reload_redirect_root(
    target: &str,
    request: &MountRequest,
    scoped_fuse_roots: &[String],
    live_layer: Option<(&str, &str)>,
) -> bool {
    // 仅映射模式不走重定向根保留：从默认重定向切到仅映射模式时，旧沙箱根绑定必须按原逻辑
    // 摘除重建，否则改了挂载方式却仍保留旧重定向根，应用读到的是上一轮重定向留下的错误视图。
    if request.is_mapping_mode_only {
        return false;
    }
    if request.operation != MountOperation::Reload || request.redirect_target.is_empty() {
        return false;
    }
    if scoped_fuse_roots
        .iter()
        .any(|root| paths::is_same_or_child(target, root))
    {
        return false;
    }
    let Some((source, root)) = live_layer else {
        return false;
    };
    if !mount_identity::is_module_redirect_mount(source, root, target, &request.package_name) {
        return false;
    }
    sandbox_root_matches_redirect_target(root, &request.redirect_target)
}

/// `root`（mountinfo 的 root 字段，`/media/<user>/...` 或 `/<user>/...` 两种等价写法）
/// 是否是 `redirect_target`（`/storage/emulated/<user>/...`）在存储树内的同一沙箱根。
///
/// 只比较 `/Android/` 之后的尾部，天然兼容两种视图写法；自定义重定向目标（尾部不是
/// `Android/...` 形态）一律返回 false，让调用方退回"摘了重建"的安全路径。
fn sandbox_root_matches_redirect_target(root: &str, redirect_target: &str) -> bool {
    let Some((_, tail)) = redirect_target.split_once("/Android/") else {
        return false;
    };
    !tail.is_empty() && root.ends_with(&format!("/Android/{tail}"))
}

/// 清理允许真实目录在 `/data/media` 下遗留的系统 FUSE 子挂载。
///
/// 这类路径是本次请求从 `/storage` 配置推导出的后端目标，不写入状态文件，
/// 因而不能交给通用的外部路径过滤。旧的 FUSE 子挂载即使仍出现在 mountinfo，
/// 访问也可能返回 ENOTCONN；先在应用私有 namespace 中摘除它，后续挂载流程
/// 才能重新绑定可用的后端目录。
fn clear_previous_allowed_real_backend_mounts(request: &MountRequest) {
    for target in allowed_real_backend_targets(request) {
        if current_mount_target_count(&target) == 0 {
            continue;
        }
        if clear_mount_target_stack(&target) {
            log::info!("daemon cleared allowed real backend target={}", target);
        }
    }
}

fn allowed_real_backend_targets(request: &MountRequest) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let data_media_root = paths::data_media_user_root_for_user(user_id);
    let mut targets = Vec::new();

    for raw_path in &request.allowed_real_paths {
        let Some(resolved) =
            resolve_request_storage_path(request, raw_path, user_id, &storage_root)
        else {
            continue;
        };
        let Some(backend) = paths::storage_to_data_media_for_user(&resolved, user_id) else {
            continue;
        };
        if backend == data_media_root || targets.iter().any(|target| target == &backend) {
            continue;
        }
        targets.push(backend);
    }

    targets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| b.cmp(a)));
    targets
}

fn clear_mount_target_stack(target: &str) -> bool {
    let mut passes = 0usize;

    loop {
        let mounted_count = current_mount_target_count(target);
        if is_mount_stack_cleared(mounted_count) {
            if passes > 1 {
                log::info!(
                    "daemon unmount stack cleared target={} passes={}",
                    target,
                    passes
                );
            }
            return true;
        }
        if passes >= MAX_UNMOUNT_PASSES_PER_TARGET {
            log::warn!(
                "daemon unmount stack exceeded target={} remaining={}",
                target,
                mounted_count
            );
            return false;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        // SAFETY: c_target 是以 NUL 结尾的合法路径且在本次调用期间保持存活；MNT_DETACH
        // 只影响当前命名空间的挂载视图，不触碰其它命名空间。
        if unsafe { umount2(c_target.as_ptr(), MNT_DETACH) } == 0 {
            passes += 1;
            // 单次成功摘除此前是静默的，只有摘掉多层的目标才留痕。热重载反复重挂时，
            // "本模块自己的层被后一次请求摘掉"只能靠这条记录才看得出来。
            log::info!(
                "daemon unmount ok target={} remaining_before={} pass={}",
                target,
                mounted_count,
                passes
            );
            continue;
        }

        let errno = last_errno();
        if errno == libc::EINVAL || errno == libc::ENOENT {
            return true;
        }

        log::warn!(
            "daemon unmount failed target={} pass={} remaining={} errno={} {}",
            target,
            passes + 1,
            mounted_count,
            errno,
            errno_text(errno)
        );
        return false;
    }
}

/// 摘除目标上的本模块挂载层，摘除前先校验归属。
///
/// 与 [`clear_mount_target_stack`] 的区别在于**摘除范围**：后者用于按配置推导出的后端
/// 目标（`/data/media` 下可能残留系统 MediaProvider 的 FUSE 子挂载，必须摘掉才能重新
/// 绑定可用后端），本函数只用于状态文件里记录的、本模块自己挂上去的目标。
///
/// 归属判据由 [`mount_identity::is_module_redirect_mount`] 给出，它同时看挂载源与 `root`：
/// 只按挂载源判断是不够的——bind 会继承底层文件系统的源，真机上本模块每一层的 source 都是
/// MediaProvider FUSE 的 `/dev/fuse`，因此**每一层都会被误判成外部挂载而拒绝摘除**，重挂
/// 只能在旧层之上叠加（真机表现为同一路径 4 层、应用读到被压在最上面的沙箱层）。
///
/// 覆盖范围必须限于本模块自己挂的目标，不能扩到 `/data/media` 后端目标：那里的层虽然可能
/// 也带沙箱 `root`，但摘除后要按 MediaProvider 语义重建，属 [`clear_mount_target_stack`] 的
/// 职责。这里的 `target` 全部来自状态文件的 `target=` 记录，即本模块挂过的路径。
///
/// 返回 true 表示该目标上已确认没有本模块的挂载层。
fn clear_mount_target_stack_verified(target: &str, ledger: Option<&MountLedger>) -> bool {
    let mut passes = 0usize;
    let normalized_target = paths::normalize_syntax(target);

    loop {
        let Some(live) = mount_identity::topmost_live_mount(0, target) else {
            if passes > 1 {
                log::info!(
                    "daemon unmount stack cleared target={} passes={}",
                    target,
                    passes
                );
            }
            return true;
        };
        if !mount_identity::is_module_redirect_mount(
            &live.source,
            &live.root,
            target,
            ledger.map_or("", |ledger| ledger.package_name.as_str()),
        ) {
            log::warn!(
                "daemon unmount skipped foreign mount target={} mount_id={} source={} root={} fs={}",
                target,
                live.mount_id,
                live.source,
                live.root,
                live.fs_type
            );
            return false;
        }
        if let Some(ledger) = ledger {
            // 只拦「比账本记录更新的一层」：那说明本模块的新会话已经接管这个挂载点，
            // 摘掉它会把正在服务的挂载打掉。比记录更旧的层是本模块上一轮留下的残留，
            // 必须一并摘净——否则每轮只能摘掉最上面一层，重挂又会补上一层，栈高永不下降。
            let recorded_mount_id = ledger
                .mounts
                .iter()
                .filter(|mount| mount.mount_point == normalized_target)
                .map(|mount| mount.mount_id)
                .max();
            if recorded_mount_id.is_some_and(|recorded| live.mount_id > recorded) {
                log::warn!(
                    "daemon unmount skipped superseded mount target={} mount_id={} recorded={} ledger_generation={}",
                    target,
                    live.mount_id,
                    recorded_mount_id.unwrap_or_default(),
                    ledger.generation
                );
                return false;
            }
        }
        if passes >= MAX_UNMOUNT_PASSES_PER_TARGET {
            log::warn!(
                "daemon unmount stack exceeded target={} mount_id={}",
                target,
                live.mount_id
            );
            return false;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        // SAFETY: c_target 是以 NUL 结尾的合法路径且在本次调用期间保持存活；MNT_DETACH
        // 只影响当前命名空间的挂载视图，不触碰其它命名空间。
        if unsafe { umount2(c_target.as_ptr(), MNT_DETACH) } == 0 {
            passes += 1;
            // 单次成功摘除此前是静默的，只有摘掉多层的目标才留痕。热重载反复重挂时，
            // "本模块自己的层被后一次请求摘掉"只能靠这条记录才看得出来。
            log::info!(
                "daemon unmount ok target={} mount_id={} pass={}",
                target,
                live.mount_id,
                passes
            );
            continue;
        }

        let errno = last_errno();
        if errno == libc::EINVAL || errno == libc::ENOENT {
            return true;
        }

        log::warn!(
            "daemon unmount failed target={} pass={} mount_id={} errno={} {}",
            target,
            passes + 1,
            live.mount_id,
            errno,
            errno_text(errno)
        );
        return false;
    }
}

fn is_mount_stack_cleared(mounted_count: usize) -> bool {
    mounted_count == 0
}

fn current_mount_target_count(target: &str) -> usize {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|content| mount_target_count_from_mountinfo(&content, target, "", &[]))
        .unwrap_or(0)
}

fn request_overlay_targets(request: &MountRequest) -> Vec<String> {
    if request.uid < 0 {
        return Vec::new();
    }
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut targets = Vec::new();

    for raw_path in request
        .allowed_real_paths
        .iter()
        .chain(request.excluded_real_paths.iter())
        .chain(request.sandboxed_paths.iter())
    {
        append_resolved_storage_alias_targets(
            &mut targets,
            request,
            raw_path,
            user_id,
            &storage_root,
        );
    }

    let (read_only_includes, _) = paths::split_exclusion_rules(&request.read_only_paths);
    for raw_path in &read_only_includes {
        append_resolved_storage_alias_targets(
            &mut targets,
            request,
            raw_path,
            user_id,
            &storage_root,
        );
    }

    for mapping in &request.path_mappings {
        append_resolved_mapping_request_targets(
            &mut targets,
            request,
            &mapping.request_path,
            user_id,
            &storage_root,
        );
    }

    module_paths::normalize_mount_targets(&targets)
}

fn append_resolved_mapping_request_targets(
    targets: &mut Vec<String>,
    request: &MountRequest,
    raw_path: &str,
    user_id: i32,
    storage_root: &str,
) {
    let resolved = paths::resolve_user_path(
        &paths::resolve_placeholders(
            &paths::normalize(raw_path),
            &request.app_data_dir,
            &request.redirect_target,
        ),
        user_id,
    );
    if resolved.is_empty()
        || paths::has_unsafe_segments(&resolved)
        || resolved == "/"
        || paths::is_application_private_root(&resolved)
    {
        return;
    }
    if paths::is_same_or_child(&resolved, storage_root) {
        targets.extend(expand_storage_alias_paths_for_user(&resolved, user_id));
    } else if paths::is_absolute(&resolved) {
        targets.push(resolved);
    }
}

fn append_resolved_storage_alias_targets(
    targets: &mut Vec<String>,
    request: &MountRequest,
    raw_path: &str,
    user_id: i32,
    storage_root: &str,
) {
    let Some(resolved) = resolve_request_storage_path(request, raw_path, user_id, storage_root)
    else {
        return;
    };
    // 上层 request_overlay_targets 统一交给 normalize_targets 过滤、排序并去重，
    // 这里无需再对每个目标做一次线性查重扫描。
    targets.extend(expand_storage_alias_paths_for_user(&resolved, user_id));
}

fn resolve_request_storage_path(
    request: &MountRequest,
    raw_path: &str,
    user_id: i32,
    storage_root: &str,
) -> Option<String> {
    let mut resolved =
        paths::resolve_placeholders(raw_path, &request.app_data_dir, &request.redirect_target);
    resolved = paths::resolve_user_path(&paths::normalize(&resolved), user_id);
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if resolved.is_empty()
        || paths::has_unsafe_segments(&resolved)
        || paths::eq_ignore_case(&resolved, storage_root)
        || !paths::is_child(&resolved, storage_root)
    {
        return None;
    }
    Some(resolved)
}

fn expand_storage_alias_paths_for_user(canonical_path: &str, user_id: i32) -> Vec<String> {
    let storage_root = paths::storage_user_root_for_user(user_id);
    if !paths::is_same_or_child(canonical_path, &storage_root) {
        return vec![canonical_path.to_string()];
    }

    let suffix = &canonical_path[storage_root.len()..];
    // 这里的别名根都是按固定规则构造的互不相同的字面量，无需再逐个线性去重；
    // 最终的过滤、排序与去重统一由 normalize_targets 完成。
    paths::storage_alias_roots_for_user(user_id)
        .into_iter()
        .map(|root| format!("{}{}", root, suffix))
        .collect()
}

#[derive(Clone)]
struct FuseMountState {
    target: String,
    child: i32,
    child_start_time_ticks: u64,
}

fn fuse_config_from_request(
    request: &MountRequest,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    crate::fuse_redirect::fuse_config_from_request(request, mount_root, real_root_override)
}

fn write_mount_state(
    request: &MountRequest,
    plan: &MountForkPlan,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    if std::fs::create_dir_all(module_paths::MOUNT_STATE_DIR).is_err() {
        log::warn!(
            "daemon mount state mkdir failed dir={}",
            module_paths::MOUNT_STATE_DIR
        );
        return false;
    }
    // 路径在 fork 之前已由 MountForkPlan 算好，直接复用，避免子进程堆分配。
    let state_path = plan.state_path.as_str();
    let temp_path = plan.temp_state_path.as_str();
    let Ok(c_temp_path) = CString::new(temp_path) else {
        return false;
    };
    let mut content = String::new();
    content.push_str(&format!("version={}\n", request.config_version));
    // 配置指纹是跨进程可比的判据，`version=` 不是（两侧计数器不同域）。见
    // `SettingsHub::config_fingerprint` 的说明；reconcile 靠它判断这份挂载是否已按当前配置建立。
    content.push_str(&format!(
        "fingerprint={}\n",
        crate::config::SettingsHub::instance().config_fingerprint()
    ));
    content.push_str(&format!("package={}\n", request.package_name));
    content.push_str(&format!("uid={}\n", request.uid));
    if let Some(start_time_ticks) = crate::platform::process_start_time_ticks(request.pid) {
        content.push_str(&format!("app_start_time={}\n", start_time_ticks));
    }
    for state in fuse_children {
        content.push_str(&format!(
            "fuse_child={}:{}\n",
            state.child, state.child_start_time_ticks
        ));
    }
    let mut all_targets = targets.to_vec();
    all_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
    for target in module_paths::normalize_mount_targets(&all_targets) {
        content.push_str("target=");
        content.push_str(&target);
        content.push('\n');
    }
    // 先写临时文件并 fsync，再原子 rename 覆盖正式文件。
    // 这样即使中途崩溃或断电，也只会残留临时文件，正式挂载清单仍是上一轮的完整内容，
    // 避免 clear_previous_mounts 因为读到空文件而永久漏卸挂载点。
    // SAFETY: c_temp_path 在调用期间保持存活，且是以 NUL 结尾的合法路径。
    let fd = unsafe {
        open(
            c_temp_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        log::warn!(
            "daemon mount state open failed path={} errno={} {}",
            temp_path,
            last_errno(),
            errno_text(last_errno())
        );
        return false;
    }
    let mut ok = fs::write_all(fd, content.as_bytes());
    // SAFETY: fd 为本函数打开且尚未关闭的有效描述符。
    if ok && unsafe { libc::fsync(fd) } != 0 {
        log::warn!(
            "daemon mount state fsync failed path={} errno={} {}",
            temp_path,
            last_errno(),
            errno_text(last_errno())
        );
        ok = false;
    }
    // SAFETY: 同上，关闭与改权限使用的都是本函数持有的 fd 与存活字符串。
    unsafe {
        libc::close(fd);
        let _ = libc::chmod(c_temp_path.as_ptr(), 0o600);
    }
    if ok {
        ok = std::fs::rename(temp_path, state_path).is_ok();
        if !ok {
            log::warn!(
                "daemon mount state rename failed temp={} path={}",
                temp_path,
                state_path
            );
        }
    }
    if ok {
        log::info!(
            "daemon mount state saved pid={} targets={} path={}",
            request.pid,
            targets.len(),
            state_path
        );
    } else {
        let _ = std::fs::remove_file(temp_path);
    }
    ok
}

fn state_file_path(request: &MountRequest) -> String {
    format!(
        "{}/{}_{}.state",
        module_paths::MOUNT_STATE_DIR,
        module_paths::sanitize_name(&request.package_name),
        request.pid
    )
}

/// 登记本次挂载的身份，供后续恢复流程判断挂载归属。
///
/// 必须在挂载成功之后、且在本进程已经 `setns` 到目标命名空间的前提下调用：`mount_id`
/// 只有在挂载真正生效后才会出现在挂载表里，而命名空间身份取自本进程所在的 ns，正是
/// 本次挂载生效的那个命名空间。
///
/// 读不到任何归属明确的挂载时不写入旧记录：宁可让账本暂时没有挂载明细（后续摘除只受
/// 挂载源约束），也不要留下一个与实际挂载不匹配的 `mount_id`——那会让下一轮恢复把本模块
/// 自己的挂载误判成"已被新会话接管"而拒绝清理。
/// 登记本次挂载的账本。
///
/// 登记纪律（何时写、何时清空、读不到归属时怎么办）由 [`crate::mount_ledger::record_mount_identity`]
/// 统一实现，两条挂载路径共用；这里只负责把 daemon 侧的目标集合拼齐——除了规划出的挂载目标，
/// 还要带上每个 scoped FUSE 会话自己的挂载点，否则这些会话在恢复流程里没有归属判据。
fn record_mount_identity(
    request: &MountRequest,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    let mut all_targets = targets.to_vec();
    all_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
    crate::mount_ledger::record_mount_identity(
        "daemon",
        &request.package_name,
        request.pid,
        &all_targets,
    )
}

fn read_mount_targets(path: &str) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| line.strip_prefix("target="))
        .filter(|target| module_paths::is_safe_mount_target(target))
        .map(ToString::to_string)
        .collect()
}

fn mount_target_count_from_mountinfo(
    content: &str,
    target: &str,
    storage_root: &str,
    alias_roots: &[String],
) -> usize {
    let canonical_target = canonical_health_target(target, storage_root, alias_roots);
    content
        .lines()
        .filter_map(mountinfo::parse_entry)
        .filter(|entry| {
            canonical_health_target(
                &mountinfo::unescape_field(entry.target),
                storage_root,
                alias_roots,
            ) == canonical_target
        })
        .count()
}

#[derive(Clone, Copy)]
struct FuseChildIdentity {
    pid: i32,
    start_time_ticks: Option<u64>,
}

fn read_fuse_children(path: &str) -> Vec<FuseChildIdentity> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let value = line.strip_prefix("fuse_child=")?;
            let (pid, start_time_ticks) = match value.split_once(':') {
                Some((pid, start)) => (pid.parse().ok()?, start.parse().ok()),
                None => (value.parse().ok()?, None),
            };
            (pid > 0).then_some(FuseChildIdentity {
                pid,
                start_time_ticks,
            })
        })
        .collect()
}

fn terminate_recorded_fuse_child(child: &FuseChildIdentity) -> bool {
    let Some(start_time_ticks) = child.start_time_ticks else {
        log::warn!(
            "daemon skip legacy fuse child signal without identity pid={}",
            child.pid
        );
        return false;
    };
    if !crate::platform::is_process_instance_alive(child.pid, start_time_ticks) {
        return true;
    }
    terminate_fuse_child(child.pid, Some(start_time_ticks));
    if crate::platform::is_process_instance_alive(child.pid, start_time_ticks) {
        // 服务进程卡在不可中断的 FUSE 请求里时 SIGKILL 也无法回收，清理会因此不完整；
        // 记下残留进程的状态，便于区分真实残留与一次性清理竞态。
        let summary = read_proc_status_summary(&format!("/proc/{}/status", child.pid))
            .unwrap_or_else(|| "state=?".to_string());
        log::warn!(
            "daemon fuse child not terminated pid={} {}",
            child.pid,
            summary
        );
        return false;
    }
    true
}

fn terminate_fuse_child(pid: i32, start_time_ticks: Option<u64>) {
    if !process_identity_alive(pid, start_time_ticks) {
        return;
    }
    if unsafe { libc::kill(pid, SIGTERM) } != 0 {
        let errno = last_errno();
        if errno != libc::ESRCH {
            log::warn!(
                "daemon fuse child term failed pid={} errno={} {}",
                pid,
                errno,
                errno_text(errno)
            );
        }
        return;
    }
    for _ in 0..30 {
        let mut status = 0;
        let ret = unsafe { waitpid(pid, &mut status, WNOHANG) };
        if ret == pid {
            return;
        }
        // `waitpid` 只能回收本进程的子进程：FUSE 服务子进程由挂载 worker fork，
        // worker 退出后会被 init 收养，此后 `waitpid` 固定返回负值（ECHILD）。
        // 把负返回值也当作「已回收」会在第一次循环就直接返回，永远走不到下面的
        // SIGKILL 升级，留下长期存活并空转的残留服务进程。因此这里以 `/proc`
        // 存活探测为准，用满整个 SIGTERM 宽限窗口后再升级信号。
        if !process_identity_alive(pid, start_time_ticks) {
            return;
        }
        unsafe { libc::usleep(10 * 1000) };
    }
    if !process_identity_alive(pid, start_time_ticks) {
        return;
    }
    let _ = unsafe { libc::kill(pid, SIGKILL) };
    let mut status = 0;
    let _ = unsafe { waitpid(pid, &mut status, WNOHANG) };
}

fn process_identity_alive(pid: i32, start_time_ticks: Option<u64>) -> bool {
    match start_time_ticks {
        Some(start) => crate::platform::is_process_instance_alive(pid, start),
        None => crate::platform::process_exists(pid),
    }
}

fn decode_wait_status(status: c_int) -> String {
    let signal = status & 0x7f;
    if signal == 0 {
        let exit_code = (status >> 8) & 0xff;
        return format!("exit={}", exit_code);
    }
    if signal == 0x7f {
        let stop_signal = (status >> 8) & 0xff;
        return format!("stop sig={}", stop_signal);
    }
    let is_core_dump = (status & 0x80) != 0;
    format!("sig={} core={}", signal, is_core_dump)
}

fn log_errno(message: &str) {
    let errno = last_errno();
    log::warn!("{} errno={} {}", message, errno, errno_text(errno));
}
