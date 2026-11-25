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

#[cfg(test)]
#[path = "../../../../tests/native-unit/redirected_mkdir.rs"]
mod tests;

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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_unlink_like(hub, "unlink", AT_FDCWD, pathname, -1, |call_path| {
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_unlink_like(hub, "unlinkat", dirfd, pathname, flags, call_original)
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
            crate::hook::caller::update_caller_for_mutation(hub, pathname);

            handle_single_path_audit(
                hub,
                SinglePathAuditRequest {
                    kind: OpKind::Rmdir,
                    op_name: "rmdir",
                    dirfd: AT_FDCWD,
                    pathname,
                    log_flags: 0,
                    extra_tail: None,
                    redirect_mutation: true,
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

    // 入口留痕：只在诊断进程（MediaProvider / 代写者等系统链路）上开，用来回答
    // 「这次 mkdir 到底有没有进到本模块」。没有这一层，hook 未被调用与 hook 判成
    // 放行在日志上完全同形——真机 2026-11-05 排查 sau 沙盒时正是卡在这里。
    let is_diagnostic_caller = {
        let pkg = hub.get_package_name();
        crate::redirect::policy::is_media_provider_package(&pkg)
            || crate::redirect::policy::is_system_writer_package(&pkg)
    };
    if is_diagnostic_caller {
        // SAFETY: pathname 已判空；c_str_to_string 按 NUL 结尾读取，指针有效期由
        // hook 调用方保证。
        let raw_path = unsafe { crate::hook::util::c_str_to_string(pathname) };
        // SAFETY: getuid 无参数、无副作用，只读取当前进程的实际 uid。
        let self_uid = unsafe { libc::getuid() };
        log::debug!(
            "{} entry pkg={} dirfd={} self_uid={} path={}",
            op_name,
            hub.get_package_name(),
            dirfd,
            self_uid,
            if raw_path.is_empty() {
                "-"
            } else {
                raw_path.as_str()
            }
        );
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

    if hub.effective_monitor_only() && !should_enforce_monitor_only_writer_policy(hub) {
        let result = call_original(pathname);
        let error_no = runtime::errno_for_result(result);
        monitor::record_mkdir_result(hub, op_name, path_for_decision.as_ref(), result, error_no);
        return result;
    }

    let mut redirect_result = process_redirect_path_for_dir_create(hub, path_for_decision.as_ref());
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
    if is_diagnostic_caller {
        // 决策结果无条件留痕：入口有日志、决策没有，就说明这次直接走了「放行到真实
        // 存储」。两者必须成对出现才能判定是「没进 hook」还是「进了但判成放行」。
        log::debug!(
            "{} decision pkg={} redirect={} denied={} mapping={} to={}",
            op_name,
            hub.get_package_name(),
            redirect_result.is_redirect(),
            redirect_result.is_denied(),
            redirect_result.is_mapping,
            if redirect_result.new_path.is_empty() {
                "-"
            } else {
                redirect_result.new_path.as_str()
            }
        );
    }
    if redirect_result.is_denied() {
        return deny_read_only_mkdir(
            hub,
            op_name,
            path_for_decision.as_ref(),
            &redirect_result.new_path,
        );
    }

    if should_passthrough_provider_allowed_parent_mkdir(
        hub,
        path_for_decision.as_ref(),
        &redirect_result,
    ) {
        log::info!(
            "{} provider allowed parent passthrough path={}",
            op_name,
            path_for_decision
        );
        let result = call_original(pathname);
        let error_no = runtime::errno_for_result(result);
        monitor::record_mkdir_result(hub, op_name, path_for_decision.as_ref(), result, error_no);
        return result;
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

    if redirect_result.is_redirect() {
        let source = path_for_decision.as_ref();
        record_redirect_hit(hub, op_name, source, &redirect_result.new_path);
        runtime::ensure_redirect_parent_dirs(&redirect_result.new_path, 0o2773);
        let (result, error_no) =
            mkdir_redirect_target(hub, source, &redirect_result, call_original);
        monitor::record_mkdir_result_from(
            hub,
            op_name,
            &redirect_result.new_path,
            source,
            result,
            error_no,
        );
        if result < 0 {
            runtime::set_errno(error_no);
        }
        return result;
    }

    let result = call_original(pathname);
    let error_no = runtime::errno_for_result(result);
    if result == 0
        && hub.with_package_name(policy::is_system_writer_package)
        && !hub.is_monitor_only()
    {
        runtime::fix_system_writer_android_private_owner(path_for_decision.as_ref(), true, -1);
    }
    monitor::record_mkdir_result(hub, op_name, path_for_decision.as_ref(), result, error_no);
    if result < 0 {
        runtime::set_errno(error_no);
    }
    result
}

/// 只在目标目录可用时登记查询别名；登记不是公共目录的清理凭据。
fn mkdir_redirect_target<F>(
    hub: &InterceptHub,
    source: &str,
    redirect: &RedirectDecision,
    call_original: F,
) -> (c_int, c_int)
where
    F: FnOnce(*const c_char) -> c_int,
{
    let target = redirect.new_path.as_str();
    let backend = if redirect.is_mapping && !storage_backend_dir(source).is_empty() {
        storage_backend_dir(target)
    } else {
        String::new()
    };
    let (result, error_no) = if backend.is_empty() {
        let Ok(path) = CString::new(target) else {
            return (-1, libc::EINVAL);
        };
        let result = call_original(path.as_ptr());
        (result, runtime::errno_for_result(result))
    } else {
        create_backend_dir(&backend)
    };
    let attempted = if backend.is_empty() { target } else { &backend };
    if result != 0
        && !(matches!(error_no, libc::EEXIST | libc::EACCES)
            && platform::fs::is_directory(attempted))
    {
        return (result, error_no);
    }
    if redirect.is_mapping {
        crate::hook::remember_process_mapped_dir(source, target);
    } else {
        if result == 0
            && hub.with_package_name(policy::is_system_writer_package)
            && !hub.is_monitor_only()
        {
            runtime::fix_system_writer_android_private_owner(target, true, -1);
        }
        crate::hook::remember_provider_passthrough_virtual_dir(source, target);
    }
    runtime::normalize_redirect_directory(target);
    (0, 0)
}

/// 把 storage 视图路径换算成后端落盘路径；非 storage 形态返回空串。
fn storage_backend_dir(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let normalized = paths::normalize(path);
    if normalized.starts_with("/storage/emulated/") {
        return paths::storage_to_data_media_path(&normalized);
    }
    if normalized.starts_with("/data/media/") {
        return normalized;
    }
    String::new()
}

/// 在后端路径上直接建目录（已存在视为成功）。
///
/// 调用点位于 hook 重入保护内，`libc::mkdir` 会落到原始实现，不再次进入重定向，
/// 因此不会把这次的请求路径又改写到映射目标上。
fn create_backend_dir(path: &str) -> (c_int, c_int) {
    if path.is_empty() {
        return (-1, libc::ENOENT);
    }
    if crate::platform::fs::is_directory(path) {
        return (0, 0);
    }
    let Ok(c_path) = CString::new(path) else {
        return (-1, libc::EINVAL);
    };
    // SAFETY: c_path 由上一行 CString::new 构造，保证以 NUL 结尾且在本次调用期间有效；
    // mkdir 只读取该路径字符串，不保留指针。
    let result = unsafe { libc::mkdir(c_path.as_ptr(), 0o2773) };
    if result == 0 {
        (0, 0)
    } else {
        (result, runtime::current_errno())
    }
}

fn should_passthrough_provider_allowed_parent_mkdir(
    hub: &InterceptHub,
    source_path: &str,
    redirect_result: &RedirectDecision,
) -> bool {
    if !redirect_result.is_redirect()
        || redirect_result.is_mapping
        // 系统代写者的 monitor-only 预装模式仍执行实时 mkdir 策略，不能禁用祖先放行。
        // 主入口已处理无需执行代写策略的纯监视请求，此处按实际目录决策判断。
        // 正常 MediaStore insert 使用 virtual 作用域，也必须创建放行目录的真实祖先。
        // 否则祖先 mkdir 被默认重定向到沙箱，成功返回后公共父目录仍不存在。
        || !(crate::hook::is_provider_passthrough_active()
            || crate::hook::is_provider_virtual_scope_active())
        || !hub.with_package_name(policy::is_system_writer_package)
    {
        return false;
    }

    let caller_package = hub.get_current_caller_package();
    let caller_uid = hub.get_current_caller_uid();
    !caller_package.is_empty()
        && writer::is_path_parent_of_caller_allowed_real_path(
            source_path,
            &caller_package,
            caller_uid,
        )
}

pub(crate) fn remember_provider_redirect_source_directory(source_path: &str, target_path: &str) {
    if !is_public_default_sandbox_redirect(source_path, target_path) {
        return;
    }

    crate::hook::remember_provider_passthrough_virtual_dir(source_path, target_path);
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
    let mut caller_uid = policy::get_fresh_uid_for_package_in_user(&caller_package, user_id);
    if caller_uid < writer::ANDROID_APP_UID_START {
        caller_uid = user_id
            .saturating_mul(platform::ANDROID_USER_ID_OFFSET)
            .saturating_add(writer::ANDROID_APP_UID_START);
    }
    let mapped_path = writer::map_caller_path(&resolved_path, &caller_package, caller_uid);
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
        || !hub.with_package_name(policy::is_system_writer_package)
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
        // 审计会执行系统调用，必须恢复原函数调用后的 errno。
        runtime::set_errno(current_errno);
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
    // 审计与日志会执行系统调用，必须恢复原函数调用后的 errno。
    runtime::set_errno(current_errno);
    result
}
