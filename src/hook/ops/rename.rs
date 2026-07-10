use super::super::{
    caller, diagnostic, monitor, path as path_utils, runtime, util::c_str_to_string,
};
use crate::redirect::{RedirectAction, RedirectDecision, policy, process_write_redirect_path};
use libc::{AT_FDCWD, O_CREAT, O_WRONLY, c_char, c_int, c_void};
use std::borrow::Cow;
use std::ffi::CString;

pub unsafe extern "C" fn hooked_rename(oldpath: *const c_char, newpath: *const c_char) -> c_int {
    let self_ptr = hooked_rename as *mut c_void;
    let call_rename = |old: *const c_char, new: *const c_char| -> c_int {
        runtime::call_prev(
            self_ptr,
            || libc::rename(old, new),
            |prev| {
                let f: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
                    unsafe { std::mem::transmute(prev) };
                unsafe { f(old, new) }
            },
        )
    };

    runtime::with_hook_guard(
        || call_rename(oldpath, newpath),
        |hub| {
            hub.increment_rename_calls();
            if runtime::should_resolve_caller_context(hub) {
                caller::update_caller_package_for_current_thread(hub);
            }

            handle_rename_like(
                hub,
                "rename",
                RenameLikeRequest {
                    olddirfd: AT_FDCWD,
                    oldpath,
                    newdirfd: AT_FDCWD,
                    newpath,
                    flags: -1,
                },
                call_rename,
            )
        },
    )
}

pub unsafe extern "C" fn hooked_renameat(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
) -> c_int {
    let self_ptr = hooked_renameat as *mut c_void;
    let call_original = |call_old: *const c_char, call_new: *const c_char| -> c_int {
        runtime::call_prev(
            self_ptr,
            || unsafe { libc::renameat(olddirfd, call_old, newdirfd, call_new) },
            |prev| {
                let f: unsafe extern "C" fn(c_int, *const c_char, c_int, *const c_char) -> c_int =
                    unsafe { std::mem::transmute(prev) };
                unsafe { f(olddirfd, call_old, newdirfd, call_new) }
            },
        )
    };

    runtime::with_hook_guard(
        || call_original(oldpath, newpath),
        |hub| {
            hub.increment_rename_calls();
            if runtime::should_resolve_caller_context(hub) {
                caller::update_caller_package_for_current_thread(hub);
            }

            handle_rename_like(
                hub,
                "renameat",
                RenameLikeRequest {
                    olddirfd,
                    oldpath,
                    newdirfd,
                    newpath,
                    flags: -1,
                },
                call_original,
            )
        },
    )
}

pub unsafe extern "C" fn hooked_renameat2(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: u32,
) -> c_int {
    let self_ptr = hooked_renameat2 as *mut c_void;
    let call_original = |call_old: *const c_char, call_new: *const c_char| -> c_int {
        runtime::call_prev(
            self_ptr,
            || unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    olddirfd,
                    call_old,
                    newdirfd,
                    call_new,
                    flags,
                ) as c_int
            },
            |prev| {
                let f: unsafe extern "C" fn(
                    c_int,
                    *const c_char,
                    c_int,
                    *const c_char,
                    u32,
                ) -> c_int = unsafe { std::mem::transmute(prev) };
                unsafe { f(olddirfd, call_old, newdirfd, call_new, flags) }
            },
        )
    };

    runtime::with_hook_guard(
        || call_original(oldpath, newpath),
        |hub| {
            hub.increment_rename_calls();
            if runtime::should_resolve_caller_context(hub) {
                caller::update_caller_package_for_current_thread(hub);
            }

            handle_rename_like(
                hub,
                "renameat2",
                RenameLikeRequest {
                    olddirfd,
                    oldpath,
                    newdirfd,
                    newpath,
                    flags: flags as i32,
                },
                call_original,
            )
        },
    )
}

struct RenameLikeRequest {
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: i32,
}

fn handle_rename_like<F>(
    hub: &super::super::stats::InterceptHub,
    op_name: &str,
    request: RenameLikeRequest,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char, *const c_char) -> c_int,
{
    if request.oldpath.is_null() || request.newpath.is_null() {
        return call_original(request.oldpath, request.newpath);
    }

    let old_text = unsafe { c_str_to_string(request.oldpath) };
    let new_text = unsafe { c_str_to_string(request.newpath) };
    let Some(old_resolved) = resolve_rename_path(
        hub,
        op_name,
        "old",
        request.olddirfd,
        &old_text,
        request.flags,
    ) else {
        return call_original(request.oldpath, request.newpath);
    };
    let Some(new_resolved) = resolve_rename_path(
        hub,
        op_name,
        "new",
        request.newdirfd,
        &new_text,
        request.flags,
    ) else {
        return call_original(request.oldpath, request.newpath);
    };

    let is_old_relative = !old_text.starts_with('/');
    let is_new_relative = !new_text.starts_with('/');
    let is_old_storage = path_utils::is_relevant_storage_path(hub, old_resolved.as_ref());
    let is_new_storage = path_utils::is_relevant_storage_path(hub, new_resolved.as_ref());
    if !is_old_storage && !is_new_storage {
        diagnostic::record_fast_bypass(op_name, old_resolved.as_ref());
        return call_original(request.oldpath, request.newpath);
    }

    diagnostic::log_diag_path_event(
        hub,
        op_name,
        "input-old",
        old_resolved.as_ref(),
        request.flags,
    );
    diagnostic::log_diag_path_event(
        hub,
        op_name,
        "input-new",
        new_resolved.as_ref(),
        request.flags,
    );

    let should_apply_policy = should_apply_rename_policy(hub);
    let mut old_redirect = RedirectDecision {
        action: RedirectAction::Allow,
        new_path: String::new(),
        is_mapping: false,
    };
    let mut new_redirect = RedirectDecision {
        action: RedirectAction::Allow,
        new_path: String::new(),
        is_mapping: false,
    };
    if should_apply_policy {
        if is_old_storage {
            old_redirect = process_redirect_path_for_rename(hub, old_resolved.as_ref());
        }
        if is_new_storage {
            new_redirect = process_redirect_path_for_rename(hub, new_resolved.as_ref());
        }
    }

    if old_redirect.is_denied() || new_redirect.is_denied() {
        let read_only_path = if old_redirect.is_denied() {
            old_redirect.new_path.as_str()
        } else {
            new_redirect.new_path.as_str()
        };
        return deny_read_only_rename(
            hub,
            op_name,
            new_resolved.as_ref(),
            old_resolved.as_ref(),
            request.flags,
            read_only_path,
        );
    }

    let final_old = if old_redirect.is_redirect() {
        old_redirect.new_path.as_str()
    } else {
        old_resolved.as_ref()
    };
    let final_new = if new_redirect.is_redirect() {
        new_redirect.new_path.as_str()
    } else {
        new_resolved.as_ref()
    };

    diagnostic::log_diag_rename_decision(
        hub,
        old_resolved.as_ref(),
        new_resolved.as_ref(),
        final_old,
        final_new,
    );
    if should_apply_policy
        && (final_old != old_resolved.as_ref() || final_new != new_resolved.as_ref())
    {
        log::trace!(
            "{}: {} -> {} (redirect {} -> {})",
            op_name,
            old_resolved.as_ref(),
            new_resolved.as_ref(),
            final_old,
            final_new
        );
        hub.increment_total_redirected();
        hub.increment_global_redirect_count();
    }

    runtime::ensure_redirect_parent_directory(
        op_name,
        new_resolved.as_ref(),
        final_new,
        O_WRONLY | O_CREAT,
    );

    let should_rewrite_old = old_redirect.is_redirect() || is_old_relative;
    let should_rewrite_new = new_redirect.is_redirect() || is_new_relative;
    let c_old = if should_rewrite_old {
        match CString::new(final_old) {
            Ok(path) => Some(path),
            Err(_) => return call_original(request.oldpath, request.newpath),
        }
    } else {
        None
    };
    let c_new = if should_rewrite_new {
        match CString::new(final_new) {
            Ok(path) => Some(path),
            Err(_) => return call_original(request.oldpath, request.newpath),
        }
    } else {
        None
    };
    let call_old = c_old.as_ref().map_or(request.oldpath, |path| path.as_ptr());
    let call_new = c_new.as_ref().map_or(request.newpath, |path| path.as_ptr());
    let result = call_original(call_old, call_new);
    let error_no = runtime::errno_for_result(result);
    if final_old != old_resolved.as_ref() || final_new != new_resolved.as_ref() {
        monitor::record_rename_result_with_display_paths(
            hub,
            op_name,
            final_new,
            final_old,
            new_resolved.as_ref(),
            old_resolved.as_ref(),
            result,
            error_no,
            request.flags,
        );
    } else {
        monitor::record_rename_result(
            hub,
            op_name,
            new_resolved.as_ref(),
            old_resolved.as_ref(),
            result,
            error_no,
            request.flags,
        );
    }
    result
}

fn resolve_rename_path<'a>(
    hub: &super::super::stats::InterceptHub,
    op_name: &str,
    side: &str,
    dirfd: c_int,
    path_text: &'a str,
    flags: i32,
) -> Option<Cow<'a, str>> {
    if path_text.is_empty() {
        return None;
    }

    if path_text.starts_with('/') {
        return Some(Cow::Borrowed(path_text));
    }

    diagnostic::log_relative_path_bypass(
        hub,
        &format!("{}-{}", op_name, side),
        dirfd,
        path_text,
        flags,
    );
    let resolved = path_utils::resolve_path_for_dirfd(dirfd, path_text);
    if resolved.is_empty() {
        None
    } else {
        Some(Cow::Owned(resolved))
    }
}

fn deny_read_only_rename(
    hub: &super::super::stats::InterceptHub,
    op_name: &str,
    new_path: &str,
    old_path: &str,
    flags: i32,
    read_only_path: &str,
) -> c_int {
    runtime::set_read_only_errno();
    monitor::record_read_only_rename_result(
        hub,
        op_name,
        new_path,
        old_path,
        flags,
        read_only_path,
    );
    log::debug!(
        "readonly deny op={} old={} new={} read_only_path={} flags=0x{:x}",
        op_name,
        old_path,
        new_path,
        read_only_path,
        flags
    );
    -1
}

fn process_redirect_path_for_rename(
    hub: &crate::hook::stats::InterceptHub,
    path: &str,
) -> RedirectDecision {
    process_write_redirect_path(hub, path)
}

fn should_apply_rename_policy(hub: &crate::hook::stats::InterceptHub) -> bool {
    should_apply_rename_policy_for_mode(hub.is_monitor_only(), &hub.get_package_name())
}

fn should_apply_rename_policy_for_mode(is_monitor_only: bool, package_name: &str) -> bool {
    !is_monitor_only || policy::is_system_writer_package(package_name)
}
