use crate::domain::PathMapping;
use crate::platform::fs;
use crate::zygisk::abi;

pub(super) struct CompanionMountRequest<'a> {
    pub(super) pid: i32,
    pub(super) uid: i32,
    pub(super) package_name: &'a str,
    pub(super) app_data_dir: &'a str,
    pub(super) is_fuse_daemon_redirect_enabled: bool,
    pub(super) is_file_monitor_enabled: bool,
    pub(super) redirect_target: &'a str,
    pub(super) allowed_real_paths: &'a [String],
    pub(super) excluded_real_paths: &'a [String],
    pub(super) sandboxed_paths: &'a [String],
    pub(super) read_only_paths: &'a [String],
    pub(super) path_mappings: &'a [PathMapping],
    pub(super) is_mapping_mode_only: bool,
    pub(super) operation: &'a str,
    pub(super) config_version: u64,
}

pub(super) fn build_companion_request_payload(request: &CompanionMountRequest<'_>) -> String {
    let mut mappings = Vec::new();
    for mapping in request.path_mappings {
        mappings.push(serde_json::json!({
            "request_path": mapping.request_path,
            "final_path": mapping.final_path,
        }));
    }

    let payload = serde_json::json!({
        "operation": request.operation,
        "pid": request.pid,
        "uid": request.uid,
        "package": request.package_name,
        "app_data_dir": request.app_data_dir,
        "fuse_daemon_redirect_enabled": request.is_fuse_daemon_redirect_enabled,
        "file_monitor_enabled": request.is_file_monitor_enabled,
        "redirect_target": request.redirect_target,
        "allowed_real_paths": request.allowed_real_paths,
        "excluded_real_paths": request.excluded_real_paths,
        "sandboxed_paths": request.sandboxed_paths,
        "read_only_paths": request.read_only_paths,
        "mapping_mode_only": request.is_mapping_mode_only,
        "path_mappings": mappings,
        "config_version": request.config_version,
    });

    serde_json::to_string(&payload).unwrap_or_default()
}

pub(super) fn send_companion_request_payload(api: Option<&abi::Api>, payload: &str) -> bool {
    let Some(api) = api else {
        return false;
    };
    if payload.is_empty() {
        return false;
    }

    let fd = api.connect_companion();
    if fd < 0 {
        log::warn!("companion connect failed");
        return false;
    }

    let payload_len = payload.len() as u32;
    let sent =
        fs::write_all(fd, &payload_len.to_ne_bytes()) && fs::write_all(fd, payload.as_bytes());
    unsafe { libc::close(fd) };

    if !sent {
        log::warn!("companion send failed");
        return false;
    }

    log::info!("companion req sent (async mount)");
    true
}
