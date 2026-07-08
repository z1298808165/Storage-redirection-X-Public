use crate::config::SettingsHub;
use crate::domain::PathMapping;
use crate::platform::{self, module_paths};
use crate::redirect::policy;
use std::ffi::CString;

pub(super) struct SystemWriterContext {
    pub(super) is_system_writer: bool,
    pub(super) is_media_provider: bool,
    pub(super) is_monitor_bridge: bool,
    pub(super) should_install_fuse_fix: bool,
    pub(super) has_merged_writer_mappings: bool,
    pub(super) merged_writer_mappings: Vec<PathMapping>,
}

// 解析系统代写进程的重定向上下文，决定挂载或 Hook 模式
pub(super) fn resolve_system_writer_context(
    package_name: &str,
    app_uid: i32,
    config: &SettingsHub,
    should_redirect: &mut bool,
    should_monitor: &mut bool,
    is_system_writer_hook_redirect: &mut bool,
) -> SystemWriterContext {
    let is_media_provider = policy::is_media_provider_package(package_name);
    let is_monitor_bridge =
        policy::is_file_monitor_bridge_package(package_name) && !is_media_provider;
    let mut context = SystemWriterContext {
        is_system_writer: policy::is_system_writer_package(package_name),
        is_media_provider,
        is_monitor_bridge,
        should_install_fuse_fix: false,
        has_merged_writer_mappings: false,
        merged_writer_mappings: Vec::new(),
    };

    let is_file_monitor_enabled = config.is_file_monitor_enabled();
    if context.is_monitor_bridge && is_file_monitor_enabled {
        *should_redirect = false;
        *is_system_writer_hook_redirect = false;
        if !*should_monitor {
            *should_monitor = true;
        }
        log::info!("monitor bridge on pkg={} uid={}", package_name, app_uid);
    }

    if !context.is_system_writer {
        return context;
    }

    policy::refresh_shared_uid_cache();
    context.merged_writer_mappings = config.get_merged_path_mappings_for_user(app_uid);
    context.has_merged_writer_mappings = !context.merged_writer_mappings.is_empty();

    let has_enabled_apps = config.has_effective_enabled_redirect_apps_for_user(app_uid);

    if context.has_merged_writer_mappings || has_enabled_apps {
        *should_redirect = true;
        *is_system_writer_hook_redirect = true;
    } else {
        *should_redirect = false;
        *is_system_writer_hook_redirect = false;
    }
    if !*should_monitor && context.is_media_provider && is_file_monitor_enabled {
        *should_monitor = true;
    }
    context.should_install_fuse_fix = context.is_media_provider
        && (is_file_monitor_enabled || context.has_merged_writer_mappings || has_enabled_apps);

    if context.has_merged_writer_mappings {
        log::info!(
            "writer map-mode on merged={} (per-caller hook)",
            context.merged_writer_mappings.len()
        );
    } else if has_enabled_apps {
        log::info!("writer map-mode on: effective enabled apps exist, caller default redirect");
    } else {
        log::info!("writer map-mode skip: no enabled apps, fallback monitor/bypass");
    }

    context
}

pub(super) fn should_defer_media_boot_extras(
    is_media_provider_context: bool,
    is_system_writer_hook_redirect: bool,
    should_install_fuse_fix: bool,
) -> bool {
    should_defer_media_boot_extras_for_state(
        is_media_provider_context,
        is_system_writer_hook_redirect,
        should_install_fuse_fix,
        platform::is_boot_completed(),
    )
}

pub(super) fn should_defer_media_boot_extras_for_state(
    is_media_provider_context: bool,
    is_system_writer_hook_redirect: bool,
    should_install_fuse_fix: bool,
    is_boot_completed: bool,
) -> bool {
    is_media_provider_context
        && (is_system_writer_hook_redirect || should_install_fuse_fix)
        && !is_boot_completed
}

pub(super) fn should_install_java_hook_for_writer(
    context: &SystemWriterContext,
    is_system_writer_hook_redirect: bool,
    _should_monitor: bool,
    should_defer_media_boot_extras: bool,
) -> bool {
    // MediaProvider 的 ContentValues 路径补丁需要 Java hook；即使当前
    // 只有 FUSE/文件监视启用，也要覆盖已经运行的 MediaProvider 进程。
    let should_hook_media_provider = context.is_system_writer
        && context.is_media_provider
        && (is_system_writer_hook_redirect || context.should_install_fuse_fix);
    should_hook_media_provider && !should_defer_media_boot_extras
}

pub(super) fn mark_media_hook_deferred() {
    let path = module_paths::MEDIA_HOOK_DEFERRED_FILE;
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, b"1\n");
    if let Ok(c_path) = CString::new(path) {
        unsafe {
            libc::chmod(c_path.as_ptr(), 0o644);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, UserProfile};
    use std::collections::HashMap;

    fn enabled_apps() -> HashMap<String, AppProfile> {
        let mut user_profiles = HashMap::new();
        user_profiles.insert(
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
        );

        let mut apps = HashMap::new();
        apps.insert(
            "com.example.redirected".to_string(),
            AppProfile { user_profiles },
        );
        apps
    }

    fn shared_media_uid_packages() -> HashMap<String, i32> {
        HashMap::from([
            ("com.android.providers.media".to_string(), 10067),
            ("com.android.providers.downloads".to_string(), 10067),
            ("com.android.mtp".to_string(), 10067),
        ])
    }

    fn temp_config_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "srx_writer_state_{}_{}_{}",
            name,
            std::process::id(),
            platform::paths::monotonic_ms()
        ))
    }

    #[test]
    fn non_media_shared_uid_process_does_not_become_writer_redirect() {
        let config = SettingsHub::instance();
        let previous_apps = config.replace_test_apps(enabled_apps());
        let previous_monitor = config.replace_test_file_monitor_enabled(true);
        let previous_uid = policy::replace_test_uid_cache(shared_media_uid_packages());

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("android.process.media", 10067);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "android.process.media",
            10067,
            config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(!context.is_system_writer);
        assert!(!context.is_monitor_bridge);
        assert!(!should_redirect);
        assert!(!should_monitor);
        assert!(!is_hook_redirect);
        assert!(!context.has_merged_writer_mappings);

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );
        config.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        config.restore_test_apps(previous_apps.0, previous_apps.1);
    }

    #[test]
    fn monitor_bridge_shared_uid_stays_monitor_only() {
        let config = SettingsHub::instance();
        let previous_apps = config.replace_test_apps(enabled_apps());
        let previous_monitor = config.replace_test_file_monitor_enabled(true);
        let previous_uid = policy::replace_test_uid_cache(shared_media_uid_packages());

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.downloads", 10067);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.downloads",
            10067,
            config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(!context.is_system_writer);
        assert!(context.is_monitor_bridge);
        assert!(!should_redirect);
        assert!(should_monitor);
        assert!(!is_hook_redirect);
        assert!(!context.has_merged_writer_mappings);

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );
        config.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        config.restore_test_apps(previous_apps.0, previous_apps.1);
    }

    #[test]
    fn media_provider_keeps_writer_redirect_when_redirected_apps_exist() {
        let config = SettingsHub::instance();
        let previous_apps = config.replace_test_apps(enabled_apps());
        let previous_monitor = config.replace_test_file_monitor_enabled(true);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(should_redirect);
        assert!(should_monitor);
        assert!(is_hook_redirect);

        config.restore_test_file_monitor_enabled(previous_monitor.0, previous_monitor.1);
        config.restore_test_apps(previous_apps.0, previous_apps.1);
    }

    #[test]
    fn media_provider_without_enabled_apps_bypasses_fuse_fix_and_redirect() {
        let config = SettingsHub::new();
        let config_dir = temp_config_dir("empty_raw");
        std::fs::create_dir_all(config_dir.join("apps")).expect("create temp apps dir");
        config.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        config.replace_test_apps(HashMap::new());
        config.replace_test_file_monitor_enabled(false);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            &config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(!context.should_install_fuse_fix);
        assert!(!should_redirect);
        assert!(!should_monitor);
        assert!(!is_hook_redirect);

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_without_enabled_apps_still_installs_java_hook() {
        let config = SettingsHub::new();
        let config_dir = temp_config_dir("empty_java_hook");
        std::fs::create_dir_all(config_dir.join("apps")).expect("create temp apps dir");
        config.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        config.replace_test_apps(HashMap::new());
        config.replace_test_file_monitor_enabled(false);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            &config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(!should_redirect);
        assert!(!should_monitor);
        assert!(!is_hook_redirect);
        assert!(should_install_java_hook_for_writer(
            &context,
            is_hook_redirect,
            should_monitor,
            false
        ));

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_without_enabled_apps_bypasses_fuse_fix_when_zero_width_fix_disabled() {
        let config = SettingsHub::new();
        let config_dir = temp_config_dir("fuse_fix_disabled");
        std::fs::create_dir_all(config_dir.join("apps")).expect("create temp apps dir");
        config.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        config.replace_test_apps(HashMap::new());
        config.replace_test_file_monitor_enabled(false);
        config.replace_test_fuse_fix_enabled(false);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            &config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(!context.should_install_fuse_fix);
        assert!(!should_redirect);
        assert!(!should_monitor);
        assert!(!is_hook_redirect);

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_file_monitor_installs_fuse_fix_without_redirect() {
        let config = SettingsHub::new();
        let config_dir = temp_config_dir("monitor_fuse_fix");
        std::fs::create_dir_all(config_dir.join("apps")).expect("create temp apps dir");
        config.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        config.replace_test_apps(HashMap::new());
        config.replace_test_file_monitor_enabled(true);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            &config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(context.should_install_fuse_fix);
        assert!(!should_redirect);
        assert!(should_monitor);
        assert!(!is_hook_redirect);

        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_uses_raw_enabled_apps_for_writer_redirect() {
        let config = SettingsHub::new();
        let config_dir = temp_config_dir("raw_sandbox");
        let apps_dir = config_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).expect("create temp apps dir");
        std::fs::write(
            apps_dir.join("org.srx.rawsandbox.json"),
            r#"{
                "users": {
                    "0": {
                        "enabled": true,
                        "mapping_mode_only": true,
                        "sandboxed_paths": [".xlDownload"]
                    }
                }
            }"#,
        )
        .expect("write raw sandbox config");
        config.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        config.replace_test_apps(HashMap::new());
        config.replace_test_file_monitor_enabled(false);

        let mut should_redirect = false;
        let mut should_monitor = config.should_monitor("com.android.providers.media.module", 10305);
        let mut is_hook_redirect = false;
        let context = resolve_system_writer_context(
            "com.android.providers.media.module",
            10305,
            &config,
            &mut should_redirect,
            &mut should_monitor,
            &mut is_hook_redirect,
        );

        assert!(context.is_system_writer);
        assert!(context.is_media_provider);
        assert!(!context.has_merged_writer_mappings);
        assert!(context.should_install_fuse_fix);
        assert!(should_redirect);
        assert!(!should_monitor);
        assert!(is_hook_redirect);

        let _ = std::fs::remove_dir_all(config_dir);
    }
}
