use super::common::*;
use crate::hook::diagnostic;
use crate::hook::monitor;
use crate::hook::path as path_utils;
use crate::hook::runtime;
use crate::hook::stats::InterceptHub;
use crate::hook::util::c_str_to_string;
use crate::monitor::OpKind;
use crate::platform::paths;
use libc::{
    AT_FDCWD, c_char, c_int, c_void, gid_t, mode_t, off_t, timespec, timeval, uid_t, utimbuf,
};
use std::ffi::CString;

/// SQLite WAL 共享内存文件至少需要一个 32 KiB 页框；过小的 truncate 会让
/// 后续 SQLite 打开数据库时把现有状态视为不可用。FUSE 后端与 system-writer
/// 后端必须使用同一阈值，避免不同访问入口产生不同的 sidecar 内容。
const SQLITE_SHM_MIN_SIZE: off_t = 32 * 1024;

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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Truncate,
                    op_name: "truncate",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("length={}", length)),
                    redirect_mutation: hub.is_redirect_via_hook(),
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Truncate,
                    op_name: "truncate64",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("length={}", length)),
                    redirect_mutation: hub.is_redirect_via_hook(),
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "chmod",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: mode as i32,
                    extra_tail: Some(format!("mode=0{:o}", mode)),
                    redirect_mutation: hub.is_redirect_via_hook(),
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "fchmodat",
                    dirfd,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("mode=0{:o}|flags=0x{:x}", mode, flags)),
                    redirect_mutation: hub.is_redirect_via_hook(),
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Utimens,
                    op_name: "utimensat",
                    dirfd,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("flags=0x{:x}", flags)),
                    redirect_mutation: hub.is_redirect_via_hook(),
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

/// 记录「按真实路径重试打开私有库后端」的失败。
///
/// owner-only（0600）入口上 EACCES/EPERM 是**预期结果**：MediaProvider 没有
/// CAP_DAC_OVERRIDE，无法代开属主私有文件，这是原生语义而不是缺陷。这类失败原先
/// 一律按 warn 落盘，会把「正常的权限拒绝」刷成告警噪声、掩盖真正的中转失败；这里
/// 只把它们降到 debug，其它 errno（ENOENT/EIO/EROFS…）仍按 warn 留痕。
// quality-allow(lint-suppression): 参数与调用点字段一一对应，避免为一次日志构造结构体。
#[allow(clippy::too_many_arguments)]
fn log_private_owner_sqlite_open_failure(
    op_name: &str,
    stage: &str,
    original_result: c_int,
    err: i32,
    fd: c_int,
    fd_flags: c_int,
    caller_uid: i32,
    path: &str,
    storage_path: &str,
    backend_path: &str,
) {
    let expected = matches!(err, libc::EACCES | libc::EPERM);
    let line = format!(
        "{} private owner sqlite backend {} open failed ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} expected={} path={} storage={} backend={}",
        op_name,
        stage,
        original_result,
        err,
        fd,
        fd_flags,
        caller_uid,
        expected,
        path,
        storage_path,
        backend_path
    );
    if expected {
        log::debug!("{}", line);
    } else {
        log::warn!("{}", line);
    }
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

    // 毒化 `-shm`（被截到 (0, 32KiB)）的处置由宿主 FUSE 侧的删除式自愈负责
    // （fuse_redirect::callbacks 的 heal_poisoned_sqlite_shm_backend）：此处若
    // 直接 unlink 后端文件会绕过 libfuse_jni 的节点簿记，留下指向已消失文件
    // 的幽灵节点，导致应用后续 O_CREAT 全部失败（duchamp 18:11 实测）。

    let Ok(c_path) = CString::new(backend_path.as_str()) else {
        return None;
    };

    // SAFETY: fd 由调用方传入并保证有效，fcntl(F_GETFL) 只读取该 fd 的状态标志，不涉及内存写入。
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: c_path 由上方 CString::new 构造，保证以 NUL 结尾且在本次调用期间有效，open 只读取该路径字符串。
    let retry_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if retry_fd < 0 {
        log_private_owner_sqlite_open_failure(
            op_name,
            "truncate",
            original_result,
            runtime::current_errno(),
            fd,
            fd_flags,
            caller_uid,
            path_for_decision,
            &storage_path,
            &backend_path,
        );
        runtime::set_errno(original_errno);
        return None;
    }

    let effective_length = clamp_sqlite_shm_truncate(path_for_decision, length);
    // SAFETY: retry_fd 是上方 open 成功返回的有效文件描述符，ftruncate 只按 effective_length 调整该 fd 对应文件大小。
    let result = unsafe { libc::ftruncate(retry_fd, effective_length) };
    let retry_errno = runtime::current_errno();
    let backend_size = backend_fd_size(retry_fd);
    // SAFETY: retry_fd 为本函数内 open 得到的有效描述符，此处关闭后不再使用，避免泄漏。
    unsafe {
        libc::close(retry_fd);
    }
    if result == 0 {
        log::debug!(
            "{} private owner sqlite backend truncate ok ret={} errno={} fd={} fd_flags=0x{:x} caller_uid={} length={} effective_length={} size={} path={} storage={} backend={}",
            op_name,
            original_result,
            original_errno,
            fd,
            fd_flags,
            caller_uid,
            length,
            effective_length,
            backend_size,
            path_for_decision,
            storage_path,
            backend_path
        );
        return Some(0);
    }

    log::warn!(
        "{} private owner sqlite backend truncate failed ret={} errno={} retry_errno={} fd={} fd_flags=0x{:x} caller_uid={} length={} effective_length={} size={} path={} storage={} backend={}",
        op_name,
        original_result,
        original_errno,
        retry_errno,
        fd,
        fd_flags,
        caller_uid,
        length,
        effective_length,
        backend_size,
        path_for_decision,
        storage_path,
        backend_path
    );
    runtime::set_errno(original_errno);
    None
}

fn clamp_sqlite_shm_truncate(path: &str, requested: off_t) -> off_t {
    if requested > 0
        && requested < SQLITE_SHM_MIN_SIZE
        && paths::is_sqlite_database_or_sidecar_path(path)
        && path.to_ascii_lowercase().ends_with("-shm")
    {
        SQLITE_SHM_MIN_SIZE
    } else {
        requested
    }
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
        log_private_owner_sqlite_open_failure(
            op_name,
            "futimens",
            original_result,
            runtime::current_errno(),
            fd,
            fd_flags,
            caller_uid,
            path_for_decision,
            &storage_path,
            &backend_path,
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
pub unsafe extern "C" fn hooked_chown(
    pathname: *const c_char,
    owner: uid_t,
    group: gid_t,
) -> c_int {
    let self_ptr = hooked_chown as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::chown(pathname, owner, group),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, uid_t, gid_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, owner, group)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "chown",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("owner={}|group={}", owner, group)),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::chown(call_path, owner, group),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, uid_t, gid_t) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, owner, group)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_lchown(
    pathname: *const c_char,
    owner: uid_t,
    group: gid_t,
) -> c_int {
    let self_ptr = hooked_lchown as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::lchown(pathname, owner, group),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, uid_t, gid_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, owner, group)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "lchown",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("owner={}|group={}", owner, group)),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::lchown(call_path, owner, group),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, uid_t, gid_t) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, owner, group)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_fchownat(
    dirfd: c_int,
    pathname: *const c_char,
    owner: uid_t,
    group: gid_t,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_fchownat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::fchownat(dirfd, pathname, owner, group, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        uid_t,
                        gid_t,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(dirfd, pathname, owner, group, flags)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "fchownat",
                    dirfd,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!(
                        "owner={}|group={}|flags=0x{:x}",
                        owner, group, flags
                    )),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::fchownat(dirfd, call_path, owner, group, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                uid_t,
                                gid_t,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, owner, group, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_utime(pathname: *const c_char, times: *const utimbuf) -> c_int {
    let self_ptr = hooked_utime as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::utime(pathname, times),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const utimbuf) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, times)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Utimens,
                    op_name: "utime",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: None,
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::utime(call_path, times),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const utimbuf) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, times)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_utimes(pathname: *const c_char, times: *const timeval) -> c_int {
    let self_ptr = hooked_utimes as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::utimes(pathname, times),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const timeval) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, times)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Utimens,
                    op_name: "utimes",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: None,
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::utimes(call_path, times),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const timeval) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, times)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_setxattr(
    pathname: *const c_char,
    name: *const c_char,
    value: *const c_void,
    size: libc::size_t,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_setxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::setxattr(pathname, name, value, size, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *const c_char,
                        *const c_void,
                        libc::size_t,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(pathname, name, value, size, flags)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "setxattr",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("name={}", c_str_to_string(name))),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::setxattr(call_path, name, value, size, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                *const c_char,
                                *const c_char,
                                *const c_void,
                                libc::size_t,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(call_path, name, value, size, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_lsetxattr(
    pathname: *const c_char,
    name: *const c_char,
    value: *const c_void,
    size: libc::size_t,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_lsetxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::lsetxattr(pathname, name, value, size, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *const c_char,
                        *const c_void,
                        libc::size_t,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(pathname, name, value, size, flags)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "lsetxattr",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: flags,
                    extra_tail: Some(format!("name={}", c_str_to_string(name))),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::lsetxattr(call_path, name, value, size, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                *const c_char,
                                *const c_char,
                                *const c_void,
                                libc::size_t,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(call_path, name, value, size, flags)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_removexattr(pathname: *const c_char, name: *const c_char) -> c_int {
    let self_ptr = hooked_removexattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::removexattr(pathname, name),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, name)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "removexattr",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("name={}", c_str_to_string(name))),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::removexattr(call_path, name),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, name)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_lremovexattr(
    pathname: *const c_char,
    name: *const c_char,
) -> c_int {
    let self_ptr = hooked_lremovexattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::lremovexattr(pathname, name),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, name)
                },
            )
        },
        |hub| {
            hub.increment_open_calls();
            crate::hook::caller::update_caller_for_mutation(hub, pathname);
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Chmod,
                    op_name: "lremovexattr",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: Some(format!("name={}", c_str_to_string(name))),
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::lremovexattr(call_path, name),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, name)
                        },
                    )
                },
            )
        },
    )
}

pub unsafe extern "C" fn hooked_getxattr(
    pathname: *const c_char,
    name: *const c_char,
    value: *mut c_void,
    size: libc::size_t,
) -> libc::ssize_t {
    let self_ptr = hooked_getxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::getxattr(pathname, name, value, size),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *const c_char,
                        *mut c_void,
                        libc::size_t,
                    ) -> libc::ssize_t = std::mem::transmute(prev);
                    f(pathname, name, value, size)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "getxattr", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::getxattr(call_path, name, value, size),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            *const c_char,
                            *const c_char,
                            *mut c_void,
                            libc::size_t,
                        ) -> libc::ssize_t = std::mem::transmute(prev);
                        f(call_path, name, value, size)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_lgetxattr(
    pathname: *const c_char,
    name: *const c_char,
    value: *mut c_void,
    size: libc::size_t,
) -> libc::ssize_t {
    let self_ptr = hooked_lgetxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::lgetxattr(pathname, name, value, size),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *const c_char,
                        *mut c_void,
                        libc::size_t,
                    ) -> libc::ssize_t = std::mem::transmute(prev);
                    f(pathname, name, value, size)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "lgetxattr", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::lgetxattr(call_path, name, value, size),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            *const c_char,
                            *const c_char,
                            *mut c_void,
                            libc::size_t,
                        ) -> libc::ssize_t = std::mem::transmute(prev);
                        f(call_path, name, value, size)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_listxattr(
    pathname: *const c_char,
    list: *mut c_char,
    size: libc::size_t,
) -> libc::ssize_t {
    let self_ptr = hooked_listxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::listxattr(pathname, list, size),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *mut c_char,
                        libc::size_t,
                    ) -> libc::ssize_t = std::mem::transmute(prev);
                    f(pathname, list, size)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "listxattr", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::listxattr(call_path, list, size),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            *const c_char,
                            *mut c_char,
                            libc::size_t,
                        ) -> libc::ssize_t = std::mem::transmute(prev);
                        f(call_path, list, size)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_llistxattr(
    pathname: *const c_char,
    list: *mut c_char,
    size: libc::size_t,
) -> libc::ssize_t {
    let self_ptr = hooked_llistxattr as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::llistxattr(pathname, list, size),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *mut c_char,
                        libc::size_t,
                    ) -> libc::ssize_t = std::mem::transmute(prev);
                    f(pathname, list, size)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "llistxattr", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::llistxattr(call_path, list, size),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            *const c_char,
                            *mut c_char,
                            libc::size_t,
                        ) -> libc::ssize_t = std::mem::transmute(prev);
                        f(call_path, list, size)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_statfs(pathname: *const c_char, buf: *mut libc::statfs) -> c_int {
    let self_ptr = hooked_statfs as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::statfs(pathname, buf),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *mut libc::statfs) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, buf)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "statfs", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::statfs(call_path, buf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::statfs) -> c_int =
                            std::mem::transmute(prev);
                        f(call_path, buf)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_statvfs(pathname: *const c_char, buf: *mut libc::statvfs) -> c_int {
    let self_ptr = hooked_statvfs as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::statvfs(pathname, buf),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *mut libc::statvfs) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, buf)
                },
            )
        },
        |hub| {
            runtime::with_redirected_path(hub, "statvfs", AT_FDCWD, pathname, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::statvfs(call_path, buf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::statvfs) -> c_int =
                            std::mem::transmute(prev);
                        f(call_path, buf)
                    },
                )
            })
        },
    )
}

fn futimesat_fallback(dirfd: c_int, pathname: *const c_char, times: *const timeval) -> c_int {
    // bionic 的 futimesat 本身是 utimensat/futimens 的兼容层；libc crate 没有
    // 导出它，所以 fallback 按同样语义转换 timeval → timespec。
    let converted = if times.is_null() {
        None
    } else {
        // SAFETY: futimesat 合约要求 times 指向两个 timeval。
        let tv = unsafe { std::slice::from_raw_parts(times, 2) };
        Some([
            timespec {
                tv_sec: tv[0].tv_sec,
                tv_nsec: tv[0].tv_usec as i64 * 1000,
            },
            timespec {
                tv_sec: tv[1].tv_sec,
                tv_nsec: tv[1].tv_usec as i64 * 1000,
            },
        ])
    };
    let times_ptr = converted
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    // SAFETY: dirfd/pathname 由 futimesat 调用方保证；times_ptr 指向本地
    // 数组或为空指针，libc 调用期间有效。
    unsafe {
        if pathname.is_null() {
            libc::futimens(dirfd, times_ptr)
        } else {
            libc::utimensat(dirfd, pathname, times_ptr, 0)
        }
    }
}

unsafe fn call_futimesat_prev(
    proxy: *mut c_void,
    dirfd: c_int,
    pathname: *const c_char,
    times: *const timeval,
) -> c_int {
    runtime::call_prev(
        proxy,
        || futimesat_fallback(dirfd, pathname, times),
        |prev| {
            let f: unsafe extern "C" fn(c_int, *const c_char, *const timeval) -> c_int =
                std::mem::transmute(prev);
            f(dirfd, pathname, times)
        },
    )
}

pub unsafe extern "C" fn hooked_futimesat(
    dirfd: c_int,
    pathname: *const c_char,
    times: *const timeval,
) -> c_int {
    let self_ptr = hooked_futimesat as *mut c_void;
    runtime::with_hook_guard(
        || call_futimesat_prev(self_ptr, dirfd, pathname, times),
        |hub| {
            if pathname.is_null() {
                return call_futimesat_prev(self_ptr, dirfd, pathname, times);
            }
            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Utimens,
                    op_name: "futimesat",
                    dirfd,
                    pathname,
                    log_flags: 0,
                    extra_tail: None,
                    redirect_mutation: hub.is_redirect_via_hook(),
                },
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || futimesat_fallback(dirfd, call_path, times),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                *const timeval,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, times)
                        },
                    )
                },
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! test_require {
        ($result:expr, $context:literal) => {
            match $result {
                Ok(value) => value,
                Err(error) => panic!("{}: {:?}", $context, error),
            }
        };
    }

    #[test]
    fn futimesat_fallback_converts_timeval() {
        let path = std::env::temp_dir().join(format!(
            "srx-futimesat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        test_require!(std::fs::write(&path, b"x"), "write");
        let c_path = test_require!(
            std::ffi::CString::new(path.to_string_lossy().as_bytes()),
            "c path"
        );
        let times = [
            timeval {
                tv_sec: 100,
                tv_usec: 250_000,
            },
            timeval {
                tv_sec: 200,
                tv_usec: 500_000,
            },
        ];
        let rc = futimesat_fallback(AT_FDCWD, c_path.as_ptr(), times.as_ptr());
        assert_eq!(rc, 0);
        let metadata = test_require!(std::fs::metadata(&path), "metadata");
        let mtime = test_require!(
            test_require!(metadata.modified(), "mtime").duration_since(std::time::UNIX_EPOCH),
            "epoch"
        )
        .as_secs();
        assert_eq!(mtime, 200);
        let _ = std::fs::remove_file(path);
    }
}
