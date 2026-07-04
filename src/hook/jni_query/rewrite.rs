use super::super::stats::InterceptHub;
use super::types::{FILE_SCHEME_PREFIX, SAMPLE_LOG_INITIAL, SAMPLE_LOG_INTERVAL, STORAGE_PREFIXES};
use crate::config::SettingsHub;
use crate::domain::PathMapping;
use crate::platform::{self, paths};
use crate::redirect::{
    RedirectAction, RedirectDecision, policy, process_redirect_path, process_write_redirect_path,
    writer,
};
use std::ffi::CString;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

static REWRITE_SAMPLE_COUNTER: AtomicU32 = AtomicU32::new(0);
static VISIBILITY_SAMPLE_COUNTER: AtomicU32 = AtomicU32::new(0);

// MediaProvider cursor rows can hit this path very frequently. Keep inotify as
// the fast path, and use a throttled fingerprint check so missed watcher events
// or an initially empty snapshot do not disable query filtering for a caller.
const REWRITE_FINGERPRINT_RELOAD_INTERVAL_MS: i64 = 1000;
static LAST_REWRITE_FINGERPRINT_CHECK_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);

fn refresh_settings_snapshot() {
    // 优先：inotify 事件命中 → 立刻重载，保证用户改配置后即时生效
    if crate::config::watcher::poll_changed() {
        crate::hook::refresh_runtime_config_after_disk_change();
        return;
    }

    let now_ms = paths::monotonic_ms();
    let last_ms = LAST_REWRITE_FINGERPRINT_CHECK_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_ms) < REWRITE_FINGERPRINT_RELOAD_INTERVAL_MS {
        return;
    }
    if LAST_REWRITE_FINGERPRINT_CHECK_MS
        .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    SettingsHub::instance().reload_if_changed();
    crate::hook::refresh_runtime_config_from_settings();
}

// 路径命中重定向且目标不存在时返回空字符串，避免展示无效媒体行
pub(crate) fn rewrite_cursor_storage_path_for_caller(
    original_text: &str,
    caller_uid: i32,
) -> Option<String> {
    if caller_uid < 0 {
        return None;
    }
    let (path_text, has_file_scheme) = split_storage_path(original_text)?;
    let caller_package = resolve_caller_package(caller_uid, path_text);
    if caller_package.is_empty() {
        return None;
    }
    let hub = InterceptHub::instance();
    let previous_package = hub.get_current_caller_package();
    let previous_uid = hub.get_current_caller_uid();
    hub.set_current_caller_package(&caller_package);
    hub.set_current_caller_uid(caller_uid);
    let _explicit_caller = crate::hook::enter_explicit_caller_decision();
    let result = rewrite_cursor_storage_path_for_mapping_view(
        original_text,
        path_text,
        has_file_scheme,
        &caller_package,
        caller_uid,
        true,
    )
    .or_else(|| rewrite_cursor_storage_path_for_non_mapping_view(original_text));
    hub.set_current_caller_package(&previous_package);
    hub.set_current_caller_uid(previous_uid);
    result
}

pub(crate) fn resolve_open_storage_path_for_caller(
    original_text: &str,
    caller_uid: i32,
) -> Option<String> {
    if caller_uid < 0 {
        return None;
    }
    let (path_text, _) = split_storage_path(original_text)?;
    let (caller_package, effective_uid) = resolve_storage_caller_context(caller_uid, path_text);
    if caller_package.is_empty() {
        return None;
    }
    let hub = InterceptHub::instance();
    let previous_package = hub.get_current_caller_package();
    let previous_uid = hub.get_current_caller_uid();
    hub.set_current_caller_package(&caller_package);
    hub.set_current_caller_uid(effective_uid);
    let _explicit_caller = crate::hook::enter_explicit_caller_decision();
    let mapped_view_target =
        resolve_mapping_view_open_target(path_text, &caller_package, effective_uid);
    let decision = process_redirect_path(hub, path_text);
    hub.set_current_caller_package(&previous_package);
    hub.set_current_caller_uid(previous_uid);
    if decision.is_redirect() && !decision.new_path.is_empty() {
        return Some(decision.new_path);
    }
    if let Some(target) = mapped_view_target
        && !target.is_empty()
        && target != path_text
    {
        return Some(target);
    }
    None
}

fn resolve_storage_caller_context(caller_uid: i32, path_text: &str) -> (String, i32) {
    let user_id = platform::user_id_from_uid(caller_uid);
    let system_writer = is_system_writer_uid(caller_uid);
    if system_writer
        && let Some(context) =
            resolve_mapping_request_caller_context(user_id, caller_uid, path_text, true)
    {
        return context;
    }

    let caller_package = resolve_caller_package(caller_uid, path_text);
    if !caller_package.is_empty() {
        return (caller_package, caller_uid);
    }

    if let Some(context) =
        resolve_mapping_request_caller_context(user_id, caller_uid, path_text, false)
    {
        return context;
    }

    (caller_package, caller_uid)
}

fn resolve_mapping_request_caller_context(
    user_id: i32,
    caller_uid: i32,
    path_text: &str,
    allow_uid_override: bool,
) -> Option<(String, i32)> {
    if user_id < 0 || path_text.is_empty() {
        return None;
    }
    let inferred = SettingsHub::instance()
        .resolve_mapping_request_package_by_path_for_user(user_id, path_text);
    if inferred.is_empty() {
        return None;
    }
    let inferred_uid = policy::get_fresh_uid_for_package(&inferred);
    if inferred_uid >= writer::ANDROID_APP_UID_START {
        if !allow_uid_override
            && caller_uid >= writer::ANDROID_APP_UID_START
            && inferred_uid != caller_uid
        {
            return None;
        }
        return Some((inferred, inferred_uid));
    }
    Some((inferred, caller_uid))
}

fn is_system_writer_uid(caller_uid: i32) -> bool {
    if caller_uid < 0 {
        return false;
    }
    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    packages
        .iter()
        .any(|package| policy::is_system_writer_package(package))
}

pub(crate) fn rewrite_media_store_storage_path_for_caller(
    original_text: &str,
    caller_uid: i32,
) -> Option<String> {
    if caller_uid < 0 {
        return None;
    }
    let (path_text, has_file_scheme) = split_media_store_value_path(original_text)?;
    let (caller_package, effective_uid) = resolve_storage_caller_context(caller_uid, path_text);
    if caller_package.is_empty() {
        return None;
    }
    if let Some(mapped_target) =
        resolve_mapping_view_open_target(path_text, &caller_package, effective_uid)
        && let Some(rewritten) =
            rewrite_media_store_mapped_value(path_text, has_file_scheme, &mapped_target)
    {
        return Some(rewritten);
    }
    if let Some(rewritten) = rewrite_default_sandbox_media_store_value(
        path_text,
        has_file_scheme,
        &caller_package,
        effective_uid,
    ) {
        return Some(rewritten);
    }
    let hub = InterceptHub::instance();
    let previous_package = hub.get_current_caller_package();
    let previous_uid = hub.get_current_caller_uid();
    hub.set_current_caller_package(&caller_package);
    hub.set_current_caller_uid(effective_uid);
    let _explicit_caller = crate::hook::enter_explicit_caller_decision();
    // MediaStore 插入/更新操作应该使用 write 模式以正确检查只读路径
    let decision = process_write_redirect_path(hub, path_text);
    log::info!(
        "[MEDIA_READONLY_FIX] caller={} uid={} path={} action={:?} is_mapping={} new_path={}",
        caller_package,
        effective_uid,
        path_text,
        decision.action,
        decision.is_mapping,
        decision.new_path
    );
    hub.set_current_caller_package(&previous_package);
    hub.set_current_caller_uid(previous_uid);
    if !decision.is_redirect() || decision.new_path.is_empty() {
        return None;
    }
    if decision.is_mapping {
        return rewrite_media_store_mapped_value(path_text, has_file_scheme, &decision.new_path);
    }
    let rewritten = if has_file_scheme {
        format!("{}{}", FILE_SCHEME_PREFIX, decision.new_path)
    } else {
        decision.new_path
    };
    sample_row_log(
        "rewrite_media_values_sandbox_backend",
        path_text,
        &rewritten,
        has_file_scheme,
        false,
    );
    Some(rewritten)
}

pub(crate) fn resolve_download_media_placeholder_path_for_caller(
    original_path: &str,
    relative_path: &str,
    display_name: &str,
    video: bool,
    caller_uid: i32,
) -> Option<String> {
    if caller_uid < 0 {
        return None;
    }
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return None;
    }
    let caller_package = resolve_caller_package_for_media_placeholder(
        caller_uid,
        original_path,
        relative_path,
        display_name,
    );
    if caller_package.is_empty() {
        return None;
    }
    let mappings = writer::get_caller_mappings(&caller_package, caller_uid);
    if mappings.is_empty() {
        return None;
    }
    let source =
        download_media_placeholder_source(original_path, relative_path, display_name, user_id)?;
    let mapped = resolve_placeholder_path_by_mappings(&source, video, &mappings, user_id)?;
    sample_row_log(
        "rewrite_download_media_placeholder",
        &source.path,
        &mapped,
        false,
        false,
    );
    Some(mapped)
}

pub(crate) fn rewrite_media_store_bucket_id_for_caller(
    bucket_id_text: &str,
    caller_uid: i32,
) -> Option<String> {
    let requested_bucket_id = bucket_id_text.trim().parse::<i32>().ok()?;
    let caller_package = resolve_caller_package(caller_uid, "");
    if caller_package.is_empty() {
        return None;
    }
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return None;
    }
    let mappings = writer::get_caller_mappings(&caller_package, caller_uid);
    for mapping in mappings {
        let request_path = normalize_bucket_path_for_user(&mapping.request_path, user_id);
        if request_path.is_empty() {
            continue;
        }
        if java_bucket_id(&request_path) != requested_bucket_id {
            continue;
        }
        let final_path = normalize_bucket_path_for_user(&mapping.final_path, user_id);
        if final_path.is_empty() {
            continue;
        }
        let mapped_bucket_id = java_bucket_id(&final_path);
        if mapped_bucket_id == requested_bucket_id {
            return None;
        }
        return Some(mapped_bucket_id.to_string());
    }
    None
}

pub(crate) fn is_redirect_enabled_for_caller_uid(caller_uid: i32) -> bool {
    if caller_uid < 0 {
        return false;
    }
    refresh_settings_snapshot();
    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    packages.sort();
    packages.dedup();
    packages.retain(|pkg| !pkg.is_empty() && !policy::is_system_writer_package(pkg));
    packages
        .iter()
        .any(|pkg| SettingsHub::instance().should_redirect(pkg, caller_uid))
}

pub(crate) fn storage_path_exists_by_syscall(path_text: &str) -> bool {
    path_exists_by_syscall(path_text)
}

pub(crate) fn should_hide_cursor_storage_path_for_caller(
    original_text: &str,
    caller_uid: i32,
) -> bool {
    let log = |reason: &str, path: &str, caller_package: &str, target: &str, hide: bool| {
        sample_visibility_log(reason, path, caller_package, caller_uid, target, hide);
        hide
    };
    if caller_uid < writer::ANDROID_APP_UID_START {
        return log("low_uid", original_text, "", "", false);
    }
    let Some((path_text, _)) = split_storage_path(original_text) else {
        return log("not_storage", original_text, "", "", false);
    };
    let caller_package = resolve_redirect_caller_package(caller_uid);
    if caller_package.is_empty() {
        return log("caller_empty", path_text, "", "", false);
    }
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return log("user_empty", path_text, &caller_package, "", false);
    }
    let resolved_path = paths::resolve_user_path(&paths::normalize(path_text), user_id);
    if !writer::is_path_in_user_storage(&resolved_path, user_id) {
        return log("outside_storage", path_text, &caller_package, "", false);
    }
    let config = SettingsHub::instance();
    let enablement = config.get_user_redirect_enablement(&caller_package, caller_uid, user_id);
    if !enablement.is_enabled() {
        return log("disabled", &resolved_path, &caller_package, "", false);
    }

    let mappings = writer::get_caller_mappings(&caller_package, caller_uid);
    if writer::is_path_in_caller_mapping_request(&resolved_path, &mappings) {
        if is_media_store_pending_path(&resolved_path) {
            return log(
                "mapping_request_pending",
                &resolved_path,
                &caller_package,
                "",
                false,
            );
        }
        let mapped_path = writer::map_path_by_caller_mappings(&resolved_path, &mappings);
        if mapping_target_path_exists(&mapped_path) {
            return log(
                "mapping_request_target_exists",
                &resolved_path,
                &caller_package,
                &mapped_path,
                false,
            );
        }
        return log("mapping_request", &resolved_path, &caller_package, "", true);
    }
    let mapped_path = writer::map_path_by_caller_mappings(&resolved_path, &mappings);
    if !mapped_path.is_empty() && mapped_path != resolved_path {
        return log(
            "mapping_target",
            &resolved_path,
            &caller_package,
            &mapped_path,
            false,
        );
    }
    if should_allow_map_only_cursor_miss(
        config,
        &resolved_path,
        &caller_package,
        caller_uid,
        user_id,
    ) {
        return log("map_only_miss", &resolved_path, &caller_package, "", false);
    }
    if writer::is_path_excluded_by_caller_real_paths(&resolved_path, &caller_package, caller_uid) {
        return log("excluded", &resolved_path, &caller_package, "", true);
    }
    if writer::is_path_allowed_by_caller_real_paths(&resolved_path, &caller_package, caller_uid) {
        return log("allowed", &resolved_path, &caller_package, "", false);
    }
    if writer::is_path_read_only_by_caller_paths(&resolved_path, &caller_package, caller_uid) {
        return log("read_only", &resolved_path, &caller_package, "", false);
    }

    let redirect_target =
        writer::resolve_system_writer_redirect_target(&caller_package, caller_uid, user_id, false);
    if redirect_target.is_empty() {
        return log("target_empty", &resolved_path, &caller_package, "", false);
    }
    if resolved_path == redirect_target
        || paths::starts_with(&resolved_path, &format!("{}/", redirect_target))
    {
        return log(
            "inside_target",
            &resolved_path,
            &caller_package,
            &redirect_target,
            false,
        );
    }
    let fallback_path =
        writer::map_path_by_caller_fallback(&resolved_path, &redirect_target, user_id);
    if fallback_path.is_empty() || fallback_path == resolved_path {
        return log(
            "fallback_empty",
            &resolved_path,
            &caller_package,
            &redirect_target,
            false,
        );
    }
    if is_probe_path(path_text) {
        return log(
            "hide_probe_nonexist",
            &resolved_path,
            &caller_package,
            &fallback_path,
            true,
        );
    }
    if path_exists_by_syscall(&fallback_path)
        || path_exists_by_syscall(&writer::storage_to_data_media_path(&fallback_path))
    {
        return log(
            "fallback_exists",
            &resolved_path,
            &caller_package,
            &fallback_path,
            false,
        );
    }

    // Keep rows that look like a still-pending MediaStore entry. Existing public
    // originals without a caller-visible target are hidden.
    let original_exists = path_exists_by_syscall(path_text);
    log(
        if original_exists {
            "hide_public_original"
        } else {
            "pending_original_missing"
        },
        &resolved_path,
        &caller_package,
        &fallback_path,
        original_exists,
    )
}

fn should_allow_map_only_cursor_miss(
    config: &SettingsHub,
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
) -> bool {
    let enablement = config.get_user_redirect_enablement(caller_package, caller_uid, user_id);
    enablement.is_mapping_mode_only
        && !writer::is_path_sandboxed_by_caller_paths(resolved_path, caller_package, caller_uid)
}

fn rewrite_cursor_storage_path_for_non_mapping_view(original_text: &str) -> Option<String> {
    let hub = InterceptHub::instance();
    if hub.is_monitor_only() {
        return None;
    }
    rewrite_cursor_storage_path_inner(original_text, true, |path_text| {
        suppress_mapping_cursor_decision(process_redirect_path(hub, path_text))
    })
}

fn suppress_mapping_cursor_decision(decision: RedirectDecision) -> RedirectDecision {
    if decision.is_mapping {
        return RedirectDecision {
            action: RedirectAction::Allow,
            new_path: String::new(),
            is_mapping: false,
        };
    }
    decision
}

fn rewrite_cursor_storage_path_inner<F>(
    original_text: &str,
    can_hide_rows: bool,
    decide: F,
) -> Option<String>
where
    F: FnOnce(&str) -> crate::redirect::RedirectDecision,
{
    let (path_text, has_file_scheme) = split_storage_path(original_text)?;
    if path_text.is_empty() {
        return None;
    }
    // MediaStore 缩略图缓存条目指向 FUSE 内部路径，重定向后不可达
    if path_text.contains("/.thumbnails/") {
        if can_hide_rows {
            sample_row_log("thumbnails", path_text, "", has_file_scheme, true);
            return Some(String::new());
        }
        return None;
    }

    let decision = decide(path_text);
    if !decision.is_redirect() {
        return None;
    }
    if decision.new_path.is_empty() {
        if can_hide_rows {
            sample_row_log("empty_target", path_text, "", has_file_scheme, true);
            return Some(String::new());
        }
        return None;
    }
    let display_path = to_public_storage_path(&decision.new_path);
    if decision.is_mapping {
        let rewritten = if has_file_scheme {
            format!("{}{}", FILE_SCHEME_PREFIX, display_path)
        } else {
            display_path
        };
        sample_row_log(
            "rewrite_mapping",
            path_text,
            &rewritten,
            has_file_scheme,
            false,
        );
        return Some(rewritten);
    }
    if should_keep_existing_rewrite_target(
        decision.is_mapping,
        path_exists_by_syscall(&decision.new_path),
        path_exists_by_syscall(&display_path),
    ) {
        let rewritten = if has_file_scheme {
            format!("{}{}", FILE_SCHEME_PREFIX, display_path)
        } else {
            display_path
        };
        sample_row_log("rewrite", path_text, &rewritten, has_file_scheme, false);
        return Some(rewritten);
    }
    // 目标文件不存在时的处理取决于重定向类型：
    // - is_mapping=true（显式 path_mapping）：保留行并返回重定向路径。
    //   原因：MediaStore insert() 创建 pending 记录后，文件尚未写入，
    //   此时 caller query 自己的记录时目标路径不存在是正常的。
    //   如果过滤掉该行，caller 将无法获取 content URI 来写入文件。
    // - is_mapping=false（fallback/excluded 到沙箱）：过滤掉该行。
    //   原因：文件被重定向到应用沙箱，沙箱中不存在该文件说明应用
    //   没有访问权限，不应该在 MediaStore 查询结果中看到该条目。
    if can_hide_rows {
        if is_probe_path(path_text) {
            sample_row_log(
                "hide_probe_nonexist",
                path_text,
                &decision.new_path,
                has_file_scheme,
                true,
            );
            return Some(String::new());
        }
        // Download 场景常见 "先写入后可见"：若在 pending 阶段直接隐藏行，
        // caller 可能拿不到可写 URI，导致保存失败。只保留 MediaStore
        // .pending-* 临时文件，避免普通已删除媒体行继续出现在相册里。
        if is_media_store_pending_path(path_text) && !path_exists_by_syscall(path_text) {
            let display_path = to_public_storage_path(&decision.new_path);
            let rewritten = if has_file_scheme {
                format!("{}{}", FILE_SCHEME_PREFIX, display_path)
            } else {
                display_path
            };
            sample_row_log(
                "rewrite_pending",
                path_text,
                &rewritten,
                has_file_scheme,
                false,
            );
            return Some(rewritten);
        }
        sample_row_log(
            "hide_nonexist",
            path_text,
            &decision.new_path,
            has_file_scheme,
            true,
        );
        Some(String::new())
    } else {
        None
    }
}

fn rewrite_media_store_mapped_value(
    path_text: &str,
    has_file_scheme: bool,
    mapped_path: &str,
) -> Option<String> {
    let display_path = to_public_storage_path(mapped_path);
    if display_path == path_text {
        return None;
    }
    let rewritten = if has_file_scheme {
        format!("{}{}", FILE_SCHEME_PREFIX, display_path)
    } else {
        display_path
    };
    sample_row_log(
        "rewrite_media_values",
        path_text,
        &rewritten,
        has_file_scheme,
        false,
    );
    Some(rewritten)
}

fn should_keep_existing_rewrite_target(
    is_mapping: bool,
    target_exists: bool,
    display_path_exists: bool,
) -> bool {
    target_exists || (is_mapping && display_path_exists)
}

fn rewrite_default_sandbox_media_store_value(
    path_text: &str,
    has_file_scheme: bool,
    caller_package: &str,
    caller_uid: i32,
) -> Option<String> {
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 || caller_package.is_empty() {
        return None;
    }
    let redirect_target =
        writer::resolve_system_writer_redirect_target(caller_package, caller_uid, user_id, false);
    rewrite_default_sandbox_media_store_value_with_target(
        path_text,
        has_file_scheme,
        user_id,
        &redirect_target,
    )
}

fn rewrite_default_sandbox_media_store_value_with_target(
    path_text: &str,
    has_file_scheme: bool,
    user_id: i32,
    redirect_target: &str,
) -> Option<String> {
    if user_id < 0 || redirect_target.is_empty() {
        return None;
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path_text), user_id);
    if normalized.is_empty() || paths::has_unsafe_segments(&normalized) {
        return None;
    }
    let display_input = to_public_storage_path(&normalized);
    let normalized_target = paths::resolve_user_path(&paths::normalize(redirect_target), user_id);
    if normalized_target.is_empty() || paths::has_unsafe_segments(&normalized_target) {
        return None;
    }
    let display_target = to_public_storage_path(&normalized_target);
    let display_target_prefix = format!("{}/", display_target);
    let suffix = if display_input == display_target {
        ""
    } else {
        display_input.strip_prefix(&display_target_prefix)?
    };
    let storage_root = format!("/storage/emulated/{}", user_id);
    let rewritten_path = if suffix.is_empty() {
        storage_root
    } else {
        format!("{}/{}", storage_root, suffix)
    };
    if rewritten_path == path_text {
        return None;
    }
    let rewritten = if has_file_scheme {
        format!("{}{}", FILE_SCHEME_PREFIX, rewritten_path)
    } else {
        rewritten_path
    };
    sample_row_log(
        "rewrite_media_values_sandbox_backend",
        path_text,
        &rewritten,
        has_file_scheme,
        false,
    );
    Some(rewritten)
}

fn to_public_storage_path(path: &str) -> String {
    const PREFIX: &str = "/data/media/";
    if !path.starts_with(PREFIX) {
        return path.to_string();
    }
    let suffix = &path[PREFIX.len()..];
    format!("/storage/emulated/{}", suffix)
}

fn rewrite_cursor_storage_path_for_mapping_view(
    original_text: &str,
    path_text: &str,
    has_file_scheme: bool,
    caller_package: &str,
    caller_uid: i32,
    can_hide_rows: bool,
) -> Option<String> {
    let display_path = reverse_map_target_to_request_path(path_text, caller_package, caller_uid)?;
    if display_path == path_text {
        return None;
    }
    if can_hide_rows
        && !mapping_view_path_exists_for_caller(&display_path, caller_package, caller_uid)
    {
        if is_media_store_pending_path(path_text) && !path_exists_by_syscall(path_text) {
            let rewritten = if has_file_scheme {
                format!("{}{}", FILE_SCHEME_PREFIX, display_path)
            } else {
                display_path
            };
            sample_row_log(
                "rewrite_mapping_view_pending",
                original_text,
                &rewritten,
                has_file_scheme,
                false,
            );
            return Some(rewritten);
        }
        sample_row_log(
            "hide_missing_mapping_view",
            path_text,
            &display_path,
            has_file_scheme,
            true,
        );
        return Some(String::new());
    }
    let rewritten = if has_file_scheme {
        format!("{}{}", FILE_SCHEME_PREFIX, display_path)
    } else {
        display_path
    };
    sample_row_log(
        "rewrite_mapping_view",
        original_text,
        &rewritten,
        has_file_scheme,
        false,
    );
    Some(rewritten)
}

fn resolve_mapping_view_open_target(
    path_text: &str,
    caller_package: &str,
    caller_uid: i32,
) -> Option<String> {
    let mappings = writer::get_caller_mappings(caller_package, caller_uid);
    if mappings.is_empty() || !writer::is_path_in_caller_mapping_request(path_text, &mappings) {
        return None;
    }
    let mapped = writer::map_path_by_caller_mappings(path_text, &mappings);
    if mapped.is_empty() {
        None
    } else {
        Some(writer::storage_to_data_media_path(&mapped))
    }
}

fn reverse_map_target_to_request_path(
    path_text: &str,
    caller_package: &str,
    caller_uid: i32,
) -> Option<String> {
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return None;
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path_text), user_id);
    if normalized.is_empty() {
        return None;
    }
    let mappings = writer::get_caller_mappings(caller_package, caller_uid);
    if mappings.is_empty() {
        return None;
    }
    let rewritten = writer::reverse_map_path_by_caller_mappings(&normalized, &mappings);
    if rewritten.is_empty() || rewritten == normalized {
        return None;
    }
    Some(to_public_storage_path(&writer::storage_to_data_media_path(
        &rewritten,
    )))
}

fn mapping_view_path_exists_for_caller(
    display_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if let Some(parent) = display_path.strip_suffix("/.srx_probe") {
        return mapping_view_path_exists_for_caller(parent, caller_package, caller_uid);
    }
    if path_exists_by_syscall(display_path) {
        return true;
    }
    let mappings = writer::get_caller_mappings(caller_package, caller_uid);
    if mappings.is_empty() {
        return false;
    }
    let mapped = writer::map_path_by_caller_mappings(display_path, &mappings);
    if mapped.is_empty() {
        return false;
    }
    path_exists_by_syscall(&mapped)
        || path_exists_by_syscall(&writer::storage_to_data_media_path(&mapped))
}

fn mapping_target_path_exists(mapped_path: &str) -> bool {
    !mapped_path.is_empty()
        && (path_exists_by_syscall(mapped_path)
            || path_exists_by_syscall(&writer::storage_to_data_media_path(mapped_path)))
}

fn resolve_caller_package_for_media_placeholder(
    caller_uid: i32,
    original_path: &str,
    relative_path: &str,
    display_name: &str,
) -> String {
    let probe_path = if !original_path.is_empty() {
        original_path.to_string()
    } else {
        let user_id = platform::user_id_from_uid(caller_uid);
        if user_id < 0 {
            String::new()
        } else {
            let relative = relative_path.trim_matches('/');
            let name = if display_name.is_empty() {
                ".srx_media"
            } else {
                display_name
            };
            format!("/storage/emulated/{}/{}/{}", user_id, relative, name)
        }
    };
    resolve_caller_package(caller_uid, &probe_path)
}

fn download_media_placeholder_source(
    original_path: &str,
    relative_path: &str,
    display_name: &str,
    user_id: i32,
) -> Option<DownloadMediaPlaceholderSource> {
    if !original_path.is_empty() {
        let normalized = normalize_public_storage_path(original_path, user_id)?;
        if !relative_path_is_under(&normalized.relative, "Download") {
            return None;
        }
        let (parent_relative, file_name) = split_relative_parent_and_name(&normalized.relative)?;
        if file_name.is_empty() {
            return None;
        }
        return Some(DownloadMediaPlaceholderSource {
            path: normalized.path,
            relative_path: parent_relative,
            file_name,
        });
    }

    let normalized_relative = normalize_relative_path(relative_path);
    if normalized_relative.is_empty() || !relative_path_is_under(&normalized_relative, "Download") {
        return None;
    }
    let file_name = if display_name.is_empty() {
        ".srx_media".to_string()
    } else {
        display_name.to_string()
    };
    if has_unsafe_relative_path_segment(&file_name)
        || file_name.contains('/')
        || file_name.contains('\\')
    {
        return None;
    }
    let path = format!(
        "/storage/emulated/{}/{}/{}",
        user_id, normalized_relative, file_name
    );
    Some(DownloadMediaPlaceholderSource {
        path,
        relative_path: normalized_relative,
        file_name,
    })
}

fn resolve_placeholder_path_by_mappings(
    source: &DownloadMediaPlaceholderSource,
    _video: bool,
    mappings: &[PathMapping],
    user_id: i32,
) -> Option<String> {
    let source_path = paths::resolve_user_path(&paths::normalize(&source.path), user_id);
    if source_path.is_empty() || paths::has_unsafe_segments(&source_path) {
        return None;
    }
    let mut candidates = configured_placeholder_candidate_paths(source, mappings, user_id);
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.request_len.cmp(&left.request_len))
            .then_with(|| left.path.cmp(&right.path))
    });
    for candidate in candidates {
        let mapped = writer::map_path_by_caller_mappings(&candidate.path, mappings);
        if is_distinct_public_mapping(&candidate.path, &mapped) {
            return Some(to_public_storage_path(&mapped));
        }
    }
    None
}

fn configured_placeholder_candidate_paths(
    source: &DownloadMediaPlaceholderSource,
    mappings: &[PathMapping],
    user_id: i32,
) -> Vec<PlaceholderCandidate> {
    let mut candidates = Vec::new();
    let download_suffix = download_relative_suffix(&source.relative_path);
    for mapping in mappings {
        let request_relative =
            relative_path_from_public_storage_path(&mapping.request_path, user_id);
        if request_relative.is_empty() {
            continue;
        }
        let request_len = request_relative.len();
        let is_download_request = relative_path_is_under(&request_relative, "Download");
        let request_keeps_suffix = !download_suffix.is_empty()
            && relative_ends_with_suffix(&request_relative, download_suffix);
        if !download_suffix.is_empty() && !request_keeps_suffix {
            let request_with_suffix = format!(
                "/storage/emulated/{}/{}/{}/{}",
                user_id, request_relative, download_suffix, source.file_name
            );
            push_unique_candidate(
                &mut candidates,
                request_with_suffix,
                if is_download_request { 80 } else { 300 },
                request_len,
            );
        }
        let request_path = format!(
            "/storage/emulated/{}/{}/{}",
            user_id, request_relative, source.file_name
        );
        push_unique_candidate(
            &mut candidates,
            request_path,
            if is_download_request {
                100
            } else if request_keeps_suffix {
                320
            } else {
                200
            },
            request_len,
        );
    }
    candidates
}

fn push_unique_candidate(
    paths: &mut Vec<PlaceholderCandidate>,
    path: String,
    score: i32,
    request_len: usize,
) {
    if path.is_empty() {
        return;
    }
    if let Some(existing) = paths.iter_mut().find(|existing| existing.path == path) {
        if score > existing.score || (score == existing.score && request_len > existing.request_len)
        {
            existing.score = score;
            existing.request_len = request_len;
        }
        return;
    }
    paths.push(PlaceholderCandidate {
        path,
        score,
        request_len,
    });
}

fn is_distinct_public_mapping(original: &str, mapped: &str) -> bool {
    if original.is_empty() || mapped.is_empty() {
        return false;
    }
    let original_public = to_public_storage_path(original)
        .trim_end_matches('/')
        .to_string();
    let mapped_public = to_public_storage_path(mapped)
        .trim_end_matches('/')
        .to_string();
    !mapped_public.is_empty() && mapped_public != original_public
}

fn normalize_public_storage_path(path: &str, user_id: i32) -> Option<NormalizedPublicPath> {
    if path.is_empty() || user_id < 0 {
        return None;
    }
    let path_text = path.strip_prefix(FILE_SCHEME_PREFIX).unwrap_or(path);
    let normalized = paths::resolve_user_path(&paths::normalize(path_text), user_id);
    if normalized.is_empty() || paths::has_unsafe_segments(&normalized) {
        return None;
    }
    let public = to_public_storage_path(&normalized);
    let storage_root = format!("/storage/emulated/{}/", user_id);
    let relative = normalize_relative_path(public.strip_prefix(&storage_root)?);
    Some(NormalizedPublicPath {
        path: public,
        relative,
    })
}

fn relative_path_from_public_storage_path(path: &str, user_id: i32) -> String {
    if path.is_empty() || user_id < 0 {
        return String::new();
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path), user_id);
    if normalized.is_empty() || paths::has_unsafe_segments(&normalized) {
        return String::new();
    }
    let public = to_public_storage_path(&normalized);
    let storage_root = format!("/storage/emulated/{}/", user_id);
    public
        .strip_prefix(&storage_root)
        .map(normalize_relative_path)
        .unwrap_or_default()
}

fn normalize_relative_path(path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    while value.starts_with('/') {
        value.remove(0);
    }
    while value.ends_with('/') {
        value.pop();
    }
    value
}

fn relative_path_is_under(relative_path: &str, root: &str) -> bool {
    let relative = normalize_relative_path(relative_path);
    relative == root || relative.starts_with(&format!("{}/", root))
}

fn download_relative_suffix(relative_path: &str) -> &str {
    let relative = relative_path.trim_matches('/');
    if relative == "Download" {
        ""
    } else {
        relative.strip_prefix("Download/").unwrap_or("")
    }
}

fn relative_ends_with_suffix(relative_path: &str, suffix: &str) -> bool {
    let relative = relative_path.trim_matches('/');
    let suffix = suffix.trim_matches('/');
    !suffix.is_empty() && (relative == suffix || relative.ends_with(&format!("/{}", suffix)))
}

fn split_relative_parent_and_name(relative_path: &str) -> Option<(String, String)> {
    let relative = normalize_relative_path(relative_path);
    let slash = relative.rfind('/')?;
    if slash == 0 || slash >= relative.len() - 1 {
        return None;
    }
    Some((
        relative[..slash].to_string(),
        relative[slash + 1..].to_string(),
    ))
}

fn has_unsafe_relative_path_segment(relative_path: &str) -> bool {
    let relative = normalize_relative_path(relative_path);
    relative
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
}

struct DownloadMediaPlaceholderSource {
    path: String,
    relative_path: String,
    file_name: String,
}

struct NormalizedPublicPath {
    path: String,
    relative: String,
}

struct PlaceholderCandidate {
    path: String,
    score: i32,
    request_len: usize,
}

fn normalize_bucket_path_for_user(path: &str, user_id: i32) -> String {
    if path.is_empty() || user_id < 0 {
        return String::new();
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path), user_id);
    if normalized.is_empty() {
        return String::new();
    }
    let public_path = to_public_storage_path(&normalized);
    public_path.trim_end_matches('/').to_string()
}

fn java_bucket_id(path: &str) -> i32 {
    let lower = path.to_lowercase();
    let mut hash = 0i32;
    for unit in lower.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
    }
    hash
}

fn resolve_caller_package(caller_uid: i32, path_text: &str) -> String {
    refresh_settings_snapshot();
    let current_package = InterceptHub::instance().get_current_caller_package();
    if is_package_uid_match(&current_package, caller_uid) {
        return current_package;
    }
    let saved_package = crate::hook::get_binder_saved_caller_package();
    if is_package_uid_match(&saved_package, caller_uid) {
        return saved_package;
    }
    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    packages.sort();
    packages.dedup();
    packages.retain(|pkg| !pkg.is_empty() && !policy::is_system_writer_package(pkg));
    if packages.len() == 1 {
        return packages[0].clone();
    }
    let inferred =
        SettingsHub::instance().resolve_redirect_package_by_path_for_user(caller_uid, path_text);
    if is_package_uid_match(&inferred, caller_uid) {
        return inferred;
    }
    let mut enabled = packages
        .into_iter()
        .filter(|pkg| SettingsHub::instance().should_redirect(pkg, caller_uid))
        .collect::<Vec<_>>();
    if enabled.len() == 1 {
        return enabled.remove(0);
    }
    String::new()
}

fn resolve_redirect_caller_package(caller_uid: i32) -> String {
    refresh_settings_snapshot();
    let current_package = InterceptHub::instance().get_current_caller_package();
    if is_package_uid_match(&current_package, caller_uid) {
        return current_package;
    }
    let saved_package = crate::hook::get_binder_saved_caller_package();
    if is_package_uid_match(&saved_package, caller_uid) {
        return saved_package;
    }
    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    packages.sort();
    packages.dedup();
    packages.retain(|pkg| !pkg.is_empty() && !policy::is_system_writer_package(pkg));
    let mut enabled = packages
        .into_iter()
        .filter(|pkg| SettingsHub::instance().should_redirect(pkg, caller_uid))
        .collect::<Vec<_>>();
    if enabled.len() == 1 {
        enabled.remove(0)
    } else {
        String::new()
    }
}

fn is_package_uid_match(package_name: &str, caller_uid: i32) -> bool {
    if package_name.is_empty() || caller_uid < writer::ANDROID_APP_UID_START {
        return false;
    }
    let mut package_uid = policy::get_uid_for_package(package_name);
    if package_uid < writer::ANDROID_APP_UID_START {
        policy::refresh_shared_uid_cache();
        package_uid = policy::get_uid_for_package(package_name);
    }
    package_uid == caller_uid
}

// 通过 syscall 检查路径是否存在，避免走 Hook 链路
fn path_exists_by_syscall(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };

    let ret = unsafe {
        libc::syscall(
            libc::SYS_faccessat,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::F_OK,
            0,
        ) as libc::c_int
    };
    ret == 0
}

// 前 SAMPLE_LOG_INITIAL 条全量输出，之后每 SAMPLE_LOG_INTERVAL 条采样一次
pub(super) fn sample_row_log(
    reason: &str,
    before: &str,
    after: &str,
    has_file_scheme: bool,
    is_filter: bool,
) {
    let index = REWRITE_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    if index >= SAMPLE_LOG_INITIAL
        && !(index - SAMPLE_LOG_INITIAL).is_multiple_of(SAMPLE_LOG_INTERVAL)
    {
        return;
    }

    let hub = InterceptHub::instance();
    if is_filter {
        log::debug!(
            "row filter reason={} caller={} uid={} path={} target={} file_scheme={} n={}",
            reason,
            hub.get_current_caller_package(),
            hub.get_current_caller_uid(),
            before,
            if after.is_empty() { "empty" } else { after },
            has_file_scheme,
            index + 1
        );
    } else {
        log::debug!(
            "row rewrite reason={} caller={} uid={} from={} to={} file_scheme={} n={}",
            reason,
            hub.get_current_caller_package(),
            hub.get_current_caller_uid(),
            before,
            after,
            has_file_scheme,
            index + 1
        );
    }
}

fn sample_visibility_log(
    reason: &str,
    path: &str,
    caller_package: &str,
    caller_uid: i32,
    target: &str,
    hide: bool,
) {
    if !is_visibility_log_path(path) && !is_visibility_log_path(target) {
        return;
    }
    let index = VISIBILITY_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    if index >= 96 && !(index - 96).is_multiple_of(256) {
        return;
    }
    log::debug!(
        "cursor visibility reason={} hide={} caller={} uid={} path={} target={} n={}",
        reason,
        hide,
        caller_package,
        caller_uid,
        path,
        if target.is_empty() { "empty" } else { target },
        index + 1
    );
}

fn is_visibility_log_path(path: &str) -> bool {
    path.contains("/SRXTest/")
        || path.contains("/srx_pathowner_verify")
        || path.contains("/srx_photosgo")
}

fn split_storage_path(text: &str) -> Option<(&str, bool)> {
    if text.is_empty() {
        return None;
    }

    if let Some(path_text) = text.strip_prefix(FILE_SCHEME_PREFIX)
        && STORAGE_PREFIXES
            .iter()
            .any(|prefix| path_text.starts_with(prefix))
    {
        return Some((path_text, true));
    }

    if STORAGE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return Some((text, false));
    }
    None
}

fn split_media_store_value_path(text: &str) -> Option<(&str, bool)> {
    if text.is_empty() {
        return None;
    }

    if let Some(path_text) = text.strip_prefix(FILE_SCHEME_PREFIX)
        && is_media_store_value_path(path_text)
    {
        return Some((path_text, true));
    }

    if is_media_store_value_path(text) {
        return Some((text, false));
    }
    None
}

fn is_media_store_value_path(path: &str) -> bool {
    STORAGE_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || path.starts_with("/data/media/")
}

fn is_probe_path(path: &str) -> bool {
    path.ends_with("/.srx_probe") || path.ends_with("/.srx_probe/")
}

fn is_media_store_pending_path(path: &str) -> bool {
    let path_text = path.strip_prefix(FILE_SCHEME_PREFIX).unwrap_or(path);
    path_text
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with(".pending-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, SettingsHub, UserProfile};
    use crate::redirect::{RedirectAction, RedirectDecision};
    use std::collections::HashMap;

    #[test]
    fn rewrites_mapping_rows_even_when_target_does_not_exist_yet() {
        let original = "/storage/emulated/0/Download/Nnngram/photo.jpg";
        let mapped = "/storage/emulated/0/Download/third-party/Nnngram/photo.jpg";

        let rewritten = rewrite_cursor_storage_path_inner(original, true, |_| RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: mapped.to_string(),
            is_mapping: true,
        });

        assert_eq!(rewritten.as_deref(), Some(mapped));
    }

    #[test]
    fn media_store_values_only_rewrite_explicit_mappings() {
        let original = "/storage/emulated/0/Download/Nnngram/photo.jpg";
        let fallback =
            "/data/media/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/photo.jpg";
        let mapped = "/data/media/0/Download/third-party/Nnngram/photo.jpg";

        let fallback_decision = RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: fallback.to_string(),
            is_mapping: false,
        };
        let mapped_decision = RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: mapped.to_string(),
            is_mapping: true,
        };

        assert!(
            if fallback_decision.is_mapping {
                rewrite_media_store_mapped_value(original, false, &fallback_decision.new_path)
            } else {
                None
            }
            .is_none()
        );
        assert_eq!(
            rewrite_media_store_mapped_value(original, false, &mapped_decision.new_path).as_deref(),
            Some("/storage/emulated/0/Download/third-party/Nnngram/photo.jpg")
        );
    }

    #[test]
    fn fallback_cursor_rows_do_not_use_public_display_path_as_existing_target() {
        assert!(!should_keep_existing_rewrite_target(false, false, true));
        assert!(should_keep_existing_rewrite_target(false, true, true));
        assert!(should_keep_existing_rewrite_target(true, false, true));
    }

    #[test]
    fn media_store_values_rewrite_pending_request_path_by_mapping_before_open() {
        let config = SettingsHub::instance();
        let caller_package = "com.tencent.mm";
        let caller_uid = 10284;
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
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
        runtime.set_current_caller_package(caller_package);
        runtime.set_current_caller_uid(caller_uid);

        let rewritten = rewrite_media_store_storage_path_for_caller(
            "/storage/emulated/0/Download/Weixin/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip",
            caller_uid,
        );

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            rewritten.as_deref(),
            Some(
                "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip"
            )
        );
    }

    #[test]
    fn media_provider_open_resolves_mapping_request_owner_before_fuse_create() {
        let config = SettingsHub::instance();
        let media_uid = 10217;
        let wechat_uid = 10284;
        let (previous_apps, previous_loaded) = config.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
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
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.media.module".to_string(), media_uid),
            ("com.tencent.mm".to_string(), wechat_uid),
        ]));
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
        runtime.clear_current_caller();
        runtime.set_current_caller_uid(media_uid);

        let rewritten = resolve_open_storage_path_for_caller(
            "/storage/emulated/0/Download/Weixin/.pending-1783089089-storage.redirect.x-v1.2.55-local.zip",
            media_uid,
        );

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            rewritten.as_deref(),
            Some(
                "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783089089-storage.redirect.x-v1.2.55-local.zip"
            )
        );
    }

    #[test]
    fn media_provider_open_resolves_mapping_request_owner_for_unresolved_app_uid() {
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
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::new());
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
        runtime.clear_current_caller();
        runtime.set_current_caller_uid(caller_uid);

        let rewritten = resolve_open_storage_path_for_caller(
            "/storage/emulated/0/Download/Nnngram/WAuxv-v1.2.7.r1418.e65079c-arm64.apk",
            caller_uid,
        );
        let rewritten_value = rewrite_media_store_storage_path_for_caller(
            "/storage/emulated/0/Download/Nnngram/WAuxv-v1.2.7.r1418.e65079c-arm64.apk",
            caller_uid,
        );

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            rewritten.as_deref(),
            Some(
                "/storage/emulated/0/Download/ThirdParty/Nnngram/WAuxv-v1.2.7.r1418.e65079c-arm64.apk"
            )
        );
        assert_eq!(
            rewritten_value.as_deref(),
            Some(
                "/storage/emulated/0/Download/ThirdParty/Nnngram/WAuxv-v1.2.7.r1418.e65079c-arm64.apk"
            )
        );
    }

    #[test]
    fn media_provider_values_resolve_mapping_request_owner_before_insert() {
        let config = SettingsHub::instance();
        let media_uid = 10217;
        let wechat_uid = 10284;
        let (previous_apps, previous_loaded) = config.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
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
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.media.module".to_string(), media_uid),
            ("com.tencent.mm".to_string(), wechat_uid),
        ]));
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
        runtime.clear_current_caller();
        runtime.set_current_caller_uid(media_uid);

        let rewritten = rewrite_media_store_storage_path_for_caller(
            "/storage/emulated/0/Download/Weixin/.pending-1783089089-storage.redirect.x-v1.2.55-local.zip",
            media_uid,
        );

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            rewritten.as_deref(),
            Some(
                "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783089089-storage.redirect.x-v1.2.55-local.zip"
            )
        );
    }

    #[test]
    fn query_enablement_keeps_hot_loaded_memory_config_when_raw_config_missing() {
        let config = SettingsHub::instance();
        let caller_package = "com.google.android.apps.photosgo";
        let caller_uid = 10242;
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
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
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        assert!(is_redirect_enabled_for_caller_uid(caller_uid));
        assert!(should_hide_cursor_storage_path_for_caller(
            "/storage/emulated/0/DCIM/Camera/photo.jpg",
            caller_uid,
        ));

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn cursor_visibility_keeps_read_only_real_path_rows() {
        let config = SettingsHub::instance();
        let caller_package = "com.aliyun.tongyi";
        let caller_uid = 10232;
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
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
                        read_only_paths: vec!["/storage/emulated/0/Pictures".to_string()],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        assert!(is_redirect_enabled_for_caller_uid(caller_uid));
        assert!(!should_hide_cursor_storage_path_for_caller(
            "/storage/emulated/0/Pictures/CoolMarket/20260615-143714-92a3f2.png",
            caller_uid,
        ));

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn media_store_values_rewrite_default_sandbox_backend_to_display_path() {
        let original =
            "/data/media/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/app.apk";
        let target = "/storage/emulated/0/Android/data/xyz.nextalone.nnngram/sdcard";

        assert_eq!(
            rewrite_default_sandbox_media_store_value_with_target(original, false, 0, target)
                .as_deref(),
            Some("/storage/emulated/0/Download/Nnngram/app.apk")
        );
    }

    #[test]
    fn download_media_placeholder_prefers_configured_media_mapping() {
        let source = DownloadMediaPlaceholderSource {
            path: "/storage/emulated/0/Download/AppBucket/File_1".to_string(),
            relative_path: "Download/AppBucket".to_string(),
            file_name: "File_1".to_string(),
        };
        let mappings = vec![
            PathMapping::new(
                "/storage/emulated/0/Download/AppBucket".to_string(),
                "/storage/emulated/0/Download/third-party/AppBucket".to_string(),
            ),
            PathMapping::new(
                "/storage/emulated/0/Pictures".to_string(),
                "/storage/emulated/0/DCIM".to_string(),
            ),
        ];

        assert_eq!(
            resolve_placeholder_path_by_mappings(&source, false, &mappings, 0).as_deref(),
            Some("/storage/emulated/0/DCIM/AppBucket/File_1")
        );
    }

    #[test]
    fn download_media_placeholder_uses_download_mapping_as_fallback() {
        let source = DownloadMediaPlaceholderSource {
            path: "/storage/emulated/0/Download/AppBucket/File_1".to_string(),
            relative_path: "Download/AppBucket".to_string(),
            file_name: "File_1".to_string(),
        };
        let mappings = vec![PathMapping::new(
            "/storage/emulated/0/Download/AppBucket".to_string(),
            "/storage/emulated/0/Download/third-party/AppBucket".to_string(),
        )];

        assert_eq!(
            resolve_placeholder_path_by_mappings(&source, false, &mappings, 0).as_deref(),
            Some("/storage/emulated/0/Download/third-party/AppBucket/File_1")
        );
    }

    #[test]
    fn download_media_placeholder_supports_explicit_bucket_mapping() {
        let source = DownloadMediaPlaceholderSource {
            path: "/storage/emulated/0/Download/AppBucket/File_1".to_string(),
            relative_path: "Download/AppBucket".to_string(),
            file_name: "File_1".to_string(),
        };
        let mappings = vec![PathMapping::new(
            "/storage/emulated/0/MyAlbums/AppBucket".to_string(),
            "/storage/emulated/0/DCIM/SavedBucket".to_string(),
        )];

        assert_eq!(
            resolve_placeholder_path_by_mappings(&source, false, &mappings, 0).as_deref(),
            Some("/storage/emulated/0/DCIM/SavedBucket/File_1")
        );
    }

    #[test]
    fn media_store_values_preserve_file_scheme_when_rewriting_sandbox_backend() {
        let original = "file:///data/media/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/app.apk";
        let target = "/storage/emulated/0/Android/data/xyz.nextalone.nnngram/sdcard";

        assert_eq!(
            rewrite_media_store_mapped_value(
                "/storage/emulated/0/Download/Nnngram/photo.jpg",
                false,
                "/data/media/0/Download/third-party/Nnngram/photo.jpg",
            )
            .as_deref(),
            Some("/storage/emulated/0/Download/third-party/Nnngram/photo.jpg")
        );
        assert_eq!(
            split_media_store_value_path(original),
            Some((
                "/data/media/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/app.apk",
                true
            ))
        );
        assert_eq!(
            rewrite_default_sandbox_media_store_value_with_target(
                "/data/media/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/app.apk",
                true,
                0,
                target,
            )
            .as_deref(),
            Some("file:///storage/emulated/0/Download/Nnngram/app.apk")
        );
    }

    #[test]
    fn hides_missing_probe_rows_instead_of_treating_them_as_pending() {
        let original = "/storage/emulated/0/DCIM/.srx_probe";
        let fallback =
            "/data/media/0/Android/data/com.google.android.apps.photosgo/sdcard/DCIM/.srx_probe";

        let rewritten = rewrite_cursor_storage_path_inner(original, true, |_| RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: fallback.to_string(),
            is_mapping: false,
        });

        assert_eq!(rewritten.as_deref(), Some(""));
    }

    #[test]
    fn hides_missing_mapping_view_rows_after_target_file_is_deleted() {
        let config = SettingsHub::instance();
        let caller_package = "com.example.mediaapp";
        let caller_uid = 10123;
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
                            "/storage/emulated/0/Pictures/MappedAlbum".to_string(),
                            "/storage/emulated/0/DCIM/MappedAlbum".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        let rewritten = rewrite_cursor_storage_path_for_mapping_view(
            "/storage/emulated/0/DCIM/MappedAlbum/srx_missing_photo.jpg",
            "/storage/emulated/0/DCIM/MappedAlbum/srx_missing_photo.jpg",
            false,
            caller_package,
            caller_uid,
            true,
        );

        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(rewritten.as_deref(), Some(""));
    }

    #[test]
    fn keeps_missing_pending_mapping_view_rows_for_media_store_writes() {
        let config = SettingsHub::instance();
        let caller_package = "com.example.mediaapp";
        let caller_uid = 10123;
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
                            "/storage/emulated/0/Pictures/MappedAlbum".to_string(),
                            "/storage/emulated/0/DCIM/MappedAlbum".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        let rewritten = rewrite_cursor_storage_path_for_mapping_view(
            "/storage/emulated/0/DCIM/MappedAlbum/.pending-77-photo.jpg",
            "/storage/emulated/0/DCIM/MappedAlbum/.pending-77-photo.jpg",
            false,
            caller_package,
            caller_uid,
            true,
        );

        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);

        assert_eq!(
            rewritten.as_deref(),
            Some("/storage/emulated/0/Pictures/MappedAlbum/.pending-77-photo.jpg")
        );
    }

    #[test]
    fn cursor_visibility_hides_real_files_under_mapping_request_path() {
        let config = SettingsHub::instance();
        let caller_package = "com.example.mediaapp";
        let caller_uid = 10123;
        let runtime = InterceptHub::instance();
        let previous_package = runtime.get_current_caller_package();
        let previous_uid = runtime.get_current_caller_uid();
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
                            "/storage/emulated/0/Pictures".to_string(),
                            "/storage/emulated/0/DCIM".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = policy::replace_test_uid_cache(HashMap::from([(
            caller_package.to_string(),
            caller_uid,
        )]));

        assert!(should_hide_cursor_storage_path_for_caller(
            "/storage/emulated/0/Pictures/MappedAlbum/photo.jpg",
            caller_uid,
        ));
        assert!(!should_hide_cursor_storage_path_for_caller(
            "/storage/emulated/0/Pictures/MappedAlbum/.pending-77-photo.jpg",
            caller_uid,
        ));

        runtime.set_current_caller_package(&previous_package);
        runtime.set_current_caller_uid(previous_uid);
        policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        config.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn map_only_cursor_visibility_allows_unconfigured_public_paths() {
        let hub = SettingsHub::instance();
        let caller_package = "com.eg.android.AlipayGphone";
        let caller_uid = 10123;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            caller_package.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/.xlDownload".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        assert!(should_allow_map_only_cursor_miss(
            hub,
            "/storage/emulated/0/DCIM/Camera/a.jpg",
            caller_package,
            caller_uid,
            0
        ));
        assert!(!should_allow_map_only_cursor_miss(
            hub,
            "/storage/emulated/0/.xldownload/task.jpg",
            caller_package,
            caller_uid,
            0
        ));

        hub.restore_test_apps(previous_apps, previous_loaded);
    }
}
