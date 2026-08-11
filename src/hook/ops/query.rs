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
// 向上查找真实祖先目录的层级上限，避免异常路径导致长循环
const PROVIDER_PASSTHROUGH_STANDIN_MAX_DEPTH: usize = 32;
// STATX_TYPE：只请求文件类型，libc 未对 Android 导出该常量
const STATX_TYPE_MASK: c_uint = 0x0000_0001;

// 转发原函数时使用的签名别名，避免在调用点反复书写跨行的裸指针参数列表
type FstatatFn = unsafe extern "C" fn(c_int, *const c_char, *mut libc::stat, c_int) -> c_int;
type StatxFn = unsafe extern "C" fn(c_int, *const c_char, c_int, c_uint, *mut libc::statx) -> c_int;

fn should_bypass_system_writer_query(hub: &InterceptHub, op_name: &str) -> bool {
    if !hub.with_package_name(policy::is_system_writer_package) {
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

// 用原始系统调用判断目录是否真实存在。这里不能走 libc 包装，否则会再次进入本模块的
// PLT hook 造成递归；直接发起 statx 系统调用可以拿到未经改写的真实结果。
fn raw_directory_exists(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 以 NUL 结尾且在调用期间有效，statxbuf 为本地栈变量，内核只写入该结构。
    unsafe {
        let mut statxbuf: libc::statx = std::mem::zeroed();
        let saved_errno = runtime::current_errno();
        let ret = call_statx_syscall(
            AT_FDCWD,
            c_path.as_ptr(),
            0,
            STATX_TYPE_MASK,
            &mut statxbuf as *mut libc::statx,
        );
        runtime::set_errno(saved_errno);
        ret == 0 && (u32::from(statxbuf.stx_mode) & libc::S_IFMT) == libc::S_IFDIR
    }
}

// 直通窗口内被拦下的公共目录，用同一路径空间内最近的真实祖先回答存在性查询。
// 只替换被查询的目标，不改写调用方看到的路径，_data 因此与未拦截时完全一致。
//
// 必须逐级验证祖先是否真实存在，不能假设「未登记的祖先就一定存在」：叶子目录的 mkdir
// 命中沙箱已存在而直接报成功后，File.mkdirs 不会再回溯创建中间层级，这些中间目录既没有
// 登记也不存在，用它们当替身会让存在性查询继续失败。
unsafe fn provider_passthrough_virtual_dir_standin_path(
    dirfd: c_int,
    pathname: *const c_char,
) -> Option<CString> {
    if !crate::hook::is_provider_passthrough_active() {
        return None;
    }
    // SAFETY: dirfd 与 pathname 由调用方 hook 原样透传，在本次调用期间保持有效。
    let path_text = unsafe { resolve_system_writer_query_path(dirfd, pathname) }?;
    // 优先尝试路径本身是虚拟目录的情况，其次只处理父目录为虚拟目录的
    // MediaStore .pending- 文件。MediaProvider 会在 ensureFileColumns 中查询
    // pending 文件，而此时公共父目录仅以虚拟形式存在；其它普通子文件仍应
    // 返回真实查询结果，避免把 .nomedia 等不存在的文件误报为存在。
    let effective_path = if crate::hook::is_provider_passthrough_virtual_dir(&path_text) {
        path_text.clone()
    } else if paths::media_store_pending_display_path(&path_text).is_some() {
        let parent = paths::parent(&path_text);
        if !parent.is_empty() && crate::hook::is_provider_passthrough_virtual_dir(&parent) {
            log::debug!(
                "virtual dir standin parent match path={} parent={}",
                path_text,
                parent
            );
            parent
        } else {
            return None;
        }
    } else {
        return None;
    };
    let is_data_media_input = effective_path.starts_with("/data/media/");
    let path_text = effective_path;
    let mut ancestor = paths::parent(&path_text);
    for _ in 0..PROVIDER_PASSTHROUGH_STANDIN_MAX_DEPTH {
        if ancestor.is_empty() || ancestor == "/" {
            return None;
        }
        if raw_directory_exists(&ancestor) {
            log::debug!(
                "virtual dir standin path={} standin={}",
                path_text,
                ancestor
            );
            // 输入是 /data/media 后端路径时替身保持同一空间，避免跨路径空间比较
            let standin = if is_data_media_input && !ancestor.starts_with("/data/media/") {
                paths::storage_to_data_media_path(&ancestor)
            } else {
                ancestor
            };
            return CString::new(standin).ok();
        }
        ancestor = paths::parent(&ancestor);
    }
    None
}

// 虚拟目录的内容列举解析到沙箱目标，保证列出的文件与应用实际可见的一致
unsafe fn provider_passthrough_virtual_dir_listing_path(
    dirfd: c_int,
    pathname: *const c_char,
) -> Option<CString> {
    if !crate::hook::is_provider_passthrough_active() {
        return None;
    }
    // SAFETY: dirfd 与 pathname 由调用方 hook 原样透传，在本次调用期间保持有效。
    let path_text = unsafe { resolve_system_writer_query_path(dirfd, pathname) }?;
    let target = crate::hook::provider_passthrough_virtual_dir_target(&path_text)?;
    if target.is_empty() {
        return None;
    }
    log::debug!("virtual dir listing path={} target={}", path_text, target);
    CString::new(target).ok()
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(AT_FDCWD, pathname)
            {
                return runtime::call_prev(
                    self_ptr,
                    || libc::stat(standin.as_ptr(), statbuf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                            std::mem::transmute(prev);
                        f(standin.as_ptr(), statbuf)
                    },
                );
            }
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(AT_FDCWD, pathname)
            {
                return runtime::call_prev(
                    self_ptr,
                    || libc::lstat(standin.as_ptr(), statbuf),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int =
                            std::mem::transmute(prev);
                        f(standin.as_ptr(), statbuf)
                    },
                );
            }
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(dirfd, pathname) {
                return runtime::call_prev(
                    self_ptr,
                    || libc::fstatat(dirfd, standin.as_ptr(), statbuf, flags),
                    |prev| {
                        let f: FstatatFn = std::mem::transmute(prev);
                        f(dirfd, standin.as_ptr(), statbuf, flags)
                    },
                );
            }
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(AT_FDCWD, pathname)
            {
                return runtime::call_prev(
                    self_ptr,
                    || libc::access(standin.as_ptr(), mode),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, c_int) -> c_int =
                            std::mem::transmute(prev);
                        f(standin.as_ptr(), mode)
                    },
                );
            }
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(dirfd, pathname) {
                return runtime::call_prev(
                    self_ptr,
                    || libc::faccessat(dirfd, standin.as_ptr(), mode, flags),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, *const c_char, c_int, c_int) -> c_int =
                            std::mem::transmute(prev);
                        f(dirfd, standin.as_ptr(), mode, flags)
                    },
                );
            }
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
            if let Some(standin) = provider_passthrough_virtual_dir_standin_path(dirfd, pathname) {
                return runtime::call_prev(
                    self_ptr,
                    || call_statx_syscall(dirfd, standin.as_ptr(), flags, mask, statxbuf),
                    |prev| {
                        let f: StatxFn = std::mem::transmute(prev);
                        f(dirfd, standin.as_ptr(), flags, mask, statxbuf)
                    },
                );
            }
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
            // 目录列举与存在性查询不同：列出沙箱内容才与应用实际可见的文件一致
            if let Some(target) = provider_passthrough_virtual_dir_listing_path(AT_FDCWD, name) {
                return runtime::call_prev(
                    self_ptr,
                    || libc::opendir(target.as_ptr()),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char) -> *mut libc::DIR =
                            std::mem::transmute(prev);
                        f(target.as_ptr())
                    },
                );
            }
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
// 让 readlink 结果与 cursor 路径使用相同的映射视图。
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

    if !has_explicit_app_caller && hub.with_package_name(policy::is_system_writer_package) {
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
    let display_path = writer::reverse_map_caller_path(&normalized, &caller_package, caller_uid);
    if display_path.is_empty() || display_path == normalized {
        path.to_string()
    } else {
        display_path
    }
}

// 系统写入进程查询遇到 ENOENT 时，通过重定向决策重试。
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
