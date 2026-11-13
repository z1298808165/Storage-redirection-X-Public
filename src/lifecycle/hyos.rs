use super::companion_mount::execute_companion_mount_request;
use super::companion_request::CompanionMountRequest;
use super::hyos_request::{self, REQUEST_SIZE};
use crate::config::{SettingsHub, StorageBackendMode};
use crate::platform::{fs, paths, proc_identity};
use crate::redirect::policy;
use std::time::Duration;

pub(super) fn handle_request(fd: i32, caller: &libc::ucred) -> bool {
    let mut data = [0; REQUEST_SIZE];
    if !fs::read_all_timeout(fd, &mut data, Duration::from_secs(5)) {
        return false;
    }
    let Some((package, process)) = hyos_request::decode(&data) else {
        return false;
    };
    apply_for_caller(package, process, caller)
}

pub(super) fn apply_for_caller(package: &str, process: &str, caller: &libc::ucred) -> bool {
    if !hyos_request::valid_identity(package, process) {
        return false;
    }
    let Ok(uid) = i32::try_from(caller.uid) else {
        return false;
    };
    let package_uid = policy::get_fresh_uid_for_package(package);
    if !super::companion_policy::package_uid_matches(uid, package_uid)
        || proc_identity::read_proc_uid(caller.pid) != Some(uid)
    {
        log::warn!(
            "hyos identity rejected pkg={} pid={} uid={}",
            package,
            caller.pid,
            uid
        );
        return false;
    }
    if !crate::runtime_control::is_module_runtime_enabled() {
        return true;
    }
    super::companion::ensure_daemon_alive();
    let config = SettingsHub::instance();
    if !config.init(None) || !config.reload_force() {
        return false;
    }
    let profile = config.get_resolved_user_profile_snapshot(package, uid);
    let monitor_only = profile.is_none()
        && crate::config::should_capture_unconfigured_app(
            package,
            uid,
            config.is_file_monitor_enabled(),
        );
    let mount_namespace = crate::platform::mount_namespace::private_mount_namespace(caller.pid);
    // 只登记已隔离的 native 进程，避免稍后的配置重载绕过共享视图拒绝。
    if mount_namespace.is_some() {
        crate::mount_intent::mark_state(
            package,
            caller.pid,
            uid,
            config.storage_backend_mode(),
            config.config_version(),
            "native_registered",
        );
    }
    if profile.is_none() && !monitor_only {
        log::info!(
            "hyos passthrough pkg={} proc={} uid={}",
            package,
            process,
            uid
        );
        return true;
    }
    // 在共享 spawner/init 的命名空间挂载会污染其它应用，必须在任何挂载操作前拒绝。
    let Some(mount_namespace) = mount_namespace else {
        log::error!(
            "hyos storage view rejected shared namespace pkg={} pid={}",
            package,
            caller.pid
        );
        return false;
    };
    let user = crate::platform::user_id_from_uid(uid);
    let mut request = CompanionMountRequest {
        mount_namespace: Some(mount_namespace),
        pid: caller.pid,
        uid,
        package_name: package.to_owned(),
        app_data_dir: format!("/data/user/{user}/{package}"),
        storage_backend_mode: StorageBackendMode::Auto,
        is_file_monitor_enabled: config.is_file_monitor_enabled(),
        redirect_target: if monitor_only {
            paths::storage_user_root_for_user(user)
        } else {
            paths::default_redirect_target(package, user)
        },
        is_monitor_only: monitor_only,
        config_version: config.config_version(),
        ..Default::default()
    };
    if let Some(profile) = profile {
        request.redirect_target = profile.redirect_target;
        request.allowed_real_paths = profile.allowed_real_paths;
        request.excluded_real_paths = profile.excluded_real_paths;
        request.sandboxed_paths = profile.sandboxed_paths;
        request.read_only_paths = profile.read_only_paths;
        request.path_mappings = profile.path_mappings;
        request.is_mapping_mode_only = profile.is_mapping_mode_only;
    }
    request.policy_fingerprint = crate::fuse_redirect::request_policy_fingerprint(&request);
    let installed = execute_companion_mount_request(&request);
    log::info!(
        "hyos storage view pkg={} proc={} pid={} uid={} installed={}",
        package,
        process,
        caller.pid,
        uid,
        installed
    );
    installed
}
