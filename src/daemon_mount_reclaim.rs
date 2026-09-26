// 挂载子进程回收与状态文件清理。
//
// 这里只负责「已经不在正常流程里的东西怎么收尾」：卡死子进程的熔断与回收、
// 过期挂载状态文件的清理。它们不参与挂载决策，只在外围兜底；
// 相关全局槽位也一并放在这里，主流程文件不再直接触碰这些状态。

use crate::daemon_mount::{
    MountOperation, MountRequest, decode_wait_status, execute_mount_request, log_errno,
    read_fuse_children, read_fuse_host_session, terminate_recorded_fuse_child,
};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::module_paths;
use crate::platform::paths::monotonic_ms;
use libc::{SIGKILL, WNOHANG, waitpid};
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

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

pub(crate) fn state_value<'a>(content: &'a str, prefix: &str) -> Option<&'a str> {
    content.lines().find_map(|line| line.strip_prefix(prefix))
}

pub(crate) fn state_file_pid(path: &std::path::Path, package_name: &str) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    let prefix = format!("{}_", module_paths::sanitize_name(package_name));
    stem.strip_prefix(&prefix)?.parse().ok()
}

pub(crate) fn legacy_state_owner_is_alive(
    pid: i32,
    package_name: &str,
    expected_uid: Option<i32>,
) -> bool {
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
pub(crate) fn has_dead_fuse_child(state_path: &str, request: &MountRequest) -> bool {
    // 接入共享宿主会话的挂载不写 `fuse_child=`，只有 `fuse_host=`。会话死亡时本应用留下的
    // 是一条 ENOTCONN 死挂载：路径仍在挂载表里、状态看上去健康，但访问全部失败。必须让它
    // 与 scoped 子进程消失走同一判定——视为状态失效，触发重挂并在重挂里换到新会话。
    if let Some(host) = read_fuse_host_session(state_path) {
        if host
            .start_time_ticks
            .is_some_and(|start| crate::platform::is_process_instance_alive(host.pid, start))
        {
            return false;
        }
        log::warn!(
            "daemon fuse host session gone host_pid={} app_pid={} pkg={}, remount pending",
            host.pid,
            request.pid,
            request.package_name
        );
        return true;
    }

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

/// 判定本次挂载请求是否因为卡死的挂载子进程而熔断。
///
/// 判据只统计与本次请求同一包名的卡死子进程，因此单个应用的挂载超时只会让该应用
/// 的后续请求被跳过，不会连坐整机其它应用的挂载。
pub(crate) fn should_skip_for_stuck_children(request: &MountRequest) -> bool {
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
pub(crate) fn stuck_mount_child_counts(package_name: &str) -> (usize, usize) {
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
pub(crate) fn prune_stuck_mount_children() {
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

pub(crate) fn remember_stuck_mount_child(child: i32, package_name: &str) {
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

pub(crate) fn reap_child(child: i32, nonblocking: bool) -> bool {
    let mut status = 0;
    let options = if nonblocking { WNOHANG } else { 0 };
    let attempts = if nonblocking { 20 } else { 1 };
    for attempt in 0..attempts {
        // SAFETY: status 为本作用域独占的可变 i32，waitpid 只写这一处；child 为本次 fork 返回的 pid。
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
            // SAFETY: usleep 无副作用、不触碰任何 Rust 内存，仅让出 10ms 再重试回收。
            unsafe { libc::usleep(10 * 1000) };
        }
    }
    log::warn!("daemon child not reaped child={}", child);
    false
}
