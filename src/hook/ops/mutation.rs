use super::super::diagnostic;
use super::super::media_fuse;
use super::super::monitor;
use super::super::path as path_utils;
use super::super::runtime;
use super::super::stats::InterceptHub;
use super::super::util::c_str_to_string;
use super::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::config::SettingsHub;
use crate::monitor::OpKind;
use crate::platform::{self, paths};
use crate::redirect::{
    RedirectAction, RedirectDecision, policy, process_write_redirect_path, record_redirect_hit,
    writer,
};
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t, off_t, timespec};
use std::ffi::CString;

pub unsafe extern "C" fn hooked_mkdir(pathname: *const c_char, mode: mode_t) -> c_int {
    let self_ptr = hooked_mkdir as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::mkdir(pathname, mode),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char, mode_t) -> c_int =
                        std::mem::transmute(prev);
                    f(pathname, mode)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_mkdir_like(hub, "mkdir", AT_FDCWD, pathname, mode, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::mkdir(call_path, mode),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char, mode_t) -> c_int =
                            std::mem::transmute(prev);
                        f(call_path, mode)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_mkdirat(
    dirfd: c_int,
    pathname: *const c_char,
    mode: mode_t,
) -> c_int {
    let self_ptr = hooked_mkdirat as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::mkdirat(dirfd, pathname, mode),
                |prev| {
                    let f: unsafe extern "C" fn(c_int, *const c_char, mode_t) -> c_int =
                        std::mem::transmute(prev);
                    f(dirfd, pathname, mode)
                },
            )
        },
        |hub| {
            hub.increment_mkdir_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_mkdir_like(hub, "mkdirat", dirfd, pathname, mode, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::mkdirat(dirfd, call_path, mode),
                    |prev| {
                        let f: unsafe extern "C" fn(c_int, *const c_char, mode_t) -> c_int =
                            std::mem::transmute(prev);
                        f(dirfd, call_path, mode)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_unlink(pathname: *const c_char) -> c_int {
    let self_ptr = hooked_unlink as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::unlink(pathname),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char) -> c_int = std::mem::transmute(prev);
                    f(pathname)
                },
            )
        },
        |hub| {
            hub.increment_unlink_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_unlink_like(hub, "unlink", AT_FDCWD, pathname, -1, false, |call_path| {
                runtime::call_prev(
                    self_ptr,
                    || libc::unlink(call_path),
                    |prev| {
                        let f: unsafe extern "C" fn(*const c_char) -> c_int =
                            std::mem::transmute(prev);
                        f(call_path)
                    },
                )
            })
        },
    )
}

pub unsafe extern "C" fn hooked_unlinkat(
    dirfd: c_int,
    pathname: *const c_char,
    flags: c_int,
) -> c_int {
    let self_ptr = hooked_unlinkat as *mut c_void;
    let call_original = |call_path: *const c_char| -> c_int {
        runtime::call_prev(
            self_ptr,
            || unsafe { libc::syscall(libc::SYS_unlinkat, dirfd, call_path, flags) as c_int },
            |prev| {
                let f: unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int =
                    unsafe { std::mem::transmute(prev) };
                unsafe { f(dirfd, call_path, flags) }
            },
        )
    };

    runtime::with_hook_guard(
        || call_original(pathname),
        |hub| {
            hub.increment_unlink_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_unlink_like(hub, "unlinkat", dirfd, pathname, flags, true, call_original)
        },
    )
}

pub unsafe extern "C" fn hooked_rmdir(pathname: *const c_char) -> c_int {
    let self_ptr = hooked_rmdir as *mut c_void;
    runtime::with_hook_guard(
        || {
            runtime::call_prev(
                self_ptr,
                || libc::rmdir(pathname),
                |prev| {
                    let f: unsafe extern "C" fn(*const c_char) -> c_int = std::mem::transmute(prev);
                    f(pathname)
                },
            )
        },
        |hub| {
            hub.increment_unlink_calls();
            if runtime::should_resolve_caller_context(hub) {
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Rmdir,
                "rmdir",
                AT_FDCWD,
                pathname,
                0,
                None,
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::rmdir(call_path),
                        |prev| {
                            let f: unsafe extern "C" fn(*const c_char) -> c_int =
                                std::mem::transmute(prev);
                            f(call_path)
                        },
                    )
                },
            )
        },
    )
}

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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_link_audit(
                hub,
                "link",
                AT_FDCWD,
                oldpath,
                AT_FDCWD,
                newpath,
                -1,
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_link_audit(
                hub,
                "linkat",
                olddirfd,
                oldpath,
                newdirfd,
                newpath,
                flags,
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            let target_text = unsafe { c_str_to_string(target) };
            let extra = if target_text.is_empty() {
                None
            } else {
                Some(format!("from={}", target_text))
            };
            handle_single_path_audit(
                hub,
                OpKind::Symlink,
                "symlink",
                AT_FDCWD,
                linkpath,
                0,
                extra,
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            let target_text = unsafe { c_str_to_string(target) };
            let extra = if target_text.is_empty() {
                None
            } else {
                Some(format!("from={}", target_text))
            };
            handle_single_path_audit(
                hub,
                OpKind::Symlink,
                "symlinkat",
                newdirfd,
                linkpath,
                0,
                extra,
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Truncate,
                "truncate",
                AT_FDCWD,
                pathname,
                0,
                Some(format!("length={}", length)),
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Truncate,
                "truncate64",
                AT_FDCWD,
                pathname,
                0,
                Some(format!("length={}", length)),
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
                super::super::caller::update_caller_package_for_current_thread(hub);
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
                super::super::caller::update_caller_package_for_current_thread(hub);
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Chmod,
                "chmod",
                AT_FDCWD,
                pathname,
                mode as i32,
                Some(format!("mode=0{:o}", mode)),
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
                super::super::caller::update_caller_package_for_current_thread(hub);
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Chmod,
                "fchmodat",
                dirfd,
                pathname,
                flags,
                Some(format!("mode=0{:o}|flags=0x{:x}", mode, flags)),
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
                super::super::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                OpKind::Utimens,
                "utimensat",
                dirfd,
                pathname,
                flags,
                Some(format!("flags=0x{:x}", flags)),
                |call_path| {
                    runtime::call_prev(
                        self_ptr,
                        || libc::utimensat(dirfd, call_path, times, flags),
                        |prev| {
                            let f: unsafe extern "C" fn(
                                c_int,
                                *const c_char,
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
                super::super::caller::update_caller_package_for_current_thread(hub);
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

fn handle_mkdir_like<F>(
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

    let PreparedPath::Ready {
        path_for_decision, ..
    } = (unsafe { prepare_relevant_path(hub, op_name, dirfd, pathname, mode as i32, true) })
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

    let mut redirect_result = process_redirect_path_for_mutation(hub, path_for_decision.as_ref());
    if !redirect_result.is_redirect()
        && !redirect_result.is_denied()
        && let Some(mapping_redirect) = resolve_mapping_request_mkdir_redirect(
            &hub.get_package_name(),
            path_for_decision.as_ref(),
        )
    {
        redirect_result = mapping_redirect;
    }
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

    if should_virtualize_anonymous_system_writer_mkdir(
        hub,
        path_for_decision.as_ref(),
        &redirect_result,
    ) {
        log::info!(
            "{} virtual success: anonymous system writer sandbox mkdir path={} redirect={}",
            op_name,
            path_for_decision,
            redirect_result.new_path
        );
        return 0;
    }

    let result = if redirect_result.is_redirect() {
        let redirected_path = redirect_result.new_path.clone();
        record_redirect_hit(hub, op_name, path_for_decision.as_ref(), &redirected_path);
        runtime::ensure_redirect_parent_dirs(&redirected_path, 0o2773);
        if let Ok(c_path) = CString::new(redirected_path) {
            call_original(c_path.as_ptr())
        } else {
            call_original(pathname)
        }
    } else {
        call_original(pathname)
    };
    let error_no = runtime::errno_for_result(result);
    if result == 0
        && policy::is_system_writer_package(&hub.get_package_name())
        && !hub.is_monitor_only()
    {
        let owner_path = if redirect_result.is_redirect() {
            redirect_result.new_path.as_str()
        } else {
            path_for_decision.as_ref()
        };
        runtime::fix_system_writer_android_private_owner(owner_path, true);
    }
    if redirect_result.is_redirect() {
        if result == 0 || error_no == libc::EEXIST {
            cleanup_empty_redirect_source_dir_if_needed(
                op_name,
                path_for_decision.as_ref(),
                &redirect_result,
            );
        }
        log::debug!(
            "mkdir redirect result: result={} errno={} path={} redirect={}",
            result,
            error_no,
            path_for_decision,
            redirect_result.new_path
        );
        if result == 0 || error_no == libc::EEXIST || error_no == libc::ENOTDIR {
            runtime::normalize_redirect_directory(&redirect_result.new_path);
            if error_no == libc::EEXIST || error_no == libc::ENOTDIR {
                log::debug!(
                    "mkdir redirect exists/notdir: returning 0 errno={} path={}",
                    error_no,
                    path_for_decision
                );
                monitor::record_mkdir_result_from(
                    hub,
                    op_name,
                    &redirect_result.new_path,
                    path_for_decision.as_ref(),
                    0,
                    0,
                );
                return 0;
            }
        }
    }
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

fn cleanup_empty_redirect_source_dir_if_needed(
    op_name: &str,
    source_path: &str,
    redirect_result: &RedirectDecision,
) {
    if !redirect_result.is_redirect()
        || redirect_result.is_mapping
        || redirect_result.new_path.is_empty()
        || !is_public_default_sandbox_redirect(source_path, &redirect_result.new_path)
    {
        return;
    }

    let source = paths::normalize(source_path);
    let source_backend = if source.starts_with("/storage/emulated/") {
        paths::storage_to_data_media_path(&source)
    } else if source.starts_with("/data/media/") {
        source
    } else {
        return;
    };
    let target_backend = paths::storage_to_data_media_path(&paths::data_media_to_storage_path(
        &paths::normalize(&redirect_result.new_path),
    ));
    if source_backend.is_empty()
        || target_backend.is_empty()
        || paths::eq_ignore_case(&source_backend, &target_backend)
    {
        return;
    }

    let Ok(c_path) = CString::new(source_backend.as_str()) else {
        return;
    };
    let saved_errno = runtime::current_errno();
    let ret = unsafe { libc::rmdir(c_path.as_ptr()) };
    let cleanup_errno = runtime::current_errno();
    runtime::set_errno(saved_errno);
    if ret == 0 {
        log::info!(
            "{} cleanup empty redirected source dir path={} backend={} target={}",
            op_name,
            source_path,
            source_backend,
            redirect_result.new_path
        );
        return;
    }
    if cleanup_errno == libc::ENOENT
        || cleanup_errno == libc::ENOTEMPTY
        || cleanup_errno == libc::EEXIST
        || cleanup_errno == libc::ENOTDIR
    {
        log::debug!(
            "{} cleanup source dir skipped path={} errno={}",
            op_name,
            source_path,
            cleanup_errno
        );
        return;
    }
    log::warn!(
        "{} cleanup source dir failed path={} errno={}",
        op_name,
        source_path,
        cleanup_errno
    );
}

fn is_public_default_sandbox_redirect(source_path: &str, target_path: &str) -> bool {
    if source_path.is_empty() || target_path.is_empty() {
        return false;
    }

    let source = paths::data_media_to_storage_path(&paths::normalize(source_path));
    let user_id = paths::extract_user_id_from_storage_path(&source);
    if user_id < 0 {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(source_suffix) = paths::relative_child_path(&source, &storage_root) else {
        return false;
    };
    if source_suffix.is_empty()
        || source_suffix == "Android"
        || source_suffix.starts_with("Android/")
    {
        return false;
    }

    let target = paths::data_media_to_storage_path(&paths::normalize(target_path));
    let android_data_root = paths::join(&paths::join(&storage_root, "Android"), "data");
    let Some(target_suffix) = paths::relative_child_path(&target, &android_data_root) else {
        return false;
    };
    let mut parts = target_suffix.splitn(3, '/');
    let package_name = parts.next().unwrap_or("");
    let sdcard_segment = parts.next().unwrap_or("");
    let redirected_suffix = parts.next().unwrap_or("");
    !package_name.is_empty()
        && sdcard_segment == "sdcard"
        && !redirected_suffix.is_empty()
        && redirected_suffix.eq_ignore_ascii_case(source_suffix)
}

fn resolve_mapping_request_mkdir_redirect(
    package_name: &str,
    path: &str,
) -> Option<RedirectDecision> {
    if !policy::is_system_writer_package(package_name) || path.is_empty() {
        return None;
    }

    let public_path = paths::data_media_to_storage_path(&paths::normalize(path));
    let user_id = paths::extract_user_id_from_storage_path(&public_path);
    if user_id < 0 {
        return None;
    }
    let resolved_path = paths::resolve_user_path(&public_path, user_id);
    if resolved_path.is_empty() || !writer::is_path_in_user_storage(&resolved_path, user_id) {
        return None;
    }

    let caller_package = SettingsHub::instance()
        .resolve_mapping_request_package_by_path_for_user(user_id, &resolved_path);
    if caller_package.is_empty() {
        return None;
    }
    let mut caller_uid = policy::get_fresh_uid_for_package(&caller_package);
    if caller_uid < writer::ANDROID_APP_UID_START {
        caller_uid = user_id
            .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
            .saturating_add(writer::ANDROID_APP_UID_START);
    }
    let mappings = writer::get_caller_mappings(&caller_package, caller_uid);
    let mapped_path = writer::map_path_by_caller_mappings(&resolved_path, &mappings);
    if mapped_path.is_empty() || paths::eq_ignore_case(&mapped_path, &resolved_path) {
        return None;
    }

    log::debug!(
        "mkdir mapping request redirect caller={} uid={} from={} to={}",
        caller_package,
        caller_uid,
        resolved_path,
        mapped_path
    );
    Some(RedirectDecision {
        action: RedirectAction::Redirect,
        new_path: mapped_path,
        is_mapping: true,
    })
}

fn should_virtualize_anonymous_system_writer_mkdir(
    hub: &InterceptHub,
    source_path: &str,
    redirect_result: &RedirectDecision,
) -> bool {
    if !redirect_result.is_redirect()
        || redirect_result.is_mapping
        || redirect_result.new_path.is_empty()
        || hub.is_monitor_only()
        || !policy::is_system_writer_package(&hub.get_package_name())
        || !hub.get_current_caller_package().is_empty()
        || hub.get_current_caller_uid() >= writer::ANDROID_APP_UID_START
    {
        return false;
    }

    is_own_default_sandbox_redirect(
        &hub.get_package_name(),
        source_path,
        &redirect_result.new_path,
    )
}

fn is_own_default_sandbox_redirect(
    package_name: &str,
    source_path: &str,
    target_path: &str,
) -> bool {
    if package_name.is_empty() || source_path.is_empty() || target_path.is_empty() {
        return false;
    }
    let source = paths::normalize(source_path);
    let user_id = paths::extract_user_id_from_storage_path(&source);
    if user_id < 0 {
        return false;
    }

    let target = paths::data_media_to_storage_path(&paths::normalize(target_path));
    let own_sandbox_root = paths::join(
        &paths::join(
            &paths::join(
                &paths::join(&paths::storage_user_root_for_user(user_id), "Android"),
                "data",
            ),
            package_name,
        ),
        "sdcard",
    );
    paths::is_same_or_child(&target, &own_sandbox_root)
}

fn handle_unlink_like<F>(
    hub: &InterceptHub,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    flags: i32,
    should_preserve_errno: bool,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char) -> c_int,
{
    if pathname.is_null() {
        return call_original(pathname);
    }

    let PreparedPath::Ready {
        path_for_decision, ..
    } = (unsafe { prepare_relevant_path(hub, op_name, dirfd, pathname, flags, true) })
    else {
        return call_original(pathname);
    };

    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), flags);

    if !should_apply_mutation_policy(hub) {
        let result = call_original(pathname);
        let current_errno = runtime::current_errno();
        monitor::record_unlink_result(
            hub,
            op_name,
            path_for_decision.as_ref(),
            result,
            if result < 0 { current_errno } else { 0 },
            flags,
        );
        if should_preserve_errno {
            runtime::set_errno(current_errno);
        }
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
        let result = deny_read_only_unlink(
            hub,
            op_name,
            path_for_decision.as_ref(),
            flags,
            &redirect_result.new_path,
        );
        if should_preserve_errno {
            runtime::set_read_only_errno();
        }
        return result;
    }

    let mut record_path = path_for_decision.as_ref();
    let mut record_from = None;
    let result = if redirect_result.is_redirect() {
        record_redirect_hit(
            hub,
            op_name,
            path_for_decision.as_ref(),
            &redirect_result.new_path,
        );
        if let Ok(c_path) = CString::new(redirect_result.new_path.as_str()) {
            record_path = redirect_result.new_path.as_str();
            record_from = Some(path_for_decision.as_ref());
            call_original(c_path.as_ptr())
        } else {
            call_original(pathname)
        }
    } else {
        call_original(pathname)
    };
    let current_errno = runtime::current_errno();
    if let Some(from_path) = record_from {
        monitor::record_unlink_result_from(
            hub,
            op_name,
            record_path,
            from_path,
            result,
            if result < 0 { current_errno } else { 0 },
            flags,
        );
    } else {
        monitor::record_unlink_result(
            hub,
            op_name,
            record_path,
            result,
            if result < 0 { current_errno } else { 0 },
            flags,
        );
    }
    if should_preserve_errno {
        runtime::set_errno(current_errno);
    }
    result
}

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
                super::super::caller::update_caller_package_for_current_thread(hub);
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
                super::super::caller::update_caller_package_for_current_thread(hub);
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

fn handle_single_path_audit<F>(
    hub: &InterceptHub,
    kind: OpKind,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    log_flags: i32,
    extra_tail: Option<String>,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char) -> c_int,
{
    if pathname.is_null() {
        return call_original(pathname);
    }

    let PreparedPath::Ready {
        path_for_decision, ..
    } = (unsafe { prepare_relevant_path(hub, op_name, dirfd, pathname, log_flags, true) })
    else {
        return call_original(pathname);
    };

    diagnostic::log_diag_path_event(hub, op_name, "input", path_for_decision.as_ref(), log_flags);
    if should_apply_mutation_policy(hub)
        && deny_read_only_single_path_if_needed(
            hub,
            kind,
            op_name,
            path_for_decision.as_ref(),
            extra_tail.as_deref(),
        )
    {
        return -1;
    }
    fix_system_writer_private_owner_for_mutation(hub, &path_for_decision);
    let result = call_original(pathname);
    let current_errno = runtime::current_errno();
    monitor::record_path_operation_result(
        hub,
        kind,
        op_name,
        path_for_decision.as_ref(),
        result,
        if result < 0 { current_errno } else { 0 },
        extra_tail.as_deref(),
    );
    runtime::set_errno(current_errno);
    result
}

fn handle_fd_path_audit<F>(
    hub: &InterceptHub,
    kind: OpKind,
    op_name: &str,
    fd: c_int,
    extra_tail: Option<String>,
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
            kind,
            op_name,
            &path_for_decision,
            extra_tail.as_deref(),
        )
    {
        return -1;
    }
    fix_system_writer_private_owner_for_mutation(hub, &path_for_decision);
    let result = call_original();
    let current_errno = runtime::current_errno();
    monitor::record_path_operation_result(
        hub,
        kind,
        op_name,
        &path_for_decision,
        result,
        if result < 0 { current_errno } else { 0 },
        extra_tail.as_deref(),
    );
    runtime::set_errno(current_errno);
    result
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

    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
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

    let result = unsafe { libc::ftruncate(retry_fd, length) };
    let retry_errno = runtime::current_errno();
    let backend_size = backend_fd_size(retry_fd);
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

    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
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

    let result = unsafe { libc::futimens(retry_fd, times) };
    let retry_errno = runtime::current_errno();
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

fn is_permission_errno(error_no: i32) -> bool {
    error_no == libc::EPERM || error_no == libc::EACCES
}

fn resolve_private_owner_sqlite_backend(
    hub: &InterceptHub,
    path_for_decision: &str,
) -> Option<(String, String, i32)> {
    resolve_private_owner_sqlite_backend_for_package(
        &hub.get_package_name(),
        path_for_decision,
        hub.get_current_caller_uid(),
        &hub.get_current_caller_package(),
    )
}

fn resolve_private_owner_sqlite_backend_for_package(
    package_name: &str,
    path_for_decision: &str,
    caller_uid: i32,
    caller_package: &str,
) -> Option<(String, String, i32)> {
    if !should_fix_system_writer_private_owner_for_package(package_name) {
        return None;
    }

    let storage_path = paths::normalize(path_for_decision);
    let effective_caller_uid = if media_fuse::should_allow_private_owner_sqlite_access_for_caller(
        &storage_path,
        caller_uid,
        caller_package,
    ) {
        caller_uid
    } else if let Some(owner_uid) =
        media_fuse::should_allow_private_owner_sqlite_owner_backend(&storage_path)
    {
        owner_uid
    } else if let Some(recent_caller_uid) =
        media_fuse::has_recent_private_owner_sqlite_access(&storage_path)
    {
        recent_caller_uid
    } else {
        return None;
    };

    let backend_path = writer::storage_to_data_media_path(&storage_path);
    if backend_path == storage_path || !backend_path.starts_with("/data/media/") {
        return None;
    }

    Some((storage_path, backend_path, effective_caller_uid))
}

fn backend_fd_size(fd: c_int) -> i64 {
    let mut statbuf = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstat(fd, statbuf.as_mut_ptr()) };
    if result != 0 {
        return -1;
    }
    let statbuf = unsafe { statbuf.assume_init() };
    statbuf.st_size as i64
}

fn fix_system_writer_private_owner_for_mutation(hub: &InterceptHub, path: &str) {
    if !should_fix_system_writer_private_owner_for_package(&hub.get_package_name()) {
        return;
    }
    runtime::fix_system_writer_android_private_owner(path, true);
}

fn handle_link_audit<F>(
    hub: &InterceptHub,
    op_name: &str,
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: i32,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char, *const c_char) -> c_int,
{
    if oldpath.is_null() || newpath.is_null() {
        return call_original(oldpath, newpath);
    }

    let PreparedPath::Ready {
        path_for_decision, ..
    } = (unsafe { prepare_relevant_path(hub, op_name, newdirfd, newpath, flags, true) })
    else {
        return call_original(oldpath, newpath);
    };

    diagnostic::log_diag_path_event(hub, op_name, "input-new", path_for_decision.as_ref(), flags);
    let old_text = unsafe { c_str_to_string(oldpath) };
    let from_path = resolve_extra_path(olddirfd, &old_text);
    let extra_tail = if flags >= 0 {
        if from_path.is_empty() {
            Some(format!("flags=0x{:x}", flags))
        } else {
            Some(format!("flags=0x{:x}|from={}", flags, from_path))
        }
    } else if from_path.is_empty() {
        None
    } else {
        Some(format!("from={}", from_path))
    };
    if should_apply_mutation_policy(hub)
        && deny_read_only_single_path_if_needed(
            hub,
            OpKind::Link,
            op_name,
            path_for_decision.as_ref(),
            extra_tail.as_deref(),
        )
    {
        return -1;
    }
    let result = call_original(oldpath, newpath);
    let current_errno = runtime::current_errno();
    monitor::record_path_operation_result(
        hub,
        OpKind::Link,
        op_name,
        path_for_decision.as_ref(),
        result,
        if result < 0 { current_errno } else { 0 },
        extra_tail.as_deref(),
    );
    runtime::set_errno(current_errno);
    result
}

fn resolve_extra_path(dirfd: c_int, path_text: &str) -> String {
    if path_text.is_empty() {
        return String::new();
    }
    let resolved = path_utils::resolve_path_for_dirfd(dirfd, path_text);
    if resolved.is_empty() {
        path_text.to_string()
    } else {
        resolved
    }
}

fn deny_read_only_mkdir(
    hub: &InterceptHub,
    op_name: &str,
    path: &str,
    read_only_path: &str,
) -> c_int {
    runtime::set_read_only_errno();
    monitor::record_read_only_mkdir_result(hub, op_name, path, read_only_path);
    log::debug!(
        "readonly deny op={} path={} read_only_path={}",
        op_name,
        path,
        read_only_path
    );
    -1
}

fn deny_read_only_unlink(
    hub: &InterceptHub,
    op_name: &str,
    path: &str,
    flags: i32,
    read_only_path: &str,
) -> c_int {
    runtime::set_read_only_errno();
    monitor::record_read_only_unlink_result(hub, op_name, path, flags, read_only_path);
    log::debug!(
        "readonly deny op={} path={} read_only_path={} flags=0x{:x}",
        op_name,
        path,
        read_only_path,
        flags
    );
    -1
}

fn deny_read_only_single_path_if_needed(
    hub: &InterceptHub,
    kind: OpKind,
    op_name: &str,
    path: &str,
    extra_tail: Option<&str>,
) -> bool {
    let redirect_result = process_redirect_path_for_mutation(hub, path);
    diagnostic::log_diag_redirect_decision(hub, op_name, path, &redirect_result);
    if !redirect_result.is_denied() {
        return false;
    }
    runtime::set_read_only_errno();
    let read_only_tail = read_only_extra_tail(path, &redirect_result.new_path, extra_tail);
    monitor::record_read_only_path_operation_result(
        hub,
        kind,
        op_name,
        path,
        read_only_tail.as_deref(),
    );
    log::debug!(
        "readonly deny op={} path={} read_only_path={}",
        op_name,
        path,
        redirect_result.new_path
    );
    true
}

fn read_only_extra_tail(
    path: &str,
    read_only_path: &str,
    extra_tail: Option<&str>,
) -> Option<String> {
    let mut tail = String::new();
    if let Some(extra_tail) = extra_tail
        && !extra_tail.is_empty()
    {
        tail.push_str(extra_tail);
    }
    if !read_only_path.is_empty() && read_only_path != path {
        if !tail.is_empty() {
            tail.push('|');
        }
        tail.push_str("read_only_path=");
        tail.push_str(read_only_path);
    }
    if tail.is_empty() { None } else { Some(tail) }
}

fn process_redirect_path_for_mutation(hub: &InterceptHub, path: &str) -> RedirectDecision {
    process_write_redirect_path(hub, path)
}

fn should_apply_mutation_policy(hub: &InterceptHub) -> bool {
    should_apply_mutation_policy_for_mode(hub.is_monitor_only(), &hub.get_package_name())
}

fn should_enforce_monitor_only_writer_policy(hub: &InterceptHub) -> bool {
    should_enforce_monitor_only_writer_policy_for_package(&hub.get_package_name())
}

fn should_apply_mutation_policy_for_mode(is_monitor_only: bool, package_name: &str) -> bool {
    !is_monitor_only || should_enforce_monitor_only_writer_policy_for_package(package_name)
}

fn should_enforce_monitor_only_writer_policy_for_package(package_name: &str) -> bool {
    policy::is_system_writer_package(package_name)
}

fn should_fix_system_writer_private_owner_for_package(package_name: &str) -> bool {
    policy::is_system_writer_package(package_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, UserProfile};
    use crate::domain::PathMapping;
    use std::collections::HashMap;

    #[test]
    fn monitor_only_writer_policy_is_limited_to_system_writers() {
        assert!(should_enforce_monitor_only_writer_policy_for_package(
            "com.android.providers.media.module"
        ));
        assert!(!should_enforce_monitor_only_writer_policy_for_package(
            "com.android.providers.downloads"
        ));
        assert!(!should_enforce_monitor_only_writer_policy_for_package(
            "com.tencent.mobileqq"
        ));
    }

    #[test]
    fn monitor_only_mutation_policy_still_applies_to_media_provider() {
        assert!(should_apply_mutation_policy_for_mode(
            true,
            "com.android.providers.media.module"
        ));
        assert!(!should_apply_mutation_policy_for_mode(
            true,
            "com.android.providers.downloads"
        ));
        assert!(!should_apply_mutation_policy_for_mode(
            true,
            "com.tencent.mobileqq"
        ));
        assert!(should_apply_mutation_policy_for_mode(
            false,
            "com.tencent.mobileqq"
        ));
    }

    #[test]
    fn system_writer_private_owner_fix_includes_monitor_only_media_provider_only() {
        assert!(should_fix_system_writer_private_owner_for_package(
            "com.android.providers.media.module"
        ));
        assert!(should_fix_system_writer_private_owner_for_package(
            "com.google.android.providers.media.module"
        ));
        assert!(!should_fix_system_writer_private_owner_for_package(
            "com.android.providers.downloads"
        ));
        assert!(!should_fix_system_writer_private_owner_for_package(
            "com.android.externalstorage"
        ));
    }

    #[test]
    fn private_owner_sqlite_backend_uses_recent_fuse_access_when_thread_caller_cleared() {
        let config = SettingsHub::instance();
        let (previous_apps, previous_loaded) = config.replace_test_apps(HashMap::new());
        let previous_monitor = config.replace_test_file_monitor_enabled(false);
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.eg.android.AlipayGphone".to_string(), 10274),
            ("com.leo.xposed.xradiant".to_string(), 10164),
        ]));

        let path = "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm";
        assert!(media_fuse::should_allow_private_owner_sqlite_access(
            path, 10164
        ));
        let backend = resolve_private_owner_sqlite_backend_for_package(
            "com.android.providers.media.module",
            path,
            -1,
            "",
        );

        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        config.restore_test_apps(previous_apps, previous_loaded);

        let (storage_path, backend_path, caller_uid) =
            backend.expect("recent FUSE access should authorize sqlite backend retry");
        assert_eq!(storage_path, path);
        assert_eq!(
            backend_path,
            "/data/media/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm"
        );
        assert_eq!(caller_uid, 10164);
    }

    #[test]
    fn private_owner_sqlite_backend_allows_enabled_owner_without_thread_caller() {
        let config = SettingsHub::instance();
        let (previous_apps, previous_loaded) = config.replace_test_apps(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            10274,
        )]));

        let path = "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm";
        let backend = resolve_private_owner_sqlite_backend_for_package(
            "com.android.providers.media.module",
            path,
            -1,
            "",
        );

        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        let (storage_path, backend_path, caller_uid) =
            backend.expect("enabled owner should authorize sqlite backend retry");
        assert_eq!(storage_path, path);
        assert_eq!(
            backend_path,
            "/data/media/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm"
        );
        assert_eq!(caller_uid, 10274);
    }

    #[test]
    fn media_provider_mkdir_redirects_mapping_request_root_before_original_call() {
        let config = SettingsHub::instance();
        let caller_package = "xyz.nextalone.nnngram";
        let caller_uid = 10312;
        let (previous_apps, previous_loaded) = config.replace_test_apps(HashMap::from([(
            caller_package.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Nnngram".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/Nnngram".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        let decision = resolve_mapping_request_mkdir_redirect(
            "com.android.providers.media.module",
            "/storage/emulated/0/Download/Nnngram",
        );

        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        let decision = decision.expect("mapping request mkdir should redirect");
        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/Nnngram"
        );
    }

    #[test]
    fn anonymous_writer_virtual_mkdir_only_matches_own_default_sandbox() {
        assert!(is_own_default_sandbox_redirect(
            "com.android.providers.media.module",
            "/storage/emulated/0/.shared-cache",
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.shared-cache",
        ));
        assert!(is_own_default_sandbox_redirect(
            "com.android.providers.media.module",
            "/storage/emulated/0/.shared-cache",
            "/storage/emulated/0/Android/data/com.android.providers.media.module/sdcard/.shared-cache",
        ));
        assert!(!is_own_default_sandbox_redirect(
            "com.android.providers.media.module",
            "/storage/emulated/0/.shared-cache",
            "/data/media/0/Android/data/com.example.app/sdcard/.shared-cache",
        ));
        assert!(!is_own_default_sandbox_redirect(
            "com.android.providers.media.module",
            "/data/local/tmp/.shared-cache",
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.shared-cache",
        ));
    }

    #[test]
    fn cleanup_source_dir_matches_default_sandbox_redirects_without_name_special_case() {
        assert!(is_public_default_sandbox_redirect(
            "/storage/emulated/0/.xlDownload",
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.xlDownload",
        ));
        assert!(is_public_default_sandbox_redirect(
            "/data/media/0/DCIM/.android/cache",
            "/data/media/0/Android/data/com.example.camera/sdcard/DCIM/.android/cache",
        ));
    }

    #[test]
    fn cleanup_source_dir_rejects_mappings_and_android_private_sources() {
        assert!(!is_public_default_sandbox_redirect(
            "/storage/emulated/0/Download/QQ",
            "/storage/emulated/0/Download/ThirdParty/QQ",
        ));
        assert!(!is_public_default_sandbox_redirect(
            "/storage/emulated/0/Android/data/com.example/files",
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/Android/data/com.example/files",
        ));
        assert!(!is_public_default_sandbox_redirect(
            "/storage/emulated/0/.xlDownload",
            "/data/media/0/Android/data/com.android.providers.media.module/files/.xlDownload",
        ));
    }
}
