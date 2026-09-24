mod diagnostics;
mod mount_state;
mod stats;
mod sys;

use super::companion_request::CompanionMountRequest;
use super::mount_timing;
use crate::config::SettingsHub;
use crate::fuse_redirect::{
    FuseRedirectConfig, ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount,
    log_scoped_mount_roots, mount_blocking_with_ready,
};
use crate::mount::MountPlanner;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{self, paths::monotonic_ms};
use diagnostics::log_child_diagnostics;
use libc::{
    AF_UNIX, CLONE_NEWNS, MNT_DETACH, MS_BIND, O_CLOEXEC, O_RDONLY, SIGKILL, SIGTERM, SO_RCVTIMEO,
    SOCK_DGRAM, SOL_SOCKET, WNOHANG, c_int, c_void, close, kill, mount, open, read, readlink, recv,
    send, setns, setsockopt, socketpair, umount2, waitpid,
};
use stats::update_redirect_stats;
use std::ffi::{CStr, CString};
use sys::{c_str, decode_wait_status, errno_text, last_errno};

// 等待目标进程就绪后在子进程中执行挂载
pub fn execute_companion_mount_request(request: &CompanionMountRequest) -> bool {
    let started_ms = monotonic_ms();
    if !is_redirect_enabled_for_request(request) {
        crate::mount_intent::mark_state(
            &request.package_name,
            request.pid,
            request.uid,
            request.storage_backend_mode,
            request.config_version,
            "disabled",
        );
        log::warn!(
            "companion mount denied redirect disabled pkg={} uid={} pid={}",
            request.package_name,
            request.uid,
            request.pid
        );
        log_companion_mount_perf(request, false, 0, 0, started_ms);
        return false;
    }
    let wait_started_ms = monotonic_ms();
    let user_id = platform::user_id_from_uid(request.uid);
    let is_ready = wait_for_process_ready(
        request.pid,
        user_id,
        mount_timing::COMPANION_PROCESS_READY_TIMEOUT_MS,
    );
    let wait_ms = monotonic_ms().saturating_sub(wait_started_ms);
    if !is_ready {
        log::warn!("wait proc not ready pid={}", request.pid);
    }
    let mount_started_ms = monotonic_ms();
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        "applying",
    );
    let is_success = run_mount_in_forked_child(request);
    let mount_ms = monotonic_ms().saturating_sub(mount_started_ms);
    crate::mount_intent::mark_state(
        &request.package_name,
        request.pid,
        request.uid,
        request.storage_backend_mode,
        request.config_version,
        if is_success { "mounted" } else { "failed" },
    );
    log_companion_mount_perf(request, is_success, wait_ms, mount_ms, started_ms);
    is_success
}

fn is_redirect_enabled_for_request(request: &CompanionMountRequest) -> bool {
    let config = SettingsHub::instance();
    if !config.init(None) {
        log::warn!(
            "companion mount config init failed pkg={} uid={} pid={}",
            request.package_name,
            request.uid,
            request.pid
        );
        return false;
    }
    config.reload_if_changed();
    config.should_redirect(&request.package_name, request.uid)
}

fn log_companion_mount_perf(
    request: &CompanionMountRequest,
    is_success: bool,
    wait_ms: i64,
    mount_ms: i64,
    started_ms: i64,
) {
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms < mount_timing::COMPANION_MOUNT_SLOW_MS && is_success {
        return;
    }
    log::info!(
        "perf companion mount pkg={} pid={} uid={} ok={} allow={} ro={} map={} map_only={} wait_ms={} mount_ms={} total_ms={}",
        request.package_name,
        request.pid,
        request.uid,
        is_success,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        wait_ms,
        mount_ms,
        total_ms
    );
}

// 切换到目标进程的挂载命名空间
fn set_mount_namespace(pid: i32, ns_path: Option<&CStr>) -> bool {
    // 路径在 fork 之前就已经转换好，这里只做 open/setns，避免子进程再次堆分配。
    let Some(c_path) = ns_path else {
        log::error!("ns path invalid pid={}", pid);
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

fn wait_for_process_ready(pid: i32, user_id: i32, timeout_ms: i32) -> bool {
    let poll_interval_us = 5 * 1000;
    let timeout_us = timeout_ms * 1000;
    let mut elapsed_us = 0;

    while elapsed_us < timeout_us {
        if is_process_context_ready(pid) {
            // storage_mount 只用于调试日志，未开启调试日志时不做整份 mountinfo 的读取与扫描。
            if crate::logging::is_debug_logging_enabled() {
                let is_storage_mount_ready = is_process_storage_mount_ready(pid, user_id);
                log::debug!(
                    "proc ready pid={} wait_us={} storage_mount={}",
                    pid,
                    elapsed_us,
                    is_storage_mount_ready
                );
            }
            return true;
        }

        unsafe { libc::usleep(poll_interval_us as u32) };
        elapsed_us += poll_interval_us;
    }

    log::warn!(
        "proc ready timeout pid={} ms={} ctx=false storage_mount=false",
        pid,
        timeout_ms
    );
    false
}

// 轮询目标进程 SELinux 上下文，等待脱离 zygote 状态
fn is_process_context_ready(pid: i32) -> bool {
    let attr_path = format!("/proc/{}/attr/current", pid);

    let Ok(c_path) = std::ffi::CString::new(attr_path.clone()) else {
        log::warn!("attr path invalid pid={}", pid);
        return false;
    };

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
    if n == 0 {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&buf[..n as usize]) else {
        log::warn!("attr not utf8 pid={} bytes={}", pid, n);
        return false;
    };
    let context = text.trim();
    if context.contains("zygote") {
        return false;
    }
    log::debug!("proc ctx ready pid={} ctx={}", pid, context);
    true
}

fn is_process_storage_mount_ready(pid: i32, user_id: i32) -> bool {
    let path = format!("/proc/{}/mountinfo", pid);
    let Ok(content) = std::fs::read_to_string(&path) else {
        let errno = last_errno();
        log::warn!(
            "mountinfo read failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    };
    let storage_user = format!(" /storage/emulated/{user_id} ");
    let mnt_user = format!(" /mnt/user/{user_id}/emulated/{user_id} ");
    let has_storage = content
        .lines()
        .any(|line| line.contains(" /storage/emulated ") || line.contains(&storage_user));
    let has_mnt_user = content.lines().any(|line| line.contains(&mnt_user));
    if has_storage && has_mnt_user {
        log::debug!("proc storage mount ready pid={} user={}", pid, user_id);
        return true;
    }
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
fn handle_parent_process(child: i32, sock: c_int, primary_timeout_sec: i64) -> bool {
    set_recv_timeout(sock, child, primary_timeout_sec);

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
fn handle_child_process(
    request: &CompanionMountRequest,
    plan: &CompanionMountForkPlan,
    sock: c_int,
) -> bool {
    if !set_mount_namespace(request.pid, plan.mount_namespace_path.as_deref()) {
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

    let scoped_fuse_roots = plan.scoped_fuse_roots.as_slice();
    let is_success = if request.is_mapping_mode_only {
        log::info!("map-only mount count={}", request.path_mappings.len());
        mount_mgr.apply_path_mappings_only(
            &request.path_mappings,
            &request.sandboxed_paths,
            &request.read_only_paths,
            scoped_fuse_roots,
        )
    } else {
        mount_mgr.apply_sdcard_redirect(
            &request.allowed_real_paths,
            &request.excluded_real_paths,
            &request.read_only_paths,
            &request.path_mappings,
            scoped_fuse_roots,
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
        log_scoped_mount_roots(
            "hybrid fuse",
            &request.package_name,
            request.pid,
            fuse_roots,
        );
        // 与 daemon 侧同源：闸门判定用同一个函数，只是在这里取出来让 selection_reason 能区分
        // 「能力未放行」与「规则本来不需要 FUSE 根」。
        let gate_allowed = crate::fuse_redirect::config::scoped_mount_allowed_for_scope(
            &request.package_name,
            request.storage_backend_mode,
        );
        let (fuse_children, attempt) = if !fuse_roots.is_empty() {
            match start_scoped_fuse_services(request, fuse_roots, mount_mgr.real_storage_anchor()) {
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
            log_prefix: "hybrid fuse",
            roots_planned: fuse_roots.len(),
            sessions: fuse_children.len(),
            attempt,
            ready_reason: "companion_scoped_mount_ready",
            failed_reason: "companion_scoped_mount_failed",
        });
        if outcome.needs_namespace_fallback {
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
        // 与 daemon 侧同源、同顺序：挂载完成后按平台需要摘掉系统 MediaProvider 的 app data
        // isolation FUSE 视图，再补回被 MNT_DETACH 级联摘掉的映射子路径 bind。摘除逻辑此前只
        // 写在 daemon 路径里，走 companion 的应用（普通应用正是这条）在 x86_64 Android 13/14
        // 上继续失败。顺序约束见 crate::system_fuse_view 模块文档。
        if crate::system_fuse_view::should_clear_system_fuse_view_for_platform() {
            crate::system_fuse_view::log_view_stack_for_package(
                request.uid,
                &request.package_name,
                "before_clear",
            );
            crate::system_fuse_view::clear_system_fuse_view_for_package(
                request.uid,
                &request.package_name,
            );
            if !request.path_mappings.is_empty() {
                mount_mgr.reapply_path_mappings_only(&request.path_mappings);
            }
            crate::system_fuse_view::log_view_stack_for_package(
                request.uid,
                &request.package_name,
                "after_clear",
            );
        }
        let mounted_targets = mount_mgr.take_mounted_targets();
        if !mount_state::write_mount_state(request, plan, &mounted_targets, &fuse_children) {
            log::warn!(
                "mount state save failed pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
        // 账本与状态文件职责不同：状态文件回答"要摘哪些路径"，账本回答"那些挂载归谁"。
        // 这条路径此前只写状态文件、不写账本，于是走 companion 的应用在恢复流程与 doctor 里
        // 没有任何归属判据，只能退化成"按挂载源判定"这个较弱的判据。两条挂载路径必须都登记，
        // 否则同一件事在两条路径上的结果不一致——这正是本项目反复踩的坑。
        let mut identity_targets = mounted_targets.clone();
        identity_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
        if !crate::mount_ledger::record_mount_identity(
            "companion",
            &request.package_name,
            request.pid,
            &identity_targets,
        ) {
            log::warn!(
                "mount identity save failed pid={} pkg={}",
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
    // Scoped FUSE 是优先采用的可记录只读路径。当已挂载的真实存储 FUSE 锚点
    // 能覆盖只读映射时，保留文件监视，使 MediaProvider/FUSE 仍可生成拒绝记录。
    // 否则使用强制只读绑定，避免写入被静默放行。
    // 主方案已经装好的 bind/overlay 必须先卸载。降级路径会对同一批目标重新执行挂载，
    // 若保留旧挂载会在同一目标上再叠一层，导致挂载栈重复、卸载顺序错乱。
    let detached = mount_mgr.unmount_recorded_targets();
    if detached > 0 {
        log::info!(
            "hybrid fuse namespace fallback rollback count={} pid={} pkg={}",
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
        && mount_mgr.can_record_read_only_mapping_denials(
            &request.path_mappings,
            &read_only_paths,
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
            &read_only_paths,
            &[],
        )
    } else {
        mount_mgr.apply_sdcard_redirect(
            &allowed_real_paths,
            &request.excluded_real_paths,
            &read_only_paths,
            &request.path_mappings,
            &[],
        )
    }
}

#[derive(Clone)]
pub(super) struct FuseMountState {
    pub target: String,
    pub child: i32,
    pub child_start_time_ticks: u64,
}

/// 按根启动 scoped FUSE 服务；单根失败只丢弃该根。
///
/// daemon 侧 [`crate::daemon_mount`] 有一份平行实现，两处必须保持一致：它们写入的是
/// 同一份 FUSE 能力快照，任一侧整组回滚都会把失败计入全局预算，累计到上限后把所有
/// 应用的 scoped 会话一起打回 mount namespace。
fn start_scoped_fuse_services(
    request: &CompanionMountRequest,
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
            "fuse partial scoped mount pkg={} pid={} mounted={} failed={} failed_roots={}",
            request.package_name,
            request.pid,
            states.len(),
            failed_roots.len(),
            failed_roots.join(",")
        );
        // 规划阶段已跳过这些预期 FUSE 根对应的 bind；部分失败时先收回已启动会话，
        // 再以单个存储根会话保留原始规则的动态匹配，避免失败根变成无规则覆盖。
        let user_id = platform::user_id_from_uid(request.uid);
        let storage_root = platform::paths::storage_user_root_for_user(user_id);
        rollback_scoped_fuse_services(&states);
        if let Some(state) = start_fuse_service_for_root(request, &storage_root, real_root_override)
        {
            log::warn!(
                "fuse partial roots collapsed to storage root pkg={} pid={} failed={}",
                request.package_name,
                request.pid,
                failed_roots.len()
            );
            return Some(vec![state]);
        }
        log::warn!(
            "fuse partial roots and storage-root retry failed pkg={} pid={}",
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
                        "fuse rollback umount failed target={} errno={} {}",
                        state.target,
                        errno,
                        errno_text(errno)
                    );
                }
            }
        }
        terminate_fuse_service(
            state.child,
            (state.child_start_time_ticks != 0).then_some(state.child_start_time_ticks),
        );
    }
}

fn scoped_fuse_mount_roots(request: &CompanionMountRequest) -> Vec<String> {
    crate::fuse_redirect::scoped_fuse_mount_roots_for_request(request)
}

fn start_fuse_service_for_root(
    request: &CompanionMountRequest,
    mount_root: &str,
    real_root_override: Option<String>,
) -> Option<FuseMountState> {
    // B2-b：优先尝试共享宿主会话接入。
    if let Some(host) = crate::fuse_host::get_fuse_host() {
        // 先把该应用的策略按 uid 登记进共享宿主会话：这是应用接入的前置条件，提前登记也让
        // 接入启用后第一帧请求就带上正确策略。虚拟根取整个存储根（mount_root=None），因为
        // 宿主会话服务的是完整存储视图，而不是某个 scoped 子根。
        let policy_config = fuse_config_from_request(request, None, real_root_override.clone());
        let registered = crate::fuse_host::register_app_policy(&policy_config);
        if !crate::fuse_host::can_attach_app(request.uid) {
            // 宿主会话还没有该 uid 的策略，接入会让应用失去重定向；保持既有 scoped 路径。
            log::debug!(
                "fuse host attach skipped pid={} pkg={} registered={} reason=policy_registration_pending",
                request.pid,
                request.package_name,
                registered
            );
        } else if let Some(state) = try_bind_to_fuse_host(&host, request, mount_root) {
            return Some(state);
        } else {
            log::warn!(
                "bind to fuse host failed, falling back to scoped fork pid={} pkg={}",
                request.pid,
                request.package_name
            );
        }
    }

    // 回退：fork 独立 scoped 会话（B2-a 前的既有路径）。
    let mut ready_sockets = [0; 2];
    // SAFETY: socketpair 系统调用，传入有效的栈数组指针。
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

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
    // SAFETY: fork 系统调用，已经通过 prepare_for_fork 避免日志通道竞态。
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
        // SAFETY: ready_sockets 是有效 fd，fork 失败后父进程负责清理。
        unsafe {
            close(ready_sockets[0]);
            close(ready_sockets[1]);
        }
        return None;
    }

    if service_child == 0 {
        // SAFETY: 子进程关闭继承的 fd。
        unsafe {
            close(ready_sockets[0]);
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
    set_recv_timeout(
        ready_sockets[0],
        service_child,
        mount_timing::FUSE_READY_TIMEOUT_SEC,
    );
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    // SAFETY: 父进程关闭读端 fd。
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
        terminate_fuse_service(service_child, None);
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

/// B2-b：通过 `setns` + `MS_BIND` 将应用接入共享宿主 FUSE 会话（companion 路径）。
fn try_bind_to_fuse_host(
    host: &crate::fuse_host::FuseHost,
    request: &CompanionMountRequest,
    mount_root: &str,
) -> Option<FuseMountState> {
    use std::ffi::CString;

    // 构造宿主 namespace 路径。
    let ns_path = format!("/proc/{}/ns/mnt", host.child_pid);
    let Ok(c_ns_path) = CString::new(ns_path.as_bytes()) else {
        log::error!("host ns path invalid");
        return None;
    };

    // fork 子进程执行 setns + MS_BIND。
    let mut ready_sockets = [0; 2];
    // SAFETY: socketpair 系统调用，传入有效的栈数组指针。
    if unsafe { socketpair(AF_UNIX, SOCK_DGRAM, 0, ready_sockets.as_mut_ptr()) } != 0 {
        let errno = last_errno();
        log::warn!(
            "host bind socketpair failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        return None;
    }

    crate::logging::prepare_for_fork();
    // SAFETY: fork 系统调用，已经通过 prepare_for_fork 避免日志通道竞态。
    let bind_child = unsafe { libc::fork() };
    if bind_child < 0 {
        let errno = last_errno();
        log::warn!(
            "host bind fork failed pid={} pkg={} errno={} {}",
            request.pid,
            request.package_name,
            errno,
            errno_text(errno)
        );
        // SAFETY: ready_sockets 是有效 fd，fork 失败后父进程负责清理。
        unsafe {
            close(ready_sockets[0]);
            close(ready_sockets[1]);
        }
        return None;
    }

    if bind_child == 0 {
        // SAFETY: 子进程关闭继承的 fd。
        unsafe {
            close(ready_sockets[0]);
        }
        // 子进程：setns 进入宿主 namespace，然后 MS_BIND 挂载。
        let ok = perform_host_bind_companion(
            &c_ns_path,
            &host.mount_point,
            mount_root,
            ready_sockets[1],
        );
        // SAFETY: 子进程直接退出，不执行析构函数（避免 fork 后的资源清理问题）。
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }

    // SAFETY: 父进程关闭写端 fd。
    unsafe { close(ready_sockets[1]) };
    set_recv_timeout(
        ready_sockets[0],
        bind_child,
        mount_timing::FUSE_READY_TIMEOUT_SEC,
    );
    let mut ready_result: i32 = -1;
    let expected = std::mem::size_of::<i32>() as isize;
    let n = recv_result(ready_sockets[0], &mut ready_result);
    // SAFETY: 父进程关闭读端 fd。
    unsafe { close(ready_sockets[0]) };
    if n != expected || ready_result != 0 {
        log::warn!(
            "host bind not ready child={} recv={} ret={} pid={} pkg={}",
            bind_child,
            n,
            ready_result,
            request.pid,
            request.package_name
        );
        // SAFETY: bind_child 是有效 pid，发送 SIGTERM 终止子进程。
        let _ = unsafe { kill(bind_child, SIGTERM) };
        let mut status = 0;
        // SAFETY: waitpid 等待子进程结束，避免僵尸进程。
        unsafe { waitpid(bind_child, &mut status, 0) };
        return None;
    }

    // 返回宿主会话的 FuseMountState（复用宿主 pid/start_time）。
    Some(FuseMountState {
        target: mount_root.to_string(),
        child: host.child_pid,
        child_start_time_ticks: host.child_start_time_ticks,
    })
}

/// 在子进程内执行 setns + MS_BIND + 确认挂载 + 发送 ready 信号（companion 路径）。
fn perform_host_bind_companion(
    ns_path: &std::ffi::CStr,
    host_mount_point: &str,
    app_mount_root: &str,
    ready_sock: c_int,
) -> bool {
    use std::ffi::CString;

    // Step 1: setns 进入宿主 namespace。
    // SAFETY: open 系统调用，ns_path 是有效的 C 字符串指针。
    let fd = unsafe { open(ns_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "host bind ns open failed errno={} {}",
            errno,
            errno_text(errno)
        );
        return false;
    }
    // SAFETY: setns 系统调用，fd 是有效的 namespace 文件描述符。
    if unsafe { setns(fd, CLONE_NEWNS) } != 0 {
        let errno = last_errno();
        log::warn!(
            "host bind setns failed errno={} {}",
            errno,
            errno_text(errno)
        );
        // SAFETY: close 系统调用，fd 是有效的文件描述符。
        unsafe { close(fd) };
        return false;
    }
    // SAFETY: close 系统调用，fd 是有效的文件描述符。
    unsafe { close(fd) };

    // Step 2: mkdir 应用挂载目标（允许已存在）。
    let Ok(c_target) = CString::new(app_mount_root.as_bytes()) else {
        log::error!("host bind target path invalid");
        return false;
    };
    // SAFETY: mkdir 系统调用，c_target 是有效的 C 字符串指针；失败时忽略（可能已存在）。
    unsafe {
        libc::mkdir(c_target.as_ptr(), 0o755);
    }

    // Step 3: MS_BIND 从宿主挂载点绑定到应用目标。
    let Ok(c_source) = CString::new(host_mount_point.as_bytes()) else {
        log::error!("host bind source path invalid");
        return false;
    };
    // SAFETY: mount 系统调用，c_source/c_target 是有效的 C 字符串指针。
    if unsafe {
        mount(
            c_source.as_ptr(),
            c_target.as_ptr(),
            std::ptr::null(),
            MS_BIND,
            std::ptr::null(),
        )
    } != 0
    {
        let errno = last_errno();
        log::warn!(
            "host bind mount MS_BIND failed errno={} {}",
            errno,
            errno_text(errno)
        );
        return false;
    }

    // Step 4: 发送 ready 信号。
    let ready: i32 = 0;
    // SAFETY: send 系统调用，ready_sock 是有效 fd，传入栈变量指针。
    let sent = unsafe {
        libc::send(
            ready_sock,
            &ready as *const i32 as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    // SAFETY: close 系统调用，ready_sock 是有效 fd。
    unsafe { close(ready_sock) };
    if sent != std::mem::size_of::<i32>() as isize {
        let errno = last_errno();
        log::warn!(
            "host bind send ready failed errno={} {}",
            errno,
            errno_text(errno)
        );
        return false;
    }

    // Step 5: 保持子进程存活（否则 MS_BIND 挂载会随进程退出消失）。
    loop {
        // SAFETY: pause 系统调用，挂起进程等待信号。
        unsafe { libc::pause() };
    }
}

fn terminate_fuse_service(pid: i32, start_time_ticks: Option<u64>) {
    if !process_identity_alive(pid, start_time_ticks) {
        return;
    }
    // SIGTERM 失败通常说明子进程已经退出成僵尸或权限受限，此时仍然必须回收；
    // 直接返回会把僵尸进程留在伴生进程下，长期运行会耗尽进程表。
    // SAFETY: kill 只接收整型参数，不涉及借用指针。
    let term_failed = unsafe { kill(pid, SIGTERM) } != 0;
    if term_failed {
        let errno = last_errno();
        log::debug!(
            "fuse service SIGTERM failed child={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
    }
    for _ in 0..30 {
        let mut status: c_int = 0;
        let wait_ret = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
        if wait_ret == pid {
            return;
        }
        // `waitpid` 返回负值只说明当前进程无法回收该目标，不表示目标已经退出
        // （FUSE 服务子进程由挂载 worker fork，worker 退出后由 init 收养，此后
        // 固定得到 ECHILD）。把它当作已退出会直接跳过下面的 SIGKILL 升级，留下
        // 长期存活并空转的残留服务进程；这里改用 `/proc` 存活探测。
        if !process_identity_alive(pid, start_time_ticks) {
            return;
        }
        unsafe { libc::usleep(10 * 1000) };
    }
    // SAFETY: kill 只接收整型参数，不涉及借用指针。
    let _ = unsafe { kill(pid, SIGKILL) };
    for _ in 0..30 {
        let mut status: c_int = 0;
        // SAFETY: status 是栈上有效的 c_int，指针在调用期间保持有效。
        let wait_ret = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
        if wait_ret == pid {
            return;
        }
        // SIGKILL 之后同样不能依赖 `waitpid` 判断目标是否消失，否则会对已经退出
        // 但无法回收的目标误报残留。
        if !process_identity_alive(pid, start_time_ticks) {
            return;
        }
        // SAFETY: usleep 只接收整型参数，不涉及借用指针。
        unsafe { libc::usleep(10 * 1000) };
    }
    log::warn!("fuse service still alive after SIGKILL child={}", pid);
}

fn process_identity_alive(pid: i32, start_time_ticks: Option<u64>) -> bool {
    match start_time_ticks {
        Some(start) => platform::is_process_instance_alive(pid, start),
        None => platform::process_exists(pid),
    }
}

fn fuse_config_from_request(
    request: &CompanionMountRequest,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    crate::fuse_redirect::fuse_config_from_request(request, mount_root, real_root_override)
}

/// fork 之前在父进程算好的挂载计划。
///
/// 子进程只保留调用线程，不能依赖其它父线程在 fork 瞬间持有的 malloc arena 或全局锁，
/// 所以所有可以提前得到的路径字符串都放在这里，子进程直接复用已分配好的内容。
struct CompanionMountForkPlan {
    /// 需要交给 FUSE 服务接管的挂载根，父子进程共用同一份结果。
    scoped_fuse_roots: Vec<String>,
    /// 挂载状态文件路径。
    state_path: String,
    /// 挂载状态文件的临时写入路径。
    temp_state_path: String,
    /// 目标进程的 mount namespace 路径，已提前转换为 C 字符串。
    mount_namespace_path: Option<CString>,
}

impl CompanionMountForkPlan {
    fn build(request: &CompanionMountRequest) -> Self {
        let state_path = mount_state::state_file_path(request);
        let temp_state_path = format!("{}.tmp", state_path);
        Self {
            scoped_fuse_roots: scoped_fuse_mount_roots(request),
            state_path,
            temp_state_path,
            mount_namespace_path: CString::new(format!("/proc/{}/ns/mnt", request.pid)).ok(),
        }
    }
}

// 通过 socketpair 创建子进程执行挂载操作
fn run_mount_in_forked_child(request: &CompanionMountRequest) -> bool {
    // fork 之后的子进程只保留调用线程。此时若再做堆分配、首次初始化或获取全局锁，
    // 可能因为其它父线程在 fork 瞬间持有 malloc arena 或全局锁而永久阻塞。
    // 因此把可以提前算出的字符串与路径列表全部在父进程算好，子进程只做 setns/mount/write。
    let plan = CompanionMountForkPlan::build(request);
    let scoped_fuse_root_count = plan.scoped_fuse_roots.len();
    let parent_timeout_sec =
        mount_timing::companion_parent_recv_primary_timeout_sec(scoped_fuse_root_count);
    log::info!(
        "mount prep pid={} uid={} pkg={} allow={} ro={} map={} map_only={} parent_recv_budget_sec={}",
        request.pid,
        request.uid,
        request.package_name,
        request.allowed_real_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        mount_timing::companion_parent_recv_budget_sec(scoped_fuse_root_count)
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

    // 先在父进程走完私有日志通道初始化，避免子进程继承处于初始化中的 OnceLock 而永久阻塞。
    crate::logging::prepare_for_fork();
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
        return handle_parent_process(child, sockets[0], parent_timeout_sec);
    }

    log::debug!(
        "child start pid={} pkg={}",
        request.pid,
        request.package_name
    );
    unsafe { close(sockets[0]) };
    let sock = sockets[1];
    let is_success = handle_child_process(request, &plan, sock);
    unsafe { libc::_exit(if is_success { 0 } else { 1 }) };
}
