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
    RedirectOwner,
    EnabledOwner,
    MappingRequestOwner,
    ReadOnlyOwner,
    MonitorOwner,
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
        resolve_package_by_path_in_apps(
            &state.apps,
            user_id,
            path,
            PackagePathMatchMode::RedirectOwner,
        )
    }

    pub fn resolve_enabled_package_by_path_for_user(&self, user_id: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(
            &state.apps,
            user_id,
            path,
            PackagePathMatchMode::EnabledOwner,
        )
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
            PackagePathMatchMode::MappingRequestOwner,
        )
    }

    pub fn resolve_read_only_package_by_path_for_user(&self, user_id: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(
            &state.apps,
            user_id,
            path,
            PackagePathMatchMode::ReadOnlyOwner,
        )
    }

    pub fn resolve_monitor_package_by_path_for_user(&self, user_id: i32, path: &str) -> String {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 || path.is_empty() {
            return String::new();
        }

        resolve_package_by_path_in_apps(
            &state.apps,
            user_id,
            path,
            PackagePathMatchMode::MonitorOwner,
        )
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

        if matches!(mode, PackagePathMatchMode::ReadOnlyOwner)
            && read_only_rule_matches_path(&user.read_only_paths, &normalized)
        {
            let (included, _) = paths::split_exclusion_rules(&user.read_only_paths);
            for rule in included {
                update_matched_packages(
                    &mut matched_packages,
                    &mut matched_prefix_len,
                    package_name,
                    &rule,
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
        PackagePathMatchMode::RedirectOwner => false,
        PackagePathMatchMode::MappingRequestOwner => false,
        PackagePathMatchMode::ReadOnlyOwner => false,
        PackagePathMatchMode::EnabledOwner => true,
        PackagePathMatchMode::MonitorOwner => is_specific_storage_owner_hint(user_id, final_path),
    }
}

fn should_match_default_redirect_target_for_mode(mode: PackagePathMatchMode) -> bool {
    !matches!(
        mode,
        PackagePathMatchMode::MappingRequestOwner | PackagePathMatchMode::ReadOnlyOwner
    )
}

fn read_only_rule_matches_path(read_only_paths: &[String], normalized: &str) -> bool {
    let (included, excluded) = paths::split_exclusion_rules(read_only_paths);
    included
        .iter()
        .any(|rule| paths::matches(rule, normalized, true))
        && !excluded
            .iter()
            .any(|rule| paths::matches(rule, normalized, true))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, UserProfile};
    use crate::redirect::writer;

    fn enabled_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![PathMapping::new(
                "/storage/emulated/0/Download/DLManager".to_string(),
                "/storage/emulated/0/Download/third-party/DLManager".to_string(),
            )],
        }
    }

    fn enabled_profile_without_mappings() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
        }
    }

    fn enabled_read_only_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: vec![
                "/storage/emulated/0/Download/SrtMonitorLocked".to_string(),
                "!/storage/emulated/0/Download/SrtMonitorLocked/Writable".to_string(),
            ],
            path_mappings: Vec::new(),
        }
    }

    fn disabled_profile() -> UserProfile {
        UserProfile {
            is_enabled: false,
            is_mapping_mode_only: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![PathMapping::new(
                "/storage/emulated/0/Download/Nnngram".to_string(),
                "/storage/emulated/0/Download/第三方下载/Nnngram".to_string(),
            )],
        }
    }

    fn enabled_pictures_to_dcim_mapping_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![PathMapping::new(
                "/storage/emulated/0/Pictures".to_string(),
                "/storage/emulated/0/DCIM".to_string(),
            )],
        }
    }

    fn qq_android_allow_with_wildcard_exclude_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: false,
            allowed_real_paths: vec!["/storage/emulated/0/Android".to_string()],
            excluded_real_paths: vec!["/storage/emulated/0/Android/.android_*".to_string()],
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
        }
    }

    fn broad_documents_mapping_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![PathMapping::new(
                "/storage/emulated/0/Documents".to_string(),
                "/storage/emulated/0/Android/data/com.tencent.lolm/files".to_string(),
            )],
        }
    }

    fn private_target_mapping_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![
                PathMapping::new(
                    "/storage/emulated/0/Download/Game".to_string(),
                    "/storage/emulated/0/Android/data/com.example.game/files".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Download/Media".to_string(),
                    "/storage/emulated/0/Android/media/com.example.game/cache".to_string(),
                ),
            ],
        }
    }

    fn relative_private_target_mapping_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![
                PathMapping::new(
                    "Download/Game".to_string(),
                    "Android/data/com.example.game/files".to_string(),
                ),
                PathMapping::new(
                    "Download/Media".to_string(),
                    "Android/media/com.example.game/cache".to_string(),
                ),
            ],
        }
    }

    #[test]
    fn explicit_redirect_resolution_does_not_resolve_mapping_target_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.mappingowner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_pictures_to_dcim_mapping_profile())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_redirect_package_by_path_for_user(
            10242,
            "/storage/emulated/0/DCIM/SharedAlbum/photo.jpg",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved.is_empty());
    }

    #[test]
    fn monitor_resolution_does_not_resolve_broad_public_mapping_target() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.mappingowner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_pictures_to_dcim_mapping_profile())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_monitor_package_by_path_for_user(
            0,
            "/storage/emulated/0/DCIM/SharedAlbum/photo.jpg",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved.is_empty());
    }

    #[test]
    fn monitor_resolution_resolves_specific_public_mapping_target() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_monitor_package_by_path_for_user(
            0,
            "/storage/emulated/0/Download/third-party/DLManager/thumbs/.nomedia",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved, "org.srx.testapp");
    }

    #[test]
    fn monitor_resolution_keeps_default_redirect_target_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile_without_mappings())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_monitor_package_by_path_for_user(
            0,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/Download/SRXTest/.pending-test.bin",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved, "org.srx.testapp");
    }

    #[test]
    fn broad_public_mapping_request_does_not_resolve_path_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "com.tencent.lolm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, broad_documents_mapping_profile())]),
            },
        );
        drop(state);

        let path = "/storage/emulated/0/Documents/MT管理器/apks/酷安_14.6.0.apk";
        let resolved_redirect = hub.resolve_redirect_package_by_path_for_user(10000, path);
        let resolved_enabled = hub.resolve_enabled_package_by_path_for_user(0, path);

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved_redirect.is_empty());
        assert!(resolved_enabled.is_empty());
    }

    #[test]
    fn specific_public_mapping_request_still_resolves_path_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        );
        drop(state);

        let path = "/storage/emulated/0/Download/DLManager/thumbs/.nomedia";
        let resolved_redirect = hub.resolve_redirect_package_by_path_for_user(10000, path);
        let resolved_enabled = hub.resolve_enabled_package_by_path_for_user(0, path);

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved_redirect, "org.srx.testapp");
        assert_eq!(resolved_enabled, "org.srx.testapp");
    }

    #[test]
    fn private_mapping_target_does_not_resolve_path_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "com.example.game".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, private_target_mapping_profile())]),
            },
        );
        drop(state);

        let private_target_request = "/storage/emulated/0/Download/Game/cache/file.bin";
        let media_target_request = "/storage/emulated/0/Download/Media/cache/file.bin";
        let resolved_private =
            hub.resolve_redirect_package_by_path_for_user(10000, private_target_request);
        let resolved_media =
            hub.resolve_redirect_package_by_path_for_user(10000, media_target_request);
        let merged = hub.get_merged_path_mappings_for_user(10000);

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved_private.is_empty());
        assert_eq!(resolved_media, "com.example.game");
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].final_path,
            "/storage/emulated/0/Android/media/com.example.game/cache"
        );
    }

    #[test]
    fn relative_private_mapping_target_does_not_resolve_path_owner() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "com.example.game".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, relative_private_target_mapping_profile())]),
            },
        );
        drop(state);

        let private_target_request = "/storage/emulated/0/Download/Game/cache/file.bin";
        let media_target_request = "/storage/emulated/0/Download/Media/cache/file.bin";
        let resolved_private =
            hub.resolve_redirect_package_by_path_for_user(10000, private_target_request);
        let resolved_media =
            hub.resolve_redirect_package_by_path_for_user(10000, media_target_request);
        let merged = hub.get_merged_path_mappings_for_user(10000);

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved_private.is_empty());
        assert_eq!(resolved_media, "com.example.game");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].request_path, "Download/Media");
        assert_eq!(merged[0].final_path, "Android/media/com.example.game/cache");
    }

    #[test]
    fn prefers_download_provider_over_ui_for_duplicate_mapping_target() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            DOWNLOAD_PROVIDER_PACKAGE.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        );
        state.apps.insert(
            DOWNLOAD_PROVIDER_UI_PACKAGE.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_enabled_package_by_path_for_user(
            0,
            "/storage/emulated/0/Download/third-party/DLManager/thumbs/.nomedia",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved, DOWNLOAD_PROVIDER_PACKAGE);
    }

    #[test]
    fn resolves_enabled_package_by_default_redirect_target() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile_without_mappings())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_enabled_package_by_path_for_user(
            0,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/Download/SRXTest/.pending-test.bin",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved, "org.srx.testapp");
    }

    #[test]
    fn public_allow_exclude_rules_do_not_infer_enabled_package() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "com.tencent.mobileqq".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    qq_android_allow_with_wildcard_exclude_profile(),
                )]),
            },
        );
        state.apps.insert(
            "org.srx.otherapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile_without_mappings())]),
            },
        );
        drop(state);

        let resolved = hub
            .resolve_enabled_package_by_path_for_user(0, "/storage/emulated/0/Android/.android_lq");

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(resolved.is_empty());
    }

    #[test]
    fn resolves_redirect_package_by_default_redirect_target() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile_without_mappings())]),
            },
        );
        drop(state);

        let resolved = hub.resolve_redirect_package_by_path_for_user(
            10000,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/Download/SRXTest/.pending-test.bin",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(resolved, "org.srx.testapp");
    }

    #[test]
    fn writer_path_override_does_not_steal_explicit_disabled_caller() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.mappingowner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_pictures_to_dcim_mapping_profile())]),
            },
        );
        state.apps.insert(
            "bin.mt.plus".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, disabled_profile())]),
            },
        );
        drop(state);

        let mut caller_uid = 10123;
        let mut caller_package = "bin.mt.plus".to_string();
        let mut is_inferred = false;
        writer::maybe_override_system_writer_caller_by_path(
            "/storage/emulated/0/Pictures/986bd33cd1145fecd3c541e98bf9b8ae.jpg",
            &mut caller_uid,
            10217,
            0,
            &mut caller_package,
            &mut is_inferred,
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(caller_uid, 10123);
        assert_eq!(caller_package, "bin.mt.plus");
        assert!(!is_inferred);
    }

    #[test]
    fn writer_path_override_does_not_infer_other_app_when_only_uid_is_explicit() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.mappingowner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_pictures_to_dcim_mapping_profile())]),
            },
        );
        drop(state);

        let mut caller_uid = 10123;
        let mut caller_package = String::new();
        let mut is_inferred = false;
        writer::maybe_override_system_writer_caller_by_path(
            "/storage/emulated/0/Pictures/986bd33cd1145fecd3c541e98bf9b8ae.jpg",
            &mut caller_uid,
            10217,
            0,
            &mut caller_package,
            &mut is_inferred,
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(caller_uid, 10123);
        assert!(caller_package.is_empty());
        assert!(!is_inferred);
    }

    #[test]
    fn writer_query_fallback_does_not_infer_mapping_owner_without_caller_signal() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "org.srx.mappingowner".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_pictures_to_dcim_mapping_profile())]),
            },
        );
        drop(state);

        let mut caller_uid = 10000;
        let mut caller_package = String::new();
        let mut is_inferred = false;
        let _no_path_owner_infer = crate::hook::enter_path_owner_inference_disabled();
        writer::maybe_override_system_writer_caller_by_path(
            "/storage/emulated/0/Pictures/986bd33cd1145fecd3c541e98bf9b8ae.jpg",
            &mut caller_uid,
            10217,
            0,
            &mut caller_package,
            &mut is_inferred,
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(caller_uid, 10000);
        assert!(caller_package.is_empty());
        assert!(!is_inferred);
    }

    #[test]
    fn disabled_profile_mappings_do_not_participate_in_redirect_resolution() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "xyz.nextalone.nnngram".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, disabled_profile())]),
            },
        );
        drop(state);

        let enabled_profile =
            hub.get_resolved_user_profile_snapshot("xyz.nextalone.nnngram", 10312);
        let resolved_enabled = hub.resolve_enabled_package_by_path_for_user(
            0,
            "/storage/emulated/0/Download/Nnngram/photo.jpg",
        );
        let resolved_redirect = hub.resolve_redirect_package_by_path_for_user(
            10312,
            "/storage/emulated/0/Download/Nnngram/photo.jpg",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(enabled_profile.is_none());
        assert!(resolved_enabled.is_empty());
        assert!(resolved_redirect.is_empty());
    }

    #[test]
    fn read_only_resolution_respects_exclusions() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "me.fakerqu.test.storageredirect".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_read_only_profile())]),
            },
        );
        drop(state);

        let locked = hub.resolve_read_only_package_by_path_for_user(
            0,
            "/storage/emulated/0/Download/SrtMonitorLocked/srt.bin",
        );
        let excluded = hub.resolve_read_only_package_by_path_for_user(
            0,
            "/storage/emulated/0/Download/SrtMonitorLocked/Writable/srt.bin",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert_eq!(locked, "me.fakerqu.test.storageredirect");
        assert!(excluded.is_empty());
    }

    #[test]
    fn public_mapping_target_is_accessible_path_hint() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "idm.internet.download.manager.plus".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        );
        drop(state);

        let mapped = hub.is_public_mapping_target_path_for_user(
            0,
            "/storage/emulated/0/Download/third-party/DLManager/archive.zip",
        );
        let request = hub.is_public_mapping_target_path_for_user(
            0,
            "/storage/emulated/0/Download/DLManager/archive.zip",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(mapped);
        assert!(!request);
    }

    #[test]
    fn public_mapping_target_hint_rejects_android_private_targets() {
        let hub = SettingsHub::instance();
        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        let previous_apps = state.apps.clone();
        let previous_loaded = state.is_loaded;

        state.apps.clear();
        state.is_loaded = true;
        state.apps.insert(
            "com.example.game".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, private_target_mapping_profile())]),
            },
        );
        drop(state);

        let data = hub.is_public_mapping_target_path_for_user(
            0,
            "/storage/emulated/0/Android/data/com.example.game/files/cache.bin",
        );
        let media = hub.is_public_mapping_target_path_for_user(
            0,
            "/storage/emulated/0/Android/media/com.example.game/cache/file.bin",
        );

        let mut state = hub.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps = previous_apps;
        state.is_loaded = previous_loaded;

        assert!(!data);
        assert!(!media);
    }
}
