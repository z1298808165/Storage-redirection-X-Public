use super::common::*;
use crate::config::SettingsHub;
use crate::hook::diagnostic;
use crate::hook::monitor;
use crate::hook::ops::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::hook::runtime;
use crate::hook::stats::InterceptHub;
use crate::monitor::OpKind;
use crate::platform::{self, paths};
use crate::redirect::{RedirectAction, RedirectDecision, policy, record_redirect_hit, writer};
use libc::{AT_FDCWD, c_char, c_int, c_void, mode_t};
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
                crate::hook::caller::update_caller_package_for_current_thread(hub);
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
                crate::hook::caller::update_caller_package_for_current_thread(hub);
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
                crate::hook::caller::update_caller_package_for_current_thread(hub);
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
            // SAFETY: 直接触发 unlinkat 系统调用；参数由调用方保证有效，call_path 指向合法 C 字符串或为空，flags 为原始调用透传值。
            || unsafe { libc::syscall(libc::SYS_unlinkat, dirfd, call_path, flags) as c_int },
            |prev| {
                // SAFETY: prev 是 hook 框架返回的原始 unlinkat 函数指针，签名与目标函数一致，transmute 到匹配的函数类型是安全的。
                let f: unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int =
                    unsafe { std::mem::transmute(prev) };
                // SAFETY: f 为上一步得到的原始 unlinkat 实现，参数直接透传原始调用值，指针有效性由调用方保证。
                unsafe { f(dirfd, call_path, flags) }
            },
        )
    };

    runtime::with_hook_guard(
        || call_original(pathname),
        |hub| {
            hub.increment_unlink_calls();
            if runtime::should_resolve_caller_context(hub) {
                crate::hook::caller::update_caller_package_for_current_thread(hub);
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
                crate::hook::caller::update_caller_package_for_current_thread(hub);
            }

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Rmdir,
                    op_name: "rmdir",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: None,
                },
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

    // SAFETY: pathname 已在上方判空，prepare_relevant_path 会按 NUL 结尾读取该 C 字符串并解析路径，指针有效性由 hook 调用方保证。
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
    // SAFETY: c_path 由上方 CString::new 构造，保证以 NUL 结尾且在本次调用期间有效，rmdir 只读取该路径字符串。
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

    // SAFETY: pathname 已在上方判空，prepare_relevant_path 会按 NUL 结尾读取该 C 字符串并解析路径，指针有效性由 hook 调用方保证。
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
