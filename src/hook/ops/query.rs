use super::super::context;
use super::super::path as path_utils;
use super::super::runtime;
use super::super::stats::InterceptHub;
use super::super::util::c_str_to_string;
use crate::platform::paths;
use crate::redirect::{policy, process_redirect_path, writer};
use libc::{AT_FDCWD, c_char, c_int, c_uint, c_void};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

static SYSTEM_WRITER_QUERY_BYPASS_COUNT: AtomicU64 = AtomicU64::new(0);
static READLINK_REVERSE_UNCHANGED_COUNT: AtomicU64 = AtomicU64::new(0);
const SYSTEM_WRITER_QUERY_BYPASS_LOG_STEP: u64 = 4096;
const READLINK_REVERSE_UNCHANGED_LOG_STEP: u64 = 4096;
const QUERY_FALLBACK_CALLER_MAX_AGE_MS: i64 = 1500;

fn should_bypass_system_writer_query(hub: &InterceptHub, op_name: &str) -> bool {
    if !policy::is_system_writer_package(&hub.get_package_name()) {
        return false;
    }
    if context::is_current_caller_scope_active() {
        return false;
    }

    let count = SYSTEM_WRITER_QUERY_BYPASS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count == 1 || count.is_multiple_of(SYSTEM_WRITER_QUERY_BYPASS_LOG_STEP) {
        log::debug!(
            "query bypass system_writer pkg={} op={} n={}",
            hub.get_package_name(),
            op_name,
            count
        );
    }
    true
}

unsafe fn call_query_with_writer_fallback<F>(
    hub: &InterceptHub,
    dirfd: c_int,
    pathname: *const c_char,
    mut call_path: F,
) -> c_int
where
    F: FnMut(*const c_char) -> c_int,
{
    let mut ret = call_path(pathname);
    let mut error_no = runtime::current_errno();
    if ret != 0 && should_fix_system_writer_private_owner_for_query_error(error_no) {
        fix_system_writer_private_owner_for_query(dirfd, pathname);
        let retry = call_path(pathname);
        if retry == 0 {
            return retry;
        }
        let retry_error = runtime::current_errno();
        if retry_error != error_no {
            ret = retry;
            error_no = retry_error;
        }
    }
    if ret == 0 || !is_retryable_system_writer_query_error(error_no) {
        return ret;
    }
    if !should_attempt_system_writer_query_fallback(hub) {
        runtime::set_errno(error_no);
        return ret;
    }

    if let Some(redirected) = writer_fallback_redirect(hub, dirfd, pathname)
        && let Ok(c_path) = CString::new(redirected)
    {
        return call_path(c_path.as_ptr());
    }

    runtime::set_errno(error_no);
    ret
}

unsafe fn call_opendir_with_writer_fallback<F>(
    hub: &InterceptHub,
    pathname: *const c_char,
    mut call_path: F,
) -> *mut libc::DIR
where
    F: FnMut(*const c_char) -> *mut libc::DIR,
{
    let mut ret = call_path(pathname);
    let mut error_no = runtime::current_errno();
    if ret.is_null() && should_fix_system_writer_private_owner_for_query_error(error_no) {
        fix_system_writer_private_owner_for_query(AT_FDCWD, pathname);
        let retry = call_path(pathname);
        if !retry.is_null() {
            return retry;
        }
        let retry_error = runtime::current_errno();
        if retry_error != error_no {
            ret = retry;
            error_no = retry_error;
        }
    }
    if !ret.is_null() || !is_retryable_system_writer_query_error(error_no) {
        return ret;
    }
    if !should_attempt_system_writer_query_fallback(hub) {
        runtime::set_errno(error_no);
        return ret;
    }

    if let Some(redirected) = writer_fallback_redirect(hub, AT_FDCWD, pathname)
        && let Ok(c_path) = CString::new(redirected)
    {
        return call_path(c_path.as_ptr());
    }

    runtime::set_errno(error_no);
    ret
}

fn is_retryable_system_writer_query_error(error_no: c_int) -> bool {
    error_no == libc::ENOENT || error_no == libc::EACCES || error_no == libc::EPERM
}

fn should_fix_system_writer_private_owner_for_query_error(error_no: c_int) -> bool {
    error_no == libc::EACCES || error_no == libc::EPERM
}

fn should_attempt_system_writer_query_fallback(hub: &InterceptHub) -> bool {
    if context::is_current_caller_scope_active() {
        return true;
    }

    has_recent_external_caller_signal_for_query_fallback(
        &hub.get_current_caller_package(),
        hub.get_current_caller_uid(),
        context::get_current_caller_age_ms(),
        context::is_current_caller_from_external_signal(),
    )
}

fn has_recent_external_caller_signal_for_query_fallback(
    caller_package: &str,
    caller_uid: i32,
    caller_age_ms: i64,
    from_external_signal: bool,
) -> bool {
    from_external_signal
        && caller_uid >= writer::ANDROID_APP_UID_START
        && (0..=QUERY_FALLBACK_CALLER_MAX_AGE_MS).contains(&caller_age_ms)
        && (caller_package.is_empty() || !policy::is_system_writer_package(caller_package))
}

unsafe fn fix_system_writer_private_owner_for_query(dirfd: c_int, pathname: *const c_char) {
    let Some(path_text) = resolve_system_writer_query_path(dirfd, pathname) else {
        return;
    };
    runtime::fix_system_writer_android_private_owner(&path_text, false);
}

unsafe fn resolve_system_writer_query_path(
    dirfd: c_int,
    pathname: *const c_char,
) -> Option<String> {
    if pathname.is_null() {
        return None;
    }
    let path_text = c_str_to_string(pathname);
    if path_text.is_empty() {
        return None;
    }
    let resolved = if path_text.starts_with('/') {
        paths::normalize(&path_text)
    } else {
        path_utils::resolve_path_for_dirfd(dirfd, &path_text)
    };
    if resolved.is_empty() || !resolved.starts_with('/') {
        None
    } else {
        Some(resolved)
    }
}

unsafe fn call_statx_syscall(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mask: c_uint,
    statxbuf: *mut libc::statx,
) -> c_int {
    libc::syscall(libc::SYS_statx, dirfd, pathname, flags, mask, statxbuf) as c_int
}

pub unsafe extern "C" fn hooked_stat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    let self_ptr = hooked_stat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::stat(pathname, statbuf),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, statbuf)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if should_bypass_system_writer_query(hub, "stat") {
                return call_query_with_writer_fallback(hub, AT_FDCWD, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::stat(call_path, statbuf),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, statbuf)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "stat", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::stat(final_path, statbuf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                            std::mem::transmute(prev);
                        f(final_path, statbuf)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_lstat(pathname: *const c_char, statbuf: *mut libc::stat) -> c_int {
    let self_ptr = hooked_lstat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::lstat(pathname, statbuf),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, statbuf)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if should_bypass_system_writer_query(hub, "lstat") {
                return call_query_with_writer_fallback(hub, AT_FDCWD, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::lstat(call_path, statbuf),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, statbuf)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "lstat", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::lstat(final_path, statbuf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                            std::mem::transmute(prev);
                        f(final_path, statbuf)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_fstatat(
    dirfd: c_int,
    pathname: *const c_char,
    statbuf: *mut libc::stat,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_fstatat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::fstatat(dirfd, pathname, statbuf, flags),
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        *mut libc::stat,
                        c_int,
                    ) -> c_int = std::mem::transmute(prev);
                    f(dirfd, pathname, statbuf, flags)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if should_bypass_system_writer_query(hub, "fstatat") {
                return call_query_with_writer_fallback(hub, dirfd, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::fstatat(dirfd, call_path, statbuf, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                *mut libc::stat,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, statbuf, flags)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "fstatat", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::fstatat(dirfd, final_path, statbuf, flags),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            c_int,
                            *const c_char,
                            *mut libc::stat,
                            c_int,
                        ) -> c_int = std::mem::transmute(prev);
                        f(dirfd, final_path, statbuf, flags)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_access(pathname: *const c_char, mode: c_int) -> c_int {
    let self_ptr = hooked_access as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::access(pathname, mode),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, c_int) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, mode)
                },
            )
        },
        |hub| {
            hub.increment_access_calls();
            if should_bypass_system_writer_query(hub, "access") {
                return call_query_with_writer_fallback(hub, AT_FDCWD, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::access(call_path, mode),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char, c_int) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path, mode)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "access", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::access(final_path, mode),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, c_int) -> c_int =
                            std::mem::transmute(prev);
                        f(final_path, mode)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_faccessat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: c_int,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_faccessat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::faccessat(dirfd, pathname, mode, flags),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, *const c_char, c_int, c_int) -> c_int =
                        std::mem::transmute(prev);
                    f(dirfd, pathname, mode, flags)
                },
            )
        },
        |hub| {
            hub.increment_access_calls();
            if should_bypass_system_writer_query(hub, "faccessat") {
                return call_query_with_writer_fallback(hub, dirfd, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::faccessat(dirfd, call_path, mode, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                c_int,
                                c_int,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, mode, flags)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "faccessat", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::faccessat(dirfd, final_path, mode, flags),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, *const c_char, c_int, c_int) -> c_int =
                            std::mem::transmute(prev);
                        f(dirfd, final_path, mode, flags)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_statx(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
    mask: c_uint,
    statxbuf: *mut libc::statx,
) -> c_int {
    let self_ptr = hooked_statx as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || call_statx_syscall(dirfd, pathname, flags, mask, statxbuf),
                |prev| {
                    let f: unsafe extern "C" fn(
                        c_int,
                        *const c_char,
                        c_int,
                        c_uint,
                        *mut libc::statx,
                    ) -> c_int = std::mem::transmute(prev);
                    f(dirfd, pathname, flags, mask, statxbuf)
                },
            )
        },
        |hub| {
            hub.increment_stat_calls();
            if should_bypass_system_writer_query(hub, "statx") {
                return call_query_with_writer_fallback(hub, dirfd, pathname, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || call_statx_syscall(dirfd, call_path, flags, mask, statxbuf),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
                                c_int,
                                c_uint,
                                *mut libc::statx,
                            ) -> c_int = std::mem::transmute(prev);
                            f(dirfd, call_path, flags, mask, statxbuf)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "statx", pathname, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || call_statx_syscall(dirfd, final_path, flags, mask, statxbuf),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            c_int,
                            *const c_char,
                            c_int,
                            c_uint,
                            *mut libc::statx,
                        ) -> c_int = std::mem::transmute(prev);
                        f(dirfd, final_path, flags, mask, statxbuf)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_opendir(name: *const c_char) -> *mut libc::DIR {
    let self_ptr = hooked_opendir as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::opendir(name),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char) -> *mut libc::DIR =
                        std::mem::transmute(prev);
                    f(name)
                },
            )
        },
        |hub| {
            hub.increment_opendir_calls();
            if should_bypass_system_writer_query(hub, "opendir") {
                return call_opendir_with_writer_fallback(hub, name, |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::opendir(call_path),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char) -> *mut libc::DIR =
                                std::mem::transmute(prev);
                            f(call_path)
                        },
                    )
                });
            }
            runtime::with_redirected_path(hub, "opendir", name, |final_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::opendir(final_path),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char) -> *mut libc::DIR =
                            std::mem::transmute(prev);
                        f(final_path)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_readlink(
    pathname: *const c_char,
    buf: *mut c_char,
    bufsiz: usize,
) -> isize {
    let self_ptr = hooked_readlink as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::readlink(pathname, buf, bufsiz),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> isize =
                        std::mem::transmute(prev);
                    f(pathname, buf, bufsiz)
                },
            )
        },
        |hub| {
            hub.increment_readlink_calls();
            let result = if should_bypass_system_writer_query(hub, "readlink") {
                runtime::call_prev(
                    self_ptr,
                    || libc::readlink(pathname, buf, bufsiz),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> isize =
                            std::mem::transmute(prev);
                        f(pathname, buf, bufsiz)
                    },
                )
            } else {
                runtime::with_redirected_path(hub, "readlink", pathname, |final_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::readlink(final_path, buf, bufsiz),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                *const c_char,
                                *mut c_char,
                                usize,
                            ) -> isize = std::mem::transmute(prev);
                            f(final_path, buf, bufsiz)
                        },
                    )
                })
            };
            reverse_readlink_result_if_visible(result, buf, bufsiz, "readlink")
        },
    )
}

pub unsafe extern "C" fn hooked_readlinkat(
    dirfd: libc::c_int,
    pathname: *const c_char,
    buf: *mut c_char,
    bufsiz: usize,
) -> isize {
    let self_ptr = hooked_readlinkat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::readlinkat(dirfd, pathname, buf, bufsiz),
                |prev| {
                    let f: unsafe extern "C" fn(
                        libc::c_int,
                        *const c_char,
                        *mut c_char,
                        usize,
                    ) -> isize = std::mem::transmute(prev);
                    f(dirfd, pathname, buf, bufsiz)
                },
            )
        },
        |hub| {
            hub.increment_readlink_calls();
            let result = if should_bypass_system_writer_query(hub, "readlinkat") {
                runtime::call_prev(
                    self_ptr,
                    || libc::readlinkat(dirfd, pathname, buf, bufsiz),
                    |prev| {
                        let f: unsafe extern "C" fn(
                            libc::c_int,
                            *const c_char,
                            *mut c_char,
                            usize,
                        ) -> isize = std::mem::transmute(prev);
                        f(dirfd, pathname, buf, bufsiz)
                    },
                )
            } else {
                runtime::with_redirected_path(hub, "readlinkat", pathname, |final_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::readlinkat(dirfd, final_path, buf, bufsiz),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                libc::c_int,
                                *const c_char,
                                *mut c_char,
                                usize,
                            ) -> isize = std::mem::transmute(prev);
                            f(dirfd, final_path, buf, bufsiz)
                        },
                    )
                })
            };
            reverse_readlink_result_if_visible(result, buf, bufsiz, "readlinkat")
        },
    )
}

unsafe fn reverse_readlink_result_if_visible(
    result: isize,
    buf: *mut c_char,
    bufsiz: usize,
    op_name: &str,
) -> isize {
    if result <= 0 || crate::hook::is_provider_passthrough_active() {
        return result;
    }
    let result_len = result as usize;
    if result_len >= bufsiz {
        return result;
    }

    *buf.add(result_len) = 0;
    let result_str = c_str_to_string(buf);
    if result_str.is_empty() {
        return result;
    }
    if should_preserve_readlink_result_for_system_writer_self(&result_str) {
        log_readlink_reverse_unchanged(op_name, &result_str);
        return result;
    }

    let display_path = reverse_mapping_readlink_path_for_visible_caller(
        &writer::reverse_readlink_sandbox_path(&result_str),
    );
    if display_path == result_str {
        log_readlink_reverse_unchanged(op_name, &result_str);
        return result;
    }

    log::debug!(
        "{} reverse: sandbox={} -> display={}",
        op_name,
        result_str,
        display_path
    );
    if display_path.len() >= bufsiz {
        return result;
    }

    let display_bytes = display_path.as_bytes();
    let copy_len = display_bytes.len();
    std::ptr::copy_nonoverlapping(display_bytes.as_ptr(), buf.cast::<u8>(), copy_len);
    *buf.add(copy_len) = 0;
    copy_len as isize
}

fn should_preserve_readlink_result_for_system_writer_self(path: &str) -> bool {
    let hub = InterceptHub::instance();
    should_preserve_readlink_result_for_system_writer_self_context(
        &hub.get_package_name(),
        &hub.get_current_caller_package(),
        hub.get_current_caller_uid(),
        context::is_current_caller_scope_active(),
        path,
    )
}

fn should_preserve_readlink_result_for_system_writer_self_context(
    process_package: &str,
    caller_package: &str,
    caller_uid: i32,
    caller_scope_active: bool,
    path: &str,
) -> bool {
    if !policy::is_system_writer_package(process_package)
        || !readlink_sandbox_reverse_may_change(path)
    {
        return false;
    }
    !(caller_scope_active
        && caller_uid >= writer::ANDROID_APP_UID_START
        && !caller_package.is_empty()
        && !policy::is_system_writer_package(caller_package))
}

fn readlink_sandbox_reverse_may_change(path: &str) -> bool {
    path.starts_with("/data/media/") || path.contains("/Android/data/")
}

fn log_readlink_reverse_unchanged(op_name: &str, path: &str) {
    let count = READLINK_REVERSE_UNCHANGED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count == 1 || count.is_multiple_of(READLINK_REVERSE_UNCHANGED_LOG_STEP) {
        log::debug!("{} reverse unchanged path={} n={}", op_name, path, count);
    }
}

/// system_writer bypass 返回 ENOENT 时，尝试对路径做重定向决策。
/// 如果路径命中重定向规则，返回重定向后的路径；否则返回 None。
// Keep readlink results in the same mapping view that cursor paths expose.
fn reverse_mapping_readlink_path_for_visible_caller(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let hub = InterceptHub::instance();

    let mut caller_package = hub.get_current_caller_package();
    let mut caller_uid = hub.get_current_caller_uid();
    let has_explicit_app_caller = context::is_current_caller_scope_active()
        && caller_uid >= writer::ANDROID_APP_UID_START
        && !caller_package.is_empty()
        && !policy::is_system_writer_package(&caller_package);

    if !has_explicit_app_caller && policy::is_system_writer_package(&hub.get_package_name()) {
        return path.to_string();
    }

    if !has_explicit_app_caller && caller_uid < writer::ANDROID_APP_UID_START {
        let self_uid = unsafe { libc::getuid() as i32 };
        let self_package = hub.get_package_name();
        if self_uid >= writer::ANDROID_APP_UID_START
            && !self_package.is_empty()
            && !policy::is_system_writer_package(&self_package)
            && !policy::is_shared_uid_process(self_uid)
        {
            caller_uid = self_uid;
            caller_package = self_package;
        }
    }
    if caller_uid < writer::ANDROID_APP_UID_START || caller_package.is_empty() {
        return path.to_string();
    }

    let normalized = paths::normalize(path);
    let mappings = writer::get_caller_mappings(&caller_package, caller_uid);
    let display_path = writer::reverse_map_path_by_caller_mappings(&normalized, &mappings);
    if display_path.is_empty() || display_path == normalized {
        path.to_string()
    } else {
        display_path
    }
}

// If a system-writer query hits ENOENT, retry through the redirect decision.
unsafe fn writer_fallback_redirect(
    hub: &InterceptHub,
    dirfd: c_int,
    pathname: *const c_char,
) -> Option<String> {
    let path_text = resolve_system_writer_query_path(dirfd, pathname)?;
    let _no_path_owner_infer = crate::hook::enter_path_owner_inference_disabled();
    let decision = process_redirect_path_for_query_fallback(hub, &path_text);
    if decision.is_redirect() && !decision.new_path.is_empty() {
        Some(decision.new_path)
    } else {
        None
    }
}

fn process_redirect_path_for_query_fallback(
    hub: &InterceptHub,
    path: &str,
) -> crate::redirect::RedirectDecision {
    process_redirect_path(hub, path)
}
