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
    _is_system_writer_hook_redirect: bool,
    _should_monitor: bool,
    _should_defer_media_boot_extras: bool,
) -> bool {
    // MediaProvider 的 ContentValues 路径补丁需要 Java hook。Android 13
    // 上 MediaProvider 可能在模块配置写入前就已经启动，且测试/部分设备
    // 不能安全重启该进程；因此 Java mutation hook 需要在 MediaProvider
    // 首次 specialize 时预装。真正是否改写路径仍由 native callback 按
    // caller uid 和实时配置决定，这里不扩大普通应用或 native/PLT hook。
    context.is_system_writer && context.is_media_provider
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
