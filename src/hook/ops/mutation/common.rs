use crate::hook::diagnostic;
use crate::hook::media_fuse;
use crate::hook::monitor;
use crate::hook::ops::path_prepare::{PreparedPath, prepare_relevant_path};
use crate::hook::path as path_utils;
use crate::hook::runtime;
use crate::hook::stats::InterceptHub;
use crate::hook::util::c_str_to_string;
use crate::monitor::OpKind;
use crate::platform::paths;
use crate::redirect::{RedirectDecision, policy, process_write_redirect_path, writer};
use libc::{c_char, c_int};

pub(super) struct SinglePathAuditRequest<'a> {
    pub(super) kind: OpKind,
    pub(super) op_name: &'a str,
    pub(super) dirfd: c_int,
    pub(super) pathname: *const c_char,
    pub(super) log_flags: i32,
    pub(super) extra_tail: Option<String>,
}

pub(super) struct LinkAuditRequest<'a> {
    pub(super) op_name: &'a str,
    pub(super) olddirfd: c_int,
    pub(super) oldpath: *const c_char,
    pub(super) newdirfd: c_int,
    pub(super) newpath: *const c_char,
    pub(super) flags: i32,
}

pub(super) fn handle_single_path_audit<F>(
    hub: &InterceptHub,
    request: SinglePathAuditRequest<'_>,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char) -> c_int,
{
    if request.pathname.is_null() {
        return call_original(request.pathname);
    }

    let PreparedPath::Ready {
        path_for_decision, ..
    // SAFETY: pathname 已在上方确认非空，并在本次拦截调用期间保持有效。
    } = (unsafe {
        prepare_relevant_path(
            hub,
            request.op_name,
            request.dirfd,
            request.pathname,
            request.log_flags,
            true,
        )
    })
    else {
        return call_original(request.pathname);
    };

    diagnostic::log_diag_path_event(
        hub,
        request.op_name,
        "input",
        path_for_decision.as_ref(),
        request.log_flags,
    );
    if should_apply_mutation_policy(hub)
        && deny_read_only_single_path_if_needed(
            hub,
            request.kind,
            request.op_name,
            path_for_decision.as_ref(),
            request.extra_tail.as_deref(),
        )
    {
        return -1;
    }
    fix_system_writer_private_owner_for_mutation(hub, &path_for_decision);
    let result = call_original(request.pathname);
    let current_errno = runtime::current_errno();
    monitor::record_path_operation_result(
        hub,
        request.kind,
        request.op_name,
        path_for_decision.as_ref(),
        result,
        if result < 0 { current_errno } else { 0 },
        request.extra_tail.as_deref(),
    );
    runtime::set_errno(current_errno);
    result
}

pub(super) fn should_apply_mutation_policy(hub: &InterceptHub) -> bool {
    should_apply_mutation_policy_for_mode(hub.is_monitor_only(), &hub.get_package_name())
}

pub(super) fn should_apply_mutation_policy_for_mode(
    is_monitor_only: bool,
    package_name: &str,
) -> bool {
    !is_monitor_only || should_enforce_monitor_only_writer_policy_for_package(package_name)
}

pub(super) fn should_enforce_monitor_only_writer_policy(hub: &InterceptHub) -> bool {
    should_enforce_monitor_only_writer_policy_for_package(&hub.get_package_name())
}

pub(super) fn should_enforce_monitor_only_writer_policy_for_package(package_name: &str) -> bool {
    policy::is_system_writer_package(package_name)
}

pub(super) fn should_fix_system_writer_private_owner_for_package(package_name: &str) -> bool {
    policy::is_system_writer_package(package_name)
}

pub(super) fn is_permission_errno(error_no: i32) -> bool {
    error_no == libc::EPERM || error_no == libc::EACCES
}

pub(super) fn backend_fd_size(fd: c_int) -> i64 {
    let mut statbuf = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: statbuf 提供足够大小的未初始化 libc::stat 空间，fstat 会向该指针写入完整结构；fd 由调用方保证有效。
    let result = unsafe { libc::fstat(fd, statbuf.as_mut_ptr()) };
    if result != 0 {
        return -1;
    }
    // SAFETY: 上方 fstat 返回 0，说明内核已完整初始化 statbuf，此处 assume_init 读取的是有效数据。
    let statbuf = unsafe { statbuf.assume_init() };
    statbuf.st_size
}

pub(super) fn fix_system_writer_private_owner_for_mutation(hub: &InterceptHub, path: &str) {
    if !should_fix_system_writer_private_owner_for_package(&hub.get_package_name()) {
        return;
    }
    runtime::fix_system_writer_android_private_owner(path, true);
}

pub(super) fn handle_fd_path_audit<F>(
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

pub(super) fn resolve_private_owner_sqlite_backend(
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

pub(super) fn resolve_private_owner_sqlite_backend_for_package(
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
    } else {
        media_fuse::has_recent_private_owner_sqlite_access(&storage_path)?
    };
    let backend_path = writer::storage_to_data_media_path(&storage_path);
    if backend_path == storage_path || !backend_path.starts_with("/data/media/") {
        return None;
    }
    Some((storage_path, backend_path, effective_caller_uid))
}

pub(super) fn handle_link_audit<F>(
    hub: &InterceptHub,
    request: LinkAuditRequest<'_>,
    call_original: F,
) -> c_int
where
    F: FnOnce(*const c_char, *const c_char) -> c_int,
{
    if request.oldpath.is_null() || request.newpath.is_null() {
        return call_original(request.oldpath, request.newpath);
    }

    let PreparedPath::Ready {
        path_for_decision, ..
    // SAFETY: newpath 已在上方确认非空，并在本次拦截调用期间保持有效。
    } = (unsafe {
        prepare_relevant_path(
            hub,
            request.op_name,
            request.newdirfd,
            request.newpath,
            request.flags,
            true,
        )
    })
    else {
        return call_original(request.oldpath, request.newpath);
    };

    diagnostic::log_diag_path_event(
        hub,
        request.op_name,
        "input-new",
        path_for_decision.as_ref(),
        request.flags,
    );
    // SAFETY: oldpath 已在上方确认非空，并在本次拦截调用期间保持有效。
    let old_text = unsafe { c_str_to_string(request.oldpath) };
    let from_path = resolve_extra_path(request.olddirfd, &old_text);
    let extra_tail = if request.flags >= 0 {
        if from_path.is_empty() {
            Some(format!("flags=0x{:x}", request.flags))
        } else {
            Some(format!("flags=0x{:x}|from={}", request.flags, from_path))
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
            request.op_name,
            path_for_decision.as_ref(),
            extra_tail.as_deref(),
        )
    {
        return -1;
    }
    let result = call_original(request.oldpath, request.newpath);
    let current_errno = runtime::current_errno();
    monitor::record_path_operation_result(
        hub,
        OpKind::Link,
        request.op_name,
        path_for_decision.as_ref(),
        result,
        if result < 0 { current_errno } else { 0 },
        extra_tail.as_deref(),
    );
    runtime::set_errno(current_errno);
    result
}

pub(super) fn resolve_extra_path(dirfd: c_int, path_text: &str) -> String {
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

pub(super) fn deny_read_only_mkdir(
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

pub(super) fn deny_read_only_unlink(
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

pub(super) fn deny_read_only_single_path_if_needed(
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

pub(super) fn read_only_extra_tail(
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

pub(super) fn process_redirect_path_for_mutation(
    hub: &InterceptHub,
    path: &str,
) -> RedirectDecision {
    process_write_redirect_path(hub, path)
}
