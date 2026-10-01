pub use crate::daemon_mount_diag::doctor_report;
use crate::daemon_mount_diag::{
    log_child_diagnostics, log_mounted_target_view, log_reload_view_root_stack,
    read_proc_status_summary,
};
pub use crate::daemon_mount_reclaim::{cleanup_all_mount_states, prune_stale_mount_states};
use crate::daemon_mount_reclaim::{
    has_dead_fuse_child, reap_child, remember_stuck_mount_child, should_skip_for_stuck_children,
    state_value,
};
use crate::domain::PathMapping;
use crate::fuse_redirect::{
    FuseRedirectConfig, ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount,
    log_scoped_mount_roots, mount_blocking_with_ready,
};
use crate::fuse_session::{
    FuseMountState, recv_result, rollback_scoped_fuse_services, send_mount_result, set_recv_timeout,
};
use crate::fuse_supervisor::{self, EndpointHealth, RecoveryAction};
use crate::mount::MountPlanner;
use crate::mount_identity::{self, MountLedger, MountVerdict};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{module_paths, mountinfo, paths};
use libc::{
    AF_UNIX, CLONE_NEWNS, MNT_DETACH, O_CLOEXEC, O_RDONLY, SIGKILL, SIGTERM, SOCK_DGRAM, c_int,
    close, open, setns, socketpair, umount2,
};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::sync::Mutex;

const PARENT_RECV_TIMEOUT_SEC: i64 = 5;
const PARENT_RECV_GRACE_TIMEOUT_SEC: i64 = 1;
const FUSE_READY_TIMEOUT_SEC: i64 = 4;
const DAEMON_MOUNT_SLOW_MS: i64 = 20;
const MAX_UNMOUNT_PASSES_PER_TARGET: usize = 32;
static ACTIVE_MOUNT_PIDS: Lazy<Mutex<HashSet<i32>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static LAST_SUCCESS_BY_PID: Lazy<Mutex<HashMap<i32, (u64, u64)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
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
pub(crate) fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

/// 判断挂载点是否覆盖给定路径。
///
/// 刻意不复用 `paths::starts_with`：那个实现是裸字符串前缀比较，`/a/Download` 会把
/// `/a/Downloads` 也算作覆盖。诊断输出必须按路径分量判断，否则会给出错误的「已覆盖」结论。
pub(crate) fn mount_point_covers(mount_point: &str, path: &str) -> bool {
    let mount_point = mount_point.trim_end_matches('/');
    path == mount_point || path.starts_with(&format!("{mount_point}/"))
}

/// 列出以该包名运行（或它的子进程）的进程。
///
/// Android 上进程名等于包名，子进程写作 `包名:后缀`，因此按 `cmdline` 首段匹配即可。
pub(crate) fn package_processes(package_name: &str) -> Vec<(i32, String)> {
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
pub(crate) fn module_mapped_into(pid: i32) -> Option<bool> {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).ok()?;
    Some(maps.contains("storage.redirect.x"))
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

    // 重载摘除前先采一次视图根/映射目标最上层挂载，作为「重载前基线」。
    log_reload_view_root_stack(request, "before_clear");

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
            if !crate::fuse_session::apply_mount_namespace_fallback(&mut planner, request) {
                log::warn!(
                    "daemon hybrid fuse namespace fallback failed pid={} pkg={}",
                    request.pid,
                    request.package_name
                );
            }
        }
        let mut mounted_targets = planner.take_mounted_targets();
        // 仍在应用命名空间内，就地核对本轮目标的可见性，给热重载类问题留下应用视角证据。
        log_mounted_target_view(&mounted_targets, request);
        // 重建后再采一次，与 before_clear 对照，判断重载是否真的换掉了视图根最上层。
        log_reload_view_root_stack(request, "after_mount");
        if crate::system_fuse_view::should_clear_system_fuse_view_for_platform() {
            crate::system_fuse_view::clear_system_fuse_view_for_package(
                request.uid,
                &request.package_name,
            );
            if !request.path_mappings.is_empty() {
                planner.reapply_path_mappings_only(&request.path_mappings);
            }
            // 摘除的 MNT_DETACH 可能已把旧映射挂载级联摘掉，无论重建是否成功都要重取一次，
            // 否则状态文件会记录已不存在的挂载，后续恢复流程据此误判为“仍在场”。
            mounted_targets = planner.take_mounted_targets();
        }
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
    crate::fuse_session::start_scoped_fuse_services(
        request,
        roots,
        real_root_override,
        |root, real_root_override| start_fuse_service_for_root(request, root, real_root_override),
    )
}

fn scoped_fuse_mount_roots(request: &MountRequest) -> Vec<String> {
    let roots = crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request);
    // Auto 模式的目标形态是共享宿主会话：把按目录 scoped 根收敛成存储视图根，宿主会话
    // 按 uid 持有同一份规则，一个整根会话即可表达相同语义。收敛后的整根在
    // `start_fuse_service_for_root` 里命中接入闸门（闸门本来就只放行存储视图根），接入
    // 失败时该函数自己回退 scoped——含整根 scoped，与部分失败收敛路径一致。
    //
    // 宿主未就绪时**等待建立**而不是按旧规划挂上再事后迁移：迁移要在应用命名空间里
    // 先卸旧 scoped 层、再挂新层，中间应用对该路径的访问会穿透到真实存储（fail-open
    // 窗口）；等待发生在挂载应答返回之前，应用进程尚未恢复运行，一次挂载到位。
    // 接入被显式关闭、等待超时或处于失败冷却期时保持原规划——旧数据面照常服务。
    if roots.is_empty()
        || !matches!(
            request.storage_backend_mode,
            crate::config::StorageBackendMode::Auto
        )
    {
        return roots;
    }
    if !crate::fuse_host::wait_for_host_session() {
        // 诊断：回落旧规划必须留下原因（开关关闭 / 等待超时 / 失败冷却），否则
        // "宿主明明活着却规划成按目录根"这类矛盾只能靠猜。
        log::info!(
            "daemon fuse host wait not ready roots={} mode={} pid={} pkg={}",
            roots.len(),
            request.storage_backend_mode.as_str(),
            request.pid,
            request.package_name
        );
        return roots;
    }
    let user_id = crate::platform::user_id_from_uid(request.uid);
    let view_root = paths::storage_user_root_for_user(user_id);
    if roots.len() == 1 && paths::normalize_syntax(&roots[0]) == paths::normalize_syntax(&view_root)
    {
        return roots;
    }
    log::info!(
        "daemon fuse host preferred collapse roots={} -> view root pid={} pkg={}",
        roots.len(),
        request.pid,
        request.package_name
    );
    vec![view_root]
}

fn start_fuse_service_for_root(
    request: &MountRequest,
    mount_root: &str,
    real_root_override: Option<String>,
) -> Option<FuseMountState> {
    // B2-b：优先尝试共享宿主会话接入。
    if let Some(host) = crate::fuse_host::get_fuse_host() {
        // 先把该应用的策略按 uid 登记进共享宿主会话：这是应用接入的前置条件，提前登记也让
        // 接入启用后第一帧请求就带上正确策略。虚拟根取整个存储根（mount_root=None），因为
        // 宿主会话服务的是完整存储视图，而不是某个 scoped 子根。
        //
        // `real_root_override` 必须丢弃（传 None）：它是**应用命名空间专属**的锚点别名
        // （`tmp/real_storage/<user>` 上的绑定只存在于应用 namespace，供 scoped 子进程读取）。
        // 宿主子进程活在自己的私有命名空间里，同一个路径是空目录，照搬覆盖会让策略把真实根
        // 读成空——真机实测表现为仅映射模式的应用整个公共存储视图消失，只剩被沙盒化的那几条
        // 路径。宿主命名空间里 `/data/media/<user>` 就是未经覆盖的真实存储，本就无需别名。
        let policy_config = fuse_config_from_request(request, None, None);
        let registered = crate::fuse_host::register_app_policy(&policy_config);
        if !crate::fuse_host::can_attach_app(request.uid, mount_root) {
            // 宿主会话的虚拟根只能是整个存储视图根，且默认不接管应用挂载；两种情况都保持
            // 既有 scoped 路径，不能因为"宿主会话可用"就顺手接上。
            log::debug!(
                "daemon fuse host attach skipped pid={} pkg={} target={} registered={} reason=attach_gate_closed",
                request.pid,
                request.package_name,
                mount_root,
                registered
            );
        } else if !registered {
            // 策略没进宿主会话时接入会让应用拿到"未登记即拒绝"的空视图；宁可继续 scoped。
            log::warn!(
                "daemon fuse host attach skipped pid={} pkg={} target={} reason=policy_registration_failed",
                request.pid,
                request.package_name,
                mount_root
            );
        } else if let Some(state) = try_bind_to_fuse_host(&host, request, mount_root) {
            return Some(state);
        } else {
            log::warn!(
                "daemon bind to fuse host failed, falling back to scoped fork pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
    }

    // 回退：fork 独立 scoped 会话（B2-a 前的既有路径）。
    let mut ready_sockets = [0; 2];
    // SAFETY: socketpair 系统调用，传入有效的栈数组指针。
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        log_errno("daemon fuse ready socketpair failed");
        return None;
    }

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    // SAFETY: fork 系统调用，已经通过 prepare_for_fork 避免日志通道竞态。
    let service_child = unsafe { libc::fork() };
    if service_child < 0 {
        log_errno("daemon fuse fork failed");
        // SAFETY: ready_sockets 是有效 fd，fork 失败后父进程负责清理。
        unsafe {
            close(ready_sockets[0]);
            close(ready_sockets[1]);
        }
        return None;
    }

    if service_child == 0 {
        // SAFETY: 子进程关闭继承的 fd；prctl 设置进程名，传入 null 结尾的字节串。
        unsafe {
            close(ready_sockets[0]);
            let name = b"srx_fuse\0";
            libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        }
        let ok = mount_blocking_with_ready(
            fuse_config_from_request(request, Some(mount_root.to_string()), real_root_override),
            Some(ready_sockets[1]),
        );
        // SAFETY: 子进程直接退出，不执行析构函数（避免 fork 后的资源清理问题）。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭写端 fd。
    unsafe { close(ready_sockets[1]) };
    set_recv_timeout(ready_sockets[0], service_child, FUSE_READY_TIMEOUT_SEC);
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    // SAFETY: 父进程关闭读端 fd。
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
        crate::fuse_terminate::terminate_fuse_process(service_child, None);
        return None;
    }

    let Some(child_start_time_ticks) = crate::platform::process_start_time_ticks(service_child)
    else {
        // SAFETY: rollback 内部会 kill + waitpid，传入有效的 pid。
        rollback_scoped_fuse_services(&[FuseMountState {
            target: mount_root.to_string(),
            child: service_child,
            child_start_time_ticks: 0,
            host_session: None,
        }]);
        return None;
    };
    Some(FuseMountState {
        target: mount_root.to_string(),
        child: service_child,
        child_start_time_ticks,
        host_session: None,
    })
}

/// B2-b：把应用接入共享宿主 FUSE 会话。
///
/// 跨命名空间的取句柄与 bind 都收敛在 [`crate::fuse_host::attach_app_to_host`]：daemon 与
/// companion 两条路径必须用同一份实现，否则一处改了命名空间处理、另一处还在旧写法，
/// 应用会因挂载落在错误命名空间而静默失去重定向。
///
/// 记账上要与 scoped 会话区分：宿主会话是跨应用共享的，`child` 不能填宿主 pid，
/// 否则下一轮清理会把它当成本应用的子进程终止掉，连带打掉其它应用的挂载。
fn try_bind_to_fuse_host(
    host: &crate::fuse_host::FuseHost,
    request: &MountRequest,
    mount_root: &str,
) -> Option<FuseMountState> {
    let attached = crate::fuse_host::attach_app_to_host(
        &crate::fuse_host::HostSessionView::from(host),
        mount_root,
    )?;
    log::info!(
        "daemon fuse host attach ok pid={} pkg={} target={} host={}",
        request.pid,
        request.package_name,
        attached.target,
        attached.host_pid
    );
    Some(FuseMountState {
        target: attached.target,
        child: 0,
        child_start_time_ticks: 0,
        host_session: Some((attached.host_pid, attached.host_start_time_ticks)),
    })
}

fn set_mount_namespace(ns_path: Option<&CStr>) -> bool {
    // 路径在 fork 之前就已经转换好，这里只做 open/setns，避免子进程再次堆分配。
    let Some(c_path) = ns_path else {
        return false;
    };
    // SAFETY: open 系统调用，c_path 是有效的 C 字符串指针。
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        log_errno("daemon ns open failed");
        return false;
    }
    let file = UniqueFd::new(fd);
    // SAFETY: setns 系统调用，file 是有效的 namespace 文件描述符。
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
    set_recv_timeout(sock, child, primary_timeout_sec);
    let mut result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let mut n = recv_result(sock, &mut result);
    let mut should_reap_nonblocking = false;
    if n != expected {
        log_child_diagnostics(child, "primary_timeout");
        let _ = unsafe { libc::kill(child, SIGTERM) };
        set_recv_timeout(sock, child, PARENT_RECV_GRACE_TIMEOUT_SEC);
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
    // 来自已被重建的宿主会话的层不能保留：它仍是本模块的层（归属判定会认），但 FUSE 连接
    // 已随旧会话结束断开，保留下来应用只会一直拿到 ENOTCONN。
    if crate::fuse_host::is_stale_host_source(source) {
        return false;
    }
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

pub(crate) fn current_mount_target_count(target: &str) -> usize {
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

fn fuse_config_from_request(
    request: &MountRequest,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    crate::fuse_redirect::fuse_config_from_request(request, mount_root, real_root_override)
}

/// 预登记指纹：宿主子进程 pid、宿主 start_ticks、登记时的配置版本。
type PreRegisterFingerprint = (i32, u64, u64);

/// 宿主策略预登记的幂等指纹表：uid -> [`PreRegisterFingerprint`]。
///
/// reconcile 常驻循环每轮都会对运行中的配置应用调用 [`pre_register_host_policy`]，
/// 而策略内容只随配置版本与宿主会话代际变化。稳态下逐轮全量重登记纯属浪费：每次
/// 登记都要在 daemon 侧为 media 视图探测 fork 一个子进程，并在宿主子进程内做一次
/// 完整的 `RedirectPolicy::new`（沙盒目录准备、规则归一化），还会把注册日志刷到
/// 秒级轮转。指纹任一分量变化都会自然失效重登记：宿主换代（pid/start_ticks）、
/// 配置变更（config_version）、MediaProvider 重启（由 reconcile 显式清空本表）。
static PRE_REGISTERED_HOST_POLICIES: Lazy<Mutex<HashMap<u32, PreRegisterFingerprint>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// MediaProvider 进程集合变化时清空预登记指纹表。
///
/// media 视图绑定指向 MediaProvider 的 FUSE 连接，其进程重启后旧连接死亡，已登记
/// 策略里的视图根随之失效；清空指纹表让本轮预登记全量重跑，触发
/// `ensure_host_media_fuse_view` 重新探测并重绑新连接，保留原有的自愈语义。
pub(crate) fn invalidate_pre_registered_host_policies() {
    if let Ok(mut registered) = PRE_REGISTERED_HOST_POLICIES.lock() {
        registered.clear();
    }
}

/// 预登记调用的结果分类：区分"本次实际登记"与"指纹命中跳过"，让调用方只对
/// 真正的登记动作记日志，避免稳态下每轮 reconcile 都刷一遍登记日志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreRegisterOutcome {
    /// 本次调用实际向宿主登记了策略并收到宿主确认。
    Registered,
    /// 指纹命中：宿主会话内该 uid 已是同版本登记状态，本次未做任何 IO。
    AlreadyRegistered,
    /// 不满足预登记条件，或登记失败。
    NotApplicable,
}

/// 在执行具体挂载前把 Auto 应用策略预登记进共享宿主。
///
/// companion 与 daemon 并行处理同一应用的挂载请求；如果等 companion 进入后才登记，
/// companion 只能看到没有该 uid 的快照并回退 scoped。预登记只写策略，不创建挂载，且
/// 必须丢弃应用 namespace 专属的 real_root_override，保证宿主看到真实存储根。
pub(crate) fn pre_register_host_policy(request: &MountRequest) -> PreRegisterOutcome {
    if request.operation != MountOperation::Reload
        || !matches!(
            request.storage_backend_mode,
            crate::config::StorageBackendMode::Auto
        )
        || !crate::fuse_host::wait_for_host_session()
    {
        return PreRegisterOutcome::NotApplicable;
    }
    // 幂等跳过以快照为准：快照自身校验了 boot 归属与宿主进程存活，指纹比对覆盖
    // 宿主换代与配置变更。跳过路径不做任何 fork、不构造策略、不发送控制消息；
    // 指纹不匹配（配置变更或宿主换代）时重登记，并在登记获宿主确认后才更新指纹，
    // 登记失败时保持旧指纹（下一轮会再次尝试），绝不把"未登记"误记为"已登记"。
    if let Some(view) = crate::fuse_host::read_host_session_view()
        && view.registered_uids.contains(&(request.uid as u32))
    {
        let fingerprint = (
            view.child_pid,
            view.child_start_time_ticks,
            request.config_version,
        );
        if let Ok(registered) = PRE_REGISTERED_HOST_POLICIES.lock()
            && registered.get(&(request.uid as u32)) == Some(&fingerprint)
        {
            return PreRegisterOutcome::AlreadyRegistered;
        }
        let config = fuse_config_from_request(request, None, None);
        if crate::fuse_host::register_app_policy(&config) {
            if let Ok(mut registered) = PRE_REGISTERED_HOST_POLICIES.lock() {
                registered.insert(request.uid as u32, fingerprint);
            }
            return PreRegisterOutcome::Registered;
        }
        return PreRegisterOutcome::NotApplicable;
    }
    let config = fuse_config_from_request(request, None, None);
    if crate::fuse_host::register_app_policy(&config) {
        PreRegisterOutcome::Registered
    } else {
        PreRegisterOutcome::NotApplicable
    }
}

fn write_mount_state(
    request: &MountRequest,
    plan: &MountForkPlan,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    crate::fuse_session::write_mount_state(
        request.pid,
        request.uid,
        &request.package_name,
        request.config_version,
        plan.state_path.as_str(),
        plan.temp_state_path.as_str(),
        targets,
        fuse_children,
    )
}

fn state_file_path(request: &MountRequest) -> String {
    format!(
        "{}/{}_{}.state",
        module_paths::MOUNT_STATE_DIR,
        module_paths::sanitize_name(&request.package_name),
        request.pid
    )
}

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
pub(crate) struct FuseChildIdentity {
    pub(crate) pid: i32,
    pub(crate) start_time_ticks: Option<u64>,
}

pub(crate) fn read_fuse_children(path: &str) -> Vec<FuseChildIdentity> {
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

/// 读取状态文件里记录的共享宿主会话身份。
///
/// 与 `fuse_child=` 分开存放：那一行的语义是"可以终止这个进程"，而宿主会话跨应用共享，
/// 只能判活、不能终止。
pub(crate) fn read_fuse_host_session(path: &str) -> Option<FuseChildIdentity> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| {
            let value = line.strip_prefix("fuse_host=")?;
            let (pid, start) = value.split_once(':')?;
            let pid: i32 = pid.parse().ok()?;
            (pid > 0).then(|| FuseChildIdentity {
                pid,
                start_time_ticks: start.parse().ok(),
            })
        })
}

pub(crate) fn terminate_recorded_fuse_child(child: &FuseChildIdentity) -> bool {
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
    crate::fuse_terminate::terminate_fuse_process(child.pid, Some(start_time_ticks));
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

pub(crate) fn log_errno(message: &str) {
    let errno = last_errno();
    log::warn!("{} errno={} {}", message, errno, errno_text(errno));
}
