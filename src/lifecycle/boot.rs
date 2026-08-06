// 启动摘要：每次开机记录一次模块状态
use crate::logging::Logger;
use crate::platform::fs;
use crate::platform::module_paths;
use crate::platform::unique_fd::UniqueFd;
use libc::{EEXIST, O_CLOEXEC, O_CREAT, O_EXCL, O_WRONLY, open};
use std::ffi::CString;

const GLOBAL_CONFIG_PATH: &str = "/data/adb/modules/storage.redirect.x/config/global.json";
const APPS_CONFIG_DIR: &str = "/data/adb/modules/storage.redirect.x/config/apps";

// 每次开机仅记录一次启动摘要，通过 marker 文件去重
pub fn log_boot_summary_once() {
    Logger::init(None);
    if !fs::create_directory(module_paths::LOG_DIR, -1) {
        log::info!("log dir create failed, skip boot summary");
        return;
    }

    let boot_id = read_boot_id();
    if boot_id.is_empty() {
        log::info!("boot_id empty, skip summary");
    }

    if let Ok(entries) = std::fs::read_dir(module_paths::LOG_DIR) {
        let mut marker_paths = Vec::new();
        let current_marker = if boot_id.is_empty() {
            String::new()
        } else {
            format!("{}/boot_{}.marker", module_paths::LOG_DIR, boot_id)
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("boot_") || !name.ends_with(".marker") {
                continue;
            }
            let marker_path = format!("{}/{}", module_paths::LOG_DIR, name);
            if !current_marker.is_empty() && marker_path == current_marker {
                continue;
            }
            marker_paths.push(marker_path);
        }

        for marker_path in marker_paths {
            let _ = std::fs::remove_file(marker_path);
        }
    }

    if boot_id.is_empty() {
        return;
    }

    let marker_path = format!("{}/boot_{}.marker", module_paths::LOG_DIR, boot_id);
    let marker_path_display = marker_path.clone();
    let Ok(c_path) = CString::new(marker_path) else {
        log::info!("boot marker path invalid");
        return;
    };
    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let error_no = unsafe { *libc::__errno() };
        // EEXIST 表示标记已存在，属于正常情况，无需打印
        if error_no != EEXIST {
            log::info!(
                "boot marker create failed path={} errno={}",
                marker_path_display,
                error_no
            );
        }
        return;
    }
    let mut marker_fd = UniqueFd::new(fd);
    marker_fd.reset();

    log::info!(
        "boot ok monitor={} apps={}",
        read_file_monitor_enabled_default_false(),
        count_app_config_files()
    );
}

fn read_boot_id() -> String {
    crate::platform::read_boot_id()
}

fn read_file_monitor_enabled_default_false() -> bool {
    let Ok(content) = std::fs::read_to_string(GLOBAL_CONFIG_PATH) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|value| {
            value
                .get("file_monitor_enabled")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

fn count_app_config_files() -> usize {
    let Ok(entries) = std::fs::read_dir(APPS_CONFIG_DIR) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .is_some_and(|name| !name.is_empty())
        })
        .count()
}
