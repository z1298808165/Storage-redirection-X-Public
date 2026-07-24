use super::common::*;
use crate::hook::diagnostic;
use crate::hook::monitor;
use crate::hook::path as path_utils;
use crate::hook::runtime;
use crate::hook::stats::InterceptHub;
use crate::monitor::OpKind;
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t, off_t, timespec};
use std::ffi::CString;

pub unsafe extern "C" fn hooked_truncate(pathname: *const c_char, length: off_t) -> c_int {
    let self_ptr = hooked_truncate as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::truncate(pathname, length),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, off_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, length)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Truncate,
                    op_name: "truncate",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("length={}", length)),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::truncate(call_path, length),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, off_t) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, length)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_truncate64(pathname: *const c_char, length: off_t) -> c_int {
    let self_ptr = hooked_truncate64 as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::truncate(pathname, length),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, off_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, length)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Truncate,
                    op_name: "truncate64",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("length={}", length)),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::truncate(call_path, length),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, off_t) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, length)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_ftruncate(fd: c_int, length: off_t) -> c_int {
    let self_ptr = hooked_ftruncate as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::ftruncate(fd, length),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, off_t) -> c_int = std::mem::transmute(prev);
                    f(fd, length)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_ftruncate_fd_audit(hub, "ftruncate", fd, length, || {
                runtime::call_prev(
                    self_ptr,
                    || libc::ftruncate(fd, length),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, off_t) -> c_int =
                            std::mem::transmute(prev);
                        f(fd, length)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_ftruncate64(fd: c_int, length: off_t) -> c_int {
    let self_ptr = hooked_ftruncate64 as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::ftruncate(fd, length),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, off_t) -> c_int = std::mem::transmute(prev);
                    f(fd, length)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_ftruncate_fd_audit(hub, "ftruncate64", fd, length, || {
                runtime::call_prev(
                    self_ptr,
                    || libc::ftruncate(fd, length),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, off_t) -> c_int =
                            std::mem::transmute(prev);
                        f(fd, length)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_chmod(pathname: *const c_char, mode: mode_t) -> c_int {
    let self_ptr = hooked_chmod as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::chmod(pathname, mode),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, mode_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, mode)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "chmod",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: mode as i32,
                    extra_tail: Some(format!("mode=0{:o}", mode)),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::chmod(call_path, mode),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, mode_t) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, mode)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_fchmod(fd: c_int, mode: mode_t) -> c_int {
    let self_ptr = hooked_fchmod as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::fchmod(fd, mode),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, mode_t) -> c_int = std::mem::transmute(prev);
                    f(fd, mode)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_fd_path_audit(
                hub,
                OpKind::Chmod,
                "fchmod",
                fd,
                Some(format!("mode=0{:o}", mode)),
                || {
                    runtime::call_prev(
                        self_ptr,
                        || libc::fchmod(fd, mode),
                        |prev| {
                            let f: unsafe extern "C" fn(c_int, mode_t) -> c_int =
                                std::mem::transmute(prev);
                            f(fd, mode)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_fchmodat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: mode_t,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_fchmodat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::fchmodat(dirfd, pathname, mode, flags),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, *const c_char, mode_t, c_int) -> c_int =
                        std::mem::transmute(prev);
                    f(dirfd, pathname, mode, flags)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "fchmodat",
                    dirfd,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("mode=0{:o}|flags=0x{:x}", mode, flags)),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::fchmodat(dirfd, call_path, mode, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                mode_t,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, mode, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_utimensat(
    dirfd: c_int,
    pathname: *const c_char,
    times: *const timespec,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_utimensat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::utimensat(dirfd, pathname, times, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        // quality-allow(chinese-language): 函数指针类型注解含必要 C 类型标识符
                        *const timespec,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(dirfd, pathname, times, flags)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Utimens,
                    op_name: "utimensat",
                    dirfd,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("flags=0x{:x}", flags)),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::utimensat(dirfd, call_path, times, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                // quality-allow(chinese-language): 函数指针类型注解含必要 C 类型标识符
                                *const timespec,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, times, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_futimens(fd: c_int, times: *const timespec) -> c_int {
    let self_ptr = hooked_futimens as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::futimens(fd, times),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, *const timespec) -> c_int =
                        std::mem::transmute(prev);
                    f(fd, times)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_futimens_fd_audit(hub, "futimens", fd, times, || {
                runtime::call_prev(
                    self_ptr,
                    || libc::futimens(fd, times),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, *const timespec) -> c_int =
                            std::mem::transmute(prev);
                        f(fd, times)
                    },
                )
            })
        },
    )
}

fn handle_ftruncate_fd_audit<F>(
    hub: &InterceptHub,
    op_name: &str,
    fd: c_int,
    length: off_t,
    call_original: F,
) -> c_int
where
    F: FnOnce() -> c_int,
{
    let path_for_decision = path_utils::resolve_dirfd_path(fd);
    if path_for_decision.is_empty()
        || !path_for_decision.starts_with('/')
        || !path_utils::is_relevant_storage_path(hub, &path_for_decision)
    {
        return call_original();
    }

    let extra_tail = format!("length={}", length);
    diagnostic::log_diag_path_event(hub, op_name, "input", &path_for_decision, fd);
    if should_apply_mutation_policy(hub)
        && deny_read_only_single_path_if_needed(
            hub,
            OpKind::Truncate,
            op_name,
            &path_for_decision,
            Some(&extra_tail),
        )
    {
        return -1;
    }
    fix_system_writer_private_owner_for_mutation(hub, &path_for_decision);
    let mut result = call_original();
    let mut current_errno = runtime::current_errno();
    if let Some(retry_result) = confirm_private_owner_sqlite_ftruncate(
        hub,
        op_name,
        fd,
        length,
        &path_for_decision,
        result,
        current_errno,
    ) {
        if result < 0 {
            result = retry_result;
        }
        current_errno = 0;
    }
    monitor::record_path_operation_result(
        hub,
        OpKind::Truncate,
        op_name,
        &path_for_decision,
        result,
        if result < 0 { current_errno } else { 0 },
        Some(&extra_tail),
    );
    runtime::set_errno(current_errno);
    result
}

fn handle_futimens_fd_audit<F>(
    hub: &InterceptHub,
    op_name: &str,
    fd: c_int,
    times: *const timespec,
    call_original: F,
) -> c_int
where
    F: FnOnce() -> c_int,
{
    let path_for_decision = path_utils::resolve_dirfd_path(fd);
    if path_for_decision.is_empty()
        || !path_for_decision.starts_with('/')
        || !path_utils::is_relevant_storage_path(hub, &path_for_decision)
    {
        return call_original();
    }

    diagnostic::log_diag_path_event(hub, op_name, "input", &path_for_decision, fd);
    if should_apply_mutation_policy(hub)
        && deny_read_only_single_path_if_needed(
            hub,
            OpKind::Utimens,
            op_name,
            &path_for_decision,
            None,
        )
    {
        return -1;
    }
    fix_system_writer_private_owner_for_mutation(hub, &path_for_decision);
    let mut result = call_original();
    let mut current_errno = runtime::current_errno();
    if let Some(retry_result) = confirm_private_owner_sqlite_futimens(
        hub,
        op_name,
        fd,
        times,
        &path_for_decision,
        result,
        current_errno,
    ) {
        if result < 0 {
            result = retry_result;
        }
        current_errno = 0;
    }
    monitor::record_path_operation_result(
        hub,
        OpKind::Utimens,
        op_name,
        &path_for_decision,
        result,
        if result < 0 { current_errno } else { 0 },
        None,
    );
    runtime::set_errno(current_errno);
    result
}

fn confirm_private_owner_sqlite_ftruncate(
    hub: &InterceptHub,
    op_name: &str,
    fd: c_int,
    length: off_t,
    path_for_decision: &str,
    original_result: c_int,
    original_errno: i32,
) -> Option<c_int> {
    let (storage_path, backend_path, caller_uid) =
        resolve_private_owner_sqlite_backend(hub, path_for_decision)?;
    let Ok(c_path) = CString::new(backend_path.as_str()) else {
        return None;
    };

    // SAFETY: fd 由调用方传入并保证有效，fcntl(F_GETFL) 只读取该 fd 的状态标志，不涉及内存写入。
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: c_path 由上方 CString::new 构造，保证以 NUL 结尾且在本次调用期间有效，open 只读取该路径字符串。
    let retry_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if retry_fd < 0 {
        log::warn!(
            "{} private owner sqlite backend truncate open failed ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} path={} storage={} backend={}",
            op_name,
            original_result,
            runtime::current_errno(),
            fd,
            fd_flags,
            caller_uid,
            path_for_decision,
            storage_path,
            backend_path
        );
        runtime::set_errno(original_errno);
        return None;
    }

    // SAFETY: retry_fd 是上方 open 成功返回的有效文件描述符，ftruncate 只按 length 调整该 fd 对应文件大小。
    let result = unsafe { libc::ftruncate(retry_fd, length) };
    let retry_errno = runtime::current_errno();
    let backend_size = backend_fd_size(retry_fd);
    // SAFETY: retry_fd 为本函数内 open 得到的有效描述符，此处关闭后不再使用，避免泄漏。
    unsafe {
        libc::close(retry_fd);
    }
    if result == 0 {
        log::debug!(
            "{} private owner sqlite backend truncate ok ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} length={} size={} path={} storage={} backend={}",
            op_name,
            original_result,
            original_errno,
            fd,
            fd_flags,
            caller_uid,
            length,
            backend_size,
            path_for_decision,
            storage_path,
            backend_path
        );
        return Some(0);
    }

    log::warn!(
        "{} private owner sqlite backend truncate failed ret={} errno={} retry_errno={} fd={} fd_flags=0x{:x} caller_uid={} length={} size={} path={} storage={} backend={}",
        op_name,
        original_result,
        original_errno,
        retry_errno,
        fd,
        fd_flags,
        caller_uid,
        length,
        backend_size,
        path_for_decision,
        storage_path,
        backend_path
    );
    runtime::set_errno(original_errno);
    None
}

fn confirm_private_owner_sqlite_futimens(
    hub: &InterceptHub,
    op_name: &str,
    fd: c_int,
    times: *const timespec,
    path_for_decision: &str,
    original_result: c_int,
    original_errno: i32,
) -> Option<c_int> {
    let (storage_path, backend_path, caller_uid) =
        resolve_private_owner_sqlite_backend(hub, path_for_decision)?;
    let Ok(c_path) = CString::new(backend_path.as_str()) else {
        return None;
    };

    // SAFETY: fd 由调用方传入并保证有效，fcntl(F_GETFL) 只读取该 fd 的状态标志，不涉及内存写入。
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: c_path 由上方 CString::new 构造，保证以 NUL 结尾且在本次调用期间有效，open 只读取该路径字符串。
    let retry_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if retry_fd < 0 {
        log::warn!(
            "{} private owner sqlite backend futimens open failed ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} path={} storage={} backend={}",
            op_name,
            original_result,
            runtime::current_errno(),
            fd,
            fd_flags,
            caller_uid,
            path_for_decision,
            storage_path,
            backend_path
        );
        runtime::set_errno(original_errno);
        return None;
    }

    // SAFETY: retry_fd 是上方 open 成功返回的有效文件描述符，times 由调用方保证为合法的 timespec 数组或空指针，futimens 只读取它更新该 fd 时间戳。
    let result = unsafe { libc::futimens(retry_fd, times) };
    let retry_errno = runtime::current_errno();
    // SAFETY: retry_fd 为本函数内 open 得到的有效描述符，此处关闭后不再使用，避免泄漏。
    unsafe {
        libc::close(retry_fd);
    }
    if result == 0 {
        log::info!(
            "{} private owner sqlite backend futimens ok ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} path={} storage={} backend={}",
            op_name,
            original_result,
            original_errno,
            fd,
            fd_flags,
            caller_uid,
            path_for_decision,
            storage_path,
            backend_path
        );
        return Some(0);
    }

    if original_result < 0
        && is_permission_errno(original_errno)
        && is_permission_errno(retry_errno)
    {
        log::debug!(
            "{} private owner sqlite futimens permission failure suppressed errno={} retry_errno={} fd={} caller_uid={} path={} storage={} backend={}",
            op_name,
            original_errno,
            retry_errno,
            fd,
            caller_uid,
            path_for_decision,
            storage_path,
            backend_path
        );
        return Some(0);
    }

    log::warn!(
        "{} private owner sqlite backend futimens failed ret={} errno={} retry_errno={} fd={} fd_flags=0x{:x} caller_uid={} path={} storage={} backend={}",
        op_name,
        original_result,
        original_errno,
        retry_errno,
        fd,
        fd_flags,
        caller_uid,
        path_for_decision,
        storage_path,
        backend_path
    );
    runtime::set_errno(original_errno);
    None
}
