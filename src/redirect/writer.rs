use crate::config::{ResolvedUserProfile, SettingsHub};
use crate::domain::{
    PathMapping, filter_valid_path_mapping_chains, sort_path_mappings_longest_request_first,
};
use crate::platform::{self, paths};
use crate::redirect::policy;
use std::cell::RefCell;
use std::collections::HashSet;

pub const ANDROID_APP_UID_START: i32 = 10000;
const DOWNLOAD_PROVIDER: &str = "com.android.providers.downloads";
const DOWNLOAD_PROVIDER_UI: &str = "com.android.providers.downloads.ui";

pub fn data_media_to_storage_path(path: &str) -> String {
    paths::data_media_to_storage_path(path)
}

pub fn storage_to_data_media_path(path: &str) -> String {
    paths::storage_to_data_media_path(path)
}

pub fn get_caller_mappings(caller_package: &str, caller_uid: i32) -> Vec<PathMapping> {
    CALLER_MAPPING_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let config_version = SettingsHub::instance().config_version();
        if cache.package_name == caller_package
            && cache.caller_uid == caller_uid
            && cache.config_version == config_version
        {
            return cache.mappings.clone();
        }
        cache.package_name = caller_package.to_string();
        cache.caller_uid = caller_uid;
        cache.config_version = config_version;
        cache.mappings = build_caller_mappings(caller_package, caller_uid);
        cache.mappings.clone()
    })
}

pub fn map_path_by_caller_mappings(path: &str, mappings: &[PathMapping]) -> String {
    for mapping in mappings {
        if let Some(suffix) = paths::child_suffix(path, &mapping.request_path) {
            if suffix.is_empty() {
                return mapping.final_path.clone();
            }
            return format!("{}{}", mapping.final_path, suffix);
        }
    }
    String::new()
}

pub fn reverse_map_path_by_caller_mappings(path: &str, mappings: &[PathMapping]) -> String {
    for mapping in mappings {
        if let Some(suffix) = paths::child_suffix(path, &mapping.final_path) {
            if suffix.is_empty() {
                return mapping.request_path.clone();
            }
            return format!("{}{}", mapping.request_path, suffix);
        }
    }
    String::new()
}

/// 将 readlink 返回的沙箱路径反向映射回展示路径。
/// 移除 /Android/data/<pkg>/sdcard 沙箱段，并将 /data/media/ 转回
/// /storage/emulated/，使调用方看到传给 open() 的原始展示路径。
///
/// 输入：/data/media/0/Android/data/com.example/sdcard/Download/file.txt
/// 输出：/storage/emulated/0/Download/file.txt
pub fn reverse_readlink_sandbox_path(path: &str) -> String {
    let storage_path = data_media_to_storage_path(path);
    if let Some(pos) = storage_path.find("/Android/data/") {
        let after = &storage_path[pos + "/Android/data/".len()..];
        if let Some(slash) = after.find('/') {
            let tail = &after[slash..];
            if let Some(stripped) = tail.strip_prefix("/sdcard") {
                return format!("{}{}", &storage_path[..pos], stripped);
            }
        }
    }
    storage_path
}

pub fn is_path_in_caller_mapping_request(path: &str, mappings: &[PathMapping]) -> bool {
    mappings
        .iter()
        .any(|mapping| paths::child_suffix(path, &mapping.request_path).is_some())
}

pub fn is_path_allowed_by_caller_real_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if resolved_path.is_empty() {
        return false;
    }

    caller_path_list_matches(
        &get_caller_allowed_real_paths(caller_package, caller_uid),
        resolved_path,
        false,
    )
}

pub fn is_path_excluded_by_caller_real_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if resolved_path.is_empty() {
        return false;
    }

    caller_path_list_matches(
        &get_caller_excluded_real_paths(caller_package, caller_uid),
        resolved_path,
        false,
    )
}

pub fn is_path_sandboxed_by_caller_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if resolved_path.is_empty() {
        return false;
    }

    caller_path_list_matches(
        &get_caller_sandboxed_paths(caller_package, caller_uid),
        resolved_path,
        true,
    )
}

pub fn is_path_read_only_by_caller_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if resolved_path.is_empty() {
        return false;
    }

    caller_path_list_matches(
        &get_caller_read_only_paths(caller_package, caller_uid),
        resolved_path,
        true,
    ) && !caller_path_list_matches(
        &get_caller_read_only_excluded_paths(caller_package, caller_uid),
        resolved_path,
        true,
    )
}

pub fn is_path_read_only_excluded_by_caller_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    if resolved_path.is_empty() {
        return false;
    }

    caller_path_list_matches(
        &get_caller_read_only_excluded_paths(caller_package, caller_uid),
        resolved_path,
        true,
    )
}

pub fn is_path_or_mapped_target_read_only_by_caller_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> bool {
    !read_only_check_path_by_caller_paths(resolved_path, caller_package, caller_uid).is_empty()
}

pub fn read_only_check_path_by_caller_paths(
    resolved_path: &str,
    caller_package: &str,
    caller_uid: i32,
) -> String {
    let mappings = get_caller_mappings(caller_package, caller_uid);
    let mapped_path = map_path_by_caller_mappings(resolved_path, &mappings);
    if !mapped_path.is_empty() && mapped_path != resolved_path {
        if is_caller_path_read_only(&mapped_path, caller_package, caller_uid) {
            return mapped_path;
        }
        return String::new();
    }

    if is_caller_path_read_only(resolved_path, caller_package, caller_uid) {
        return resolved_path.to_string();
    }
    String::new()
}

pub(crate) fn is_caller_path_read_only(path: &str, caller_package: &str, caller_uid: i32) -> bool {
    !is_path_excluded_by_caller_real_paths(path, caller_package, caller_uid)
        && is_path_read_only_by_caller_paths(path, caller_package, caller_uid)
}

fn caller_path_list_matches(
    configured_paths: &[String],
    resolved_path: &str,
    include_xldownload_alias: bool,
) -> bool {
    let pending_display_path = media_store_pending_display_path(resolved_path);
    configured_paths.iter().any(|configured| {
        !configured.is_empty()
            && (caller_path_matches(configured, resolved_path, include_xldownload_alias)
                || pending_display_path.as_deref().is_some_and(|display_path| {
                    caller_path_matches(configured, display_path, include_xldownload_alias)
                }))
    })
}

fn caller_path_matches(
    configured: &str,
    resolved_path: &str,
    include_xldownload_alias: bool,
) -> bool {
    paths::matches(configured, resolved_path, true)
        || (include_xldownload_alias && paths::matches_xldownload_alias(configured, resolved_path))
}

fn media_store_pending_display_path(resolved_path: &str) -> Option<String> {
    let slash = resolved_path.rfind('/')?;
    let file_name = &resolved_path[slash + 1..];
    let pending_tail = file_name.strip_prefix(".pending-")?;
    let display_name_start = pending_tail.find('-')? + 1;
    if display_name_start >= pending_tail.len() {
        return None;
    }

    Some(format!(
        "{}/{}",
        resolved_path[..slash].trim_end_matches('/'),
        &pending_tail[display_name_start..]
    ))
}

// 无映射命中时的 fallback：原路 → redirect_target
pub fn map_path_by_caller_fallback(
    normalized_path: &str,
    redirect_target: &str,
    user_id: i32,
) -> String {
    if normalized_path.is_empty() || redirect_target.is_empty() {
        return String::new();
    }

    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::is_same_or_child(normalized_path, redirect_target) {
        return String::new();
    }

    if paths::eq_ignore_case(normalized_path, &storage_root) {
        return redirect_target.to_string();
    }

    let Some(suffix) = paths::child_suffix(normalized_path, &storage_root) else {
        return String::new();
    };
    if suffix.is_empty() {
        return redirect_target.to_string();
    }

    let fallback = format!("{}{}", redirect_target, suffix);
    if paths::has_unsafe_segments(&fallback) {
        return String::new();
    }
    fallback
}

pub fn resolve_system_writer_redirect_target(
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
    is_caller_from_inferred_mapping: bool,
) -> String {
    if caller_package.is_empty() || user_id < 0 {
        return String::new();
    }

    let target = get_caller_default_redirect_target(caller_package, caller_uid);
    if !target.is_empty() {
        return target;
    }

    let config = SettingsHub::instance();
    let raw_enabled = config.has_enabled_user_profile_in_raw_config(caller_package, user_id);
    if !is_caller_from_inferred_mapping && !raw_enabled {
        return String::new();
    }

    let mut target = paths::default_redirect_target(caller_package, user_id);
    target = paths::resolve_user_path(&paths::normalize(&target), user_id);
    if target.is_empty() || paths::has_unsafe_segments(&target) {
        return String::new();
    }

    log::debug!(
        "writer force default caller={} uid={} reason={} target={}",
        caller_package,
        caller_uid,
        if is_caller_from_inferred_mapping {
            "inferred"
        } else {
            "config"
        },
        target
    );
    target
}

// 低 UID 时从路径反推 user_id，并回填 effective_caller_uid
pub fn resolve_system_writer_user_id(normalized_path: &str, effective_caller_uid: &mut i32) -> i32 {
    if *effective_caller_uid >= ANDROID_APP_UID_START {
        return platform::user_id_from_uid(*effective_caller_uid);
    }

    let user_id = paths::extract_user_id_from_storage_path(normalized_path);
    if user_id >= 0 {
        *effective_caller_uid = user_id * platform::ANDROID_USER_ID_OFFSET + ANDROID_APP_UID_START;
    }
    user_id
}

// 路径命中已配置应用时改写 caller_package，避免错误按原调用方重定向
pub fn maybe_override_system_writer_caller_by_path(
    normalized_path: &str,
    effective_caller_uid: &mut i32,
    self_uid: i32,
    user_id: i32,
    effective_caller_package: &mut String,
    is_caller_from_inferred_mapping: &mut bool,
) {
    if user_id < 0 {
        return;
    }
    if crate::hook::is_path_owner_inference_disabled() {
        return;
    }

    let config = SettingsHub::instance();
    let inferred =
        config.resolve_redirect_package_by_path_for_user(*effective_caller_uid, normalized_path);
    if inferred.is_empty() {
        return;
    }

    let inferred_uid = policy::get_fresh_uid_for_package(&inferred);
    if effective_caller_package.is_empty()
        && *effective_caller_uid >= ANDROID_APP_UID_START
        && *effective_caller_uid != self_uid
        && inferred_uid != *effective_caller_uid
    {
        log::debug!(
            "writer path override skip explicit_uid uid={} inferred={} inferred_uid={} path={}",
            *effective_caller_uid,
            inferred,
            inferred_uid,
            normalized_path
        );
        return;
    }

    let mut should_replace = effective_caller_package.is_empty();
    if !should_replace && policy::is_system_writer_package(effective_caller_package) {
        should_replace = true;
    }

    if !should_replace {
        return;
    }

    if inferred_uid >= ANDROID_APP_UID_START {
        *effective_caller_uid = inferred_uid;
    }

    log::debug!(
        "writer path override caller={} uid={} path={}",
        inferred,
        *effective_caller_uid,
        normalized_path
    );
    *effective_caller_package = inferred;
    *is_caller_from_inferred_mapping = true;
}

pub fn is_path_in_user_storage(resolved_path: &str, user_id: i32) -> bool {
    if resolved_path.is_empty() || user_id < 0 {
        return false;
    }

    paths::is_child(resolved_path, &paths::storage_user_root_for_user(user_id))
}

pub fn log_system_writer_caller_unresolved(
    hub_package: &str,
    effective_caller_uid: i32,
    pathname: &str,
) {
    let count = SYSTEM_WRITER_CALLER_MISS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !should_log_every_step(count) {
        return;
    }
    log::debug!(
        "writer caller unresolved proc={} uid={} path={} n={}",
        hub_package,
        effective_caller_uid,
        pathname,
        count
    );
}

pub fn log_system_writer_user_unresolved(
    caller_package: &str,
    effective_caller_uid: i32,
    pathname: &str,
) {
    let count = SYSTEM_WRITER_CALLER_MISS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !should_log_every_step(count) {
        return;
    }
    log::debug!(
        "writer user unresolved caller={} uid={} path={} n={}",
        caller_package,
        effective_caller_uid,
        pathname,
        count
    );
}

pub fn log_system_writer_redirect_disabled(
    caller_package: &str,
    effective_caller_uid: i32,
    pathname: &str,
) {
    let count =
        SYSTEM_WRITER_CALLER_DISABLED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !should_log_every_step(count) {
        return;
    }
    log::debug!(
        "writer redirect disabled caller={} uid={} path={} n={}",
        caller_package,
        effective_caller_uid,
        pathname,
        count
    );
}

pub fn log_system_writer_skip_path_infer_for_low_uid(original_caller_uid: i32, pathname: &str) {
    let count =
        SYSTEM_WRITER_PATH_INFER_SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !should_log_every_step(count) {
        return;
    }
    log::debug!(
        "writer skip path infer low-uid uid={} path={} n={}",
        original_caller_uid,
        pathname,
        count
    );
}

// 过滤跨存储/越界/同一路径/Android/data 与 Android/obb 及重复项
fn build_caller_mappings(caller_package: &str, caller_uid: i32) -> Vec<PathMapping> {
    if caller_package.is_empty() || caller_uid < ANDROID_APP_UID_START {
        return Vec::new();
    }

    let config = SettingsHub::instance();
    let raw_mappings = get_effective_path_mappings(config, caller_package, caller_uid);
    if raw_mappings.is_empty() {
        return Vec::new();
    }

    let user_id = platform::user_id_from_uid(caller_uid);
    let storage_root = paths::storage_user_root_for_user(user_id);

    let mut seen_current: HashSet<String> = HashSet::new();
    let mut mappings: Vec<PathMapping> = Vec::new();

    for mapping in raw_mappings {
        let current_path =
            paths::resolve_user_path(&paths::normalize(&mapping.request_path), user_id);
        let target_path = paths::resolve_user_path(&paths::normalize(&mapping.final_path), user_id);

        if current_path.is_empty() || target_path.is_empty() {
            continue;
        }
        if paths::has_unsafe_segments(&current_path) || paths::has_unsafe_segments(&target_path) {
            continue;
        }
        if paths::eq_ignore_case(&current_path, &target_path) {
            continue;
        }
        if !paths::is_child(&current_path, &storage_root)
            || !paths::is_child(&target_path, &storage_root)
        {
            continue;
        }
        if paths::is_android_data_or_obb_path(&target_path) {
            continue;
        }
        if !seen_current.insert(paths::match_key(&current_path)) {
            continue;
        }
        mappings.push(PathMapping::new(current_path, target_path));
    }

    sort_path_mappings_longest_request_first(&mut mappings);
    filter_valid_path_mapping_chains(mappings)
}

fn get_caller_default_redirect_target(caller_package: &str, caller_uid: i32) -> String {
    CALLER_TARGET_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let config_version = SettingsHub::instance().config_version();
        if cache.package_name == caller_package
            && cache.caller_uid == caller_uid
            && cache.config_version == config_version
        {
            return cache.redirect_target.clone();
        }

        cache.package_name = caller_package.to_string();
        cache.caller_uid = caller_uid;
        cache.config_version = config_version;
        cache.redirect_target.clear();

        if caller_package.is_empty() || caller_uid < ANDROID_APP_UID_START {
            return cache.redirect_target.clone();
        }

        let config = SettingsHub::instance();
        let Some(profile) = config.get_resolved_user_profile_snapshot(caller_package, caller_uid)
        else {
            return cache.redirect_target.clone();
        };

        let resolved =
            paths::resolve_user_path(&paths::normalize(&profile.redirect_target), profile.user_id);
        if !resolved.is_empty() && !paths::has_unsafe_segments(&resolved) {
            cache.redirect_target = resolved;
        }
        cache.redirect_target.clone()
    })
}

fn get_caller_allowed_real_paths(caller_package: &str, caller_uid: i32) -> Vec<String> {
    get_cached_caller_real_paths(caller_package, caller_uid, CallerRealPathKind::Allowed)
}

fn get_caller_excluded_real_paths(caller_package: &str, caller_uid: i32) -> Vec<String> {
    get_cached_caller_real_paths(caller_package, caller_uid, CallerRealPathKind::Excluded)
}

fn get_caller_sandboxed_paths(caller_package: &str, caller_uid: i32) -> Vec<String> {
    get_cached_caller_real_paths(caller_package, caller_uid, CallerRealPathKind::Sandboxed)
}

fn get_caller_read_only_paths(caller_package: &str, caller_uid: i32) -> Vec<String> {
    get_cached_caller_real_paths(caller_package, caller_uid, CallerRealPathKind::ReadOnly)
}

fn get_caller_read_only_excluded_paths(caller_package: &str, caller_uid: i32) -> Vec<String> {
    get_cached_caller_real_paths(
        caller_package,
        caller_uid,
        CallerRealPathKind::ReadOnlyExcluded,
    )
}

fn get_cached_caller_real_paths(
    caller_package: &str,
    caller_uid: i32,
    kind: CallerRealPathKind,
) -> Vec<String> {
    CALLER_ALLOWED_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let config_version = SettingsHub::instance().config_version();

        if !cache.is_valid_for(caller_package, caller_uid, config_version) {
            cache.config_version = config_version;
            refresh_caller_real_paths_cache(&mut cache, caller_package, caller_uid);
        }

        cache.paths(kind).to_vec()
    })
}

fn refresh_caller_real_paths_cache(
    cache: &mut CallerAllowedCache,
    caller_package: &str,
    caller_uid: i32,
) {
    cache.package_name = caller_package.to_string();
    cache.caller_uid = caller_uid;
    cache.is_loaded = true;
    cache.allowed_real_paths.clear();
    cache.excluded_real_paths.clear();
    cache.sandboxed_paths.clear();
    cache.read_only_paths.clear();
    cache.read_only_excluded_paths.clear();

    if caller_package.is_empty() || caller_uid < ANDROID_APP_UID_START {
        return;
    }

    let config = SettingsHub::instance();
    let expand_mount_fallbacks = !config.is_fuse_daemon_redirect_enabled();
    if let Some(profile) = get_effective_profile(config, caller_package, caller_uid) {
        cache.allowed_real_paths = expand_wildcard_mount_fallback_rules(
            profile.allowed_real_paths,
            caller_uid,
            expand_mount_fallbacks,
        );
        cache.excluded_real_paths = profile.excluded_real_paths;
        cache.sandboxed_paths = profile.sandboxed_paths;
        cache.read_only_paths = expand_wildcard_mount_fallback_rules(
            profile.read_only_paths,
            caller_uid,
            expand_mount_fallbacks,
        );
    }
    let user_id = platform::user_id_from_uid(caller_uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    append_raw_profile_paths(
        &mut cache.sandboxed_paths,
        config.get_user_sandboxed_paths_in_raw_config(caller_package, user_id),
        user_id,
        &storage_root,
        false,
        false,
    );
    paths::sort_dedup_paths_case_insensitive(&mut cache.sandboxed_paths);

    let raw_expand_mount_fallbacks = !config.is_fuse_daemon_redirect_enabled();
    append_raw_profile_paths(
        &mut cache.read_only_paths,
        config.get_user_read_only_paths_in_raw_config(caller_package, user_id),
        user_id,
        &storage_root,
        true,
        raw_expand_mount_fallbacks,
    );
    paths::sort_dedup_paths_case_insensitive(&mut cache.read_only_paths);
    let (included_read_only_paths, excluded_read_only_paths) =
        paths::split_exclusion_rules(&cache.read_only_paths);
    let excluded_read_only_paths =
        paths::overlapping_exclusion_rules(&included_read_only_paths, &excluded_read_only_paths);
    cache.read_only_paths = included_read_only_paths;
    cache.read_only_excluded_paths = excluded_read_only_paths;
}

fn append_raw_profile_paths(
    target: &mut Vec<String>,
    raw_paths: Vec<String>,
    user_id: i32,
    storage_root: &str,
    allow_wildcards: bool,
    expand_mount_fallbacks: bool,
) {
    for raw_path in raw_paths {
        let raw_path = raw_path.trim();
        let (excluded, body) = if let Some(stripped) = raw_path.strip_prefix('!') {
            (true, stripped.trim_start())
        } else {
            (false, raw_path)
        };
        if excluded && !allow_wildcards {
            continue;
        }
        let resolved = paths::resolve_user_path(&paths::normalize(body), user_id);
        if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
            continue;
        }
        if !allow_wildcards && paths::contains_wildcards(&resolved) {
            continue;
        }
        if !paths::is_child(&resolved, storage_root) {
            continue;
        }
        append_real_path_rule(
            target,
            resolved,
            excluded,
            storage_root,
            expand_mount_fallbacks && !excluded,
        );
    }
}

fn expand_wildcard_mount_fallback_rules(
    rules: Vec<String>,
    caller_uid: i32,
    expand_mount_fallbacks: bool,
) -> Vec<String> {
    if !expand_mount_fallbacks {
        return rules;
    }

    let user_id = platform::user_id_from_uid(caller_uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut expanded = Vec::with_capacity(rules.len());
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (excluded, body) = if let Some(stripped) = trimmed.strip_prefix('!') {
            (true, stripped.trim_start().to_string())
        } else {
            (false, trimmed.to_string())
        };
        append_real_path_rule(&mut expanded, body, excluded, &storage_root, !excluded);
    }
    paths::sort_dedup_paths_case_insensitive(&mut expanded);
    expanded
}

fn append_real_path_rule(
    target: &mut Vec<String>,
    resolved: String,
    excluded: bool,
    storage_root: &str,
    expand_mount_fallbacks: bool,
) {
    let resolved = if expand_mount_fallbacks && paths::contains_wildcards(&resolved) {
        if let Some(fallback) = paths::wildcard_policy_fallback_parent(&resolved, storage_root) {
            log::warn!(
                "writer fallback wildcard to mount parent: {} -> {}",
                resolved,
                fallback
            );
            fallback
        } else {
            resolved
        }
    } else {
        resolved
    };

    target.push(if excluded {
        format!("!{resolved}")
    } else {
        resolved
    });
}

fn normalize_download_provider_package(package_name: &str) -> &str {
    if package_name == DOWNLOAD_PROVIDER {
        DOWNLOAD_PROVIDER_UI
    } else {
        package_name
    }
}

fn get_effective_profile(
    config: &SettingsHub,
    caller_package: &str,
    caller_uid: i32,
) -> Option<ResolvedUserProfile> {
    let profile = config.get_resolved_user_profile_snapshot(caller_package, caller_uid);
    if caller_package != DOWNLOAD_PROVIDER {
        return profile;
    }
    let fallback = config.get_resolved_user_profile_snapshot(
        normalize_download_provider_package(caller_package),
        caller_uid,
    );

    match (profile, fallback) {
        (Some(mut primary), Some(fallback)) => {
            if primary.allowed_real_paths.is_empty() {
                primary.allowed_real_paths = fallback.allowed_real_paths;
            }
            if primary.excluded_real_paths.is_empty() {
                primary.excluded_real_paths = fallback.excluded_real_paths;
            }
            if primary.sandboxed_paths.is_empty() {
                primary.sandboxed_paths = fallback.sandboxed_paths;
            }
            if primary.read_only_paths.is_empty() {
                primary.read_only_paths = fallback.read_only_paths;
            }
            if primary.path_mappings.is_empty() {
                primary.path_mappings = fallback.path_mappings;
            }
            Some(primary)
        }
        (Some(primary), None) => Some(primary),
        (None, fallback) => fallback,
    }
}

fn get_effective_path_mappings(
    config: &SettingsHub,
    caller_package: &str,
    caller_uid: i32,
) -> Vec<PathMapping> {
    let profile = get_effective_profile(config, caller_package, caller_uid);
    if let Some(profile) = profile {
        return profile.path_mappings;
    }
    Vec::new()
}

// 首次和每 256 次输出一次
fn should_log_every_step(count: u64) -> bool {
    count == 1 || count.is_multiple_of(256)
}

struct CallerMappingCache {
    package_name: String,
    caller_uid: i32,
    config_version: u64,
    mappings: Vec<PathMapping>,
}

struct CallerTargetCache {
    package_name: String,
    caller_uid: i32,
    config_version: u64,
    redirect_target: String,
}

struct CallerAllowedCache {
    package_name: String,
    caller_uid: i32,
    config_version: u64,
    is_loaded: bool,
    allowed_real_paths: Vec<String>,
    excluded_real_paths: Vec<String>,
    sandboxed_paths: Vec<String>,
    read_only_paths: Vec<String>,
    read_only_excluded_paths: Vec<String>,
}

#[derive(Clone, Copy)]
enum CallerRealPathKind {
    Allowed,
    Excluded,
    Sandboxed,
    ReadOnly,
    ReadOnlyExcluded,
}

impl CallerAllowedCache {
    fn is_valid_for(&self, package_name: &str, caller_uid: i32, config_version: u64) -> bool {
        self.package_name == package_name
            && self.caller_uid == caller_uid
            && self.config_version == config_version
            && self.is_loaded
    }

    fn paths(&self, kind: CallerRealPathKind) -> &[String] {
        match kind {
            CallerRealPathKind::Allowed => &self.allowed_real_paths,
            CallerRealPathKind::Excluded => &self.excluded_real_paths,
            CallerRealPathKind::Sandboxed => &self.sandboxed_paths,
            CallerRealPathKind::ReadOnly => &self.read_only_paths,
            CallerRealPathKind::ReadOnlyExcluded => &self.read_only_excluded_paths,
        }
    }
}

thread_local! {
    static CALLER_MAPPING_CACHE: RefCell<CallerMappingCache> = const { RefCell::new(CallerMappingCache {
        package_name: String::new(),
        caller_uid: -1,
        config_version: 0,
        mappings: Vec::new(),
    }) };
    static CALLER_TARGET_CACHE: RefCell<CallerTargetCache> = const { RefCell::new(CallerTargetCache {
        package_name: String::new(),
        caller_uid: -1,
        config_version: 0,
        redirect_target: String::new(),
    }) };
    static CALLER_ALLOWED_CACHE: RefCell<CallerAllowedCache> = const { RefCell::new(CallerAllowedCache {
        package_name: String::new(),
        caller_uid: -1,
        config_version: 0,
        is_loaded: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        read_only_excluded_paths: Vec::new(),
    }) };
}

static SYSTEM_WRITER_CALLER_MISS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SYSTEM_WRITER_CALLER_DISABLED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SYSTEM_WRITER_PATH_INFER_SKIPPED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
