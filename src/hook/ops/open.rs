use super::super::diagnostic;
use super::super::fuse_fix;
use super::super::monitor;
use super::super::runtime;
use super::super::stats::InterceptHub;
use super::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::platform::paths;
use crate::redirect::{
    PathRouter, RedirectAction, RedirectDecision, policy, process_redirect_path,
    process_write_redirect_path, record_redirect_hit,
};
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t};
use std::borrow::Cow;
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

const REDIRECTED_OPEN_FD_LOG_SAMPLE_STEP: u64 = 256;
static REDIRECTED_OPEN_FD_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
fn is_android_private_storage_path(path: &str) -> bool {
    let prefix = "/storage/emulated/";
    let Some((_, relative)) = path
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once("/"))
    else {
        return false;
    };
    paths::matches("Android/data", relative, true)
        || paths::matches("Android/media", relative, true)
        || paths::matches("Android/obb", relative, true)
}

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
                if super::super::monitor::has_write_intent_flags(flags) {
                    super::super::caller::update_caller_for_mutation(hub, pathname);
                } else {
                    super::super::caller::update_caller_package_for_current_thread(hub);
                }
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
                if super::super::monitor::has_write_intent_flags(flags) {
                    super::super::caller::update_caller_for_mutation(hub, pathname);
                } else {
                    super::super::caller::update_caller_package_for_current_thread(hub);
                }
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

/// 用 newfstatat(AT_SYMLINK_NOFOLLOW) 直接探测末段，避免递归进入已安装的
/// stat/lstat hook。路径不存在时返回 false（创建语义下没有末段链接）。
/// O_NOFOLLOW 只在末段确实是符号链接时需要保留原路径；O_PATH/O_TMPFILE
/// 本身不改变这个判断，统一走同一入口，避免两处 handler 语义漂移。
fn should_preserve_original_path(flags: c_int, dirfd: c_int, pathname: *const c_char) -> bool {
    flags & libc::O_NOFOLLOW != 0 && final_component_is_symlink(dirfd, pathname)
}

fn final_component_is_symlink(dirfd: c_int, pathname: *const c_char) -> bool {
    if pathname.is_null() {
        return false;
    }
    // SAFETY: stat 是 POD；pathname 由调用方保证为 NUL 结尾。
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // fstatat 自身也在 hook 表里；当前处于 ReentryGuard 内，会直接走原函数。
    let ret = unsafe { libc::fstatat(dirfd, pathname, &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    ret == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
}
const RESOLVE_BENEATH: u64 = 0x08;
const RESOLVE_IN_ROOT: u64 = 0x10;

fn openat2_requires_original_dirfd(resolve: u64) -> bool {
    resolve & (RESOLVE_BENEATH | RESOLVE_IN_ROOT) != 0
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
                // SAFETY: how 非空时按 OpenHow 布局读取 flags 字段，指针有效性
                // 由 openat2 调用方保证；空指针按无写意图处理。
                let write_intent = !how.is_null()
                    && unsafe {
                        super::super::monitor::has_write_intent_flags((*how).flags as i32)
                    };
                if write_intent {
                    super::super::caller::update_caller_for_mutation(hub, pathname);
                } else {
                    super::super::caller::update_caller_package_for_current_thread(hub);
                }
            }

            if pathname.is_null() || how.is_null() {
                return call_openat2(dirfd, pathname, how, how_size);
            }
            // RESOLVE_BENEATH / RESOLVE_IN_ROOT 把路径解释绑定在原 dirfd 的
            // 根上；改写成绝对沙盒路径后无法保持这个根，只能保持原调用。
            // 其余 RESOLVE_*（NO_XDEV/NO_SYMLINKS/NO_MAGICLINKS）是对最终
            // 路径的约束，仍可交给内核在重定向后的路径上校验：不满足时内核
            // 返回错误，不会变成“错误地成功打开”。
            if openat2_requires_original_dirfd((*how).resolve) {
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
    // O_NOFOLLOW 只在“末段本身是符号链接”时必须失败。先按原 dirfd 探测末段；
    // 不是符号链接时重定向不会改变 ELOOP 语义；是符号链接时保持原调用。
    if should_preserve_original_path(flags, AT_FDCWD, pathname) {
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
    if monitor::has_write_intent_flags(flags)
        && is_android_private_storage_path(path_for_decision.as_ref())
    {
        runtime::ensure_redirect_parent_dirs(path_for_decision.as_ref(), 0o2773);
    }
    runtime::fix_mapped_private_alias_access(hub, &path_for_decision, flags);
    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), flags);

    let is_system_writer = hub.with_package_name(policy::is_system_writer_package);
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
    if is_system_writer && is_sqlite_shm_sidecar_path(path_for_decision.as_ref()) {
        crate::hook::media_fuse::prepare_private_owner_sqlite_sidecar(path_for_decision.as_ref());
    }

    let call_target =
        OpenCallTarget::for_open(pathname, final_path.as_ref(), is_relative, is_redirected);
    // 「打开前是否已存在」只用于文件监视记录（record_open_result 在监视关闭时立即返回）。
    // 监视关闭时不做这次 stat：它走应用视图里的 FUSE 路径，每次 open 都要付一次往返，
    // 而原生 open 不做任何额外的存在性查询。
    let existed_before =
        hub.is_monitor_enabled() && std::path::Path::new(final_path.as_ref()).exists();
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
        &mut call_original,
    ) {
        final_path = Cow::Owned(retry.new_path);
        is_redirected = true;
        result = retry.result;
        error_no = retry.error_no;
    }

    if result >= 0 && is_system_writer {
        runtime::fix_system_writer_android_private_owner(final_path.as_ref(), true, result);
    }

    finalize_open_result(
        hub,
        op_name,
        path_for_decision.as_ref(),
        final_path.as_ref(),
        flags,
        is_redirected,
        redirect_result.is_mapping,
        existed_before,
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
    // 同上：只有末段确实是符号链接时才必须保持原调用。
    if should_preserve_original_path(flags, dirfd, pathname) {
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
    if monitor::has_write_intent_flags(flags)
        && is_android_private_storage_path(path_for_decision.as_ref())
    {
        runtime::ensure_redirect_parent_dirs(path_for_decision.as_ref(), 0o2773);
    }
    runtime::fix_mapped_private_alias_access(hub, &path_for_decision, flags);
    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), flags);

    let is_system_writer = hub.with_package_name(policy::is_system_writer_package);
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

    // 「打开前是否已存在」只用于文件监视记录（record_open_result 在监视关闭时立即返回）。
    // 监视关闭时不做这次 stat：它走应用视图里的 FUSE 路径，每次 open 都要付一次往返，
    // 而原生 open 不做任何额外的存在性查询。
    let existed_before =
        hub.is_monitor_enabled() && std::path::Path::new(final_path.as_ref()).exists();
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
    if is_system_writer && is_sqlite_shm_sidecar_path(path_for_decision.as_ref()) {
        crate::hook::media_fuse::prepare_private_owner_sqlite_sidecar(path_for_decision.as_ref());
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
        existed_before,
        result,
        error_no,
        || call_original(call_target.dirfd, call_target.path),
    );

    if result >= 0 && is_system_writer {
        runtime::fix_system_writer_android_private_owner(final_path.as_ref(), true, result);
    }

    result
}

fn retry_fuse_fix_for_media_provider(hub: &InterceptHub) {
    // 该函数在每次 open/openat 上都会执行，因此先用借用做判定，只有确认是
    // MediaProvider 且需要安装时才克隆包名，避免热路径上的无谓分配。
    if !hub.with_package_name(policy::is_media_provider_package) {
        return;
    }
    if crate::platform::is_boot_completed() {
        crate::hook::install_fuse_fix_if_enabled(&hub.get_package_name());
    } else {
        fuse_fix::retry_if_target_enabled();
    }
}

fn is_sqlite_shm_sidecar_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    (lower.ends_with(".db-shm")
        || lower.ends_with(".sqlite-shm")
        || lower.ends_with(".sqlite3-shm"))
        && crate::platform::paths::is_sqlite_database_or_sidecar_path(path)
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
        _is_relative: bool,
        is_redirected: bool,
    ) -> Self {
        // 未重定向时必须透传调用方原始路径：决策路径可能折叠了 `.`/`..`，
        // 而内核在符号链接下解析这些段，改写会改变实际目标。
        if !is_redirected {
            return Self::borrowed(AT_FDCWD, pathname);
        }
        Self::from_final_path(AT_FDCWD, pathname, final_path)
    }

    fn for_openat(
        dirfd: c_int,
        pathname: *const c_char,
        final_path: &str,
        _is_relative: bool,
        should_call_with_absolute: bool,
    ) -> Self {
        let call_dirfd = if should_call_with_absolute {
            AT_FDCWD
        } else {
            dirfd
        };
        // 未重定向时保留原始 dirfd 与相对路径，避免把 dirfd 语义换成绝对路径。
        if !should_call_with_absolute {
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
    existed_before: bool,
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

    monitor::record_open_result(
        hub,
        monitor::OpenResultRecord {
            op_name,
            flags,
            pathname: final_path,
            original_pathname: from_path,
            is_mapping,
            result,
            error_no,
            existed_before,
        },
    );
    // 审计与日志会执行系统调用，必须把 errno 恢复成原函数调用后的值，
    // 否则调用方读到的是 hook 内部产生的无关 errno。
    runtime::set_errno(error_no);
    result
}

fn log_redirected_open_fd(
    op_name: &str,
    from_path: &str,
    to_path: &str,
    flags: c_int,
    fd_diag: &OpenFdDiag,
) {
    let is_anomaly = fd_diag.result < 0
        || fd_diag.retried
        || fd_diag.fd_flags < 0
        || fd_diag.fd_path.is_empty()
        || fd_diag.fd_path.ends_with(" (deleted)");
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

    let count = REDIRECTED_OPEN_FD_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if !is_anomaly && count != 1 && !count.is_multiple_of(REDIRECTED_OPEN_FD_LOG_SAMPLE_STEP) {
        return;
    }

    log::info!(
        "write op={} from={} to={} flags=0x{:x} ret={} errno={} fd_flags=0x{:x} fd_retry={} fd_path={} n={}",
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
        },
        count
    );
}

fn should_redirect_open_operation(
    hub: &InterceptHub,
    is_system_writer: bool,
    flags: c_int,
) -> bool {
    should_redirect_open_operation_for_mode(
        hub.effective_monitor_only(),
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
            if mapping_read_result.is_redirect()
                && should_allow_app_write_read_redirect(from_path, &mapping_read_result)
            {
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

fn should_allow_app_write_read_redirect(
    from_path: &str,
    redirect_result: &RedirectDecision,
) -> bool {
    redirect_result.is_mapping || is_unallowed_mapping_target_read(from_path)
}

fn is_unallowed_mapping_target_read(from_path: &str) -> bool {
    let normalized_path = paths::data_media_to_storage_path(&paths::normalize(from_path));
    let router = PathRouter::instance();
    router.is_path_mapping_target(&normalized_path)
        && !router.is_path_allowed_real(&normalized_path)
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
        if retry_error_no == libc::EEXIST
            && let Some((existing_result, existing_error_no, retry_flags)) =
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
            // SAFETY: result 是原始 open 调用返回且当前仍持有的描述符。
            unsafe {
                libc::close(result);
            }
            // SAFETY: existing_result 是重试辅助函数成功返回的描述符。
            let fd_flags = unsafe { libc::fcntl(existing_result, libc::F_GETFL) };
            return OpenFdDiag {
                result: existing_result,
                error_no: existing_error_no,
                fd_path: resolve_fd_path(existing_result),
                fd_flags,
                retried: true,
            };
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
    monitor::record_read_only_open_result(hub, op_name, flags, path, path, read_only_path);
    log::debug!(
        "readonly deny op={} path={} read_only_path={} flags=0x{:x}",
        op_name,
        path,
        read_only_path,
        flags
    );
    // 审计与日志会执行系统调用，只读拒绝的 EROFS 必须在它们之后设置。
    runtime::set_read_only_errno();
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
                super::super::caller::update_caller_for_mutation(hub, pathname);
            }

            handle_open_like(hub, "creat", pathname, creat_flags, false, |call_path| {
                call_creat(call_path, mode)
            })
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
    fn resolve_beneath_and_in_root_keep_original_dirfd() {
        assert!(openat2_requires_original_dirfd(RESOLVE_BENEATH));
        assert!(openat2_requires_original_dirfd(RESOLVE_IN_ROOT));
        assert!(openat2_requires_original_dirfd(
            RESOLVE_BENEATH | RESOLVE_IN_ROOT
        ));
    }

    #[test]
    fn nofollow_only_rejects_actual_symlink() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!(
            "srx-nofollow-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        test_require!(std::fs::create_dir_all(&base), "create base");
        let file = base.join("file");
        test_require!(std::fs::write(&file, b"x"), "write file");
        let link = base.join("link");
        test_require!(symlink(&file, &link), "symlink");
        let file_c = test_require!(
            std::ffi::CString::new(file.to_string_lossy().as_bytes()),
            "file c"
        );
        let link_c = test_require!(
            std::ffi::CString::new(link.to_string_lossy().as_bytes()),
            "link c"
        );
        assert!(!final_component_is_symlink(AT_FDCWD, file_c.as_ptr()));
        assert!(final_component_is_symlink(AT_FDCWD, link_c.as_ptr()));
        let _ = std::fs::remove_dir_all(base);
    }
    #[test]
    // O_PATH/O_TMPFILE 不改变 O_NOFOLLOW 的末段判定。
    fn o_path_and_tmpfile_share_nofollow_rule() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!(
            "srx-o-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        test_require!(std::fs::create_dir_all(base.join("dir")), "create dir");
        let link = base.join("link");
        test_require!(symlink(base.join("dir"), &link), "symlink");
        let link_c = test_require!(
            std::ffi::CString::new(link.to_string_lossy().as_bytes()),
            "link c"
        );
        let dir_c = test_require!(
            std::ffi::CString::new(base.join("dir").to_string_lossy().as_bytes()),
            "dir c"
        );
        assert!(should_preserve_original_path(
            libc::O_PATH | libc::O_NOFOLLOW,
            AT_FDCWD,
            link_c.as_ptr()
        ));
        assert!(!should_preserve_original_path(
            libc::O_PATH | libc::O_NOFOLLOW,
            AT_FDCWD,
            dir_c.as_ptr()
        ));
        assert!(!should_preserve_original_path(
            libc::O_TMPFILE | libc::O_RDWR,
            AT_FDCWD,
            dir_c.as_ptr()
        ));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn final_path_only_resolve_flags_can_still_redirect() {
        assert!(!openat2_requires_original_dirfd(0));
        assert!(!openat2_requires_original_dirfd(0x01)); // NO_XDEV
        assert!(!openat2_requires_original_dirfd(0x02)); // NO_MAGICLINKS
        assert!(!openat2_requires_original_dirfd(0x04)); // NO_SYMLINKS
    }
}
