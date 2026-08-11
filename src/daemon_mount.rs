use crate::domain::PathMapping;
use crate::fuse_redirect::{FuseRedirectConfig, mount_blocking_with_ready};
use crate::mount::MountPlanner;
use crate::mount_status_marker::write_mount_status_marker;
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{fs, module_paths, paths};
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
const MAX_STUCK_MOUNT_CHILDREN: usize = 2;
const STUCK_MOUNT_SKIP_LOG_STEP: u64 = 32;

static ACTIVE_MOUNT_PIDS: Lazy<Mutex<HashSet<i32>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static LAST_SUCCESS_BY_PID: Lazy<Mutex<HashMap<i32, (u64, u64)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static STUCK_MOUNT_CHILDREN: Lazy<Mutex<Vec<i32>>> = Lazy::new(|| Mutex::new(Vec::new()));
static STUCK_MOUNT_SKIP_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

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
    pub is_fuse_daemon_redirect_enabled: bool,
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
    fn is_fuse_daemon_redirect_enabled(&self) -> bool {
        self.is_fuse_daemon_redirect_enabled
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
    let state_path = state_file_path(request);
    if std::fs::metadata(&state_path).is_err() {
        return false;
    }
    // 记录过 FUSE 服务却已经死掉时，挂载点会留在目标 namespace 里变成 ENOTCONN 死挂载，
    // 应用访问会直接失败。此时把挂载状态视为无效，让周期 reconcile 重新执行挂载；
    // 若 FUSE 再次启动失败，启动阶段的 mount namespace 降级会接管。
    !has_dead_fuse_child(&state_path, request)
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
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("daemon pruned stale mount states count={}", removed);
    }
    removed
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
        return true;
    }
    false
}

pub fn execute_mount_request(request: &MountRequest) -> bool {
    let started_ms = monotonic_ms();
    if should_skip_for_stuck_children(request) {
        return false;
    }
    let Some(_guard) = MountPidGuard::try_acquire(request) else {
        return recently_mounted(request);
    };
    let is_success = run_mount_in_forked_child(request);
    if is_success {
        remember_successful_mount(request);
        if request.operation == MountOperation::Reload {
            let _ =
                write_mount_status_marker(&request.app_data_dir, request.pid, request.uid, true);
        }
    }
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms >= DAEMON_MOUNT_SLOW_MS || !is_success {
        log::info!(
            "daemon mount pkg={} pid={} op={:?} ok={} allow={} excl={} sandbox={} ro={} map={} map_only={} fuse_daemon={} ms={}",
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
            request.is_fuse_daemon_redirect_enabled,
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

fn should_skip_for_stuck_children(request: &MountRequest) -> bool {
    let stuck = prune_stuck_mount_children();
    if stuck <= MAX_STUCK_MOUNT_CHILDREN {
        return false;
    }

    let count = STUCK_MOUNT_SKIP_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 8 || count.is_multiple_of(STUCK_MOUNT_SKIP_LOG_STEP) {
        log::warn!(
            "daemon mount circuit open stuck_children={} pkg={} pid={} op={:?} n={}",
            stuck,
            request.package_name,
            request.pid,
            request.operation,
            count
        );
    }
    true
}

/// 清理已经卡住的挂载子进程，返回仍未回收的数量。
///
/// `waitpid` 与 `kill` 都是可能被信号打断、耗时不确定的系统调用，绝不能在持有全局
/// 挂载状态锁时执行：挂载请求线程也要拿同一把锁，一旦回收阶段变慢，所有请求都会
/// 跟着阻塞。因此这里先在锁内取走整份待清理列表，立即释放锁，在锁外完成回收，
/// 最后再把仍然存活的子进程合并回列表。
fn prune_stuck_mount_children() -> usize {
    let pending = {
        let mut children = STUCK_MOUNT_CHILDREN
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if children.is_empty() {
            return 0;
        }
        std::mem::take(&mut *children)
    };

    let mut alive = Vec::with_capacity(pending.len());
    for child in pending {
        let mut status = 0;
        // SAFETY: status 是栈上有效的整数，指针在调用期间保持有效。
        let ret = unsafe { waitpid(child, &mut status, WNOHANG) };
        if ret == child {
            log::warn!(
                "daemon stuck child finally reaped child={} status={}",
                child,
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
                "daemon stuck child waitpid failed child={} errno={} {}",
                child,
                errno,
                errno_text(errno)
            );
            alive.push(child);
            continue;
        }
        // SAFETY: kill 只接收整型参数，不涉及借用指针。
        let _ = unsafe { libc::kill(child, SIGKILL) };
        alive.push(child);
    }

    // 回收期间其它线程可能又登记了新的卡住子进程，这里只做合并，不覆盖。
    let mut children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    for child in alive {
        if !children.contains(&child) {
            children.push(child);
        }
    }
    children.len()
}

fn remember_stuck_mount_child(child: i32) {
    let mut children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if !children.contains(&child) {
        children.push(child);
    }
    log::warn!(
        "daemon mount child stuck child={} stuck_children={}",
        child,
        children.len()
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
        return handle_parent_process(child, sockets[0], parent_timeout_sec);
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

    if !clear_previous_mounts(plan) {
        log::warn!(
            "daemon mount cleanup incomplete pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }

    if request.operation == MountOperation::Disable {
        let _ = send_mount_result(sock, 0);
        unsafe { close(sock) };
        return true;
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
        if !fuse_roots.is_empty() {
            log::info!(
                "daemon hybrid fuse roots pkg={} pid={} enabled={} count={}",
                request.package_name,
                request.pid,
                request.is_fuse_daemon_redirect_enabled,
                fuse_roots.len()
            );
            for root in fuse_roots {
                log::info!("daemon hybrid fuse root {}", root);
            }
        }
        let fuse_children = if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, fuse_roots, planner.real_storage_anchor()) {
                Some(children) => children,
                None => {
                    log::warn!(
                        "daemon hybrid fuse scoped service failed pid={} pkg={}",
                        request.pid,
                        request.package_name
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let hybrid_degraded = !fuse_roots.is_empty() && fuse_children.is_empty();
        if hybrid_degraded {
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
        if !write_mount_state(request, plan, &mounted_targets, &fuse_children) {
            log::warn!("daemon mount state save failed pid={}", request.pid);
        }
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
    let can_record_fallback = request.is_file_monitor_enabled
        && planner.can_record_read_only_mapping_denials(
            &request.path_mappings,
            &request.read_only_paths,
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
            &request.read_only_paths,
            &[],
        )
    } else {
        planner.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            &[],
        )
    }
}

fn start_scoped_fuse_services(
    request: &MountRequest,
    roots: &[String],
    real_root_override: Option<String>,
) -> Option<Vec<FuseMountState>> {
    if roots.is_empty() {
        return Some(Vec::new());
    }

    let mut states = Vec::with_capacity(roots.len());
    for root in roots {
        match start_fuse_service_for_root(request, root, real_root_override.clone()) {
            Some(state) => states.push(state),
            None => {
                rollback_scoped_fuse_services(&states);
                return None;
            }
        }
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
        terminate_fuse_child(state.child);
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
        terminate_fuse_child(service_child);
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

fn handle_parent_process(child: i32, sock: c_int, primary_timeout_sec: i64) -> bool {
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
        remember_stuck_mount_child(child);
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

fn clear_previous_mounts(plan: &MountForkPlan) -> bool {
    let state_path = plan.state_path.as_str();
    let fuse_children = read_fuse_children(state_path);
    let mut targets = read_mount_targets(state_path);
    targets.extend(plan.overlay_targets.iter().cloned());
    let targets = module_paths::normalize_mount_targets(&targets);
    if targets.is_empty() && fuse_children.is_empty() {
        return true;
    }
    let mut ok = true;
    for target in targets.iter().rev() {
        if !clear_mount_target_stack(target) {
            ok = false;
        }
    }
    for child in &fuse_children {
        terminate_recorded_fuse_child(child);
    }
    let _ = std::fs::remove_file(state_path);
    ok
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
        if unsafe { umount2(c_target.as_ptr(), MNT_DETACH) } == 0 {
            passes += 1;
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

fn is_mount_stack_cleared(mounted_count: usize) -> bool {
    mounted_count == 0
}

fn current_mount_target_count(target: &str) -> usize {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|content| mount_target_count_from_mountinfo(&content, target))
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
        append_resolved_storage_alias_targets(
            &mut targets,
            request,
            &mapping.request_path,
            user_id,
            &storage_root,
        );
    }

    module_paths::normalize_mount_targets(&targets)
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
    let user_str = user_id.to_string();
    let storage_root = paths::storage_user_root_for_user(user_id);
    if !paths::is_same_or_child(canonical_path, &storage_root) {
        return vec![canonical_path.to_string()];
    }

    let suffix = &canonical_path[storage_root.len()..];
    // 这里的别名根都是按固定规则构造的互不相同的字面量，无需再逐个线性去重；
    // 最终的过滤、排序与去重统一由 normalize_targets 完成。
    // 不再展开 /data/media/<user>：该前缀必定被 is_safe_mount_target 拒绝，
    // 生成后只会被 normalize_targets 丢弃，属于无效分配与比较。
    let mut alias_roots = Vec::with_capacity(14);
    alias_roots.push(storage_root);
    alias_roots.push("/storage/self/primary".to_string());
    if user_id == 0 {
        alias_roots.push("/storage/emulated/legacy".to_string());
    }
    alias_roots.push(format!("/mnt/user/{}/emulated/{}", user_str, user_str));
    alias_roots.push(format!("/mnt/runtime/default/emulated/{}", user_str));
    alias_roots.push(format!("/mnt/runtime/read/emulated/{}", user_str));
    alias_roots.push(format!("/mnt/runtime/write/emulated/{}", user_str));
    alias_roots.push(format!("/mnt/runtime/full/emulated/{}", user_str));
    alias_roots.push(format!("/mnt/installer/{}/emulated/{}", user_str, user_str));
    alias_roots.push(format!("/mnt/installer/emulated/{}", user_str));
    alias_roots.push(format!(
        "/mnt/androidwritable/{}/emulated/{}",
        user_str, user_str
    ));
    alias_roots.push(format!("/mnt/androidwritable/emulated/{}", user_str));
    alias_roots.push(format!(
        "/mnt/pass_through/{}/emulated/{}",
        user_str, user_str
    ));
    alias_roots.push(format!("/mnt/pass_through/emulated/{}", user_str));

    for root in &mut alias_roots {
        root.push_str(suffix);
    }
    alias_roots
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

fn mount_target_count_from_mountinfo(content: &str, target: &str) -> usize {
    content
        .lines()
        .filter(|line| mountinfo_line_target_matches(line, target))
        .count()
}

/// 卸载重试循环会按次数反复统计挂载点，这里逐行比较挂载目标字段：
/// 字段为纯 ASCII 且不含转义时直接按原始切片比较，只有含转义的行才展开为 String。
fn mountinfo_line_target_matches(line: &str, target: &str) -> bool {
    let Some(raw_target) = parse_mountinfo_raw_target(line) else {
        return false;
    };
    if raw_target.is_ascii() && !raw_target.contains('\\') {
        return raw_target == target;
    }
    unescape_mountinfo_field(raw_target) == target
}

fn parse_mountinfo_raw_target(line: &str) -> Option<&str> {
    let separator = line.find(" - ")?;
    let before_separator = &line[..separator];
    let mut fields = before_separator.split_whitespace();
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let _major_minor = fields.next()?;
    let _root = fields.next()?;
    fields.next()
}

fn unescape_mountinfo_field(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = &value[index + 1..index + 4];
            if octal.as_bytes().iter().all(|ch| (b'0'..=b'7').contains(ch))
                && let Ok(code) = u8::from_str_radix(octal, 8)
            {
                out.push(code as char);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
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

fn terminate_recorded_fuse_child(child: &FuseChildIdentity) {
    let Some(start_time_ticks) = child.start_time_ticks else {
        log::warn!(
            "daemon skip legacy fuse child signal without identity pid={}",
            child.pid
        );
        return;
    };
    if !crate::platform::is_process_instance_alive(child.pid, start_time_ticks) {
        return;
    }
    terminate_fuse_child(child.pid);
}

fn terminate_fuse_child(pid: i32) {
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
        if ret == pid || ret < 0 {
            return;
        }
        unsafe { libc::usleep(10 * 1000) };
    }
    let _ = unsafe { libc::kill(pid, SIGKILL) };
    let mut status = 0;
    let _ = unsafe { waitpid(pid, &mut status, WNOHANG) };
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
