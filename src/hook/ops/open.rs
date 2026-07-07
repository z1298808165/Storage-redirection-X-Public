use super::super::diagnostic;
use super::super::fuse_fix;
use super::super::monitor;
use super::super::runtime;
use super::super::stats::InterceptHub;
use super::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::redirect::{
    RedirectAction, RedirectDecision, policy, process_redirect_path, process_write_redirect_path,
    record_redirect_hit,
};
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t};
use std::borrow::Cow;
use std::ffi::CString;

#[repr(C)]
pub struct OpenHow {
    pub flags: u64,
    pub mode: u64,
    pub resolve: u64,
}

unsafe fn call_open(pathname: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    let self_ptr = hooked_open as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || libc::open(pathname, flags, mode),
        |prev| {
            let f: unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int =
                std::mem::transmute(prev);
            f(pathname, flags, mode)
        },
    )
}

unsafe fn call_openat(dirfd: c_int, pathname: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    let self_ptr = hooked_openat as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || libc::openat(dirfd, pathname, flags, mode),
        |prev| {
            let f: unsafe extern "C" fn(c_int, *const c_char, c_int, mode_t) -> c_int =
                std::mem::transmute(prev);
            f(dirfd, pathname, flags, mode)
        },
    )
}

// libc 无 openat2 封装，fallback 走 SYS_openat2 直调
unsafe fn call_openat2(
    dirfd: c_int,
    pathname: *const c_char,
    how: *const OpenHow,
    how_size: usize,
) -> c_int {
    let self_ptr = hooked_openat2 as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || libc::syscall(libc::SYS_openat2, dirfd, pathname, how, how_size) as c_int,
        |prev| {
            let f: unsafe extern "C" fn(c_int, *const c_char, *const OpenHow, usize) -> c_int =
                std::mem::transmute(prev);
            f(dirfd, pathname, how, how_size)
        },
    )
}

pub unsafe extern "C" fn hooked_open(pathname: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    runtime::with_hook_guard(
        || call_open(pathname, flags, mode),
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_open_like(hub, "open", pathname, flags, true, |call_path| {
                call_open(call_path, flags, mode)
            })
        },
    )
}

pub unsafe extern "C" fn hooked_openat(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mode: mode_t,
) -> c_int {
    runtime::with_hook_guard(
        || call_openat(dirfd, pathname, flags, mode),
        |hub| {
            hub.increment_openat_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_openat_like(
                hub,
                "openat",
                dirfd,
                pathname,
                flags,
                |call_dirfd, call_path| call_openat(call_dirfd, call_path, flags, mode),
            )
        },
    )
}

pub unsafe extern "C" fn hooked_openat2(
    dirfd: c_int,
    pathname: *const c_char,
    how: *const OpenHow,
    how_size: usize,
) -> c_int {
    runtime::with_hook_guard(
        || call_openat2(dirfd, pathname, how, how_size),
        |hub| {
            hub.increment_openat_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            if pathname.is_null() || how.is_null() {
                return call_openat2(dirfd, pathname, how, how_size);
            }

            let open_flags = (*how).flags as i32;
            handle_openat_like(
                hub,
                "openat2",
                dirfd,
                pathname,
                open_flags,
                |call_dirfd, call_path| call_openat2(call_dirfd, call_path, how, how_size),
            )
        },
    )
}

// open/creat 共用主体
fn handle_open_like<F>(
    hub: &InterceptHub,
    op_name: &str,
    pathname: *const c_char,
    flags: c_int,
    record_fast_bypass: bool,
    mut call_original: F,
) -> c_int
where
    F: FnMut(*const c_char) -> c_int,
{
    if pathname.is_null() {
        return call_original(pathname);
    }

    let PreparedPath::Ready {
        path_for_decision,
        is_relative,
        ..
    } = (unsafe {
        prepare_relevant_path(hub, op_name, AT_FDCWD, pathname, flags, record_fast_bypass)
    })
    else {
        return call_original(pathname);
    };

    retry_fuse_fix_for_media_provider(hub);
    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), flags);

    let is_system_writer = policy::is_system_writer_package(&hub.get_package_name());
    let mut is_redirected = false;
    let mut final_path: Cow<'_, str> = Cow::Borrowed(path_for_decision.as_ref());
    let redirect_result = resolve_open_redirect_path(
        hub,
        op_name,
        path_for_decision.as_ref(),
        is_system_writer,
        flags,
    );
    if redirect_result.is_denied() {
        return deny_read_only_open(
            hub,
            op_name,
            path_for_decision.as_ref(),
            flags,
            &redirect_result.new_path,
        );
    }
    if redirect_result.is_redirect() {
        final_path = Cow::Owned(redirect_result.new_path);
        is_redirected = true;
    }

    runtime::ensure_redirect_parent_directory(
        op_name,
        path_for_decision.as_ref(),
        final_path.as_ref(),
        flags,
    );
    if should_fix_system_writer_private_owner(hub, flags) {
        runtime::fix_system_writer_android_private_owner(final_path.as_ref(), false);
    }

    let call_target =
        OpenCallTarget::for_open(pathname, final_path.as_ref(), is_relative, is_redirected);
    let mut result = call_original(call_target.path);
    let mut error_no = runtime::errno_for_result(result);

    if let Some(retry) = maybe_retry_system_writer_read_fallback(
        hub,
        op_name,
        path_for_decision.as_ref(),
        is_system_writer,
        is_redirected,
        flags,
        result,
        error_no,
        |c_path| call_original(c_path),
    ) {
        final_path = Cow::Owned(retry.new_path);
        is_redirected = true;
        result = retry.result;
        error_no = retry.error_no;
    }

    finalize_open_result(
        hub,
        op_name,
        path_for_decision.as_ref(),
        final_path.as_ref(),
        flags,
        is_redirected,
        redirect_result.is_mapping,
        result,
        error_no,
        || {
            if let Ok(c_path) = CString::new(final_path.as_ref()) {
                call_original(c_path.as_ptr())
            } else {
                -1
            }
        },
    )
}
// openat/openat2 共用主体
fn handle_openat_like<F>(
    hub: &InterceptHub,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mut call_original: F,
) -> c_int
where
    F: FnMut(c_int, *const c_char) -> c_int,
{
    if pathname.is_null() {
        return call_original(dirfd, pathname);
    }

    let PreparedPath::Ready {
        path_for_decision,
        is_relative,
        ..
    } = (unsafe { prepare_relevant_path(hub, op_name, dirfd, pathname, flags, true) })
    else {
        return call_original(dirfd, pathname);
    };

    retry_fuse_fix_for_media_provider(hub);
    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), flags);

    let is_system_writer = policy::is_system_writer_package(&hub.get_package_name());
    let mut is_redirected = false;
    let mut final_path: Cow<'_, str> = Cow::Borrowed(path_for_decision.as_ref());
    let mut should_call_with_absolute = false;

    let redirect_result = resolve_open_redirect_path(
        hub,
        op_name,
        path_for_decision.as_ref(),
        is_system_writer,
        flags,
    );
    if redirect_result.is_denied() {
        return deny_read_only_open(
            hub,
            op_name,
            path_for_decision.as_ref(),
            flags,
            &redirect_result.new_path,
        );
    }
    if redirect_result.is_redirect() {
        final_path = Cow::Owned(redirect_result.new_path);
        is_redirected = true;
        should_call_with_absolute = is_relative;
    }

    let call_target = OpenCallTarget::for_openat(
        dirfd,
        pathname,
        final_path.as_ref(),
        is_relative,
        should_call_with_absolute,
    );

    runtime::ensure_redirect_parent_directory(
        op_name,
        path_for_decision.as_ref(),
        final_path.as_ref(),
        flags,
    );
    if should_fix_system_writer_private_owner(hub, flags) {
        runtime::fix_system_writer_android_private_owner(final_path.as_ref(), false);
    }
    let mut result = call_original(call_target.dirfd, call_target.path);
    let mut error_no = runtime::errno_for_result(result);
    if let Some(retry) = maybe_retry_system_writer_read_fallback(
        hub,
        op_name,
        path_for_decision.as_ref(),
        is_system_writer,
        is_redirected,
        flags,
        result,
        error_no,
        |c_path| call_original(AT_FDCWD, c_path),
    ) {
        final_path = Cow::Owned(retry.new_path);
        is_redirected = true;
        result = retry.result;
        error_no = retry.error_no;
    }

    result = finalize_open_result(
        hub,
        op_name,
        path_for_decision.as_ref(),
        final_path.as_ref(),
        flags,
        is_redirected,
        redirect_result.is_mapping,
        result,
        error_no,
        || call_original(call_target.dirfd, call_target.path),
    );

    result
}

fn retry_fuse_fix_for_media_provider(hub: &InterceptHub) {
    let package_name = hub.get_package_name();
    if policy::is_media_provider_package(&package_name) {
        if crate::platform::is_boot_completed() {
            crate::hook::install_fuse_fix_if_enabled(&package_name);
        } else {
            fuse_fix::retry_if_target_enabled();
        }
    }
}

struct OpenCallTarget {
    dirfd: c_int,
    path: *const c_char,
    _path_owner: Option<CString>,
}

impl OpenCallTarget {
    fn for_open(
        pathname: *const c_char,
        final_path: &str,
        is_relative: bool,
        is_redirected: bool,
    ) -> Self {
        if is_relative && !is_redirected {
            return Self::borrowed(AT_FDCWD, pathname);
        }
        Self::from_final_path(AT_FDCWD, pathname, final_path)
    }

    fn for_openat(
        dirfd: c_int,
        pathname: *const c_char,
        final_path: &str,
        is_relative: bool,
        should_call_with_absolute: bool,
    ) -> Self {
        let call_dirfd = if should_call_with_absolute {
            AT_FDCWD
        } else {
            dirfd
        };
        if is_relative && !should_call_with_absolute {
            return Self::borrowed(call_dirfd, pathname);
        }
        Self::from_final_path(call_dirfd, pathname, final_path)
    }

    fn borrowed(dirfd: c_int, path: *const c_char) -> Self {
        Self {
            dirfd,
            path,
            _path_owner: None,
        }
    }

    fn from_final_path(dirfd: c_int, fallback_path: *const c_char, final_path: &str) -> Self {
        let path_owner = CString::new(final_path).ok();
        let path = path_owner
            .as_ref()
            .map(|path| path.as_ptr())
            .unwrap_or(fallback_path);
        Self {
            dirfd,
            path,
            _path_owner: path_owner,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_open_result<F>(
    hub: &InterceptHub,
    op_name: &str,
    from_path: &str,
    final_path: &str,
    flags: c_int,
    is_redirected: bool,
    is_mapping: bool,
    mut result: c_int,
    mut error_no: c_int,
    mut retry_open: F,
) -> c_int
where
    F: FnMut() -> c_int,
{
    if is_redirected && monitor::has_write_intent_flags(flags) {
        let fd_diag = retry_deleted_redirected_fd(
            op_name,
            from_path,
            final_path,
            flags,
            result,
            error_no,
            &mut retry_open,
        );
        result = fd_diag.result;
        error_no = fd_diag.error_no;
        log_redirected_open_fd(op_name, from_path, final_path, flags, &fd_diag);
    }

    if result >= 0 && should_fix_system_writer_private_owner(hub, flags) {
        runtime::fix_system_writer_android_private_owner(
            final_path,
            has_create_intent_flags_for_owner_fix(flags),
        );
    }
    monitor::record_open_result(
        hub, op_name, flags, final_path, from_path, is_mapping, result, error_no,
    );
    result
}

fn log_redirected_open_fd(
    op_name: &str,
    from_path: &str,
    to_path: &str,
    flags: c_int,
    fd_diag: &OpenFdDiag,
) {
    if fd_diag.result < 0 {
        log::info!(
            "write op={} from={} to={} flags=0x{:x} ret={} errno={}",
            op_name,
            from_path,
            to_path,
            flags,
            fd_diag.result,
            fd_diag.error_no
        );
        return;
    }

    log::info!(
        "write op={} from={} to={} flags=0x{:x} ret={} errno={} fd_flags=0x{:x} fd_retry={} fd_path={}",
        op_name,
        from_path,
        to_path,
        flags,
        fd_diag.result,
        fd_diag.error_no,
        fd_diag.fd_flags,
        fd_diag.retried,
        if fd_diag.fd_path.is_empty() {
            "<empty>"
        } else {
            fd_diag.fd_path.as_str()
        }
    );
}

fn should_redirect_open_operation(
    hub: &InterceptHub,
    is_system_writer: bool,
    flags: c_int,
) -> bool {
    should_redirect_open_operation_for_mode(
        hub.is_monitor_only(),
        hub.is_app_write_only(),
        is_system_writer,
        flags,
    )
}

fn should_redirect_open_operation_for_mode(
    is_monitor_only: bool,
    is_app_write_only: bool,
    is_system_writer: bool,
    flags: c_int,
) -> bool {
    if is_app_write_only && !is_system_writer {
        return monitor::has_write_intent_flags(flags);
    }
    if is_system_writer {
        return monitor::has_write_intent_flags(flags);
    }
    !is_monitor_only
}

fn should_check_monitor_only_read_only_guard(
    is_monitor_only: bool,
    is_system_writer: bool,
    package_name: &str,
    flags: c_int,
) -> bool {
    is_monitor_only
        && !is_system_writer
        && !policy::is_saf_native_monitor_bridge_package(package_name)
        && monitor::has_write_intent_flags(flags)
}

fn allow_redirect_decision() -> RedirectDecision {
    RedirectDecision {
        action: RedirectAction::Allow,
        new_path: String::new(),
        is_mapping: false,
    }
}

fn resolve_open_redirect_path(
    hub: &InterceptHub,
    op_name: &str,
    from_path: &str,
    is_system_writer: bool,
    flags: c_int,
) -> RedirectDecision {
    if should_check_monitor_only_read_only_guard(
        hub.is_monitor_only(),
        is_system_writer,
        &hub.get_package_name(),
        flags,
    ) {
        let guard_result = process_write_redirect_path_for_open(hub, from_path);
        if guard_result.is_denied() {
            diagnostic::log_diag_redirect_decision(hub, op_name, from_path, &guard_result);
            return guard_result;
        }
    }

    // 系统代写进程仅对写入操作重定向，读取探测保持原路径避免触发 MediaProvider 路径校验。
    if !should_redirect_open_operation(hub, is_system_writer, flags) {
        if hub.is_app_write_only() && !is_system_writer && !monitor::has_write_intent_flags(flags) {
            let mapping_read_result = process_redirect_path_for_read_fallback(hub, from_path);
            if mapping_read_result.is_redirect() && mapping_read_result.is_mapping {
                diagnostic::log_diag_redirect_decision(
                    hub,
                    op_name,
                    from_path,
                    &mapping_read_result,
                );
                return mapping_read_result;
            }
        }
        return allow_redirect_decision();
    }

    let redirect_result = if monitor::has_write_intent_flags(flags) {
        process_write_redirect_path_for_open(hub, from_path)
    } else {
        process_redirect_path_for_read_fallback(hub, from_path)
    };
    diagnostic::log_diag_redirect_decision(hub, op_name, from_path, &redirect_result);
    if redirect_result.is_redirect() {
        record_redirect_hit(hub, op_name, from_path, &redirect_result.new_path);
    }
    redirect_result
}

fn should_retry_system_writer_read_fallback(
    hub: &InterceptHub,
    is_system_writer: bool,
    is_redirected: bool,
    flags: c_int,
) -> bool {
    !is_redirected
        && is_system_writer
        && !hub.is_monitor_only()
        && !monitor::has_write_intent_flags(flags)
}

#[allow(clippy::too_many_arguments)]
fn maybe_retry_system_writer_read_fallback<F>(
    hub: &InterceptHub,
    op_name: &str,
    from_path: &str,
    is_system_writer: bool,
    is_redirected: bool,
    flags: c_int,
    result: c_int,
    error_no: c_int,
    retry_open: F,
) -> Option<ReadFallbackRetry>
where
    F: FnMut(*const c_char) -> c_int,
{
    if result >= 0 || !is_retryable_system_writer_read_error(error_no) {
        return None;
    }
    if !should_retry_system_writer_read_fallback(hub, is_system_writer, is_redirected, flags) {
        return None;
    }

    retry_system_writer_read_fallback(hub, op_name, from_path, error_no, retry_open)
}

struct ReadFallbackRetry {
    new_path: String,
    result: c_int,
    error_no: c_int,
}

fn retry_system_writer_read_fallback<F>(
    hub: &InterceptHub,
    op_name: &str,
    from_path: &str,
    original_error_no: c_int,
    mut retry_open: F,
) -> Option<ReadFallbackRetry>
where
    F: FnMut(*const c_char) -> c_int,
{
    let redirect_result = process_redirect_path_for_read_fallback(hub, from_path);
    diagnostic::log_diag_redirect_decision(hub, op_name, from_path, &redirect_result);
    if !redirect_result.is_redirect() || redirect_result.new_path.is_empty() {
        return None;
    }

    let new_path = redirect_result.new_path;
    let Ok(c_path) = CString::new(new_path.as_str()) else {
        return None;
    };

    let retry = retry_open(c_path.as_ptr());
    let retry_errno = runtime::errno_for_result(retry);
    if retry < 0 {
        runtime::set_errno(original_error_no);
        return None;
    }

    log::debug!(
        "system writer read fallback op={} from={} to={} original_errno={}",
        op_name,
        from_path,
        new_path,
        original_error_no
    );
    record_redirect_hit(hub, op_name, from_path, &new_path);
    Some(ReadFallbackRetry {
        new_path,
        result: retry,
        error_no: retry_errno,
    })
}

fn is_retryable_system_writer_read_error(error_no: c_int) -> bool {
    error_no == libc::ENOENT
        || error_no == libc::EACCES
        || error_no == libc::EAGAIN
        || error_no == libc::EWOULDBLOCK
}

struct OpenFdDiag {
    result: c_int,
    error_no: c_int,
    fd_path: String,
    fd_flags: c_int,
    retried: bool,
}

fn retry_deleted_redirected_fd<F>(
    op_name: &str,
    from_path: &str,
    to_path: &str,
    flags: c_int,
    result: c_int,
    error_no: c_int,
    mut retry_open: F,
) -> OpenFdDiag
where
    F: FnMut() -> c_int,
{
    if result < 0 {
        return OpenFdDiag {
            result,
            error_no,
            fd_path: String::new(),
            fd_flags: -1,
            retried: false,
        };
    }

    let fd_path = resolve_fd_path(result);
    let fd_flags = unsafe { libc::fcntl(result, libc::F_GETFL) };
    if !fd_path.ends_with(" (deleted)") {
        return OpenFdDiag {
            result,
            error_no,
            fd_path,
            fd_flags,
            retried: false,
        };
    }

    let retry_result = retry_open();
    let retry_error_no = runtime::errno_for_result(retry_result);
    if retry_result < 0 {
        if retry_error_no == libc::EEXIST {
            if let Some((existing_result, existing_error_no, retry_flags)) =
                retry_deleted_existing_path(to_path, flags)
            {
                log::debug!(
                    "write op={} deleted fd retry opened existing from={} to={} flags=0x{:x} retry_flags=0x{:x} old_fd={} new_fd={}",
                    op_name,
                    from_path,
                    to_path,
                    flags,
                    retry_flags,
                    result,
                    existing_result
                );
                unsafe {
                    libc::close(result);
                }
                return OpenFdDiag {
                    result: existing_result,
                    error_no: existing_error_no,
                    fd_path: resolve_fd_path(existing_result),
                    fd_flags: unsafe { libc::fcntl(existing_result, libc::F_GETFL) },
                    retried: true,
                };
            }
        }
        log::warn!(
            "write op={} deleted fd retry failed from={} to={} flags=0x{:x} old_fd={} retry_ret={} retry_errno={}",
            op_name,
            from_path,
            to_path,
            flags,
            result,
            retry_result,
            retry_error_no
        );
        return OpenFdDiag {
            result,
            error_no,
            fd_path,
            fd_flags,
            retried: false,
        };
    }

    unsafe {
        libc::close(result);
    }
    OpenFdDiag {
        result: retry_result,
        error_no: retry_error_no,
        fd_path: resolve_fd_path(retry_result),
        fd_flags: unsafe { libc::fcntl(retry_result, libc::F_GETFL) },
        retried: true,
    }
}

fn resolve_fd_path(fd: c_int) -> String {
    let link_path = format!("/proc/self/fd/{}", fd);
    let Ok(c_path) = CString::new(link_path) else {
        return String::new();
    };

    let mut link_buf = [0u8; 512];
    let len = unsafe {
        libc::readlink(
            c_path.as_ptr(),
            link_buf.as_mut_ptr() as *mut c_char,
            link_buf.len() - 1,
        )
    };
    if len <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&link_buf[..len as usize]).into_owned()
}

fn retry_deleted_existing_path(to_path: &str, flags: c_int) -> Option<(c_int, c_int, c_int)> {
    let retry_flags = retry_deleted_existing_flags(flags)?;
    let c_path = CString::new(to_path).ok()?;
    let result = unsafe { libc::open(c_path.as_ptr(), retry_flags, 0) };
    let error_no = runtime::errno_for_result(result);
    if result < 0 {
        return None;
    }
    Some((result, error_no, retry_flags))
}

fn retry_deleted_existing_flags(flags: c_int) -> Option<c_int> {
    if flags < 0 || (flags & libc::O_CREAT) == 0 || (flags & libc::O_EXCL) == 0 {
        return None;
    }
    Some(flags & !libc::O_EXCL)
}

fn deny_read_only_open(
    hub: &InterceptHub,
    op_name: &str,
    path: &str,
    flags: c_int,
    read_only_path: &str,
) -> c_int {
    runtime::set_read_only_errno();
    monitor::record_read_only_open_result(hub, op_name, flags, path, path, read_only_path);
    log::debug!(
        "readonly deny op={} path={} read_only_path={} flags=0x{:x}",
        op_name,
        path,
        read_only_path,
        flags
    );
    -1
}

fn process_redirect_path_for_read_fallback(hub: &InterceptHub, path: &str) -> RedirectDecision {
    process_redirect_path(hub, path)
}

fn process_write_redirect_path_for_open(hub: &InterceptHub, path: &str) -> RedirectDecision {
    process_write_redirect_path(hub, path)
}

unsafe fn call_creat(pathname: *const c_char, mode: mode_t) -> c_int {
    let self_ptr = hooked_creat as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || {
            libc::open(
                pathname,
                libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC,
                mode,
            )
        },
        |prev| {
            let f: unsafe extern "C" fn(*const c_char, mode_t) -> c_int = std::mem::transmute(prev);
            f(pathname, mode)
        },
    )
}

pub unsafe extern "C" fn hooked_creat(pathname: *const c_char, mode: mode_t) -> c_int {
    let creat_flags = libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC;
    runtime::with_hook_guard(
        || call_creat(pathname, mode),
        |hub| {
            hub.increment_open_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_open_like(hub, "creat", pathname, creat_flags, false, |call_path| {
                call_creat(call_path, mode)
            })
        },
    )
}

fn should_fix_system_writer_private_owner(hub: &InterceptHub, flags: c_int) -> bool {
    !hub.is_monitor_only()
        && policy::is_system_writer_package(&hub.get_package_name())
        && monitor::has_write_intent_flags(flags)
}

fn has_create_intent_flags_for_owner_fix(flags: c_int) -> bool {
    if flags < 0 || (flags & libc::O_PATH) != 0 {
        return false;
    }
    (flags & libc::O_CREAT) != 0 || (flags & libc::O_TMPFILE) == libc::O_TMPFILE
}

#[cfg(test)]
mod tests {
    use super::{
        is_retryable_system_writer_read_error, retry_deleted_existing_flags,
        should_check_monitor_only_read_only_guard, should_redirect_open_operation_for_mode,
    };

    #[test]
    fn retry_deleted_existing_flags_drop_exclusive_create() {
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC;
        let retry_flags = retry_deleted_existing_flags(flags).expect("retry flags");

        assert_eq!(retry_flags & libc::O_EXCL, 0);
        assert_ne!(retry_flags & libc::O_CREAT, 0);
        assert_ne!(retry_flags & libc::O_WRONLY, 0);
        assert_ne!(retry_flags & libc::O_CLOEXEC, 0);
    }

    #[test]
    fn retry_deleted_existing_flags_require_exclusive_create() {
        assert_eq!(
            retry_deleted_existing_flags(libc::O_WRONLY | libc::O_CREAT),
            None
        );
        assert_eq!(
            retry_deleted_existing_flags(libc::O_WRONLY | libc::O_EXCL),
            None
        );
    }

    #[test]
    fn system_writer_read_fallback_retries_missing_denied_and_temporarily_unavailable() {
        assert!(is_retryable_system_writer_read_error(libc::ENOENT));
        assert!(is_retryable_system_writer_read_error(libc::EACCES));
        assert!(is_retryable_system_writer_read_error(libc::EAGAIN));
        assert!(is_retryable_system_writer_read_error(libc::EWOULDBLOCK));
    }

    #[test]
    fn monitor_only_system_writer_open_redirects_write_intent_only() {
        assert!(should_redirect_open_operation_for_mode(
            true,
            false,
            true,
            libc::O_WRONLY | libc::O_CREAT
        ));
        assert!(!should_redirect_open_operation_for_mode(
            true,
            false,
            true,
            libc::O_RDONLY
        ));
    }

    #[test]
    fn monitor_only_non_writer_open_stays_observe_only() {
        assert!(!should_redirect_open_operation_for_mode(
            true,
            false,
            false,
            libc::O_WRONLY | libc::O_CREAT
        ));
        assert!(should_redirect_open_operation_for_mode(
            false,
            false,
            false,
            libc::O_RDONLY
        ));
    }

    #[test]
    fn app_write_only_redirects_write_intent_only() {
        assert!(should_redirect_open_operation_for_mode(
            false,
            true,
            false,
            libc::O_WRONLY | libc::O_CREAT
        ));
        assert!(!should_redirect_open_operation_for_mode(
            false,
            true,
            false,
            libc::O_RDONLY
        ));
    }

    #[test]
    fn monitor_only_non_writer_write_checks_read_only_guard() {
        assert!(should_check_monitor_only_read_only_guard(
            true,
            false,
            "com.tencent.mobileqq",
            libc::O_WRONLY | libc::O_CREAT
        ));
        assert!(!should_check_monitor_only_read_only_guard(
            true,
            false,
            "com.tencent.mobileqq",
            libc::O_RDONLY
        ));
        assert!(!should_check_monitor_only_read_only_guard(
            true,
            true,
            "com.android.providers.media.module",
            libc::O_WRONLY | libc::O_CREAT
        ));
        assert!(!should_check_monitor_only_read_only_guard(
            true,
            false,
            "com.android.externalstorage",
            libc::O_WRONLY | libc::O_CREAT
        ));
    }
}
