mod diagnostics;
mod mount_state;
mod stats;
mod sys;

use super::companion_request::CompanionMountRequest;
use super::mount_timing;
use crate::fuse_redirect::{
    FuseRedirectConfig, mount_blocking_with_ready, scoped_mount_roots_for_hybrid_rules,
};
use crate::mount::MountPlanner;
use crate::mount_status_marker::write_mount_status_marker;
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use diagnostics::log_child_diagnostics;
use libc::{
    AF_UNIX, CLONE_NEWNS, O_CLOEXEC, O_RDONLY, SIGKILL, SIGTERM, SO_RCVTIMEO, SOCK_DGRAM,
    SOL_SOCKET, WNOHANG, c_int, c_void, close, kill, open, read, readlink, recv, send, setns,
    setsockopt, socketpair, waitpid,
};
use stats::update_redirect_stats;
use sys::{c_str, decode_wait_status, errno_text, last_errno};

const FUSE_READY_TIMEOUT_SEC: i64 = 1;

// 等待目标进程就绪后在子进程中执行挂载
pub fn execute_companion_mount_request(request: &CompanionMountRequest) -> bool {
    let started_ms = monotonic_ms();
    let wait_started_ms = monotonic_ms();
    let is_ready = wait_for_process(
        request.pid,
        mount_timing::COMPANION_PROCESS_READY_TIMEOUT_MS,
    );
    let wait_ms = monotonic_ms().saturating_sub(wait_started_ms);
    if !is_ready {
        log::warn!("wait proc not ready pid={}", request.pid);
    }
    let mount_started_ms = monotonic_ms();
    let is_success = run_mount_in_forked_child(request);
    let mount_ms = monotonic_ms().saturating_sub(mount_started_ms);
    let marker_started_ms = monotonic_ms();
    let marker_ok =
        write_mount_status_marker(&request.app_data_dir, request.pid, request.uid, is_success);
    let marker_ms = monotonic_ms().saturating_sub(marker_started_ms);
    log_companion_mount_perf(
        request, is_success, marker_ok, wait_ms, mount_ms, marker_ms, started_ms,
    );
    is_success
}

fn log_companion_mount_perf(
    request: &CompanionMountRequest,
    is_success: bool,
    marker_ok: bool,
    wait_ms: i64,
    mount_ms: i64,
    marker_ms: i64,
    started_ms: i64,
) {
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms < mount_timing::COMPANION_MOUNT_SLOW_MS && is_success && marker_ok {
        return;
    }
    log::info!(
        "perf companion mount pkg={} pid={} uid={} ok={} marker={} allow={} ro={} map={} map_only={} fuse_daemon={} wait_ms={} mount_ms={} marker_ms={} total_ms={}",
        request.package_name,
        request.pid,
        request.uid,
        is_success,
        marker_ok,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        request.is_fuse_daemon_redirect_enabled,
        wait_ms,
        mount_ms,
        marker_ms,
        total_ms
    );
}

// 切换到目标进程的挂载命名空间
fn set_mount_namespace(pid: i32) -> bool {
    let ns_path = format!("/proc/{}/ns/mnt", pid);
    let Ok(c_path) = std::ffi::CString::new(ns_path.clone()) else {
        log::error!("ns path invalid pid={} path={}", pid, ns_path);
        return false;
    };
    let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        let errno = last_errno();
        log::error!(
            "ns open failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }
    let file = UniqueFd::new(fd);

    if unsafe { setns(file.get(), CLONE_NEWNS) } != 0 {
        let errno = last_errno();
        log::error!(
            "setns failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }

    log::info!("entered ns pid={}", pid);
    let mut buf = [0u8; 256];
    let Some(self_ns_path) = c_str("/proc/self/ns/mnt") else {
        log::warn!("ns readlink path failed");
        return true;
    };
    let len = unsafe {
        readlink(
            self_ns_path.as_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len() - 1,
        )
    };
    if len > 0 {
        buf[len as usize] = 0;
        let text = String::from_utf8_lossy(&buf[..len as usize]);
        log::info!("ns now={}", text);
    } else {
        let errno = last_errno();
        log::warn!(
            "ns read failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
    }
    true
}

// 轮询目标进程 SELinux 上下文，等待脱离 zygote 状态
fn wait_for_process(pid: i32, timeout_ms: i32) -> bool {
    let poll_interval_us = 5 * 1000;
    let timeout_us = timeout_ms * 1000;
    let mut elapsed_us = 0;
    let attr_path = format!("/proc/{}/attr/current", pid);
    let mut last_context = String::new();

    let Ok(c_path) = std::ffi::CString::new(attr_path.clone()) else {
        log::warn!("attr path invalid pid={}", pid);
        return false;
    };

    while elapsed_us < timeout_us {
        let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
        if fd < 0 {
            let errno = last_errno();
            log::warn!(
                "attr open failed pid={} errno={} {}",
                pid,
                errno,
                errno_text(errno)
            );
            return false;
        }
        let file = UniqueFd::new(fd);
        let mut buf = [0u8; 256];
        let n = unsafe { read(file.get(), buf.as_mut_ptr() as *mut c_void, buf.len() - 1) };
        if n < 0 {
            let errno = last_errno();
            log::warn!(
                "attr read failed pid={} errno={} {}",
                pid,
                errno,
                errno_text(errno)
            );
            return false;
        }
        if n > 0 {
            if let Ok(text) = std::str::from_utf8(&buf[..n as usize]) {
                let context = text.trim().to_string();
                last_context = context.clone();
                if !context.contains("zygote") {
                    log::debug!("proc ctx ready pid={} ctx={}", pid, context);
                    return true;
                }
            } else {
                log::warn!("attr not utf8 pid={} bytes={}", pid, n);
            }
        }

        unsafe { libc::usleep(poll_interval_us as u32) };
        elapsed_us += poll_interval_us;
    }

    log::warn!(
        "proc ctx timeout pid={} ms={} last={}",
        pid,
        timeout_ms,
        if last_context.is_empty() {
            "<empty>"
        } else {
            &last_context
        }
    );
    false
}

fn send_mount_result(sock: c_int, result: i32) -> bool {
    let expected_size = std::mem::size_of::<i32>() as isize;
    let sent = unsafe {
        send(
            sock,
            &result as *const _ as *const c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    if sent != expected_size {
        if sent < 0 {
            let errno = last_errno();
            log::warn!(
                "send result failed sock={} errno={} {}",
                sock,
                errno,
                errno_text(errno)
            );
        } else {
            log::warn!(
                "send result short sock={} sent={} want={}",
                sock,
                sent,
                expected_size
            );
        }
        return false;
    }
    log::debug!("send result sock={} ret={}", sock, result);
    true
}

// 父进程等待子进程挂载结果并回收子进程
fn handle_parent_process(child: i32, sock: c_int) -> bool {
    set_recv_timeout(
        sock,
        child,
        mount_timing::COMPANION_PARENT_RECV_PRIMARY_TIMEOUT_SEC,
    );

    let mut result: i32 = -1;
    let expected_size = std::mem::size_of::<i32>() as isize;
    let mut n = recv_result(sock, &mut result);
    let mut should_reap_nonblocking = false;

    // 主超时未拿到结果时按 SIGTERM -> grace -> SIGKILL 渐进推进，
    // 避免在子进程仍持有 mount writer 时立刻 SIGKILL 损伤 FUSE 状态。
    if n != expected_size {
        log_recv_failure(child, n, expected_size, "primary");
        log_child_diagnostics(child, "primary_timeout");

        if unsafe { kill(child, SIGTERM) } != 0 {
            let errno = last_errno();
            log::warn!(
                "term child failed child={} errno={} {}",
                child,
                errno,
                errno_text(errno)
            );
        }

        set_recv_timeout(
            sock,
            child,
            mount_timing::COMPANION_PARENT_RECV_GRACE_TIMEOUT_SEC,
        );
        n = recv_result(sock, &mut result);
        if n == expected_size {
            log::warn!("child late result child={} ret={}", child, result);
        } else {
            log_recv_failure(child, n, expected_size, "grace");
            log_child_diagnostics(child, "grace_timeout");
            log::warn!("child stuck after term child={} forcing kill", child);
            should_reap_nonblocking = true;
            if unsafe { kill(child, SIGKILL) } != 0 {
                let errno = last_errno();
                log::warn!(
                    "kill child failed child={} errno={} {}",
                    child,
                    errno,
                    errno_text(errno)
                );
            }
        }
    }
    unsafe { close(sock) };

    reap_child(child, should_reap_nonblocking);

    let is_success = result == 0;
    if is_success {
        update_redirect_stats();
    } else {
        log::warn!("mount failed child={} recv={} ret={}", child, n, result);
    }
    is_success
}

fn reap_child(child: i32, nonblocking: bool) {
    let mut status: c_int = 0;
    let options = if nonblocking { WNOHANG } else { 0 };
    let attempts = if nonblocking { 20 } else { 1 };
    for attempt in 0..attempts {
        let wait_ret = unsafe { waitpid(child, &mut status as *mut _, options) };
        if wait_ret < 0 {
            let errno = last_errno();
            log::warn!(
                "waitpid failed child={} errno={} {}",
                child,
                errno,
                errno_text(errno)
            );
            return;
        }
        if wait_ret > 0 {
            log::info!(
                "child reaped child={} status={} raw={}",
                child,
                decode_wait_status(status),
                status
            );
            return;
        }
        if !nonblocking {
            break;
        }
        if attempt + 1 < attempts {
            unsafe { libc::usleep(10 * 1000) };
        }
    }

    log::warn!(
        "child not reaped child={} status=still_running reason=nonblocking_timeout",
        child
    );
}

fn set_recv_timeout(sock: c_int, child: i32, seconds: i64) {
    let tv = libc::timeval {
        tv_sec: seconds,
        tv_usec: 0,
    };
    let opt_ret = unsafe {
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &tv as *const _ as *const c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        )
    };
    if opt_ret != 0 {
        let errno = last_errno();
        log::warn!(
            "setsockopt failed child={} sec={} errno={} {}",
            child,
            seconds,
            errno,
            errno_text(errno)
        );
    }
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

fn log_recv_failure(child: i32, n: isize, expected: isize, phase: &str) {
    if n < 0 {
        let errno = last_errno();
        log::warn!(
            "recv result failed child={} phase={} errno={} {}",
            child,
            phase,
            errno,
            errno_text(errno)
        );
    } else {
        log::warn!(
            "recv result short child={} phase={} recv={} want={}",
            child,
            phase,
            n,
            expected
        );
    }
}

// 子进程切换命名空间并执行实际挂载
fn handle_child_process(request: &CompanionMountRequest, sock: c_int) -> bool {
    if !set_mount_namespace(request.pid) {
        log::error!(
            "child setns failed pid={} pkg={}",
            request.pid,
            request.package_name
        );
        let _ = send_mount_result(sock, -1);
        unsafe { close(sock) };
        return false;
    }

    let mut mount_mgr = MountPlanner::new(
        &request.package_name,
        request.uid,
        &request.app_data_dir,
        &request.redirect_target,
        false,
    );
    mount_mgr.set_file_monitor_enabled(request.is_file_monitor_enabled);

    let scoped_fuse_roots = scoped_fuse_mount_roots(request);
    let is_success = if request.is_mapping_mode_only {
        log::info!("map-only mount count={}", request.path_mappings.len());
        mount_mgr.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            &scoped_fuse_roots,
        )
    } else {
        mount_mgr.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            &scoped_fuse_roots,
        )
    };

    let result = if is_success { 0 } else { -1 };
    if !is_success {
        log::warn!(
            "child mount failed pid={} pkg={} map_only={}",
            request.pid,
            request.package_name,
            request.is_mapping_mode_only
        );
    } else {
        let fuse_roots = scoped_fuse_roots;
        if !fuse_roots.is_empty() {
            log::info!(
                "hybrid fuse roots pkg={} pid={} enabled={} count={}",
                request.package_name,
                request.pid,
                request.is_fuse_daemon_redirect_enabled,
                fuse_roots.len()
            );
            for root in &fuse_roots {
                log::info!("hybrid fuse root {}", root);
            }
        }
        let fuse_children = if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, &fuse_roots) {
                Some(children) => children,
                None => {
                    log::warn!(
                        "hybrid fuse scoped service failed pid={} pkg={}",
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
                "hybrid fuse no scoped service mounted, fallback to mount namespace pid={} pkg={}",
                request.pid,
                request.package_name
            );
            if !apply_mount_namespace_fallback(&mut mount_mgr, request) {
                log::warn!(
                    "hybrid fuse namespace fallback failed pid={} pkg={}",
                    request.pid,
                    request.package_name
                );
            }
        }
        let mounted_targets = mount_mgr.take_mounted_targets();
        if !mount_state::write_mount_state(request, &mounted_targets, &fuse_children) {
            log::warn!(
                "mount state save failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        if !send_mount_result(sock, 0) {
            log::warn!(
                "child send result failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        unsafe { close(sock) };
        return true;
    }
    log::info!(
        "companion mount {} pid={}",
        if is_success { "ok" } else { "fail" },
        request.pid
    );

    if !send_mount_result(sock, result) {
        log::warn!(
            "child send result failed pid={} pkg={}",
            request.pid,
            request.package_name
        );
    }
    unsafe { close(sock) };
    is_success
}

fn apply_mount_namespace_fallback(
    mount_mgr: &mut MountPlanner,
    request: &CompanionMountRequest,
) -> bool {
    // Scoped FUSE was the preferred recordable read-only path. When the
    // already-mounted real-storage FUSE anchor can cover the read-only mapping,
    // keep file monitoring enabled so MediaProvider/FUSE can still emit the
    // denial record. Otherwise use a hard read-only bind so writes cannot slip
    // through silently.
    let can_record_fallback = request.is_file_monitor_enabled
        && mount_mgr.can_record_read_only_mapping_denials(
            &request.path_mappings,
            &request.read_only_paths,
            &request.excluded_real_paths,
        );
    mount_mgr.set_file_monitor_enabled(can_record_fallback);
    log::info!(
        "hybrid fuse namespace fallback file_monitor={} pid={} pkg={}",
        can_record_fallback,
        request.pid,
        request.package_name
    );
    if request.is_mapping_mode_only {
        mount_mgr.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            &[],
        )
    } else {
        mount_mgr.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            &[],
        )
    }
}

#[derive(Clone)]
pub(super) struct FuseMountState {
    pub target: String,
    pub child: i32,
}

fn start_scoped_fuse_services(
    request: &CompanionMountRequest,
    roots: &[String],
) -> Option<Vec<FuseMountState>> {
    if roots.is_empty() {
        return Some(Vec::new());
    }

    let mut states = Vec::with_capacity(roots.len());
    for root in roots {
        match start_fuse_service_for_root(request, root) {
            Some(state) => states.push(state),
            None => {
                for state in &states {
                    terminate_fuse_service(state.child);
                }
                return None;
            }
        }
    }
    Some(states)
}

fn scoped_fuse_mount_roots(request: &CompanionMountRequest) -> Vec<String> {
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

fn start_fuse_service_for_root(
    request: &CompanionMountRequest,
    mount_root: &str,
) -> Option<FuseMountState> {
    let mut ready_sockets = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        let errno = last_errno();
        log::warn!(
            "fuse ready socketpair failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        return None;
    }

    let service_child = unsafe { libc::fork() };
    if service_child < 0 {
        let errno = last_errno();
        log::warn!(
            "fuse fork failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
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
    set_recv_timeout(ready_sockets[0], service_child, FUSE_READY_TIMEOUT_SEC);
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    unsafe { close(ready_sockets[0]) };
    if n != expected || ready_result != 0 {
        log::warn!(
            "fuse service not ready child={} recv={} ret={} pid={} pkg={}",
            service_child,
            n,
            ready_result,
            request.pid,
            request.package_name
        );
        terminate_fuse_service(service_child);
        return None;
    }

    Some(FuseMountState {
        target: mount_root.to_string(),
        child: service_child,
    })
}

fn terminate_fuse_service(pid: i32) {
    if unsafe { kill(pid, SIGTERM) } != 0 {
        return;
    }
    for _ in 0..30 {
        let mut status: c_int = 0;
        let wait_ret = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
        if wait_ret == pid || wait_ret < 0 {
            return;
        }
        unsafe { libc::usleep(10 * 1000) };
    }
    let _ = unsafe { kill(pid, SIGKILL) };
    let mut status: c_int = 0;
    let _ = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
}

fn fuse_config_from_request(
    request: &CompanionMountRequest,
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

// 通过 socketpair 创建子进程执行挂载操作
fn run_mount_in_forked_child(request: &CompanionMountRequest) -> bool {
    log::info!(
        "mount prep pid={} uid={} pkg={} allow={} ro={} map={} map_only={} fuse_daemon={} parent_recv_budget_sec={}",
        request.pid,
        request.uid,
        request.package_name,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        request.is_fuse_daemon_redirect_enabled,
        mount_timing::companion_parent_recv_budget_sec()
    );

    let mut sockets = [0; 2];
    let ret = unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, sockets.as_mut_ptr()) };
    if ret != 0 {
        let errno = last_errno();
        log::error!(
            "socketpair failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        return false;
    }

    let child = unsafe { libc::fork() };
    if child < 0 {
        let errno = last_errno();
        log::error!(
            "fork failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        unsafe {
            close(sockets[0]);
            close(sockets[1]);
        }
        return false;
    }

    if child > 0 {
        log::debug!("parent wait child={}", child);
        unsafe { close(sockets[1]) };
        return handle_parent_process(child, sockets[0]);
    }

    log::debug!(
        "child start pid={} pkg={}",
        request.pid,
        request.package_name
    );
    unsafe { close(sockets[0]) };
    let sock = sockets[1];
    let is_success = handle_child_process(request, sock);
    unsafe { libc::_exit(if is_success { 0 } else { 1 }) };
}
