use super::{AppProfile, MonitorFilterConfig, SettingsState, UserProfile};
use crate::domain::{PathMapping, filter_valid_path_mapping_chains};
use crate::platform::paths;
use serde_json::Value;
use std::collections::HashMap;

pub fn parse_global_config(state: &mut SettingsState, json_content: &str) -> bool {
    state.is_fuse_fix_enabled = true;
    state.is_fuse_daemon_redirect_enabled = false;
    state.is_verbose_logging_enabled = false;
    let parsed: Value = match serde_json::from_str(json_content) {
        Ok(value) => value,
        Err(error) => {
            log::error!("global config parse err: {}", error);
            state.is_file_monitor_enabled = false;
            state.is_fuse_fix_enabled = true;
            state.is_fuse_daemon_redirect_enabled = false;
            state.is_verbose_logging_enabled = false;
            return false;
        }
    };

    if let Some(value) = parsed.get("file_monitor_enabled") {
        state.is_file_monitor_enabled = value.as_bool().unwrap_or(false);
    } else {
        state.is_file_monitor_enabled = false;
    }

    if let Some(value) = parsed.get("fuse_fix_enabled") {
        state.is_fuse_fix_enabled = value.as_bool().unwrap_or(true);
    }

    if let Some(value) = parsed.get("fuse_daemon_redirect_enabled") {
        state.is_fuse_daemon_redirect_enabled = value.as_bool().unwrap_or(false);
    }

    if let Some(value) = parsed.get("verbose_logging_enabled") {
        state.is_verbose_logging_enabled = value.as_bool().unwrap_or(false);
    }

    if state.should_log_summary {
        log::debug!(
            "global monitor={} fuse_fix={} fuse_daemon={} verbose_log={}",
            state.is_file_monitor_enabled,
            state.is_fuse_fix_enabled,
            state.is_fuse_daemon_redirect_enabled,
            state.is_verbose_logging_enabled
        );
    }
    true
}

pub fn parse_monitor_filter_config(state: &mut SettingsState, json_content: &str) -> bool {
    let parsed: Value = match serde_json::from_str(json_content) {
        Ok(value) => value,
        Err(error) => {
            log::error!("monitor filter config parse err: {}", error);
            state.monitor_filters = MonitorFilterConfig::default();
            return false;
        }
    };

    let excluded_paths = parse_monitor_path_filter_list(parsed.get("excluded_paths"));
    let excluded_operations =
        normalize_monitor_operation_defaults(parse_filter_list(parsed.get("excluded_operations")));
    state.monitor_filters = MonitorFilterConfig {
        excluded_paths,
        excluded_operations,
    };

    if state.should_log_summary {
        log::debug!(
            "monitor filters paths={} ops={}",
            state.monitor_filters.excluded_paths.len(),
            state.monitor_filters.excluded_operations.len()
        );
    }
    true
}

fn parse_filter_list(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(raw) = value.as_str() {
        push_filter_pattern(&mut out, raw);
    } else if let Some(items) = value.as_array() {
        for item in items {
            if let Some(raw) = item.as_str() {
                push_filter_pattern(&mut out, raw);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn normalize_monitor_operation_defaults(operations: Vec<String>) -> Vec<String> {
    if is_legacy_default_monitor_operation_filter(&operations) {
        return MonitorFilterConfig::default().excluded_operations;
    }
    operations
}

fn is_legacy_default_monitor_operation_filter(operations: &[String]) -> bool {
    // 旧版 1: 4 个操作的早期默认配置
    if operations.len() == 4 {
        let mut values: Vec<String> = operations
            .iter()
            .map(|value| value.to_lowercase())
            .collect();
        values.sort();
        return values
            == vec![
                "delete*".to_string(),
                "open:read".to_string(),
                "rename*".to_string(),
                "unlink*".to_string(),
            ];
    }

    // 旧版 2: 15 个操作的默认配置（包含 rename*）
    if operations.len() == 15 {
        let mut values: Vec<String> = operations
            .iter()
            .map(|value| value.to_lowercase())
            .collect();
        values.sort();
        return values
            == vec![
                "attrib*".to_string(),
                "chmod*".to_string(),
                "delete*".to_string(),
                "fchmod*".to_string(),
                "ftruncate*".to_string(),
                "futimens*".to_string(),
                "link*".to_string(),
                "open*:read".to_string(),
                "open:read".to_string(),
                "rename*".to_string(),
                "rmdir*".to_string(),
                "symlink*".to_string(),
                "truncate*".to_string(),
                "unlink*".to_string(),
                "utimens*".to_string(),
            ];
    }

    false
}

fn parse_monitor_path_filter_list(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(raw) = value.as_str() {
        push_monitor_path_filter_pattern(&mut out, raw);
    } else if let Some(items) = value.as_array() {
        for item in items {
            if let Some(raw) = item.as_str() {
                push_monitor_path_filter_pattern(&mut out, raw);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn push_monitor_path_filter_pattern(out: &mut Vec<String>, raw: &str) {
    if let Some(value) = sanitize_monitor_path_filter(raw, true) {
        out.push(value);
    }
}

fn sanitize_monitor_path_filter(raw: &str, allow_legacy_absolute: bool) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.len() > 512 {
        return None;
    }
    let mut value = collapse_filter_path_slashes(&trimmed.replace('\\', "/"));
    if value.starts_with('!') {
        return None;
    }
    if has_storage_root_prefix(value.trim_start_matches('/')) {
        return None;
    }
    if value.starts_with('/') {
        if !allow_legacy_absolute {
            return None;
        }
        value = value.trim_start_matches('/').to_string();
    }
    value = value.trim_matches('/').to_string();
    if value.is_empty()
        || value
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || has_storage_root_prefix(&value)
        || value
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '\u{0000}'..='\u{001f}'))
    {
        return None;
    }
    Some(value)
}

fn collapse_filter_path_slashes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_slash = false;
    for ch in value.chars() {
        if ch == '/' {
            if !last_was_slash {
                out.push(ch);
            }
            last_was_slash = true;
        } else {
            out.push(ch);
            last_was_slash = false;
        }
    }
    out
}

fn has_storage_root_prefix(path: &str) -> bool {
    let lower = path.trim_start_matches('/').to_ascii_lowercase();
    lower == "sdcard"
        || lower.starts_with("sdcard/")
        || lower == "storage/emulated"
        || lower.starts_with("storage/emulated/")
        || lower == "storage/self/primary"
        || lower.starts_with("storage/self/primary/")
        || lower == "data/media"
        || lower.starts_with("data/media/")
}

fn push_filter_pattern(out: &mut Vec<String>, raw: &str) {
    let value = raw.trim();
    if value.is_empty() || value.contains('\0') || value.len() > 512 {
        return;
    }
    out.push(value.to_string());
}

pub fn parse_app_config(state: &mut SettingsState, package_name: &str, json_content: &str) -> bool {
    let parsed: Value = match serde_json::from_str(json_content) {
        Ok(value) => value,
        Err(error) => {
            log::error!("app config parse err [{}]: {}", package_name, error);
            return false;
        }
    };

    let users = match parsed.get("users") {
        Some(value) if value.is_object() => value,
        _ => {
            log::warn!("app config missing users, skip: {}", package_name);
            return false;
        }
    };

    let mut app_profile = AppProfile {
        user_profiles: HashMap::new(),
    };

    let Some(users_map) = users.as_object() else {
        log::warn!("app config users not object, skip: {}", package_name);
        return false;
    };
    for (user_key, user_value) in users_map {
        let Some(user_id) = try_parse_user_id(user_key) else {
            continue;
        };
        let Some(user_obj) = user_value.as_object() else {
            continue;
        };

        let mut user_profile = UserProfile {
            is_enabled: true,
            is_mapping_mode_only: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
        };

        if let Some(enabled) = user_obj.get("enabled")
            && let Some(flag) = enabled.as_bool()
        {
            user_profile.is_enabled = flag;
        }

        if let Some(mapping_mode_only) = user_obj.get("mapping_mode_only")
            && let Some(flag) = mapping_mode_only.as_bool()
        {
            user_profile.is_mapping_mode_only = flag;
        }

        let storage_root = crate::platform::paths::storage_user_root_for_user(user_id);

        if let Some(paths_value) = user_obj.get("allowed_real_paths")
            && let Some(paths_list) = paths_value.as_array()
        {
            for item in paths_list {
                let Some(raw) = item.as_str() else {
                    continue;
                };
                let Some((is_excluded, resolved)) =
                    resolve_allowed_path_rule_for_user(raw, &storage_root)
                else {
                    log::warn!(
                        "skip allow rule (invalid, relative only): user={} path={}",
                        user_id,
                        raw
                    );
                    continue;
                };
                if resolved.is_empty() {
                    continue;
                }
                if is_excluded {
                    user_profile.excluded_real_paths.push(resolved);
                } else {
                    user_profile.allowed_real_paths.push(resolved);
                }
            }
            normalize_paths(&mut user_profile.allowed_real_paths);
            normalize_paths(&mut user_profile.excluded_real_paths);
        }

        if let Some(paths_value) = user_obj.get("excluded_real_paths")
            && let Some(paths_list) = paths_value.as_array()
        {
            for item in paths_list {
                let Some(raw) = item.as_str() else {
                    continue;
                };
                let raw = raw.strip_prefix('!').unwrap_or(raw);
                let resolved = resolve_allowed_path_for_user(raw, &storage_root);
                if resolved.is_empty() {
                    log::warn!(
                        "skip excluded rule (invalid, relative only): user={} path={}",
                        user_id,
                        raw
                    );
                    continue;
                }
                user_profile.excluded_real_paths.push(resolved);
            }
            normalize_paths(&mut user_profile.excluded_real_paths);
        }

        parse_sandboxed_paths(
            user_obj.get("sandboxed_paths"),
            user_id,
            &storage_root,
            &mut user_profile.sandboxed_paths,
        );
        parse_read_only_paths(
            user_obj.get("read_only_paths"),
            user_id,
            &storage_root,
            &mut user_profile.read_only_paths,
        );
        remove_excluded_read_only_paths(
            user_id,
            &user_profile.excluded_real_paths,
            &mut user_profile.read_only_paths,
        );
        if let Some(mappings_value) = user_obj.get("path_mappings") {
            let mut index_by_current_path: HashMap<String, usize> = HashMap::new();
            let mut upsert_mapping = |current_raw: &str, target_raw: &str| {
                let resolved_current = resolve_allowed_path_for_user(current_raw, &storage_root);
                if resolved_current.is_empty() {
                    log::warn!(
                        "skip map (current invalid, relative only): user={} path={}",
                        user_id,
                        current_raw
                    );
                    return;
                }

                let resolved_target = resolve_allowed_path_for_user(target_raw, &storage_root);
                if resolved_target.is_empty() {
                    log::warn!(
                        "skip map (target invalid, relative only): user={} path={}",
                        user_id,
                        target_raw
                    );
                    return;
                }
                if paths::is_android_data_or_obb_path(&resolved_target) {
                    log::warn!(
                        "skip map (target private): user={} cur={} target={}",
                        user_id,
                        resolved_current,
                        resolved_target
                    );
                    return;
                }

                if paths::eq_ignore_case(&resolved_current, &resolved_target) {
                    return;
                }

                let current_key = paths::match_key(&resolved_current);
                if let Some(&idx) = index_by_current_path.get(&current_key) {
                    if let Some(existing) = user_profile.path_mappings.get_mut(idx) {
                        existing.final_path = resolved_target.clone();
                    }
                    log::warn!(
                        "override map (current dup): user={} cur={}",
                        user_id,
                        resolved_current
                    );
                    return;
                }

                index_by_current_path.insert(current_key, user_profile.path_mappings.len());
                user_profile
                    .path_mappings
                    .push(PathMapping::new(resolved_current, resolved_target));
            };

            if mappings_value.is_object() {
                let Some(map) = mappings_value.as_object() else {
                    continue;
                };
                for (current_key, target_value) in map {
                    let Some(target_str) = target_value.as_str() else {
                        continue;
                    };
                    upsert_mapping(current_key, target_str);
                }
            } else if let Some(list) = mappings_value.as_array() {
                for item in list {
                    let Some(obj) = item.as_object() else {
                        continue;
                    };
                    let (Some(current_value), Some(target_value)) =
                        (obj.get("request_path"), obj.get("final_path"))
                    else {
                        continue;
                    };
                    let (Some(current_str), Some(target_str)) =
                        (current_value.as_str(), target_value.as_str())
                    else {
                        continue;
                    };
                    upsert_mapping(current_str, target_str);
                }
            } else {
                log::warn!(
                    "skip mappings (unsupported type): pkg={} user={}",
                    package_name,
                    user_id
                );
            }
        }

        user_profile.path_mappings = filter_valid_path_mapping_chains(user_profile.path_mappings);
        app_profile.user_profiles.insert(user_id, user_profile);
    }

    if app_profile.user_profiles.is_empty() {
        log::warn!("app config users empty, skip: {}", package_name);
        return false;
    }

    state.apps.insert(package_name.to_string(), app_profile);
    if state.should_log_summary {
        log::debug!(
            "app loaded: {} users={}",
            package_name,
            state
                .apps
                .get(package_name)
                .map(|app| app.user_profiles.len())
                .unwrap_or(0)
        );
    }
    true
}

// 仅接受纯数字
fn try_parse_user_id(value: &str) -> Option<i32> {
    if value.is_empty() {
        return None;
    }
    if !value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let parsed = value.parse::<i32>().ok()?;
    if parsed < 0 { None } else { Some(parsed) }
}

fn normalize_paths(path_list: &mut Vec<String>) {
    if path_list.is_empty() {
        return;
    }
    paths::sort_dedup_paths_case_insensitive(path_list);
}

fn parse_sandboxed_paths(
    value: Option<&Value>,
    user_id: i32,
    storage_root: &str,
    sandboxed_paths: &mut Vec<String>,
) {
    let Some(value) = value else {
        return;
    };

    if let Some(raw) = value.as_str() {
        push_sandboxed_path(raw, user_id, storage_root, sandboxed_paths);
    } else if let Some(list) = value.as_array() {
        for item in list {
            if let Some(raw) = item.as_str() {
                push_sandboxed_path(raw, user_id, storage_root, sandboxed_paths);
            }
        }
    } else {
        log::warn!("skip sandboxed_paths (unsupported type): user={}", user_id);
        return;
    }

    normalize_paths(sandboxed_paths);
}

fn push_sandboxed_path(
    raw: &str,
    user_id: i32,
    storage_root: &str,
    sandboxed_paths: &mut Vec<String>,
) {
    let resolved = resolve_allowed_path_for_user(raw, storage_root);
    if resolved.is_empty() {
        log::warn!(
            "skip sandbox path (invalid, relative only): user={} path={}",
            user_id,
            raw
        );
        return;
    }
    sandboxed_paths.push(resolved);
}

fn parse_read_only_paths(
    value: Option<&Value>,
    user_id: i32,
    storage_root: &str,
    read_only_paths: &mut Vec<String>,
) {
    let Some(value) = value else {
        return;
    };

    if let Some(raw) = value.as_str() {
        push_read_only_path(raw, user_id, storage_root, read_only_paths);
    } else if let Some(list) = value.as_array() {
        for item in list {
            if let Some(raw) = item.as_str() {
                push_read_only_path(raw, user_id, storage_root, read_only_paths);
            }
        }
    } else {
        log::warn!("skip read_only_paths (unsupported type): user={}", user_id);
        return;
    }

    normalize_paths(read_only_paths);
}

fn push_read_only_path(
    raw: &str,
    user_id: i32,
    storage_root: &str,
    read_only_paths: &mut Vec<String>,
) {
    let Some((is_excluded, resolved)) = resolve_allowed_path_rule_for_user(raw, storage_root)
    else {
        log::warn!(
            "skip read-only path (invalid, relative only): user={} path={}",
            user_id,
            raw
        );
        return;
    };
    let resolved = if is_excluded {
        format!("!{resolved}")
    } else {
        resolved
    };
    read_only_paths.push(resolved);
}

fn remove_excluded_read_only_paths(
    user_id: i32,
    excluded_real_paths: &[String],
    read_only_paths: &mut Vec<String>,
) {
    if excluded_real_paths.is_empty() || read_only_paths.is_empty() {
        return;
    }

    read_only_paths.retain(|path| {
        if path.trim_start().starts_with('!') {
            return true;
        }
        let is_excluded = excluded_real_paths
            .iter()
            .any(|excluded| paths::matches(excluded, path, true));
        if is_excluded {
            log::warn!(
                "skip read-only path (excluded conflict): user={} path={}",
                user_id,
                path
            );
        }
        !is_excluded
    });
}

// ! 前缀表示排除规则，返回 (is_excluded, 绝对路径)
fn resolve_allowed_path_rule_for_user(
    raw_path: &str,
    storage_root: &str,
) -> Option<(bool, String)> {
    let raw_trimmed = raw_path.trim();
    if raw_trimmed.is_empty() {
        return None;
    }

    let (is_excluded, path_body) = if let Some(stripped) = raw_trimmed.strip_prefix('!') {
        (true, stripped.trim_start())
    } else {
        (false, raw_trimmed)
    };

    let mut normalized = paths::normalize(path_body);
    if normalized.is_empty() {
        return None;
    }
    if paths::has_unsafe_segments(&normalized) {
        return None;
    }
    if normalized.starts_with('/') {
        return None;
    }

    normalized = paths::join(storage_root, &normalized);
    normalized = paths::normalize(&normalized);
    if paths::eq_ignore_case(&normalized, storage_root) {
        return None;
    }
    if !paths::is_child(&normalized, storage_root) {
        return None;
    }

    Some((is_excluded, normalized))
}

// 拒绝 ! 排除前缀，只接受普通相对路径
fn resolve_allowed_path_for_user(raw_path: &str, storage_root: &str) -> String {
    let Some((is_excluded, resolved)) = resolve_allowed_path_rule_for_user(raw_path, storage_root)
    else {
        return String::new();
    };
    if is_excluded {
        return String::new();
    }
    resolved
}
