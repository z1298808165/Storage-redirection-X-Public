//! 进程内配置监视线程：hook 兜底拓扑（监视桥接进程保留自身重定向）的规则即时生效。
//!
//! 这类进程没有 FUSE 会话，规则完全由进程内 [`crate::redirect::router::PathRouter`]
//! 承载；若只在决策路径上节流轮询，空闲期（无下载活动）延迟可能长达数分钟。
//!
//! 为什么不用 inotify：桥接进程运行在应用 SELinux 域，对 `/data/adb/modules/...`
//! 目录的 inotify watch 被 SELinux 拒绝（真机实测 errno=13 EACCES；配置读取本身
//! 走 Zygisk 保留 FD 的 `/proc/self/fd/<fd>/config` 路径，文件读可用而目录 watch
//! 不可用）。因此退化为专用线程对自身配置与全局配置做 1 秒级指纹比对
//! （size + mtime + inode，tmp+rename 原子写会同时改变三者），变更立即触发
//! SettingsHub 重载 + PathRouter 重建，实测秒级生效且不依赖下载活动。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, UNIX_EPOCH};

const CHECK_INTERVAL_MS: u64 = 1000;

/// 启动配置监视线程；失败仅告警，决策路径上的节流轮询仍作为兜底。
pub(crate) fn start_for(package_name: String) {
    // 配置目录取自 SettingsHub：桥接进程拿到的是 Zygisk 保留 FD 的
    // /proc/self/fd/<fd>/config 路径。
    let config_dir = crate::config::SettingsHub::instance().config_dir();
    let result = std::thread::Builder::new()
        .name("srx-cfg-watch".to_string())
        .spawn(move || watch_loop(package_name, config_dir));
    if let Err(error) = result {
        log::warn!("config watcher spawn failed: {}", error);
    }
}

fn watch_loop(package_name: String, config_dir: String) {
    let app_config_path = format!("{config_dir}/apps/{package_name}.json");
    let global_config_path = format!("{config_dir}/global.json");
    let mut last = snapshot(&app_config_path, &global_config_path);
    loop {
        std::thread::sleep(Duration::from_millis(CHECK_INTERVAL_MS));
        let current = snapshot(&app_config_path, &global_config_path);
        if current == last {
            continue;
        }
        last = current;
        apply_reload(&package_name);
    }
}

/// 两个配置文件的指纹（不存在/不可读视为稳定状态，避免读失败误触发重载）。
fn snapshot(app_config_path: &str, global_config_path: &str) -> (u64, u64) {
    (
        file_fingerprint(app_config_path),
        file_fingerprint(global_config_path),
    )
}

fn file_fingerprint(path: &str) -> u64 {
    use std::os::unix::fs::MetadataExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return 0;
    };
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    metadata.ino().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified_nanos.hash(&mut hasher);
    hasher.finish()
}

fn apply_reload(package_name: &str) {
    crate::hook::refresh_runtime_config_after_disk_change();
    if crate::redirect::reconfigure_router_from_settings(package_name) {
        log::info!("config watcher reloaded router pkg={}", package_name);
    }
}
