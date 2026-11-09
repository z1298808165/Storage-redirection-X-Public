//! 进程内配置监视：hook 兜底拓扑（监视桥接进程保留自身重定向）的规则即时生效。
//!
//! 这类进程没有 FUSE 会话，规则完全由进程内 [`crate::redirect::router::PathRouter`]
//! 承载；若只在决策路径上节流轮询，空闲期（无下载活动）延迟可能长达数分钟。
//!
//! 为什么不自己在本进程建 inotify watch：桥接进程运行在应用 SELinux 域，对
//! `/data/adb/modules/...` 目录的 inotify watch 被 SELinux 拒绝（真机实测 errno=13
//! EACCES；配置读取本身走 Zygisk 保留 FD 的 `/proc/self/fd/<fd>/config` 路径，文件读
//! 可用而目录 watch 不可用）。
//!
//! 但**本进程并不需要自己建 watch**：root 在 specialize 之前已经建好配置目录的
//! inotify fd（`config::watcher::init`），该 fd 随 Zygisk 保留被本进程继承。因此：
//!
//! - 事件通道可用（`watcher::events_live()`）时，直接订阅它的变更回调——事件线程阻塞
//!   在 `poll(-1)` 上，空闲期**零唤醒、零系统调用**，配置改动到达即重载；
//! - 只有事件通道不可用（init 或目录 watch 建立失败、读取线程退出）时，才退回对自身
//!   配置与全局配置做 1 秒一次的指纹比对（size + mtime + inode，tmp + rename 原子写
//!   会同时改变三者）——这是纯降级路径，正常机型不会走到。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use crate::config::watcher::{self, ChangeKind};

/// 退化路径（事件通道不可用）的指纹复查间隔。
const FALLBACK_CHECK_INTERVAL_MS: u64 = 1000;

/// 启动配置监视；失败仅告警，决策路径上的节流轮询仍作为兜底。
pub(crate) fn start_for(package_name: String) {
    if watcher::events_live() {
        // 事件驱动：把「配置变更」事件直接接到本进程的重载上，不再有任何轮询。
        let name = package_name.clone();
        watcher::set_change_callback(Arc::new(move |kind| {
            if change_touches_package(kind, &name) {
                apply_reload(&name);
            }
        }));
        watcher::start_event_reader();
        log::info!("config watch event-driven pkg={}", package_name);
        return;
    }

    // 降级路径：没有可用事件源时只能比对指纹。配置目录取自 SettingsHub，桥接进程拿到
    // 的是 Zygisk 保留 FD 的 /proc/self/fd/<fd>/config 路径。
    log::warn!(
        "config watch fallback to fingerprint pkg={} interval_ms={}",
        package_name,
        FALLBACK_CHECK_INTERVAL_MS
    );
    let config_dir = crate::config::SettingsHub::instance().config_dir();
    let result = std::thread::Builder::new()
        .name("srx-cfg-watch".to_string())
        .spawn(move || watch_loop(package_name, config_dir));
    if let Err(error) = result {
        log::warn!("config watcher spawn failed: {}", error);
    }
}

/// 一次变更是否可能影响本进程的规则。
///
/// `Full` 表示全局配置、或事件无法定位到具体包（含 inotify 队列溢出）——按最坏情况
/// 重载；`Apps` 只在名单为空或命中本进程包名时才重载。
fn change_touches_package(kind: &ChangeKind, package_name: &str) -> bool {
    match kind {
        ChangeKind::None => false,
        ChangeKind::Full(_) => true,
        ChangeKind::Apps(packages) => {
            packages.is_empty() || packages.iter().any(|name| name == package_name)
        }
    }
}

fn watch_loop(package_name: String, config_dir: String) {
    let app_config_path = format!("{config_dir}/apps/{package_name}.json");
    let global_config_path = format!("{config_dir}/global.json");
    let mut last = snapshot(&app_config_path, &global_config_path);
    loop {
        std::thread::sleep(Duration::from_millis(FALLBACK_CHECK_INTERVAL_MS));
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
