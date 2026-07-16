use super::SettingsHub;
use crate::domain::{PathMapping, sort_path_mappings_longest_request_first};
use crate::platform::{self, paths};
use crate::redirect::policy;
use std::collections::{HashMap, HashSet};

const SELF_PACKAGE_NAME: &str = "com.storage.redirect.x";
const DOWNLOAD_PROVIDER_PACKAGE: &str = "com.android.providers.downloads";
const DOWNLOAD_PROVIDER_UI_PACKAGE: &str = "com.android.providers.downloads.ui";

#[derive(Clone, Copy)]
enum PackagePathMatchMode {
    Redirect,
    Enabled,
    MappingRequest,
    Monitor,
}

fn is_public_storage_collection_root(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        "alarms"
            | "android"
            | "audiobooks"
            | "dcim"
            | "documents"
            | "download"
            | "downloads"
            | "movies"
            | "music"
            | "notifications"
            | "pictures"
            | "podcasts"
            | "recordings"
            | "ringtones"
    )
}

fn is_specific_storage_owner_hint(user_id: i32, path: &str) -> bool {
    let resolved_path = resolve_mapping_storage_path_for_user(user_id, path);
    let Some(relative_path) = paths::storage_relative_path_for_user(&resolved_path, user_id) else {
        return false;
    };

    let mut segments = relative_path
        .split('/')
        .filter(|segment| !segment.is_empty());
    let Some(first_segment) = segments.next() else {
        return false;
    };

    segments.next().is_some() || !is_public_storage_collection_root(first_segment)
}

fn is_specific_mapping_request_owner_hint(user_id: i32, request_path: &str) -> bool {
    is_specific_storage_owner_hint(user_id, request_path)
}

fn resolve_mapping_storage_path_for_user(user_id: i32, path: &str) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut resolved = paths::resolve_user_path(&paths::normalize(path), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return String::new();
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(&storage_root, &resolved));
    }
    if paths::eq_ignore_case(&resolved, &storage_root) || !paths::is_child(&resolved, &storage_root)
    {
        return String::new();
    }
    resolved
}

fn is_valid_mapping_target_for_redirect_owner(user_id: i32, final_path: &str) -> bool {
    let resolved_path = resolve_mapping_storage_path_for_user(user_id, final_path);
    !resolved_path.is_empty() && !paths::is_android_data_or_obb_path(&resolved_path)
}

fn update_matched_packages(
    matched_packages: &mut Vec<String>,
    matched_prefix_len: &mut usize,
    package_name: &str,
    rule: &str,
    normalized: &str,
) {
    if rule.is_empty() || !paths::matches(rule, normalized, true) {
        return;
    }

    let prefix_len = rule.len();
    if prefix_len < *matched_prefix_len {
        return;
    }
    if prefix_len > *matched_prefix_len {
        matched_packages.clear();
        matched_packages.push(package_name.to_string());
        *matched_prefix_len = prefix_len;
        return;
    }
    if !matched_packages
        .iter()
        .any(|matched| matched == package_name)
    {
        matched_packages.push(package_name.to_string());
    }
}

impl SettingsHub {
    pub fn get_merged_path_mappings_for_user(&self, app_uid: i32) -> Vec<PathMapping> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded {
            return Vec::new();
        }

        let user_id = platform::user_id_from_uid(app_uid);
        let mut merged: Vec<PathMapping> = Vec::new();
        let mut target_by_current: HashMap<String, String> = HashMap::new();
        let mut applied_shared_keys: HashSet<String> = HashSet::new();

        for (package_name, app) in &state.apps {
            if policy::is_shared_group_package(package_name) {
                let mut members = policy::get_shared_group_members(package_name);
                if members.is_empty() {
                    continue;
                }
                members.sort();
                let group_key = members.join("|");
                if !group_key.is_empty() && applied_shared_keys.contains(&group_key) {
                    continue;
                }
                if !super::consensus::is_shared_group_consistent(
                    &state.apps,
                    package_name,
                    user_id,
                    true,
                ) {
                    continue;
                }
                if !group_key.is_empty() {
                    applied_shared_keys.insert(group_key);
                }
            }

            let user = match app.user_profiles.get(&user_id) {
                Some(user) if user.is_enabled => user,
                _ => continue,
            };

            for mapping in &user.path_mappings {
                if mapping.request_path.is_empty() || mapping.final_path.is_empty() {
                    continue;
                }
                if !is_valid_mapping_target_for_redirect_owner(user_id, &mapping.final_path) {
                    continue;
                }
                let request_key = paths::match_key(&mapping.request_path);
                if let Some(existing) = target_by_current.get(&request_key) {
                    if !paths::eq_ignore_case(existing, &mapping.final_path) {
                        log::warn!(
                            "user {} map conflict, skip: cur={} old={} new={}",
                            user_id,
                            mapping.request_path,
                            existing,
                            mapping.final_path
                        );
                    }
                    continue;
                }

                target_by_current.insert(request_key, mapping.final_path.clone());
                merged.push(mapping.clone());
            }
        }

        sort_path_mappings_longest_request_first(&mut merged);
        merged
    }

    pub fn resolve_redirect_package_by_path_for_user(&self, app_uid: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || path.is_empty() {
            return String::new();
        }

        let user_id = platform::user_id_from_uid(app_uid);
        resolve_package_by_path_in_apps(&state.apps, user_id, path, PackagePathMatchMode::Redirect)
    }

    pub fn resolve_enabled_package_by_path_for_user(&self, user_id: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(&state.apps, user_id, path, PackagePathMatchMode::Enabled)
    }

    pub fn resolve_mapping_request_package_by_path_for_user(
        &self,
        user_id: i32,
        path: &str,
    ) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(
            &state.apps,
            user_id,
            path,
            PackagePathMatchMode::MappingRequest,
        )
    }

    pub fn resolve_monitor_package_by_path_for_user(&self, user_id: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(&state.apps, user_id, path, PackagePathMatchMode::Monitor)
    }

    pub fn is_public_mapping_target_path_for_user(&self, user_id: i32, path: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return false;
        }

        let normalized = paths::normalize(path);
        if !is_public_mapping_target_path(user_id, &normalized) {
            return false;
        }

        state.apps.iter().any(|(package_name, app)| {
            if package_name == SELF_PACKAGE_NAME || policy::is_system_writer_package(package_name) {
                return false;
            }

            let Some(user) = app.user_profiles.get(&user_id) else {
                return false;
            };
            if !user.is_enabled {
                return false;
            }

            user.path_mappings.iter().any(|mapping| {
                let final_path = paths::normalize(&mapping.final_path);
                is_public_mapping_target_path(user_id, &final_path)
                    && paths::is_same_or_child(&normalized, &final_path)
            })
        })
    }
}

fn is_public_mapping_target_path(user_id: i32, path: &str) -> bool {
    if path.is_empty() || paths::has_unsafe_segments(path) {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    if !paths::is_child(path, &storage_root) {
        return false;
    }
    if paths::is_android_data_or_obb_path(path) {
        return false;
    }
    paths::extract_android_private_path_owner(path).is_empty()
}

fn resolve_package_by_path_in_apps(
    apps: &HashMap<String, super::AppProfile>,
    user_id: i32,
    path: &str,
    mode: PackagePathMatchMode,
) -> String {
    let normalized = paths::normalize(path);
    let storage_root = paths::storage_user_root_for_user(user_id);
    if !paths::is_child(&normalized, &storage_root) {
        return String::new();
    }

    let mut matched_packages: Vec<String> = Vec::new();
    let mut matched_prefix_len = 0usize;
    for (package_name, app) in apps {
        if package_name == SELF_PACKAGE_NAME || policy::is_system_writer_package(package_name) {
            continue;
        }

        let user = match app.user_profiles.get(&user_id) {
            Some(user) if user.is_enabled => user,
            _ => continue,
        };

        // Only paths that carry ownership information should infer a caller.
        // Public allow/exclude/sandbox rules are policy for a known caller, not proof
        // that an otherwise anonymous public-path request belongs to that app.
        if should_match_default_redirect_target_for_mode(mode) {
            let default_redirect_target = paths::default_redirect_target(package_name, user_id);
            update_matched_packages(
                &mut matched_packages,
                &mut matched_prefix_len,
                package_name,
                &default_redirect_target,
                &normalized,
            );
        }

        for mapping in &user.path_mappings {
            let request_path =
                resolve_mapping_storage_path_for_user(user_id, &mapping.request_path);
            let final_path = resolve_mapping_storage_path_for_user(user_id, &mapping.final_path);
            if request_path.is_empty()
                || final_path.is_empty()
                || !is_valid_mapping_target_for_redirect_owner(user_id, &final_path)
            {
                continue;
            }
            if is_specific_mapping_request_owner_hint(user_id, &request_path) {
                update_matched_packages(
                    &mut matched_packages,
                    &mut matched_prefix_len,
                    package_name,
                    &request_path,
                    &normalized,
                );
            }
            if should_match_mapping_target_for_mode(mode, user_id, &final_path) {
                update_matched_packages(
                    &mut matched_packages,
                    &mut matched_prefix_len,
                    package_name,
                    &final_path,
                    &normalized,
                );
            }
        }
    }

    if matched_packages.len() == 1 {
        return matched_packages.remove(0);
    }
    prefer_download_provider_match(&matched_packages)
        .map(str::to_string)
        .unwrap_or_default()
}

fn should_match_mapping_target_for_mode(
    mode: PackagePathMatchMode,
    user_id: i32,
    final_path: &str,
) -> bool {
    match mode {
        PackagePathMatchMode::Redirect => false,
        PackagePathMatchMode::MappingRequest => false,
        PackagePathMatchMode::Enabled => true,
        PackagePathMatchMode::Monitor => is_specific_storage_owner_hint(user_id, final_path),
    }
}

fn should_match_default_redirect_target_for_mode(mode: PackagePathMatchMode) -> bool {
    !matches!(mode, PackagePathMatchMode::MappingRequest)
}

fn prefer_download_provider_match(packages: &[String]) -> Option<&str> {
    if packages.len() < 2 {
        return None;
    }

    let has_provider = packages
        .iter()
        .any(|package_name| package_name == DOWNLOAD_PROVIDER_PACKAGE);
    if !has_provider {
        return None;
    }

    let only_download_components = packages.iter().all(|package_name| {
        package_name == DOWNLOAD_PROVIDER_PACKAGE || package_name == DOWNLOAD_PROVIDER_UI_PACKAGE
    });
    if only_download_components {
        Some(DOWNLOAD_PROVIDER_PACKAGE)
    } else {
        None
    }
}
