use super::common::*;
use crate::hook::diagnostic;
use crate::hook::monitor;
use crate::hook::ops::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::hook::runtime;
use crate::hook::stats::InterceptHub;
use crate::redirect::record_redirect_hit;
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t};
use std::ffi::CString;

// FuseDaemon 通过 mknod 创建文件节点，必须 hook
pub unsafe extern "C" fn hooked_mknod(
    pathname: *const c_char,
    mode: mode_t,
    dev: libc::dev_t,
) -> c_int {
    let self_ptr = hooked_mknod as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::mknod(pathname, mode, dev),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, mode_t, libc::dev_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, mode, dev)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_mknod_like(hub, "mknod", AT_FDCWD, pathname, mode, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::mknod(call_path, mode, dev),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, mode_t, libc::dev_t) -> c_int =
                            std::mem::transmute(prev);
                        f(call_path, mode, dev)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_mknodat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: mode_t,
    dev: libc::dev_t,
) -> c_int {
    let self_ptr = hooked_mknodat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::syscall(libc::SYS_mknodat, dirfd, pathname, mode, dev) as c_int,
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        mode_t,
                        libc::dev_t,
                    ) -> c_int = std::mem::transmute(prev);
                    f(dirfd, pathname, mode, dev)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_mknod_like(hub, "mknodat", dirfd, pathname, mode, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::syscall(libc::SYS_mknodat, dirfd, call_path, mode, dev) as c_int,
                    |prev| {
                        let f: unsafe extern "C" fn(
                            c_int,
                            *const c_char,
                            mode_t,
                            libc::dev_t,
                        ) -> c_int = std::mem::transmute(prev);
                        f(dirfd, call_path, mode, dev)
                    },
                )
            })
        },
    )
}

// 统一按文件创建事件记录
fn handle_mknod_like<F>(
    hub: &InterceptHub,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    mode: mode_t,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char) -> c_int,
{
    if pathname.is_null() {
        return call_original(pathname);
    }

    // SAFETY: pathname 已在上方判空，prepare_relevant_path 会按 NUL 结尾读取该 C 字符串并解析路径，指针有效性由 hook 调用方保证。
    let PreparedPath::Ready {
        path_for_decision, ..
    } = (unsafe { prepare_relevant_path(hub, op_name, dirfd, pathname, mode as i32, false) })
    else {
        return call_original(pathname);
    };

    diagnostic::log_diag_path_event(
        hub,
        op_name,
        "input",
        path_for_decision.as_ref(),
        mode as i32,
    );

    if hub.is_monitor_only() && !should_enforce_monitor_only_writer_policy(hub) {
        let result = call_original(pathname);
        let error_no = runtime::errno_for_result(result);
        monitor::record_mkdir_result(hub, op_name, path_for_decision.as_ref(), result, error_no);
        return result;
    }

    let redirect_result = process_redirect_path_for_mutation(hub, path_for_decision.as_ref());
    diagnostic::log_diag_redirect_decision(
        hub,
        op_name,
        path_for_decision.as_ref(),
        &redirect_result,
    );
    if redirect_result.is_denied() {
        return deny_read_only_mkdir(
            hub,
            op_name,
            path_for_decision.as_ref(),
            &redirect_result.new_path,
        );
    }

    let result = if redirect_result.is_redirect() {
        record_redirect_hit(
            hub,
            op_name,
            path_for_decision.as_ref(),
            &redirect_result.new_path,
        );
        runtime::ensure_redirect_parent_dirs(&redirect_result.new_path, 0o2770);
        if let Ok(c_path) = CString::new(redirect_result.new_path.as_str()) {
            call_original(c_path.as_ptr())
        } else {
            call_original(pathname)
        }
    } else {
        call_original(pathname)
    };
    let error_no = runtime::errno_for_result(result);
    if redirect_result.is_redirect() {
        monitor::record_mkdir_result_from(
            hub,
            op_name,
            &redirect_result.new_path,
            path_for_decision.as_ref(),
            result,
            error_no,
        );
    } else {
        monitor::record_mkdir_result(hub, op_name, path_for_decision.as_ref(), result, error_no);
    }
    result
}
