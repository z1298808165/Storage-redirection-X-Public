use super::common::*;
use crate::hook::runtime;
use crate::hook::util::c_str_to_string;
use crate::monitor::OpKind;
use libc::{AT_FDCWD, c_char, c_int, c_void};

pub unsafe extern "C" fn hooked_link(oldpath: *const c_char, newpath: *const c_char) -> c_int {
    let self_ptr = hooked_link as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::link(oldpath, newpath),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                        std::mem::transmute(prev);
                    f(oldpath, newpath)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_link_audit(
                hub,
                LinkAuditRequest {
                    op_name: "link",
                    olddirfd: AT_FDCWD,
                    oldpath,
                    newdirfd: AT_FDCWD,
                    newpath,
                    flags: -1,
                },
                |call_old, call_new| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::link(call_old, call_new),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                                std::mem::transmute(prev);
                            f(call_old, call_new)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_linkat(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_linkat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::linkat(olddirfd, oldpath, newdirfd, newpath, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        c_int,
                        *const c_char,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(olddirfd, oldpath, newdirfd, newpath, flags)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_link_audit(
                hub,
                LinkAuditRequest {
                    op_name: "linkat",
                    olddirfd,
                    oldpath,
                    newdirfd,
                    newpath,
                    flags,
                },
                |call_old, call_new| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::linkat(olddirfd, call_old, newdirfd, call_new, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                c_int,
                                *const c_char,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(olddirfd, call_old, newdirfd, call_new, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_symlink(target: *const c_char, linkpath: *const c_char) -> c_int {
    let self_ptr = hooked_symlink as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::symlink(target, linkpath),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                        std::mem::transmute(prev);
                    f(target, linkpath)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            // SAFETY: target 是 symlink 调用传入的原始 C 字符串指针，c_str_to_string 内部处理空指针并按 NUL 结尾读取，指针有效性由调用方保证。
            let target_text = unsafe { c_str_to_string(target) };
            let extra = if target_text.is_empty() {
                None
            } else {
                Some(format!("from={}", target_text))
            };
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Symlink,
                    op_name: "symlink",
                    dirfd: AT_FDCWD,
                    pathname: linkpath,
                    log_flags: 0,
                    extra_tail: extra,
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::symlink(target, call_path),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                                std::mem::transmute(prev);
                            f(target, call_path)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_symlinkat(
    target: *const c_char,
    newdirfd: c_int,
    linkpath: *const c_char,
) -> c_int {
    let self_ptr = hooked_symlinkat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::symlinkat(target, newdirfd, linkpath),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, c_int, *const c_char) -> c_int =
                        std::mem::transmute(prev);
                    f(target, newdirfd, linkpath)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            // SAFETY: target 是 symlinkat 调用传入的原始 C 字符串指针，c_str_to_string 内部处理空指针并按 NUL 结尾读取，指针有效性由调用方保证。
            let target_text = unsafe { c_str_to_string(target) };
            let extra = if target_text.is_empty() {
                None
            } else {
                Some(format!("from={}", target_text))
            };
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Symlink,
                    op_name: "symlinkat",
                    dirfd: newdirfd,
                    pathname: linkpath,
                    log_flags: 0,
                    extra_tail: extra,
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::symlinkat(target, newdirfd, call_path),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                *const c_char,
                                c_int,
                                *const c_char,
                            ) -> c_int = std::mem::transmute(prev);
                            f(target, newdirfd, call_path)
                        },
                    )
                },
            )
        },
    )
}
