use super::stats::InterceptHub;
use crate::monitor::{AuditTrail, OpKind};
use crate::platform::{self, paths};
use crate::redirect::policy;
use libc::{AT_REMOVEDIR, O_APPEND, O_CREAT, O_PATH, O_RDWR, O_TMPFILE, O_TRUNC, O_WRONLY};

const ANDROID_APP_UID_START: i32 = 10_000;
const SHOULD_MONITOR_LOG_DIR_CREATE: bool = true;
const READ_ONLY_DENY_REASON: &str = "deny_reason=read_only_rule";

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

pub fn record_open_result(
    hub: &InterceptHub,
    op_name: &str,
    flags: i32,
    pathname: &str,
    original_pathname: &str,
    is_mapping: bool,
    result: i32,
    error_no: i32,
) {
    record_open_result_with_extra(
        hub,
        op_name,
        flags,
        pathname,
        original_pathname,
        is_mapping,
        result,
        error_no,
        None,
    );
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
    record_open_result_with_extra(
        hub,
        op_name,
        flags,
        pathname,
        original_pathname,
        false,
        -1,
        libc::EROFS,
        Some(extra_tail.as_str()),
    );
}

#[allow(clippy::too_many_arguments)]
fn record_open_result_with_extra(
    hub: &InterceptHub,
    op_name: &str,
    flags: i32,
    pathname: &str,
    original_pathname: &str,
    is_mapping: bool,
    result: i32,
    error_no: i32,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }
    if should_skip_media_provider_pending_probe(
        &hub.get_package_name(),
        flags,
        pathname,
        original_pathname,
        is_mapping,
        result,
        error_no,
    ) {
        return;
    }

    if result >= 0
        && record_saf_bridge_provider_path(hub, pathname, saf_provider_open_filter(flags))
    {
        return;
    }

    remember_private_path_caller_hint_for_monitor(hub, pathname);

    let mut extra =
        build_open_result_extra(op_name, flags, pathname, original_pathname, is_mapping);
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Open,
        &caller_package,
        pathname,
        result,
        error_no,
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
    _is_mapping: bool,
    _result: i32,
    _error_no: i32,
) -> bool {
    if !has_write_intent_flags(flags)
        || !crate::redirect::policy::is_media_provider_package(package_name)
    {
        return false;
    }

    is_media_store_pending_file(pathname) || is_media_store_pending_file(original_pathname)
}

fn is_media_store_pending_file(pathname: &str) -> bool {
    media_store_pending_display_path(pathname).is_some()
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
    op_name: &str,
    new_pathname: &str,
    old_pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
    extra_tail: Option<&str>,
) -> bool {
    if !is_media_store_pending_commit(&hub.get_package_name(), old_pathname, new_pathname, result) {
        return false;
    }

    let mut extra = if flags >= 0 {
        format!(
            "op={}|op_filter=open:create|flags=0x{:x}|from={}|source=media_store_pending_commit",
            op_name, flags, old_pathname
        )
    } else {
        format!(
            "op={}|op_filter=open:create|from={}|source=media_store_pending_commit",
            op_name, old_pathname
        )
    };
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Open,
        &caller_package,
        new_pathname,
        result,
        error_no,
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
    record_rename_result_with_extra(
        hub,
        op_name,
        new_pathname,
        old_pathname,
        result,
        error_no,
        flags,
        None,
    );
}

pub fn record_rename_result_with_display_paths(
    hub: &InterceptHub,
    op_name: &str,
    new_pathname: &str,
    old_pathname: &str,
    display_new_pathname: &str,
    display_old_pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
) {
    let extra_tail = if new_pathname != display_new_pathname || old_pathname != display_old_pathname
    {
        Some(format!(
            "display_from={}|display_to={}",
            display_old_pathname, display_new_pathname
        ))
    } else {
        None
    };
    record_rename_result_with_extra(
        hub,
        op_name,
        new_pathname,
        old_pathname,
        result,
        error_no,
        flags,
        extra_tail.as_deref(),
    );
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
    record_rename_result_with_extra(
        hub,
        op_name,
        new_pathname,
        old_pathname,
        -1,
        libc::EROFS,
        flags,
        Some(&extra_tail),
    );
}

#[allow(clippy::too_many_arguments)]
fn record_rename_result_with_extra(
    hub: &InterceptHub,
    op_name: &str,
    new_pathname: &str,
    old_pathname: &str,
    result: i32,
    error_no: i32,
    flags: i32,
    extra_tail: Option<&str>,
) {
    if !hub.is_monitor_enabled() {
        return;
    }

    if record_media_store_pending_commit_if_needed(
        hub,
        op_name,
        new_pathname,
        old_pathname,
        result,
        error_no,
        flags,
        extra_tail,
    ) {
        return;
    }

    if result >= 0 && record_saf_bridge_provider_path(hub, new_pathname, "provider_open:create") {
        return;
    }

    let mut extra = if flags >= 0 {
        format!("op={}|flags=0x{:x}|from={}", op_name, flags, old_pathname)
    } else {
        format!("op={}|from={}", op_name, old_pathname)
    };
    append_extra_tail(&mut extra, extra_tail);
    let caller_package = hub.get_current_caller_package();
    AuditTrail::instance().record_operation_result(
        OpKind::Rename,
        &caller_package,
        new_pathname,
        result,
        error_no,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_operation_filter_distinguishes_create_write_and_read() {
        assert_eq!(
            open_operation_filter_name("open", O_WRONLY | O_CREAT),
            "open:create"
        );
        assert_eq!(
            open_operation_filter_name("open", O_WRONLY | O_TRUNC),
            "open:write"
        );
        assert_eq!(
            open_operation_filter_name("open", libc::O_RDONLY),
            "open:read"
        );
        assert_eq!(
            open_operation_filter_name("open", O_PATH | O_WRONLY),
            "open:read"
        );
    }

    #[test]
    fn media_store_pending_file_detection_requires_display_name() {
        assert!(is_media_store_pending_file(
            "/storage/emulated/0/Download/Weixin/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip",
        ));
        assert!(!is_media_store_pending_file(
            "/storage/emulated/0/Download/Weixin/.pending-1783058689-",
        ));
        assert!(!is_media_store_pending_file(
            "/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
        ));
    }

    #[test]
    fn media_store_pending_commit_is_final_file_create() {
        assert!(is_media_store_pending_commit(
            "com.android.providers.media.module",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip",
            0,
        ));
        assert!(is_media_store_pending_commit(
            "com.android.providers.media.module",
            "/data/media/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip",
            0,
        ));
    }

    #[test]
    fn media_store_pending_commit_rejects_non_commit_rename() {
        assert!(!is_media_store_pending_commit(
            "com.android.providers.media.module",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            0,
        ));
        assert!(!is_media_store_pending_commit(
            "com.android.providers.media.module",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/other.zip",
            0,
        ));
        assert!(!is_media_store_pending_commit(
            "com.tencent.mm",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip",
            0,
        ));
        assert!(!is_media_store_pending_commit(
            "com.android.providers.media.module",
            "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip",
            -1,
        ));
    }

    #[test]
    fn media_provider_pending_intermediate_records_are_filtered() {
        let original = "/storage/emulated/0/Download/Weixin/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip";
        let target = "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip";

        assert!(should_skip_media_provider_pending_probe(
            "com.android.providers.media.module",
            O_RDWR | O_CREAT,
            original,
            original,
            false,
            -1,
            libc::ENOENT,
        ));
        assert!(should_skip_media_provider_pending_probe(
            "com.android.providers.media.module",
            O_RDWR | O_CREAT,
            original,
            original,
            false,
            3,
            0,
        ));
        assert!(should_skip_media_provider_pending_probe(
            "com.android.providers.media.module",
            O_RDWR | O_CREAT,
            target,
            original,
            true,
            -1,
            libc::ENOENT,
        ));
        assert!(!should_skip_media_provider_pending_probe(
            "com.tencent.mm",
            O_RDWR | O_CREAT,
            original,
            original,
            false,
            -1,
            libc::ENOENT,
        ));
    }

    #[test]
    fn monitor_private_path_caller_hint_accepts_system_writer_external_caller() {
        assert!(should_remember_monitor_private_path_caller_hint(
            "com.android.providers.media.module",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        ));
        assert!(should_remember_monitor_private_path_caller_hint(
            "com.android.providers.downloads",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        ));
    }

    #[test]
    fn monitor_private_path_caller_hint_rejects_owners_and_intermediates() {
        assert!(!should_remember_monitor_private_path_caller_hint(
            "com.android.providers.media.module",
            "com.eg.android.AlipayGphone",
            "com.eg.android.AlipayGphone",
            10217,
            0,
        ));
        assert!(!should_remember_monitor_private_path_caller_hint(
            "com.android.providers.media.module",
            "com.eg.android.AlipayGphone",
            "com.android.documentsui",
            10071,
            0,
        ));
        assert!(!should_remember_monitor_private_path_caller_hint(
            "com.leo.xposed.xradiant",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        ));
    }

    #[test]
    fn private_storage_owner_path_normalizes_data_media_alias() {
        let parsed = private_storage_owner_path(
            "/data/media/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
        )
        .expect("private owner path should parse");

        assert_eq!(
            parsed.0,
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db"
        );
        assert_eq!(parsed.1, "com.eg.android.AlipayGphone");
        assert_eq!(parsed.2, 0);
        assert!(
            private_storage_owner_path("/storage/emulated/0/Download/XRadiant_backup.json")
                .is_none()
        );
    }
}
