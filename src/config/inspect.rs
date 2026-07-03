use super::{MonitorFilterConfig, SettingsHub, SettingsState};
use crate::config::{
    MonitorAppSpec, ResolvedUserProfile, ResolvedUserProfileFlags, UserProfile,
    UserRedirectEnablement,
};
use crate::platform;
use crate::redirect::policy;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const SELF_PACKAGE_NAME: &str = "com.storage.redirect.x";

// 优化：监控路径匹配缓存
const MONITOR_PATH_CACHE_SIZE: usize = 128;
const MONITOR_DECISION_LOG_STEP: u64 = 1024;
static MONITOR_PATH_MATCH_CACHE: Lazy<Mutex<HashMap<String, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::with_capacity(MONITOR_PATH_CACHE_SIZE)));
static SYSTEM_WRITER_MONITOR_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
static BRIDGE_MONITOR_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
static UI_MONITOR_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

fn should_log_monitor_decision(counter: &AtomicU64) -> bool {
    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
    count == 1 || count.is_multiple_of(MONITOR_DECISION_LOG_STEP)
}

impl SettingsHub {
    pub fn get_resolved_user_profile_snapshot(
        &self,
        package_name: &str,
        app_uid: i32,
    ) -> Option<ResolvedUserProfile> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let (user_id, user) = find_enabled_user_profile(&state, package_name, app_uid)?;
        Some(ResolvedUserProfile {
            user_id,
            redirect_target: platform::paths::default_redirect_target(package_name, user_id),
            is_mapping_mode_only: user.is_mapping_mode_only,
            allowed_real_paths: user.allowed_real_paths.clone(),
            excluded_real_paths: user.excluded_real_paths.clone(),
            sandboxed_paths: user.sandboxed_paths.clone(),
            read_only_paths: user.read_only_paths.clone(),
            path_mappings: user.path_mappings.clone(),
        })
    }

    pub fn get_resolved_user_profile_flags(
        &self,
        package_name: &str,
        app_uid: i32,
    ) -> Option<ResolvedUserProfileFlags> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let (_, user) = find_enabled_user_profile(&state, package_name, app_uid)?;
        Some(ResolvedUserProfileFlags {
            is_mapping_mode_only: user.is_mapping_mode_only,
        })
    }

    pub fn is_file_monitor_enabled(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.is_file_monitor_enabled
    }

    pub fn is_fuse_fix_enabled(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.is_fuse_fix_enabled
    }

    pub fn is_fuse_daemon_redirect_enabled(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.is_fuse_daemon_redirect_enabled
    }

    pub fn should_redirect(&self, package_name: &str, app_uid: i32) -> bool {
        self.get_resolved_user_profile_flags(package_name, app_uid)
            .is_some()
    }

    pub fn is_user_profile_enabled_in_memory(&self, package_name: &str, user_id: i32) -> bool {
        if package_name.is_empty() || user_id < 0 {
            return false;
        }
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded {
            return false;
        }
        state
            .apps
            .get(package_name)
            .and_then(|app| app.user_profiles.get(&user_id))
            .is_some_and(|profile| profile.is_enabled)
    }

    pub fn get_user_redirect_enablement(
        &self,
        package_name: &str,
        app_uid: i32,
        user_id: i32,
    ) -> UserRedirectEnablement {
        let memory_flags = self.get_resolved_user_profile_flags(package_name, app_uid);
        let raw_flags = self.get_user_flags_in_raw_config(package_name, user_id);
        UserRedirectEnablement {
            enabled_in_memory: memory_flags.is_some(),
            has_raw_config: raw_flags.has_config,
            enabled_in_raw: raw_flags.is_enabled,
            is_mapping_mode_only: memory_flags
                .map(|profile| profile.is_mapping_mode_only)
                .unwrap_or(false)
                || raw_flags.is_mapping_mode_only,
        }
    }

    pub fn should_monitor(&self, package_name: &str, app_uid: i32) -> bool {
        if package_name == SELF_PACKAGE_NAME {
            return false;
        }

        let (is_loaded, is_file_monitor_enabled) = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            (state.is_loaded, state.is_file_monitor_enabled)
        };

        if !is_loaded || !is_file_monitor_enabled {
            return false;
        }

        if policy::is_system_writer_package(package_name) {
            if should_log_monitor_decision(&SYSTEM_WRITER_MONITOR_LOG_COUNT) {
                log::info!(
                    "monitor on: writer proc pkg={} uid={}",
                    package_name,
                    app_uid
                );
            }
            return true;
        }

        if policy::is_file_monitor_bridge_package(package_name) {
            if should_log_monitor_decision(&BRIDGE_MONITOR_LOG_COUNT) {
                log::info!(
                    "monitor on: bridge proc pkg={} uid={}",
                    package_name,
                    app_uid
                );
            }
            return true;
        }

        if policy::is_file_monitor_ui_package(package_name) {
            if should_log_monitor_decision(&UI_MONITOR_LOG_COUNT) {
                log::info!("monitor on: ui proc pkg={} uid={}", package_name, app_uid);
            }
            return true;
        }

        false
    }

    #[allow(dead_code)]
    pub fn get_monitor_app_specs(&self) -> Vec<MonitorAppSpec> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || !state.is_file_monitor_enabled {
            return Vec::new();
        }

        let mut specs = Vec::new();
        for (package_name, app) in &state.apps {
            if package_name == SELF_PACKAGE_NAME || policy::is_system_writer_package(package_name) {
                continue;
            }
            for (user_id, profile) in &app.user_profiles {
                specs.push(MonitorAppSpec {
                    package_name: package_name.clone(),
                    user_id: *user_id,
                    is_enabled: profile.is_enabled,
                    is_mapping_mode_only: profile.is_mapping_mode_only,
                    allowed_real_paths: profile.allowed_real_paths.clone(),
                    excluded_real_paths: profile.excluded_real_paths.clone(),
                    sandboxed_paths: profile.sandboxed_paths.clone(),
                    read_only_paths: profile.read_only_paths.clone(),
                    path_mappings: profile.path_mappings.clone(),
                });
            }
        }
        specs.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.package_name.cmp(&right.package_name))
        });
        specs
    }

    pub fn has_enabled_redirect_apps_for_user(&self, app_uid: i32) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded {
            return false;
        }
        let user_id = platform::user_id_from_uid(app_uid);
        for app in state.apps.values() {
            if let Some(user) = app.user_profiles.get(&user_id)
                && user.is_enabled
            {
                return true;
            }
        }
        false
    }

    pub fn has_effective_enabled_redirect_apps_for_user(&self, app_uid: i32) -> bool {
        if self.has_enabled_redirect_apps_for_user(app_uid) {
            return true;
        }

        let user_id = platform::user_id_from_uid(app_uid);
        self.has_any_enabled_user_profile_in_raw_config(user_id)
    }

    pub fn has_enabled_read_only_paths_for_user(&self, user_id: i32) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if !state.is_loaded || user_id < 0 {
            return false;
        }
        state.apps.values().any(|app| {
            app.user_profiles.get(&user_id).is_some_and(|profile| {
                profile.is_enabled && profile.read_only_paths.iter().any(|path| !path.is_empty())
            })
        })
    }

    pub fn get_app_count(&self) -> usize {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.apps.len()
    }

    pub fn should_filter_monitor_record(&self, path: &str, operation: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        should_filter_monitor_record_locked_for_version(
            &state.monitor_filters,
            self.config_version(),
            path,
            operation,
        )
    }
}

fn find_enabled_user_profile<'a>(
    state: &'a SettingsState,
    package_name: &str,
    app_uid: i32,
) -> Option<(i32, &'a UserProfile)> {
    if !state.is_loaded || package_name == SELF_PACKAGE_NAME {
        return None;
    }
    let app = state.apps.get(package_name)?;
    let user_id = platform::user_id_from_uid(app_uid);
    let user = app.user_profiles.get(&user_id)?;
    user.is_enabled.then_some((user_id, user))
}

#[cfg(test)]
fn should_filter_monitor_record_locked(
    filters: &MonitorFilterConfig,
    path: &str,
    operation: &str,
) -> bool {
    should_filter_monitor_record_locked_for_version(filters, 0, path, operation)
}

fn should_filter_monitor_record_locked_for_version(
    filters: &MonitorFilterConfig,
    config_version: u64,
    path: &str,
    operation: &str,
) -> bool {
    let normalized_path = crate::platform::paths::normalize(path);
    let op = operation.trim().to_lowercase();

    // 优化：为高频路径和操作组合提供快速缓存查找
    let cache_key = format!("{:x}|{}|{}", config_version, normalized_path, op);
    if let Ok(cache) = MONITOR_PATH_MATCH_CACHE.try_lock() {
        if let Some(&cached_result) = cache.get(&cache_key) {
            return cached_result;
        }
    }

    let path_matched = filters
        .excluded_paths
        .iter()
        .any(|rule| monitor_path_filter_matches(rule, &normalized_path));

    let op_matched = filters
        .excluded_operations
        .iter()
        .any(|rule| monitor_operation_filter_matches(rule, &op));

    let result = path_matched || op_matched;

    // 优化：缓存匹配结果
    if let Ok(mut cache) = MONITOR_PATH_MATCH_CACHE.try_lock() {
        if cache.len() >= MONITOR_PATH_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(cache_key, result);
    }

    result
}

fn monitor_path_filter_matches(rule: &str, path: &str) -> bool {
    let pattern = normalize_monitor_path_filter_rule(rule);
    if pattern.is_empty() || path.is_empty() {
        return false;
    }
    if !has_monitor_path_wildcard(&pattern) {
        return crate::platform::paths::is_same_or_child(path, &pattern);
    }
    if crate::platform::paths::matches(&pattern, path, true) {
        return true;
    }
    if let Some(base) = pattern.strip_suffix("/**") {
        return crate::platform::paths::matches(base, path, true);
    }
    false
}

fn has_monitor_path_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

fn normalize_monitor_path_filter_rule(rule: &str) -> String {
    let collapsed = collapse_monitor_filter_path_slashes(&rule.trim().replace('\\', "/"));
    if collapsed.is_empty()
        || collapsed.contains('\0')
        || collapsed.len() > 512
        || collapsed.starts_with('!')
    {
        return String::new();
    }
    if has_monitor_filter_storage_root_prefix(collapsed.trim_start_matches('/')) {
        return String::new();
    }
    let relative = collapsed.trim_matches('/');
    if relative.is_empty()
        || relative
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || relative
            .chars()
            .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '\u{0000}'..='\u{001f}'))
    {
        return String::new();
    }
    let relative = crate::platform::paths::normalize(relative);
    format!("/storage/emulated/*/{}", relative.trim_matches('/'))
}

fn collapse_monitor_filter_path_slashes(value: &str) -> String {
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

fn has_monitor_filter_storage_root_prefix(path: &str) -> bool {
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

fn monitor_operation_filter_matches(rule: &str, operation: &str) -> bool {
    let pattern = rule.trim().to_lowercase();
    if pattern.is_empty() || pattern.contains('/') || operation.is_empty() {
        return false;
    }
    crate::platform::paths::matches(&pattern, operation, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppProfile;
    use std::collections::HashMap;

    fn enabled_profile() -> UserProfile {
        UserProfile {
            is_enabled: true,
            is_mapping_mode_only: true,
            allowed_real_paths: vec!["/storage/emulated/0/Download".to_string()],
            excluded_real_paths: Vec::new(),
            sandboxed_paths: vec!["/storage/emulated/0/Pictures".to_string()],
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
        }
    }

    #[test]
    fn resolved_user_profile_snapshot_includes_user_and_redirect_target() {
        let hub = SettingsHub::new();
        hub.replace_test_apps(HashMap::from([(
            "org.srx.snapshot".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        )]));

        let resolved = hub
            .get_resolved_user_profile_snapshot("org.srx.snapshot", 10123)
            .expect("profile should resolve");

        assert_eq!(resolved.user_id, 0);
        assert_eq!(
            resolved.redirect_target,
            "/storage/emulated/0/Android/data/org.srx.snapshot/sdcard"
        );
        assert!(resolved.is_mapping_mode_only);
        assert_eq!(
            resolved.allowed_real_paths,
            vec!["/storage/emulated/0/Download".to_string()]
        );
        assert_eq!(
            resolved.sandboxed_paths,
            vec!["/storage/emulated/0/Pictures".to_string()]
        );
    }

    #[test]
    fn resolved_user_profile_flags_include_mapping_mode() {
        let hub = SettingsHub::new();
        hub.replace_test_apps(HashMap::from([(
            "org.srx.flags".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(10, enabled_profile())]),
            },
        )]));

        let flags = hub
            .get_resolved_user_profile_flags("org.srx.flags", 1010123)
            .expect("profile flags should resolve");

        assert!(flags.is_mapping_mode_only);
        assert!(hub.should_redirect("org.srx.flags", 1010123));
    }

    #[test]
    fn enabled_read_only_path_probe_is_user_scoped() {
        let hub = SettingsHub::new();
        hub.replace_test_apps(HashMap::from([
            (
                "org.srx.readonly".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(0, {
                        let mut profile = enabled_profile();
                        profile.read_only_paths = vec!["/storage/emulated/0/Download".to_string()];
                        profile
                    })]),
                },
            ),
            (
                "org.srx.empty".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(10, enabled_profile())]),
                },
            ),
        ]));

        assert!(hub.has_enabled_read_only_paths_for_user(0));
        assert!(!hub.has_enabled_read_only_paths_for_user(10));
        assert!(!hub.has_enabled_read_only_paths_for_user(-1));
    }

    #[test]
    fn user_redirect_enablement_combines_memory_profile_flags() {
        let hub = SettingsHub::new();
        let package_name = format!("org.srx.enablement.{}", std::process::id());
        hub.replace_test_apps(HashMap::from([(
            package_name.clone(),
            AppProfile {
                user_profiles: HashMap::from([(0, enabled_profile())]),
            },
        )]));

        let enablement = hub.get_user_redirect_enablement(&package_name, 10123, 0);

        assert!(enablement.is_enabled());
        assert!(enablement.enabled_in_memory);
        assert!(!enablement.enabled_in_raw);
        assert!(enablement.is_mapping_mode_only);
    }

    #[test]
    fn resolved_user_profile_snapshot_rejects_disabled_profiles() {
        let hub = SettingsHub::new();
        let mut profile = enabled_profile();
        profile.is_enabled = false;
        hub.replace_test_apps(HashMap::from([(
            "org.srx.disabled".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, profile)]),
            },
        )]));

        assert!(
            hub.get_resolved_user_profile_snapshot("org.srx.disabled", 10123)
                .is_none()
        );
        assert!(
            hub.get_resolved_user_profile_flags("org.srx.disabled", 10123)
                .is_none()
        );
    }

    #[test]
    fn monitor_specs_include_disabled_profiles_for_external_monitoring() {
        let hub = SettingsHub::new();
        let previous_monitor = hub.replace_test_file_monitor_enabled(true);
        let mut profile = enabled_profile();
        profile.is_enabled = false;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.monitor.disabled".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(0, profile)]),
            },
        )]));

        let specs = hub.get_monitor_app_specs();

        hub.restore_test_apps(previous_apps, previous_loaded);
        hub.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].package_name, "org.srx.monitor.disabled");
        assert!(!specs[0].is_enabled);
        assert_eq!(
            specs[0].allowed_real_paths,
            vec!["/storage/emulated/0/Download".to_string()]
        );
    }

    #[test]
    fn monitor_path_filters_support_android_data_shortcut() {
        let filters = MonitorFilterConfig {
            excluded_paths: vec!["Android/data".to_string()],
            excluded_operations: Vec::new(),
        };
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Android/data/com.demo/files/a.txt",
            "open:create"
        ));
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/10/Android/data/com.demo/files/a.txt",
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Android/media/com.demo/files/a.txt",
            "open:create"
        ));
    }

    #[test]
    fn monitor_path_filters_keep_legacy_storage_relative_slash() {
        let filters = MonitorFilterConfig {
            excluded_paths: vec!["/Android/data".to_string()],
            excluded_operations: Vec::new(),
        };
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Android/data/com.demo/files/a.txt",
            "open:create"
        ));
    }

    #[test]
    fn monitor_path_filters_treat_plain_rules_as_prefixes() {
        let filters = MonitorFilterConfig {
            excluded_paths: vec!["Android/media".to_string()],
            excluded_operations: Vec::new(),
        };
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Android/media",
            "open:create"
        ));
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/10/Android/media/com.demo/files/a.txt",
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Android/data/com.demo/files/a.txt",
            "open:create"
        ));
    }

    #[test]
    fn monitor_path_filters_expand_storage_shortcuts() {
        let filters = MonitorFilterConfig {
            excluded_paths: vec!["Download/*.tmp".to_string()],
            excluded_operations: Vec::new(),
        };
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.tmp",
            "open:create"
        ));
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/10/Download/b.tmp",
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.jpg",
            "open:create"
        ));
    }

    #[test]
    fn monitor_path_filters_reject_storage_roots() {
        let filters = MonitorFilterConfig {
            excluded_paths: vec![
                "/storage/emulated/0/MIUI".to_string(),
                "/data/media/0/Download".to_string(),
                "storage/emulated/0/Pictures".to_string(),
            ],
            excluded_operations: Vec::new(),
        };
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/MIUI/theme.db",
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.txt",
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Pictures/a.jpg",
            "open:create"
        ));
    }

    #[test]
    fn monitor_operation_filters_support_wildcards() {
        let filters = MonitorFilterConfig {
            excluded_paths: Vec::new(),
            excluded_operations: vec!["open:read".to_string(), "unlink*".to_string()],
        };
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.txt",
            "open:read"
        ));
        assert!(!should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.txt",
            "open:create"
        ));
        assert!(should_filter_monitor_record_locked(
            &filters,
            "/storage/emulated/0/Download/a.txt",
            "unlinkat"
        ));
    }

    #[test]
    fn file_monitor_enables_ui_shell_monitoring() {
        let hub = SettingsHub::new();
        let previous = hub.replace_test_file_monitor_enabled(true);

        assert!(hub.should_monitor("com.android.providers.media.module", 10305));
        assert!(hub.should_monitor("com.android.providers.downloads", 10067));
        assert!(hub.should_monitor("com.android.externalstorage", 10073));

        assert!(hub.should_monitor("com.android.documentsui", 10071));
        assert!(hub.should_monitor("com.android.photopicker", 10304));
        assert!(hub.should_monitor("com.android.providers.downloads.ui", 10067));

        hub.restore_test_file_monitor_enabled(previous.0, previous.1);
    }

    #[test]
    fn file_monitor_skips_plain_shared_media_process() {
        let hub = SettingsHub::new();
        let previous = hub.replace_test_file_monitor_enabled(true);
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.media".to_string(), 10067),
            ("com.android.providers.downloads".to_string(), 10067),
        ]));

        assert!(policy::is_shared_uid_process(10067));
        assert!(!hub.should_monitor("android.process.media", 10067));
        assert!(hub.should_monitor("com.android.providers.downloads", 10067));

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );
        hub.restore_test_file_monitor_enabled(previous.0, previous.1);
    }

    #[test]
    fn monitor_filter_cache_is_scoped_by_config_version() {
        let filtered = MonitorFilterConfig {
            excluded_paths: vec!["Download/SrxCacheVersionProbe".to_string()],
            excluded_operations: Vec::new(),
        };
        let unfiltered = MonitorFilterConfig {
            excluded_paths: Vec::new(),
            excluded_operations: Vec::new(),
        };
        let path = "/storage/emulated/0/Download/SrxCacheVersionProbe/a.txt";

        assert!(should_filter_monitor_record_locked_for_version(
            &filtered,
            0x10,
            path,
            "open:create"
        ));
        assert!(!should_filter_monitor_record_locked_for_version(
            &unfiltered,
            0x11,
            path,
            "open:create"
        ));
        assert!(should_filter_monitor_record_locked_for_version(
            &filtered,
            0x10,
            path,
            "open:create"
        ));
    }
}
