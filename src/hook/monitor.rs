use super::stats::InterceptHub;
use crate::monitor::{AuditTrail, OpKind};
use crate::platform::{self, paths};
use crate::redirect::policy;
use libc::{AT_REMOVEDIR, O_APPEND, O_CREAT, O_PATH, O_RDWR, O_TMPFILE, O_TRUNC, O_WRONLY};

const ANDROID_APP_UID_START: i32 = 10_000;
const SHOULD_MONITOR_LOG_DIR_CREATE: bool = true;
const READ_ONLY_DENY_REASON: &str = "deny_reason=read_only_rule";

pub(super) struct OpenResultRecord<'a> {
    pub(super) op_name: &'a str,
    pub(super) flags: i32,
    pub(super) pathname: &'a str,
    pub(super) original_pathname: &'a str,
    pub(super) is_mapping: bool,
    pub(super) result: i32,
    pub(super) error_no: i32,
}

pub(super) struct RenameResultRecord<'a> {
    pub(super) op_name: &'a str,
    pub(super) new_pathname: &'a str,
    pub(super) old_pathname: &'a str,
    pub(super) result: i32,
    pub(super) error_no: i32,
    pub(super) flags: i32,
}

pub fn has_write_intent_flags(flags: i32) -> bool {
    if flags < 0 {
        return false;
    }
    if (flags & O_PATH) != 0 {
        return false;
    }

    let write_mask = O_WRONLY | O_RDWR | O_CREAT | O_TRUNC | O_APPEND;
    (flags & write_mask) != 0 || (flags & O_TMPFILE) == O_TMPFILE
}

fn has_create_intent_flags(flags: i32) -> bool {
    if flags < 0 {
        return false;
    }
    if (flags & O_PATH) != 0 {
        return false;
    }

    (flags & O_CREAT) != 0 || (flags & O_TMPFILE) == O_TMPFILE
}

pub fn record_open_result(hub: &InterceptHub, record: OpenResultRecord<'_>) {
    record_open_result_with_extra(hub, &record, None);
}

pub fn record_read_only_open_result(
    hub: &InterceptHub,
    op_name: &str,
    flags: i32,
    pathname: &str,
    original_pathname: &str,
    read_only_path: &str,
) {
    let extra_tail = read_only_extra_tail(pathname, read_only_path);
    let record = OpenResultRecord {
        op_name,
        flags,
        pathname,
        original_pathname,
        is_mapping: false,
        result: -1,
        error_no: libc::EROFS,
    };
    record_open_result_with_extra(hub, &record, Some(extra_tail.as_str()));
}

fn record_open_result_with_extra(
    hub: &InterceptHub,
    record: &OpenResultRecord<'_>,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }
    if should_skip_media_provider_pending_probe(
        &hub.get_package_name(),
        record.flags,
        record.pathname,
        record.original_pathname,
        record.is_mapping,
        record.result,
        record.error_no,
    ) {
        return;
    }

    if record.result >= 0
        && record_saf_bridge_provider_path(
            hub,
            record.pathname,
            saf_provider_open_filter(record.flags),
        )
    {
        return;
    }

    let display_path = media_store_pending_open_display_path(
        &hub.get_package_name(),
        record.flags,
        record.pathname,
        record.result,
    );
    let display_original_path = display_path
        .as_ref()
        .and_then(|_| media_store_pending_display_path(record.original_pathname));
    let record_path = display_path.as_deref().unwrap_or(record.pathname);
    let record_original_path = display_original_path
        .as_deref()
        .unwrap_or(record.original_pathname);

    remember_private_path_caller_hint_for_monitor(hub, record_path);

    let mut extra = build_open_result_extra(
        record.op_name,
        record.flags,
        record_path,
        record_original_path,
        record.is_mapping,
    );
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Open,
        &caller_package,
        record_path,
        record.result,
        record.error_no,
        &extra,
    );
}

fn build_open_result_extra(
    op_name: &str,
    flags: i32,
    pathname: &str,
    original_pathname: &str,
    is_mapping: bool,
) -> String {
    let operation_filter_name = open_operation_filter_name(op_name, flags);
    let mut extra = format!(
        "op={}|op_filter={}|flags=0x{:x}",
        op_name, operation_filter_name, flags
    );
    if !original_pathname.is_empty() && original_pathname != pathname {
        extra.push_str("|from=");
        extra.push_str(original_pathname);
        extra.push_str("|backend=");
        extra.push_str(pathname);
        extra.push_str("|source=");
        extra.push_str(redirect_source_name(pathname, is_mapping));
    }
    extra
}

fn should_skip_media_provider_pending_probe(
    package_name: &str,
    flags: i32,
    pathname: &str,
    original_pathname: &str,
    is_mapping: bool,
    result: i32,
    error_no: i32,
) -> bool {
    result == -1
        && error_no == libc::ENOENT
        && has_create_intent_flags(flags)
        && !is_mapping
        && (original_pathname.is_empty() || original_pathname == pathname)
        && crate::redirect::policy::is_media_provider_package(package_name)
        && is_media_store_pending_file(pathname)
}

fn is_media_store_pending_file(pathname: &str) -> bool {
    media_store_pending_display_path(pathname).is_some()
}

fn media_store_pending_open_display_path(
    package_name: &str,
    flags: i32,
    pathname: &str,
    result: i32,
) -> Option<String> {
    if result < 0
        || !has_create_intent_flags(flags)
        || !policy::is_media_provider_package(package_name)
    {
        return None;
    }
    media_store_pending_display_path(pathname)
}

fn media_store_pending_display_path(pathname: &str) -> Option<String> {
    let normalized = normalize_storage_alias_for_monitor(pathname);
    let slash = normalized.rfind('/')?;
    let file_name = &normalized[slash + 1..];
    let tail = file_name.strip_prefix(".pending-")?;
    let display_start = tail.find('-')? + 1;
    if display_start >= tail.len() {
        return None;
    }

    Some(format!(
        "{}/{}",
        normalized[..slash].trim_end_matches('/'),
        &tail[display_start..]
    ))
}

fn normalize_storage_alias_for_monitor(pathname: &str) -> String {
    let normalized = paths::normalize(pathname);
    if paths::starts_with(&normalized, "/data/media/") {
        return paths::data_media_to_storage_path(&normalized);
    }
    normalized
}

fn is_media_store_pending_commit(
    package_name: &str,
    old_pathname: &str,
    new_pathname: &str,
    result: i32,
) -> bool {
    if result < 0 || !policy::is_media_provider_package(package_name) {
        return false;
    }
    if is_media_store_pending_file(new_pathname) {
        return false;
    }

    media_store_pending_display_path(old_pathname).is_some_and(|display_path| {
        display_path == normalize_storage_alias_for_monitor(new_pathname)
    })
}

fn record_media_store_pending_commit_if_needed(
    hub: &InterceptHub,
    record: &RenameResultRecord<'_>,
    extra_tail: Option<&str>,
) -> bool {
    if !is_media_store_pending_commit(
        &hub.get_package_name(),
        record.old_pathname,
        record.new_pathname,
        record.result,
    ) {
        return false;
    }

    let mut extra = if record.flags >= 0 {
        format!(
            "op={}|op_filter=open:create|flags=0x{:x}|from={}|source=media_store_pending_commit",
            record.op_name, record.flags, record.old_pathname
        )
    } else {
        format!(
            "op={}|op_filter=open:create|from={}|source=media_store_pending_commit",
            record.op_name, record.old_pathname
        )
    };
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Open,
        &caller_package,
        record.new_pathname,
        record.result,
        record.error_no,
        &extra,
    );
    true
}

fn open_operation_filter_name(op_name: &str, flags: i32) -> String {
    if has_create_intent_flags(flags) {
        return format!("{}:create", op_name);
    }
    if has_write_intent_flags(flags) {
        return format!("{}:write", op_name);
    }
    format!("{}:read", op_name)
}

fn saf_provider_open_filter(flags: i32) -> &'static str {
    if has_create_intent_flags(flags) {
        return "provider_open:create";
    }
    if has_write_intent_flags(flags) {
        return "provider_open:write";
    }
    "provider_open:read"
}

fn remember_private_path_caller_hint_for_monitor(hub: &InterceptHub, pathname: &str) {
    let Some((normalized_path, owner_package, user_id)) = private_storage_owner_path(pathname)
    else {
        return;
    };

    let process_package = hub.get_package_name();
    let caller_uid = hub.get_current_caller_uid();
    let caller_package = hub.get_current_caller_package();
    if !should_remember_monitor_private_path_caller_hint(
        &process_package,
        &owner_package,
        &caller_package,
        caller_uid,
        user_id,
    ) {
        return;
    }

    crate::monitor::remember_private_path_caller_hint_in_memory(
        &normalized_path,
        &owner_package,
        &caller_package,
        caller_uid,
        user_id,
    );
}

fn private_storage_owner_path(pathname: &str) -> Option<(String, String, i32)> {
    if pathname.is_empty() {
        return None;
    }

    let normalized = paths::normalize(pathname);
    let normalized = if paths::starts_with(&normalized, "/storage/emulated/") {
        normalized
    } else if paths::starts_with(&normalized, "/data/media/") {
        paths::data_media_to_storage_path(&normalized)
    } else {
        return None;
    };

    let user_id = paths::extract_user_id_from_storage_path(&normalized);
    if user_id < 0 {
        return None;
    }
    let owner_package = paths::extract_android_private_path_owner(&normalized);
    if owner_package.is_empty()
        || policy::is_system_writer_package(&owner_package)
        || policy::is_media_intermediate_package(&owner_package)
    {
        return None;
    }

    Some((normalized, owner_package, user_id))
}

fn should_remember_monitor_private_path_caller_hint(
    process_package: &str,
    owner_package: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
) -> bool {
    if user_id < 0
        || caller_uid < ANDROID_APP_UID_START
        || platform::user_id_from_uid(caller_uid) != user_id
    {
        return false;
    }
    if owner_package.is_empty()
        || caller_package.is_empty()
        || caller_package == owner_package
        || policy::is_system_writer_package(caller_package)
        || policy::is_media_intermediate_package(caller_package)
    {
        return false;
    }

    policy::is_system_writer_package(process_package)
        || policy::is_file_monitor_bridge_package(process_package)
}

fn record_saf_bridge_provider_path(hub: &InterceptHub, pathname: &str, op_filter: &str) -> bool {
    if !policy::is_saf_native_monitor_bridge_package(&hub.get_package_name()) {
        return false;
    }
    let caller_uid = hub.get_current_caller_uid();
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_saf_provider_path(
        pathname,
        caller_uid,
        &caller_package,
        op_filter,
    )
}

pub fn record_mkdir_result(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    result: i32,
    error_no: i32,
) {
    record_mkdir_result_with_extra(hub, op_name, pathname, result, error_no, None);
}

pub fn record_mkdir_result_from(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    original_pathname: &str,
    result: i32,
    error_no: i32,
) {
    if !original_pathname.is_empty() && original_pathname != pathname {
        let extra_tail = format!(
            "from={}|backend={}|source={}",
            original_pathname,
            pathname,
            redirect_source_name(pathname, false)
        );
        record_mkdir_result_with_extra(hub, op_name, pathname, result, error_no, Some(&extra_tail));
    } else {
        record_mkdir_result_with_extra(hub, op_name, pathname, result, error_no, None);
    }
}

pub fn record_read_only_mkdir_result(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    read_only_path: &str,
) {
    let extra_tail = read_only_extra_tail(pathname, read_only_path);
    record_mkdir_result_with_extra(hub, op_name, pathname, -1, libc::EROFS, Some(&extra_tail));
}

fn record_mkdir_result_with_extra(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    result: i32,
    error_no: i32,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }

    if !SHOULD_MONITOR_LOG_DIR_CREATE {
        return;
    }
    if result >= 0 && record_saf_bridge_provider_path(hub, pathname, "provider_open:create") {
        return;
    }
    let mut extra = format!("op={}", op_name);
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    let kind = if op_name == "mknod" || op_name == "mknodat" {
        OpKind::Mknod
    } else {
        OpKind::Mkdir
    };
    AuditTrail::instance().record_operation_result(
        kind,
        &caller_package,
        pathname,
        result,
        error_no,
        &extra,
    );
}

pub fn record_rename_result(
    hub: &InterceptHub,
    op_name: &str,
    new_pathname: &str,
    old_pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
) {
    let record = RenameResultRecord {
        op_name,
        new_pathname,
        old_pathname,
        result,
        error_no,
        flags,
    };
    record_rename_result_with_extra(hub, &record, None);
}

pub fn record_rename_result_with_display_paths(
    hub: &InterceptHub,
    record: RenameResultRecord<'_>,
    display_new_pathname: &str,
    display_old_pathname: &str,
) {
    let extra_tail = if record.new_pathname != display_new_pathname
        || record.old_pathname != display_old_pathname
    {
        Some(format!(
            "display_from={}|display_to={}",
            display_old_pathname, display_new_pathname
        ))
    } else {
        None
    };
    record_rename_result_with_extra(hub, &record, extra_tail.as_deref());
}

pub fn record_read_only_rename_result(
    hub: &InterceptHub,
    op_name: &str,
    new_pathname: &str,
    old_pathname: &str,
    flags: i32,
    read_only_path: &str,
) {
    let extra_tail = read_only_extra_tail(new_pathname, read_only_path);
    let record = RenameResultRecord {
        op_name,
        new_pathname,
        old_pathname,
        result: -1,
        error_no: libc::EROFS,
        flags,
    };
    record_rename_result_with_extra(hub, &record, Some(&extra_tail));
}

fn record_rename_result_with_extra(
    hub: &InterceptHub,
    record: &RenameResultRecord<'_>,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }

    if record_media_store_pending_commit_if_needed(hub, record, extra_tail) {
        return;
    }

    if record.result >= 0
        && record_saf_bridge_provider_path(hub, record.new_pathname, "provider_open:create")
    {
        return;
    }

    let mut extra = if record.flags >= 0 {
        format!(
            "op={}|flags=0x{:x}|from={}",
            record.op_name, record.flags, record.old_pathname
        )
    } else {
        format!("op={}|from={}", record.op_name, record.old_pathname)
    };
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Rename,
        &caller_package,
        record.new_pathname,
        record.result,
        record.error_no,
        &extra,
    );
}

pub fn record_unlink_result(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
) {
    record_unlink_result_with_extra(hub, op_name, pathname, result, error_no, flags, None);
}

pub fn record_unlink_result_from(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    original_pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
) {
    if !original_pathname.is_empty() && original_pathname != pathname {
        let extra_tail = format!("from={}", original_pathname);
        record_unlink_result_with_extra(
            hub,
            op_name,
            pathname,
            result,
            error_no,
            flags,
            Some(&extra_tail),
        );
    } else {
        record_unlink_result_with_extra(hub, op_name, pathname, result, error_no, flags, None);
    }
}

pub fn record_read_only_unlink_result(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    flags: i32,
    read_only_path: &str,
) {
    let extra_tail = read_only_extra_tail(pathname, read_only_path);
    record_unlink_result_with_extra(
        hub,
        op_name,
        pathname,
        -1,
        libc::EROFS,
        flags,
        Some(&extra_tail),
    );
}

fn record_unlink_result_with_extra(
    hub: &InterceptHub,
    op_name: &str,
    pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }

    let is_rmdir = flags >= 0 && (flags & AT_REMOVEDIR) != 0;
    let kind = if is_rmdir {
        OpKind::Rmdir
    } else {
        OpKind::Unlink
    };
    let mut extra = if flags >= 0 {
        if is_rmdir {
            format!("op={}|op_filter=rmdir|flags=0x{:x}", op_name, flags)
        } else {
            format!("op={}|flags=0x{:x}", op_name, flags)
        }
    } else {
        format!("op={}", op_name)
    };
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        kind,
        &caller_package,
        pathname,
        result,
        error_no,
        &extra,
    );
}

pub fn record_path_operation_result(
    hub: &InterceptHub,
    kind: OpKind,
    op_name: &str,
    pathname: &str,
    result: i32,
    error_no: i32,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }

    let mut extra = format!("op={}", op_name);
    if let Some(tail) = extra_tail
        && !tail.is_empty()
    {
        extra.push('|');
        extra.push_str(tail);
    }
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        kind,
        &caller_package,
        pathname,
        result,
        error_no,
        &extra,
    );
}

pub fn record_read_only_path_operation_result(
    hub: &InterceptHub,
    kind: OpKind,
    op_name: &str,
    pathname: &str,
    extra_tail: Option<&str>,
) {
    let mut extra = String::new();
    append_extra_tail(&mut extra, extra_tail);
    append_extra_tail(&mut extra, Some(READ_ONLY_DENY_REASON));
    let extra_tail = if extra.is_empty() {
        None
    } else {
        Some(extra.as_str())
    };
    record_path_operation_result(hub, kind, op_name, pathname, -1, libc::EROFS, extra_tail);
}

fn append_extra_tail(extra: &mut String, extra_tail: Option<&str>) {
    if let Some(tail) = extra_tail
        && !tail.is_empty()
    {
        if !extra.is_empty() {
            extra.push('|');
        }
        extra.push_str(tail);
    }
}

fn redirect_source_name(pathname: &str, is_mapping: bool) -> &'static str {
    if is_mapping {
        return "path_mapping";
    }
    if pathname.contains("/Android/data/") && pathname.contains("/sdcard") {
        "sandbox_path"
    } else {
        "redirect_root"
    }
}

fn read_only_extra_tail(record_path: &str, read_only_path: &str) -> String {
    if !read_only_path.is_empty() && read_only_path != record_path {
        format!(
            "read_only_path={}|{}",
            read_only_path, READ_ONLY_DENY_REASON
        )
    } else {
        READ_ONLY_DENY_REASON.to_string()
    }
}
