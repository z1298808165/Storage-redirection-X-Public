use crate::config::StorageBackendMode;
use crate::domain::PathMapping;
use crate::platform::errno::last as last_errno;
use crate::platform::{fs, module_paths, mountinfo, paths};
use fuser::{MountOption, SessionACL};
use std::ffi::CString;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FuseCapability {
    Unknown,
    Available,
    Unavailable,
}

/// 单个应用 scoped 挂载连续失败达到该次数后，本轮开机内该应用不再尝试 scoped 挂载。
///
/// 单次失败可能来自开机竞态、目标进程正在退出或挂载点标签尚未就绪等可恢复事件，因此用少量
/// 额外尝试换取可恢复性。预算**按应用单独计**：某个应用自身的失败只影响它自己，不会让别的
/// 应用一起退回 namespace。
const SCOPED_MOUNT_FAILURE_BUDGET: u32 = 3;

/// 升格为「设备级不支持 scoped 挂载」所需的不同应用数。
///
/// 单个应用反复失败说明不了设备能力：它可能是每次启动都被杀、或配置本身有问题的应用。只有当
/// **多个不同应用**在没有成功插入的情况下接连失败，才足以判定是设备层面的问题。这样既保留
/// 了对真实设备缺陷的快速收敛，又消除了「一个坏应用把全体拖下水」的放大器。
const DEVICE_UNAVAILABLE_MIN_SCOPES: u32 = 2;

/// 设备级退避的起点与上限（毫秒）。
///
/// `unavailable` 不再是终态：到期后允许一次探测性尝试，成功即整体恢复，失败则把退避加倍。
/// 这条自愈通路取代了过去「只能靠重启恢复」的行为。
const RETRY_BACKOFF_BASE_MS: u64 = 30_000;
const RETRY_BACKOFF_MAX_MS: u64 = 600_000;

/// 快照中记录的 scope 数量上限。
///
/// 每个应用进入列表后会保留到下一次任意成功或重启为止，因此需要上限防止长期运行后无限增长；
/// 超出时丢弃最久未失败的那些（列表按最近失败时间排序）。
const MAX_TRACKED_SCOPES: usize = 64;

/// 能力快照内容。
///
/// 快照同时承载两类计数，二者**互不替代**：
///
/// - `device_failures`：设备级连续失败，只在失败来自与上一次不同的应用时累加，用于把「设备
///   不支持」与「某个应用自己有问题」区分开；
/// - `scope_failures`：每个应用自己的连续失败数，成功即清除该应用的分桶。
///
/// `teardown_failures` 只统计收尾（unmount）失败，**不参与挂载准入**：一次摘不掉旧挂载说明
/// 不了下一次 `mount(2)` 会失败，用它去关掉全设备的 FUSE 是口径错配。它保留下来只为可观测
/// 性与 `doctor` 展示。
struct FuseCapabilitySnapshot {
    /// 快照文件是否解析成功（存在且 boot_id 匹配）。
    ///
    /// 用于区分「没有信息」（daemon 从未写过快照）与「信息就是 unknown」（daemon 已给出结论口径、
    /// 只是还没有真实会话结果）。两者在 `Auto` 下的处置不同：前者只剩应用侧节点探测可用，后者
    /// 必须放行尝试。
    present: bool,
    capability: FuseCapability,
    device_failures: u32,
    last_failed_scope: String,
    scope_failures: Vec<(String, u32)>,
    /// `Unavailable` 后允许下一次探测的 `CLOCK_MONOTONIC` 毫秒；`0` 表示立即可探测。
    retry_at_ms: u64,
    backoff_step: u32,
    teardown_failures: u32,
}

impl Default for FuseCapabilitySnapshot {
    fn default() -> Self {
        Self {
            present: false,
            capability: FuseCapability::Unknown,
            device_failures: 0,
            last_failed_scope: String::new(),
            scope_failures: Vec::new(),
            retry_at_ms: 0,
            backoff_step: 0,
            teardown_failures: 0,
        }
    }
}

impl FuseCapabilitySnapshot {
    fn scope_failures_for(&self, scope: &str) -> u32 {
        self.scope_failures
            .iter()
            .find(|(name, _)| name == scope)
            .map(|(_, count)| *count)
            .unwrap_or(0)
    }

    /// 写入某个应用的失败计数，并按最近失败优先排序、按上限截断。
    fn bump_scope_failure(&mut self, scope: &str) {
        let count = self.scope_failures_for(scope).saturating_add(1);
        self.scope_failures.retain(|(name, _)| name != scope);
        self.scope_failures.insert(0, (scope.to_string(), count));
        self.scope_failures.truncate(MAX_TRACKED_SCOPES);
    }

    fn clear_scope_failure(&mut self, scope: &str) {
        self.scope_failures.retain(|(name, _)| name != scope);
    }

    /// 该应用是否已用完自己的 scoped 挂载预算。
    fn scope_budget_exhausted(&self, scope: &str) -> bool {
        self.scope_failures_for(scope) >= SCOPED_MOUNT_FAILURE_BUDGET
    }

    /// 当前是否已越过退避窗口，允许一次探测性尝试。
    fn retry_window_open(&self, now_ms: u64) -> bool {
        self.retry_at_ms == 0 || now_ms >= self.retry_at_ms
    }
}

#[derive(Clone)]
pub struct FuseRedirectConfig {
    pub package_name: String,
    pub app_pid: i32,
    pub app_start_time_ticks: Option<u64>,
    pub uid: i32,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub mount_root: Option<String>,
    pub real_root_override: Option<String>,
    pub is_file_monitor_enabled: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub is_mapping_mode_only: bool,
}

impl FuseRedirectConfig {
    pub(super) fn user_id(&self) -> i32 {
        crate::platform::user_id_from_uid(self.uid)
    }
}

/// 挂载请求中构造 FUSE 配置与计算 scoped 挂载根所需的字段。
///
/// daemon 侧的 `MountRequest` 与 companion 侧的 `CompanionMountRequest` 各自演进，字段
/// 相同但类型不同（前者另有 `operation`），此前两处的配置构造与挂载根计算是逐字重复的。
/// 由该 trait 统一取值，两处共用下面的 [`fuse_config_from_request`] 与
/// [`scoped_fuse_mount_roots_for_request`]，避免规则字段增减时漏改一侧。
pub trait MountRequestFields {
    fn package_name(&self) -> &str;
    fn pid(&self) -> i32;
    fn uid(&self) -> i32;
    fn app_data_dir(&self) -> &str;
    fn redirect_target(&self) -> &str;
    fn is_file_monitor_enabled(&self) -> bool;
    fn storage_backend_mode(&self) -> StorageBackendMode;
    fn allowed_real_paths(&self) -> &[String];
    fn excluded_real_paths(&self) -> &[String];
    fn sandboxed_paths(&self) -> &[String];
    fn read_only_paths(&self) -> &[String];
    fn path_mappings(&self) -> &[PathMapping];
    fn is_mapping_mode_only(&self) -> bool;
}

/// 按挂载请求构造 FUSE 重定向配置。
pub fn fuse_config_from_request<R: MountRequestFields + ?Sized>(
    request: &R,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    FuseRedirectConfig {
        package_name: request.package_name().to_string(),
        app_pid: request.pid(),
        app_start_time_ticks: crate::platform::process_start_time_ticks(request.pid()),
        uid: request.uid(),
        app_data_dir: request.app_data_dir().to_string(),
        redirect_target: request.redirect_target().to_string(),
        mount_root,
        real_root_override,
        is_file_monitor_enabled: request.is_file_monitor_enabled(),
        allowed_real_paths: request.allowed_real_paths().to_vec(),
        excluded_real_paths: request.excluded_real_paths().to_vec(),
        sandboxed_paths: request.sandboxed_paths().to_vec(),
        read_only_paths: request.read_only_paths().to_vec(),
        path_mappings: request.path_mappings().to_vec(),
        is_mapping_mode_only: request.is_mapping_mode_only(),
    }
}

/// 计算挂载请求对应的 scoped 挂载根；未启用 FUSE daemon 重定向时返回空列表。
pub fn scoped_fuse_mount_roots_for_request<R: MountRequestFields + ?Sized>(
    request: &R,
) -> Vec<String> {
    if matches!(
        request.storage_backend_mode(),
        StorageBackendMode::Namespace
    ) {
        return Vec::new();
    }

    let backend_mode = request.storage_backend_mode();
    if !scoped_mount_allowed_for_scope(request.package_name(), backend_mode) {
        return Vec::new();
    }

    if matches!(backend_mode, StorageBackendMode::Fuse) {
        let user_id = crate::platform::user_id_from_uid(request.uid());
        return vec![paths::storage_user_root_for_user(user_id)];
    }

    // 只有 wildcard 只读规则时使用 namespace fallback。此类规则需要把通配
    // 收敛到父目录并依赖真实存储种子；启动 scoped FUSE 会把 Download 根接管，
    // 反而丢失 fallback 的真实文件视图。
    if request.allowed_real_paths().is_empty()
        && request.path_mappings().is_empty()
        && request
            .read_only_paths()
            .iter()
            .any(|rule| paths::contains_wildcards(rule))
    {
        return Vec::new();
    }

    scoped_mount_roots_for_hybrid_rules(
        request.uid(),
        request.allowed_real_paths(),
        request.excluded_real_paths(),
        request.sandboxed_paths(),
        request.read_only_paths(),
        request.path_mappings(),
        request.is_mapping_mode_only(),
    )
}

/// 检查设备是否暴露可读写的 `/dev/fuse`。
///
/// 这只是设备节点探测，不代表当前内核、挂载 namespace 或 FUSE 修复链路支持完整
/// scoped 会话。真实能力必须由实际挂载结果确认，失败后回退到 namespace。
pub fn fuse_device_present() -> bool {
    std::fs::metadata("/dev/fuse")
        .map(|metadata| metadata.file_type().is_char_device())
        .unwrap_or(false)
}

/// 返回 daemon 最近一次记录的设备级 FUSE 能力。
///
/// 普通应用只读取这个原子替换的快照，不直接打开 `/dev/fuse`。快照缺失或来自其它开机时
/// 保持 `Unknown`，由规划层走保守的 namespace fallback 路径。
///
/// 注意这是**设备级**结论：判断某个应用自己是否还能尝试 FUSE，必须用
/// [`scoped_mount_allowed_for_scope`]，否则会把别的应用的失败当成自己的。
pub fn fuse_capability() -> FuseCapability {
    load_fuse_capability_snapshot().capability
}

/// 读取当前开机的能力快照；缺失、来自其它开机或不可解析时返回默认值（`Unknown` 且无计数）。
fn load_fuse_capability_snapshot() -> FuseCapabilitySnapshot {
    read_fuse_capability_snapshot().unwrap_or_default()
}

/// 读取当前开机的能力快照；快照缺失、boot_id 不匹配或内容不可解析时返回 None。
fn read_fuse_capability_snapshot() -> Option<FuseCapabilitySnapshot> {
    let content = std::fs::read_to_string(module_paths::FUSE_CAPABILITY_FILE).ok()?;
    let current_boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string());
    if current_boot_id.as_deref() != snapshot_field(&content, "boot_id") {
        return None;
    }
    let mut snapshot = FuseCapabilitySnapshot {
        present: true,
        capability: match snapshot_field(&content, "state") {
            Some("available") => FuseCapability::Available,
            Some("unavailable") => FuseCapability::Unavailable,
            _ => FuseCapability::Unknown,
        },
        // schema 2 的 `fail_count` 是未分桶的全局计数，按设备级计数读入即可：升级前写下的
        // `unavailable` 会带 `retry_at_ms=0`，因此升级后第一次请求就能探测一次并自愈，
        // 不需要等设备重启。
        device_failures: snapshot_field(&content, "fail_count")
            .or_else(|| snapshot_field(&content, "device_fail_count"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
        last_failed_scope: snapshot_field(&content, "last_failed_scope")
            .unwrap_or_default()
            .to_string(),
        scope_failures: parse_scope_failures(&content),
        retry_at_ms: snapshot_field(&content, "retry_at_ms")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        backoff_step: snapshot_field(&content, "backoff_step")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
        teardown_failures: snapshot_field(&content, "teardown_fail_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
    };
    snapshot.scope_failures.truncate(MAX_TRACKED_SCOPES);
    Some(snapshot)
}

/// 解析 `scope_fail=<包名>:<次数>` 行。
///
/// 包名不会包含冒号，因此按最后一个冒号切分即可；解析失败的行直接跳过，不让一行坏数据
/// 使整份快照退化成默认值（那会把已经积累的退避一起丢掉）。
fn parse_scope_failures(content: &str) -> Vec<(String, u32)> {
    content
        .lines()
        .filter_map(|line| line.strip_prefix("scope_fail="))
        .filter_map(|value| {
            let (name, count) = value.trim().rsplit_once(':')?;
            let count = count.parse::<u32>().ok()?;
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), count))
        })
        .collect()
}

/// 从能力快照内容中读取一个 `key=value` 字段。
fn snapshot_field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    content
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

pub fn fuse_capability_as_str(capability: FuseCapability) -> &'static str {
    match capability {
        FuseCapability::Unknown => "unknown",
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
    }
}

/// 某个应用是否允许尝试 scoped 挂载。
///
/// 判定同时看设备级状态与**该应用自己**的失败分桶，这是把「一个应用的失败」与「设备的失败」
/// 分开的关键：
///
/// - `Namespace`：显式要求 namespace 后端，不尝试；
/// - `Fuse`：用户显式选择 FUSE，只做设备节点探测，不受 `Auto` 的熔断约束；
/// - `Auto`：设备可用即尝试；设备未知时按节点探测；设备被判 `Unavailable` 时等退避窗口到期
///   再放行一次探测（成功即整体恢复，失败则退避加倍），不再需要重启。
///
/// 任何状态下，若该应用自己的失败分桶已到预算，本轮开机内它不再尝试；其它应用不受影响。
pub fn scoped_mount_allowed_for_scope(scope: &str, mode: StorageBackendMode) -> bool {
    match mode {
        StorageBackendMode::Namespace => false,
        StorageBackendMode::Fuse => fuse_first_capability_available(),
        StorageBackendMode::Auto => {
            let snapshot = load_fuse_capability_snapshot();
            if snapshot.scope_budget_exhausted(scope) {
                return false;
            }
            match snapshot.capability {
                FuseCapability::Available => true,
                // `Unknown` 只表示还没有真实会话结果，必须放行尝试：真实能力由实际挂载结果确认
                // （`fuse_device_present` 的文档也是这么写的）。**不能**在这里退回应用侧节点探测：
                // 这个判定同样会在应用进程里执行，而应用视角的 `/dev/fuse` 常因 SELinux 不可读
                // （实测 HyperOS 上是 `crw------- root root`，连 stat 都被拒），据此否定会让应用
                // 永远规划不出 FUSE 根、只能退回 namespace，且永远等不到那个能解锁的失败计数。
                // 只有快照完全不存在（daemon 从未写过）时才退回节点探测，此时没有更好的信息源。
                FuseCapability::Unknown => snapshot.present || fuse_device_present(),
                FuseCapability::Unavailable => {
                    snapshot.retry_window_open(paths::monotonic_ms().max(0) as u64)
                }
            }
        }
    }
}

/// 自动后端使用的 fallback 路径决策，应用侧与 daemon 共享同一个判断接口。
///
/// 设备级判定保持原语义：只要不是明确 `Available`（`Unknown` 或 `Unavailable`）就收敛通配
/// 规则，因为这两种状态下规划拿不到可靠的 FUSE 根。在此之上叠加**该应用自己**的失败分桶：
/// 该应用已用完预算时它必然走 namespace，它的规则必须收敛，而这一条不再改写其它应用的规则。
pub fn expand_mount_fallbacks_for_mode(mode: StorageBackendMode, scope: &str) -> bool {
    match mode {
        StorageBackendMode::Namespace => true,
        StorageBackendMode::Fuse => false,
        StorageBackendMode::Auto => {
            let device_not_confirmed_available = fuse_capability() != FuseCapability::Available;
            device_not_confirmed_available
                || load_fuse_capability_snapshot().scope_budget_exhausted(scope)
        }
    }
}

/// 在实际 FUSE 启动失败后，把 namespace fallback 所需的通配规则收敛到父目录。
pub fn expand_namespace_fallback_rules(uid: i32, rules: &[String]) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut expanded = Vec::with_capacity(rules.len());
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (excluded, body) = if let Some(stripped) = trimmed.strip_prefix('!') {
            (true, stripped.trim_start())
        } else {
            (false, trimmed)
        };
        let resolved = if !excluded && paths::contains_wildcards(body) {
            paths::wildcard_policy_fallback_parent(body, &storage_root)
                .unwrap_or_else(|| body.to_string())
        } else {
            body.to_string()
        };
        expanded.push(if excluded {
            format!("!{resolved}")
        } else {
            resolved
        });
    }
    paths::sort_dedup_paths_case_insensitive(&mut expanded);
    expanded
}

/// 第 `step` 次退避的等待毫秒数：基数逐次加倍，封顶 [`RETRY_BACKOFF_MAX_MS`]。
fn retry_backoff_ms(step: u32) -> u64 {
    let shift = step.saturating_sub(1).min(5);
    RETRY_BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(RETRY_BACKOFF_MAX_MS)
}

fn write_fuse_capability_snapshot(
    snapshot: &FuseCapabilitySnapshot,
    reason: &str,
) -> FuseCapability {
    let capability = snapshot.capability;
    let state = match capability {
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
        FuseCapability::Unknown => "unknown",
    };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let mut content = format!(
        "schema=3\nboot_id={boot_id}\nstate={state}\nreason={reason}\ndevice_fail_count={}\nlast_failed_scope={}\nretry_at_ms={}\nbackoff_step={}\nteardown_fail_count={}\n",
        snapshot.device_failures,
        snapshot.last_failed_scope,
        snapshot.retry_at_ms,
        snapshot.backoff_step,
        snapshot.teardown_failures,
    );
    for (scope, count) in &snapshot.scope_failures {
        content.push_str(&format!("scope_fail={scope}:{count}\n"));
    }
    let path = std::path::Path::new(module_paths::FUSE_CAPABILITY_FILE);
    // 快照会被 daemon 与多个 scoped 会话子进程同时写入，固定 temp 名会让并发写入
    // 互相 rename 掉对方的临时文件，这里带上 pid 与自增序号保证唯一。
    let temp = capability_snapshot_temp_path(path);
    let write_result = std::fs::write(&temp, content).and_then(|()| std::fs::rename(&temp, path));
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp);
        log::warn!(
            "fuse capability snapshot write failed state={} reason={} err={}",
            state,
            reason,
            error
        );
    } else {
        log::info!(
            "fuse capability snapshot state={} reason={} device_fail={} scopes={} retry_at_ms={} backoff_step={} teardown_fail={}",
            state,
            reason,
            snapshot.device_failures,
            snapshot.scope_failures.len(),
            snapshot.retry_at_ms,
            snapshot.backoff_step,
            snapshot.teardown_failures
        );
    }
    capability
}

/// 生成能力快照的临时文件路径，供同目录下的原子替换使用。
///
/// 每次写入都使用独立文件名，避免 daemon 与 scoped 会话子进程并发写入时互相
/// 覆盖临时文件，导致其中一方 rename 失败。
fn capability_snapshot_temp_path(path: &std::path::Path) -> std::path::PathBuf {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.{}.tmp", std::process::id(), sequence));
    std::path::PathBuf::from(temp)
}

/// 在 root daemon 或 companion 挂载路径中刷新能力快照。
// quality-allow(lint-suppression): 该入口由 Android daemon 二进制调用，cdylib 目标不会直接调用。
#[allow(dead_code)]
pub fn refresh_fuse_capability_snapshot(reason: &str) -> FuseCapability {
    // 开机 / daemon 重启时整份快照归零：设备级计数、各应用分桶与退避全部清空，让新的一轮
    // 从「未知」重新探测。这正是「重启能恢复」的机制，现在退避窗口也提供了不开机的等价通路。
    let snapshot = FuseCapabilitySnapshot {
        capability: if fuse_first_capability_available() {
            // 打开设备只能证明节点存在；避免在首次真实会话前把 HyperOS/MIUI 的
            // FUSE 兼容性误报为 Available。首次 scoped 挂载会把状态推进到最终结果。
            FuseCapability::Unknown
        } else {
            FuseCapability::Unavailable
        },
        ..FuseCapabilitySnapshot::default()
    };
    write_fuse_capability_snapshot(&snapshot, reason)
}

/// 记录实际 scoped FUSE 挂载结果。
///
/// `scope` 是发起本次挂载的应用（包名）。计数分两层，二者用途不同：
///
/// - **应用分桶**：该应用的连续失败次数，成功即清零；到 [`SCOPED_MOUNT_FAILURE_BUDGET`] 后
///   只让这个应用停止尝试 FUSE，其它应用照旧；
/// - **设备级计数**：只在失败来自与上一次**不同的应用**时累加；达到
///   [`DEVICE_UNAVAILABLE_MIN_SCOPES`] 才把设备判为 `unavailable`，避免单个应用自己的问题
///   （每次启动都被杀、配置异常）把整机 FUSE 一起关掉。
///
/// 判为 `unavailable` 时同时写入退避截止时间；到期后 [`scoped_mount_allowed_for_scope`] 会
/// 放行一次探测，成功即整体恢复，失败则退避加倍。因此 `unavailable` 不再是终态。
pub fn record_fuse_capability_result(available: bool, reason: &str, scope: &str) -> FuseCapability {
    // 计数是读改写序列，而 daemon 会并发处理不同应用的挂载请求；用同目录锁文件串行化，
    // 避免并发失败互相覆盖计数导致预算迟迟达不到。锁获取失败只降低计数精度，不影响写入。
    let _lock = CapabilitySnapshotLock::acquire();
    let mut snapshot = read_fuse_capability_snapshot().unwrap_or_default();
    if available {
        snapshot.capability = FuseCapability::Available;
        snapshot.device_failures = 0;
        snapshot.last_failed_scope.clear();
        snapshot.clear_scope_failure(scope);
        snapshot.retry_at_ms = 0;
        snapshot.backoff_step = 0;
    } else {
        if snapshot.last_failed_scope != scope {
            snapshot.device_failures = snapshot.device_failures.saturating_add(1);
        }
        snapshot.last_failed_scope = scope.to_string();
        snapshot.bump_scope_failure(scope);
        if snapshot.device_failures >= DEVICE_UNAVAILABLE_MIN_SCOPES {
            snapshot.capability = FuseCapability::Unavailable;
            snapshot.backoff_step = snapshot.backoff_step.saturating_add(1);
            // 时间基准是 `CLOCK_MONOTONIC`（Android 上按开机计），快照本身带 boot_id 校验，
            // 因此跨进程比较是安全的；换开机后 boot_id 不匹配，整份快照直接失效。
            let now_ms = paths::monotonic_ms().max(0) as u64;
            snapshot.retry_at_ms = now_ms.saturating_add(retry_backoff_ms(snapshot.backoff_step));
        } else {
            snapshot.capability = FuseCapability::Unknown;
            snapshot.retry_at_ms = 0;
        }
    }
    write_fuse_capability_snapshot(&snapshot, reason)
}

/// 记录一次 scoped 会话收尾（unmount）失败。
///
/// 收尾失败**不参与**挂载准入：一次摘不掉旧挂载说明不了下一次 `mount(2)` 会失败，用它去关掉
/// 全设备的 FUSE 属于口径错配（历史上它确实能单独把整机锁成 `unavailable`）。这里只累计计数
/// 供 `doctor` 与日志观察；挂载准入完全由 [`record_fuse_capability_result`] 的分桶决定。
/// 收尾本身有挂载账本与监督流程兜底，泄漏是可控且有界的。
pub fn record_fuse_teardown_failure(reason: &str) {
    let _lock = CapabilitySnapshotLock::acquire();
    let mut snapshot = read_fuse_capability_snapshot().unwrap_or_default();
    snapshot.teardown_failures = snapshot.teardown_failures.saturating_add(1);
    write_fuse_capability_snapshot(&snapshot, reason);
}

/// 供诊断输出使用的能力快照摘要。
// quality-allow(lint-suppression): 只被 Android daemon 二进制的 doctor 子命令使用，lib 目标不会构造它。
#[allow(dead_code)]
pub struct FuseCapabilitySummary {
    pub capability: FuseCapability,
    pub device_failures: u32,
    pub scope_failures: Vec<(String, u32)>,
    pub retry_at_ms: u64,
    pub backoff_step: u32,
    pub teardown_failures: u32,
}

/// 读取当前能力快照的摘要，供 `doctor` 展示判定依据（而不是只显示一个 state）。
// quality-allow(lint-suppression): 同 `FuseCapabilitySummary`，只服务 daemon 的 doctor 子命令。
#[allow(dead_code)]
pub fn fuse_capability_summary() -> FuseCapabilitySummary {
    let snapshot = load_fuse_capability_snapshot();
    FuseCapabilitySummary {
        capability: snapshot.capability,
        device_failures: snapshot.device_failures,
        scope_failures: snapshot.scope_failures,
        retry_at_ms: snapshot.retry_at_ms,
        backoff_step: snapshot.backoff_step,
        teardown_failures: snapshot.teardown_failures,
    }
}

/// 能力快照的跨进程互斥锁。
///
/// daemon 按 pid 并发处理挂载请求，多个 scoped 会话子进程也会写同一份快照，因此失败计数
/// 需要跨进程串行化。锁文件与快照同目录，关闭 fd 即释放锁。
struct CapabilitySnapshotLock {
    fd: libc::c_int,
}

impl CapabilitySnapshotLock {
    fn acquire() -> Option<Self> {
        let path = CString::new(format!("{}.lock", module_paths::FUSE_CAPABILITY_FILE)).ok()?;
        // SAFETY: path 是以 NUL 结尾的合法路径，flags 与 mode 只用于创建打开锁文件。
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: fd 来自上面的 open，且在本次调用中尚未交给其它所有者。
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(Self { fd })
    }
}

impl Drop for CapabilitySnapshotLock {
    fn drop(&mut self) {
        // SAFETY: fd 来自 CapabilitySnapshotLock::acquire，并且在此之后不再使用。
        unsafe { libc::close(self.fd) };
    }
}

fn fuse_first_capability_available() -> bool {
    if !fuse_device_present() {
        log::warn!("fuse-first capability missing /dev/fuse");
        return false;
    }
    let Ok(path) = CString::new("/dev/fuse") else {
        return false;
    };
    // SAFETY: path 指向以 NUL 结尾的固定字符串，flags 只读写设备能力探测；返回 fd 立即关闭。
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        log::warn!("fuse-first capability cannot open /dev/fuse");
        return false;
    }
    // SAFETY: fd 来自上面的 open，且在当前线程中尚未交给其它所有者。
    unsafe { libc::close(fd) };
    true
}

pub fn mount_blocking_with_ready(
    config: FuseRedirectConfig,
    ready_sock: Option<libc::c_int>,
) -> bool {
    let app_pid = config.app_pid;
    let Some(app_start_time_ticks) = config.app_start_time_ticks else {
        log::warn!(
            "fuse redirect app identity unavailable pkg={} app_pid={}",
            config.package_name,
            app_pid
        );
        send_ready_result(ready_sock, -1);
        return false;
    };
    let package_name = config.package_name.clone();
    let user_id = config.user_id();
    let mount_point = fuse_mount_point(&config, user_id);
    // 挂载源必须唯一：daemon 的重新挂载会在同一路径叠加新会话的挂载，收尾时只能靠它
    // 确认挂载点是否仍属于本次会话。
    let session_mount_source = scoped_mount_source(std::process::id());
    let metadata_dir = mount_point_metadata_dir(&mount_point, user_id);
    let metadata_uid = if crate::metadata_repair::enabled() {
        config.uid
    } else {
        -1
    };
    if !fs::create_directory(&metadata_dir, metadata_uid) {
        log::error!(
            "fuse redirect mount point missing: {} metadata={}",
            mount_point,
            metadata_dir
        );
        send_ready_result(ready_sock, -1);
        return false;
    }

    let fs = match super::FuseRedirectFs::new(config) {
        Some(fs) => fs,
        None => {
            send_ready_result(ready_sock, -1);
            return false;
        }
    };
    let mut mount_options = fuser::Config::default();
    mount_options.mount_options = vec![
        MountOption::FSName(session_mount_source.clone()),
        MountOption::Subtype("srx".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoAtime,
        MountOption::Async,
    ];
    mount_options.acl = SessionACL::All;
    mount_options.n_threads = Some(4);
    mount_options.clone_fd = true;

    log::info!(
        "fuse redirect mount start pkg={} uid={} user={} mp={} rel={} real={} map_only={} allow={} excl={} sandbox={} ro={} map={}",
        fs.policy.package_name,
        fs.policy.uid,
        user_id,
        mount_point,
        fs.policy.mount_rel,
        fs.policy.real_root.display(),
        fs.policy.is_mapping_mode_only,
        fs.policy.allowed_real_paths.len(),
        fs.policy.excluded_real_paths.len(),
        fs.policy.sandboxed_paths.len(),
        fs.policy.read_only_paths.len(),
        fs.policy.path_mappings.len()
    );

    let background = match fuser::spawn_mount2(fs, &mount_point, &mount_options) {
        Ok(background) => background,
        Err(error) => {
            send_ready_result(ready_sock, -1);
            log::warn!(
                "fuse redirect mount failed mp={} err={}",
                mount_point,
                error
            );
            return false;
        }
    };
    // spawn_mount2 返回只代表后台线程已创建；必须等待 mountinfo 出现本会话挂载，
    // 再向父进程报告 ready，避免父进程在挂载栈尚未稳定时误判为成功。
    let Some(session_mount_identity) =
        wait_for_stable_session_mount(&mount_point, &session_mount_source)
    else {
        log::warn!(
            "fuse redirect mount not stable mp={} source={}",
            mount_point,
            session_mount_source
        );
        // 就绪失败也按本会话身份收尾，避免卸载同路径上新建的其它会话。
        let identity = ScopedMountIdentity {
            source: session_mount_source.clone(),
            mount_id: 0,
        };
        finish_background_session(background, &mount_point, false, Some(&identity));
        send_ready_result(ready_sock, -1);
        return false;
    };
    // 挂载后登记本次会话身份；挂载表不可读时不向父进程报告 ready。
    log::info!(
        "fuse redirect session mount registered mp={} mount_id={} source={}",
        mount_point,
        session_mount_identity.mount_id,
        session_mount_identity.source
    );
    send_ready_result(ready_sock, 0);

    loop {
        if background.guard.is_finished() {
            // 会话线程先结束：此时应用可能已经退出，而应用退出会连带清掉它的挂载
            // namespace 与这里的 scoped 挂载。必须先判断应用是否还活着，否则会把
            // 应用退出、重启记成 scoped 会话失败，进而把整机 FUSE 能力锁成不可用。
            let app_alive =
                crate::platform::is_process_instance_alive(app_pid, app_start_time_ticks);
            return finish_background_session(
                background,
                &mount_point,
                !app_alive,
                Some(&session_mount_identity),
            );
        }
        if !crate::platform::is_process_instance_alive(app_pid, app_start_time_ticks) {
            log::info!(
                "fuse redirect app exited, unmount session pkg={} app_pid={} mp={}",
                package_name,
                app_pid,
                mount_point
            );
            return finish_background_session(
                background,
                &mount_point,
                true,
                Some(&session_mount_identity),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn finish_background_session(
    background: fuser::BackgroundSession,
    mount_point: &str,
    app_exited: bool,
    identity: Option<&ScopedMountIdentity>,
) -> bool {
    // 收尾前必须确认挂载点仍由本次会话持有：daemon 的重新挂载会在同一路径叠加新会话的
    // 挂载，此时按路径卸载摘掉的是新会话的挂载，应用会直接看到 ENOTCONN。fuser 的
    // `umount_and_join` 与 `BackgroundSession` 的 Drop 都会按路径卸载，因此这种情况下只能
    // 跳过收尾，让服务进程直接退出（进程退出会关闭 `/dev/fuse`，内核随之结束本次会话）。
    let ownership = mount_ownership(mount_point, identity);
    if !matches!(ownership, MountOwnership::Session) {
        log::info!(
            "fuse redirect session mount not owned mp={} app_exited={} state={} session_mount_id={}",
            mount_point,
            app_exited,
            ownership.as_str(),
            identity
                .map(|identity| identity.mount_id)
                .unwrap_or_default()
        );
        std::mem::forget(background);
        return true;
    }
    match background.umount_and_join() {
        Ok(()) => {
            log::info!(
                "fuse redirect session ended cleanly mp={} app_exited={}",
                mount_point,
                app_exited
            );
            true
        }
        Err(error) => finish_failed_session(mount_point, app_exited, &error, identity),
    }
}

/// 处理 scoped 会话收尾失败。
///
/// 挂载点被回收时 `umount` 可能失败：daemon 重新挂载会先摘除目标挂载点再终止旧服务
/// 进程，应用退出时系统也会连带销毁它的挂载 namespace。此时 EINVAL/ENOENT 只说明
/// 挂载点已经不存在；ENOTCONN 则表示 FUSE 连接已断但挂载记录仍可能留在应用 namespace
/// 中，必须继续用会话身份执行延迟卸载，否则应用会永久看到失效挂载。应用仍在运行且
/// 挂载点仍有引用时则是 EBUSY，两者都只是收尾事件，不能证明设备不支持 scoped 会话。
/// 能力快照是整机状态，一旦写成 unavailable，Auto 后端在本轮开机内不会再次尝试 scoped
/// 挂载，因此只有挂载点仍由本次会话持有且延迟卸载也失败时才记录能力失败。
fn finish_failed_session(
    mount_point: &str,
    app_exited: bool,
    error: &io::Error,
    identity: Option<&ScopedMountIdentity>,
) -> bool {
    let error_no = error.raw_os_error().unwrap_or_default();
    if app_exited || is_already_unmounted_errno(error_no) {
        log::info!(
            "fuse redirect session ended mp={} app_exited={} err={}",
            mount_point,
            app_exited,
            error
        );
        return true;
    }

    if detach_mount_point(mount_point, identity) {
        log::warn!(
            "fuse redirect session detach ok mp={} app_exited={} err={}",
            mount_point,
            app_exited,
            error
        );
        return true;
    }

    // 挂载点仍由本次会话持有且延迟卸载也没能摘除，说明这次 scoped 会话确实无法收尾。
    // 注意这里**不能**改动挂载准入：摘不掉旧挂载并不能说明下一次 mount(2) 会失败，用它去关掉
    // 全设备的 FUSE 是口径错配。只累计收尾失败计数供观察，准入由应用分桶决定。
    record_fuse_teardown_failure("scoped_session_end_error");
    log::warn!(
        "fuse redirect session ended with error mp={} app_exited={} err={}",
        mount_point,
        app_exited,
        error
    );
    false
}

/// 判断 errno 是否表示挂载点已经不在当前挂载命名空间。
///
/// - EINVAL：`umount2` 要求目标仍是挂载点，重新挂载流程已用 `MNT_DETACH` 摘掉旧挂载时
///   就会返回该错误；
/// - ENOENT：挂载点路径已不存在；
///
/// `ENOTCONN` 不在此列：它通常表示挂载记录还在但 FUSE 服务已退出，必须由调用方继续
/// 执行 `MNT_DETACH`，否则目标进程会永久保留返回 ENOTCONN 的死挂载。
fn is_already_unmounted_errno(error_no: i32) -> bool {
    matches!(error_no, libc::EINVAL | libc::ENOENT)
}

/// 用延迟卸载兜底清理会话挂载点。
///
/// 返回 true 表示挂载点已经不在当前命名空间：本次卸载成功，或系统（应用退出、挂载
/// namespace 销毁）已经把它摘掉。
fn detach_mount_point(mount_point: &str, identity: Option<&ScopedMountIdentity>) -> bool {
    // 只有挂载点最顶层确实是本次会话的 scoped 挂载才做延迟卸载；否则宁可保留告警，
    // 也不能在收尾失败时误摘同路径上新挂载的其它文件系统。
    if !matches!(
        mount_ownership(mount_point, identity),
        MountOwnership::Session
    ) {
        return false;
    }

    let Ok(c_target) = CString::new(mount_point) else {
        return false;
    };
    // SAFETY: c_target 是以 NUL 结尾的合法 C 字符串，并在调用期间保持存活。
    if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } == 0 {
        return true;
    }
    is_already_unmounted_errno(last_errno())
}

/// scoped 挂载使用的挂载源前缀。
///
/// 测试流按该前缀在 `/proc/<pid>/mountinfo` 中识别本模块的 scoped 挂载，格式不能改动；
/// 会话标识以 `[pid]` 追加在后缀里，用于区分同一路径上被新会话替换的挂载。
const SCOPED_MOUNT_SOURCE_PREFIX: &str = "srx_fuse_redirect";

/// 生成本次 scoped 会话唯一的挂载源（`MountOption::FSName`）。
///
/// 挂载源会作为 `mount(2)` 的 source 出现在 `/proc/self/mountinfo` 里，不参与内核的 FUSE
/// 参数解析，因此可以安全地携带会话标识。
fn scoped_mount_source(service_pid: u32) -> String {
    format!("{SCOPED_MOUNT_SOURCE_PREFIX}[{service_pid}]")
}

/// `/proc/self/mountinfo` 中一条挂载记录里用于判定挂载归属的字段。
struct MountEntry {
    mount_id: u64,
    fs_type: String,
    source: String,
}

impl MountEntry {
    /// 判断这条记录是否本模块的 scoped FUSE 挂载。
    ///
    /// scoped 挂载在内核 `mount(2)` 直挂时文件系统类型是 `fuse`，经 fusermount 回退时由
    /// `subtype=srx` 记为 `fuse.srx`；两者都要再看挂载源前缀，避免把系统媒体 FUSE 挂载
    /// （挂载源是 `/dev/fuse`）当成模块挂载。
    fn is_scoped_fuse(&self) -> bool {
        matches!(self.fs_type.as_str(), "fuse" | "fuse.srx")
            && self.source.starts_with(SCOPED_MOUNT_SOURCE_PREFIX)
    }
}

/// scoped 会话在挂载后登记的挂载身份。
///
/// 收尾时必须确认挂载点仍由本次会话持有，因此保存本次会话唯一的挂载源；`mount_id` 只用于
/// 诊断日志。
struct ScopedMountIdentity {
    source: String,
    mount_id: u64,
}

impl ScopedMountIdentity {
    /// 挂载完成后登记本次会话身份；挂载表不可读或最顶层挂载不是本次会话时返回 None。
    fn capture(mount_point: &str, source: &str) -> Option<Self> {
        let entry = topmost_mount_entry(mount_point)?;
        (entry.source == source).then(|| Self {
            source: source.to_string(),
            mount_id: entry.mount_id,
        })
    }
}

/// 在有限窗口内连续确认本会话挂载，避免启动后立即报告不稳定状态。
fn wait_for_stable_session_mount(mount_point: &str, source: &str) -> Option<ScopedMountIdentity> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
    let mut consecutive_matches = 0;
    while std::time::Instant::now() < deadline {
        let identity = ScopedMountIdentity::capture(mount_point, source);
        if identity.is_some() {
            consecutive_matches += 1;
            if consecutive_matches >= 2 {
                return identity;
            }
        } else {
            consecutive_matches = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// 读取当前挂载表中指定挂载点上的全部记录。
///
/// 同一路径可能叠着多层挂载（daemon 重挂载期间旧会话与新会话并存），因此返回列表，由
/// [`topmost_mount_entry`] 按挂载 ID 选出最顶层的一条。
fn mount_entries_at(mount_point: &str) -> Vec<MountEntry> {
    let Ok(content) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let normalized = paths::normalize(mount_point);
    content
        .lines()
        .filter_map(|line| {
            let entry = mountinfo::parse_entry(line)?;
            if !paths::eq_ignore_case(
                &paths::normalize(&mountinfo::unescape_field(entry.target)),
                &normalized,
            ) {
                return None;
            }
            Some(MountEntry {
                mount_id: entry.mount_id,
                fs_type: entry.fs_type.to_string(),
                source: mountinfo::unescape_field(entry.source),
            })
        })
        .collect()
}

/// 返回挂载点上最顶层的挂载记录（挂载 ID 最大的一条）。
fn topmost_mount_entry(mount_point: &str) -> Option<MountEntry> {
    mount_entries_at(mount_point)
        .into_iter()
        .max_by_key(|entry| entry.mount_id)
}

/// 判断挂载点当前由谁持有。
fn mount_ownership(mount_point: &str, identity: Option<&ScopedMountIdentity>) -> MountOwnership {
    let Some(entry) = topmost_mount_entry(mount_point) else {
        return MountOwnership::Released;
    };
    let owned_by_session = match identity {
        Some(identity) => entry.source == identity.source,
        // 未能登记会话身份（挂载时挂载表不可读）时退回按 scoped 挂载特征判断，至少不会把
        // 系统媒体 FUSE 挂载当成模块挂载。
        None => entry.is_scoped_fuse(),
    };
    if owned_by_session {
        MountOwnership::Session
    } else {
        MountOwnership::Superseded
    }
}

/// scoped 会话收尾前判断挂载点归属的结果。
enum MountOwnership {
    /// 挂载点最顶层仍是本次会话的挂载。
    Session,
    /// 挂载点上已没有记录：本次会话的挂载已经被摘除。
    Released,
    /// 挂载点被其它挂载接管（daemon 重新挂载叠加了新会话的挂载）。
    Superseded,
}

impl MountOwnership {
    fn as_str(&self) -> &'static str {
        match self {
            MountOwnership::Session => "session",
            MountOwnership::Released => "released",
            MountOwnership::Superseded => "superseded",
        }
    }
}

pub(super) fn fuse_mount_point(config: &FuseRedirectConfig, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(raw_mount_root) = config.mount_root.as_deref() else {
        return storage_root;
    };
    let mut mount_root = paths::resolve_user_path(&paths::normalize(raw_mount_root), user_id);
    if !paths::is_absolute(&mount_root) {
        mount_root = paths::normalize(&paths::join(&storage_root, &mount_root));
    }
    if paths::eq_ignore_case(&mount_root, &storage_root)
        || paths::is_child(&mount_root, &storage_root)
    {
        mount_root
    } else {
        storage_root
    }
}

fn mount_point_metadata_dir(mount_point: &str, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(mount_point, &storage_root) {
        return paths::data_media_user_root_for_user(user_id);
    }
    paths::storage_to_data_media_for_user(mount_point, user_id)
        .unwrap_or_else(|| mount_point.to_string())
}

pub fn scoped_mount_roots_for_wildcard_rules<'a>(
    uid: i32,
    rules: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut roots = Vec::new();
    for raw in rules {
        let raw = raw.trim_start();
        let raw = raw.strip_prefix('!').unwrap_or(raw).trim_start();
        let mut resolved = paths::resolve_user_path(&paths::normalize(raw), user_id);
        if resolved.is_empty()
            || paths::has_unsafe_segments(&resolved)
            || !paths::contains_wildcards(&resolved)
        {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if !paths::is_child(&resolved, &storage_root)
            && !paths::eq_ignore_case(&resolved, &storage_root)
        {
            continue;
        }
        let prefix = paths::concrete_prefix_before_wildcard(&resolved);
        if let Some(root) = scoped_mount_root_for_wildcard_prefix(&prefix, &storage_root) {
            roots.push(root);
        }
    }
    compact_scoped_mount_roots(roots, &storage_root)
}

fn scoped_mount_root_for_wildcard_prefix(prefix: &str, storage_root: &str) -> Option<String> {
    if prefix.is_empty() || !paths::is_child(prefix, storage_root) {
        return Some(storage_root.to_string());
    }
    if let Some(root) = public_collection_mount_root(prefix, storage_root) {
        return Some(root);
    }
    Some(prefix.to_string())
}

fn public_collection_mount_root(prefix: &str, storage_root: &str) -> Option<String> {
    public_collection_name(prefix, storage_root).map(|first| paths::join(storage_root, first))
}

fn public_collection_name<'a>(prefix: &'a str, storage_root: &str) -> Option<&'a str> {
    let rel = paths::relative_child_path(prefix, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    match first {
        "Alarms" | "Audiobooks" | "DCIM" | "Documents" | "Download" | "Movies" | "Music"
        | "Notifications" | "Pictures" | "Podcasts" | "Recordings" | "Ringtones" => Some(first),
        _ => None,
    }
}

pub fn scoped_mount_roots_for_hybrid_rules(
    uid: i32,
    allowed_real_paths: &[String],
    excluded_real_paths: &[String],
    sandboxed_paths: &[String],
    read_only_paths: &[String],
    path_mappings: &[crate::domain::PathMapping],
    is_mapping_mode_only: bool,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    // 映射源中的通配也依赖动态目录匹配，不能仅交给 namespace 的启动时展开。
    // 先复用映射校验，避免无效目标或 namespace 外路径扩大 FUSE 接管范围。
    let scoped_path_mappings = resolve_scoped_path_mappings(path_mappings, user_id, &storage_root);
    let mapping_wildcard_rules = scoped_path_mappings
        .iter()
        .map(|(request, _)| request.as_str());
    let scoped_allowed_rules = allowed_real_paths.iter().map(String::as_str);
    let sandbox_include_rules = sandboxed_paths
        .iter()
        .filter(|rule| !rule.trim_start().starts_with('!'))
        .map(String::as_str);
    let mut roots = scoped_mount_roots_for_wildcard_rules(
        uid,
        scoped_allowed_rules
            .chain(excluded_real_paths.iter().map(String::as_str))
            .chain(sandbox_include_rules)
            .chain(mapping_wildcard_rules),
    );

    if is_mapping_mode_only {
        for sandboxed_path in sandboxed_paths {
            if sandboxed_path.trim_start().starts_with('!') {
                continue;
            }
            let sandboxed_root =
                resolve_concrete_scoped_rule_parent(sandboxed_path, user_id, &storage_root);
            if !sandboxed_root.is_empty() {
                roots.push(sandboxed_root);
            }
        }
    }

    for allowed_path in allowed_real_paths {
        // 放行规则以自身为 scoped 根，而不是父目录。取父目录会带来两个问题：顶层规则
        // （如 DCIM、Pictures）的父目录就是存储根，会让整个存储被 FUSE 接管并在压缩时
        // 吞并其余更精确的根；而 Download/SrtMonitor 这类规则取到的 Download 也会吞并
        // 同配置下的兄弟目录（如只读的 Download/SrtMonitorLocked），使其拿不到独立挂载点。
        // 以规则自身为根即可覆盖该目录下的放行需求，且与兄弟根共存。
        // mapping_mode_only 的 sandboxed 分支仍沿用父目录，其语义要求按父目录整体接管。
        let allowed_root = resolve_scoped_rule_path(allowed_path, user_id, &storage_root);
        if allowed_root.is_empty()
            || paths::contains_wildcards(&allowed_root)
            || paths::eq_ignore_case(&allowed_root, &storage_root)
            || !paths::is_child(&allowed_root, &storage_root)
        {
            continue;
        }
        roots.push(allowed_root);
    }

    // 仅有通配只读规则时不单独启动 scoped FUSE：namespace fallback 会把规则收敛到
    // 具体父目录，既能保留真实文件可读性，也避免 FUSE 根覆盖后无法准备真实种子目录。
    let normalized_read_only_paths = super::normalize_rule_list(read_only_paths.to_vec(), user_id);
    let (read_only_includes, _) = paths::split_exclusion_rules(&normalized_read_only_paths);
    // 具体只读规则一律由 scoped FUSE 就地承接，而不是在 namespace 里把可见路径改绑到
    // 真实后端：`/data/media/<user>` 后端带 media_rw_data_file 上下文，普通应用即使
    // 目录属于自身 UID 也会被 SELinux 拒绝列举该目录（应用侧表现为 listFiles 返回 null，
    // 目录列举为空）。交给 FUSE 后可见路径仍是应用可访问的视图，内容直接读真实后端，
    // 只读仍按 open-for-write、truncate、chmod、link、rename、delete 与 W_OK 逐条判定。
    // 带排除子项或嵌套映射的规则还需要运行期动态目录匹配，同样必须依赖 scoped FUSE。
    for read_only_root in &read_only_includes {
        if read_only_root.is_empty() || paths::contains_wildcards(read_only_root) {
            continue;
        }
        roots.push(read_only_root.clone());
    }

    // 挂载根的三级降级（去重剔子路径、退化顶层、退化整个存储根）只输出最终结果，
    // 一旦降级就看不出是哪条规则贡献了多余的根。这里在压缩前后各记录一次：该函数只在
    // 应用挂载时执行一次，频率极低。
    log::info!(
        "scoped roots raw count={} list={}",
        roots.len(),
        roots.join(",")
    );
    let compacted = compact_scoped_mount_roots(roots, &storage_root);
    log::info!(
        "scoped roots compacted count={} list={}",
        compacted.len(),
        compacted.join(",")
    );
    compacted
}

fn resolve_scoped_path_mappings(
    path_mappings: &[crate::domain::PathMapping],
    user_id: i32,
    storage_root: &str,
) -> Vec<(String, String)> {
    let mut resolved = Vec::with_capacity(path_mappings.len());
    for mapping in path_mappings {
        let request_path = resolve_scoped_rule_path(&mapping.request_path, user_id, storage_root);
        let final_path = resolve_scoped_rule_path(&mapping.final_path, user_id, storage_root);
        if request_path.is_empty()
            || final_path.is_empty()
            || paths::is_application_private_root(&request_path)
            || paths::eq_ignore_case(&request_path, &final_path)
            || !paths::is_same_or_child(&request_path, storage_root)
            || !paths::is_same_or_child(&final_path, storage_root)
        {
            continue;
        }
        resolved.push((request_path, final_path));
    }
    resolved
}

fn resolve_concrete_scoped_rule_parent(path: &str, user_id: i32, storage_root: &str) -> String {
    let resolved = resolve_scoped_rule_path(path, user_id, storage_root);
    if resolved.is_empty()
        || paths::contains_wildcards(&resolved)
        || paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }

    let parent = paths::parent(&resolved);
    if paths::eq_ignore_case(&parent, storage_root) || paths::is_child(&parent, storage_root) {
        parent
    } else {
        String::new()
    }
}

fn resolve_scoped_rule_path(path: &str, user_id: i32, storage_root: &str) -> String {
    let mut resolved = paths::resolve_user_path(&paths::normalize(path), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return String::new();
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }
    resolved
}

fn compact_scoped_mount_roots(mut roots: Vec<String>, storage_root: &str) -> Vec<String> {
    paths::sort_dedup_paths_case_insensitive(&mut roots);
    roots.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    let mut effective: Vec<String> = Vec::new();
    for root in roots {
        if effective
            .iter()
            .any(|kept| paths::eq_ignore_case(kept, &root) || paths::is_child(&root, kept))
        {
            continue;
        }
        effective.push(root);
    }

    if effective.len() <= super::TARGET_SCOPED_FUSE_ROOTS {
        return effective;
    }

    // 第二级压缩到顶层子目录；超出硬预算时使用单个存储根会话。
    // 预算只限制会话数量，不应改变原规则的动态匹配语义。
    let mut top_level: Vec<String> = effective
        .iter()
        .filter_map(|root| top_level_storage_child(root, storage_root))
        .collect();
    paths::sort_dedup_paths_case_insensitive(&mut top_level);
    if !top_level.is_empty() && top_level.len() <= super::MAX_SCOPED_FUSE_ROOTS {
        // 超过软目标意味着该应用会常驻多于 TARGET 个 FUSE 会话（空闲时各自阻塞在
        // /dev/fuse read 上不耗 CPU，但每个会话都是一个进程的内存开销）。这是
        // 功耗/内存排查时最需要一眼看到的信号，必须在现场日志里可见。
        log::warn!(
            "scoped roots expanded past soft target count={} raw={} list={}",
            top_level.len(),
            effective.len(),
            top_level.join(",")
        );
        return top_level;
    }

    // 自定义顶层目录数量没有固定上限；返回空列表会静默退回 namespace，
    // 丢失之后新建目录的通配映射。存储根会话仍逐路径应用原始规则。
    log::warn!(
        "scoped roots exceed limit after top-level fallback: effective={} top_level={} limit={}, \
         collapse to single storage-root fuse session",
        effective.len(),
        top_level.len(),
        super::MAX_SCOPED_FUSE_ROOTS
    );
    vec![storage_root.to_string()]
}

fn top_level_storage_child(path: &str, storage_root: &str) -> Option<String> {
    if paths::eq_ignore_case(path, storage_root) {
        return None;
    }
    let rel = paths::relative_child_path(path, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    Some(paths::join(storage_root, first))
}

fn send_ready_result(sock: Option<libc::c_int>, result: i32) {
    let Some(sock) = sock else {
        return;
    };
    // SAFETY: sock 是有效的 socket fd，buffer 指针指向栈上有效数据，size 与类型匹配，调用期间保持有效。
    let _ = unsafe {
        libc::send(
            sock,
            &result as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    // SAFETY: sock 是有效的 socket fd，此处是唯一的关闭点，调用后不再使用。
    unsafe { libc::close(sock) };
}
