use crate::domain::PathMapping;
use crate::fuse_redirect::{
    FuseRedirectConfig, mount_blocking_with_ready, scoped_mount_roots_for_hybrid_rules,
};
use crate::mount::MountPlanner;
use crate::mount_status_marker::write_mount_status_marker;
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
const FUSE_READY_TIMEOUT_SEC: i64 = 1;
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

pub fn has_mount_state(request: &MountRequest) -> bool {
    std::fs::metadata(state_file_path(request)).is_ok()
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
    if count <= 8 || count % STUCK_MOUNT_SKIP_LOG_STEP == 0 {
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

fn prune_stuck_mount_children() -> usize {
    let mut children = STUCK_MOUNT_CHILDREN
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if children.is_empty() {
        return 0;
    }

    children.retain(|child| {
        let mut status = 0;
        let ret = unsafe { waitpid(*child, &mut status, WNOHANG) };
        if ret == *child {
            log::warn!(
                "daemon stuck child finally reaped child={} status={}",
                child,
                decode_wait_status(status)
            );
            false
        } else if ret < 0 {
            let errno = last_errno();
            if errno == libc::ECHILD || errno == libc::ESRCH {
                false
            } else {
                log::warn!(
                    "daemon stuck child waitpid failed child={} errno={} {}",
                    child,
                    errno,
                    errno_text(errno)
                );
                true
            }
        } else {
            let _ = unsafe { libc::kill(*child, SIGKILL) };
            true
        }
    });
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

fn run_mount_in_forked_child(request: &MountRequest) -> bool {
    let mut sockets = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, sockets.as_mut_ptr()) } != 0 {
        log_errno("daemon socketpair failed");
        return false;
    }

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
        return handle_parent_process(child, sockets[0]);
    }

    unsafe { close(sockets[0]) };
    let ok = handle_child_process(request, sockets[1]);
    unsafe { libc::_exit(if ok { 0 } else { 1 }) };
}

fn handle_child_process(request: &MountRequest, sock: c_int) -> bool {
    if !set_mount_namespace(request.pid) {
        let _ = send_mount_result(sock, -1);
        unsafe { close(sock) };
        return false;
    }

    if !clear_previous_mounts(request) {
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
    let scoped_fuse_roots = scoped_fuse_mount_roots(request);
    let ok = if request.is_mapping_mode_only {
        planner.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            &scoped_fuse_roots,
        )
    } else {
        planner.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            &scoped_fuse_roots,
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
            for root in &fuse_roots {
                log::info!("daemon hybrid fuse root {}", root);
            }
        }
        let fuse_children = if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, &fuse_roots) {
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
        if !write_mount_state(request, &mounted_targets, &fuse_children) {
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
    // Scoped FUSE was the preferred recordable read-only path. When the
    // already-mounted real-storage FUSE anchor can cover the read-only mapping,
    // keep file monitoring enabled so MediaProvider/FUSE can still emit the
    // denial record. Otherwise use a hard read-only bind so writes cannot slip
    // through silently.
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
) -> Option<Vec<FuseMountState>> {
    if roots.is_empty() {
        return Some(Vec::new());
    }

    let mut states = Vec::with_capacity(roots.len());
    for root in roots {
        match start_fuse_service_for_root(request, &root) {
            Some(state) => states.push(state),
            None => {
                for state in &states {
                    terminate_fuse_child(state.child);
                }
                return None;
            }
        }
    }
    Some(states)
}

fn scoped_fuse_mount_roots(request: &MountRequest) -> Vec<String> {
    if !request.is_fuse_daemon_redirect_enabled {
        return Vec::new();
    }

    scoped_mount_roots_for_hybrid_rules(
        request.uid,
        &request.allowed_real_paths,
        &request.excluded_real_paths,
        &request.sandboxed_paths,
        &request.read_only_paths,
        &request.path_mappings,
        request.is_mapping_mode_only,
    )
}

fn start_fuse_service_for_root(request: &MountRequest, mount_root: &str) -> Option<FuseMountState> {
    let mut ready_sockets = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        log_errno("daemon fuse ready socketpair failed");
        return None;
    }

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
            fuse_config_from_request(request, Some(mount_root.to_string())),
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

    Some(FuseMountState {
        target: mount_root.to_string(),
        child: service_child,
    })
}

fn set_mount_namespace(pid: i32) -> bool {
    let ns_path = format!("/proc/{}/ns/mnt", pid);
    let Ok(c_path) = CString::new(ns_path) else {
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

fn handle_parent_process(child: i32, sock: c_int) -> bool {
    set_recv_timeout(sock, PARENT_RECV_TIMEOUT_SEC);
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

fn clear_previous_mounts(request: &MountRequest) -> bool {
    let state_path = state_file_path(request);
    let fuse_children = read_fuse_child_pids(&state_path);
    let mut targets = read_mount_targets(&state_path);
    targets.extend(request_overlay_targets(request));
    let targets = normalize_targets(&targets);
    for pid in &fuse_children {
        terminate_fuse_child(*pid);
    }
    if targets.is_empty() && fuse_children.is_empty() {
        return true;
    }
    let mut ok = true;
    for target in targets.iter().rev() {
        if !clear_mount_target_stack(target) {
            ok = false;
        }
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

    normalize_targets(&targets)
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
    for target in expand_storage_alias_paths_for_user(&resolved, user_id) {
        append_unique_target(targets, target);
    }
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
    let mut alias_roots = Vec::with_capacity(13);
    append_unique_target(&mut alias_roots, storage_root);
    append_unique_target(
        &mut alias_roots,
        paths::data_media_user_root_for_user(user_id),
    );
    append_unique_target(&mut alias_roots, "/storage/self/primary".to_string());
    if user_id == 0 {
        append_unique_target(&mut alias_roots, "/storage/emulated/legacy".to_string());
    }
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/user/{}/emulated/{}", user_str, user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/runtime/default/emulated/{}", user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/runtime/read/emulated/{}", user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/runtime/write/emulated/{}", user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/installer/{}/emulated/{}", user_str, user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/installer/emulated/{}", user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/androidwritable/{}/emulated/{}", user_str, user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/androidwritable/emulated/{}", user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/pass_through/{}/emulated/{}", user_str, user_str),
    );
    append_unique_target(
        &mut alias_roots,
        format!("/mnt/pass_through/emulated/{}", user_str),
    );

    let mut expanded = Vec::with_capacity(alias_roots.len());
    for root in alias_roots {
        append_unique_target(&mut expanded, format!("{}{}", root, suffix));
    }
    expanded
}

fn append_unique_target(list: &mut Vec<String>, value: String) {
    if value.is_empty() || list.iter().any(|item| item == &value) {
        return;
    }
    list.push(value);
}

#[derive(Clone)]
struct FuseMountState {
    target: String,
    child: i32,
}

fn fuse_config_from_request(
    request: &MountRequest,
    mount_root: Option<String>,
) -> FuseRedirectConfig {
    FuseRedirectConfig {
        package_name: request.package_name.clone(),
        uid: request.uid,
        app_data_dir: request.app_data_dir.clone(),
        redirect_target: request.redirect_target.clone(),
        mount_root,
        is_file_monitor_enabled: request.is_file_monitor_enabled,
        allowed_real_paths: request.allowed_real_paths.clone(),
        excluded_real_paths: request.excluded_real_paths.clone(),
        sandboxed_paths: request.sandboxed_paths.clone(),
        read_only_paths: request.read_only_paths.clone(),
        path_mappings: request.path_mappings.clone(),
        is_mapping_mode_only: request.is_mapping_mode_only,
    }
}

fn write_mount_state(
    request: &MountRequest,
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
    let state_path = state_file_path(request);
    let Ok(c_path) = CString::new(state_path.clone()) else {
        return false;
    };
    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        log::warn!(
            "daemon mount state open failed path={} errno={} {}",
            state_path,
            last_errno(),
            errno_text(last_errno())
        );
        return false;
    }
    let mut content = String::new();
    content.push_str(&format!("version={}\n", request.config_version));
    content.push_str(&format!("package={}\n", request.package_name));
    content.push_str(&format!("uid={}\n", request.uid));
    for state in fuse_children {
        content.push_str(&format!("fuse_child={}\n", state.child));
    }
    let mut all_targets = targets.to_vec();
    all_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
    for target in normalize_targets(&all_targets) {
        content.push_str("target=");
        content.push_str(&target);
        content.push('\n');
    }
    let ok = fs::write_all(fd, content.as_bytes());
    unsafe {
        libc::close(fd);
        let _ = libc::chmod(c_path.as_ptr(), 0o600);
    }
    if ok {
        log::info!(
            "daemon mount state saved pid={} targets={} path={}",
            request.pid,
            targets.len(),
            state_path
        );
    }
    ok
}

fn state_file_path(request: &MountRequest) -> String {
    format!(
        "{}/{}_{}.state",
        module_paths::MOUNT_STATE_DIR,
        sanitize_name(&request.package_name),
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
        .filter(|target| is_safe_mount_target(target))
        .map(ToString::to_string)
        .collect()
}

fn mount_target_count_from_mountinfo(content: &str, target: &str) -> usize {
    content
        .lines()
        .filter_map(parse_mountinfo_target)
        .filter(|mount_target| mount_target == target)
        .count()
}

fn parse_mountinfo_target(line: &str) -> Option<String> {
    let separator = line.find(" - ")?;
    let before_separator = &line[..separator];
    let mut fields = before_separator.split_whitespace();
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let _major_minor = fields.next()?;
    let _root = fields.next()?;
    fields.next().map(unescape_mountinfo_field)
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

fn read_fuse_child_pids(path: &str) -> Vec<i32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("fuse_child=")
                .and_then(|pid| pid.parse::<i32>().ok())
                .filter(|pid| *pid > 0)
        })
        .collect()
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

fn normalize_targets(targets: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = targets
        .iter()
        .filter(|target| is_safe_mount_target(target))
        .cloned()
        .collect();
    normalized.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| b.cmp(a)));
    normalized.dedup();
    normalized
}

fn is_safe_mount_target(target: &str) -> bool {
    if target.is_empty() || target.contains('\0') || target.contains("/../") {
        return false;
    }
    target.starts_with("/storage/")
        || target.starts_with("/mnt/")
        || target.starts_with(module_paths::REAL_STORAGE_TMP_PREFIX)
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}

fn errno_text(code: i32) -> String {
    unsafe {
        CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_stacked_mountinfo_targets() {
        let content = "\
1 0 0:1 / /storage/emulated/0 rw - ext4 /dev/block/dm-0 rw\n\
2 1 0:207 /0/Download/QQ /storage/emulated/0/Download/QQ rw - fuse /dev/fuse rw\n\
3 2 0:207 /0/Download/QQ /storage/emulated/0/Download/QQ rw - fuse /dev/fuse rw\n\
4 1 0:207 /0/DCIM /storage/emulated/0/DCIM rw - fuse /dev/fuse rw\n";

        assert_eq!(
            mount_target_count_from_mountinfo(content, "/storage/emulated/0/Download/QQ"),
            2
        );
        assert_eq!(
            mount_target_count_from_mountinfo(content, "/storage/emulated/0/DCIM"),
            1
        );
    }

    #[test]
    fn zero_mount_count_is_already_cleared() {
        assert!(is_mount_stack_cleared(0));
        assert!(!is_mount_stack_cleared(1));
    }

    #[test]
    fn unescapes_mountinfo_target_field() {
        let line = "10 1 0:2 /foo /storage/emulated/0/My\\040Docs rw - fuse /dev/fuse rw";

        assert_eq!(
            parse_mountinfo_target(line).as_deref(),
            Some("/storage/emulated/0/My Docs")
        );
    }
}
