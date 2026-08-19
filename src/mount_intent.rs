use crate::config::StorageBackendMode;
use crate::platform::module_paths;
use std::fs;
use std::path::PathBuf;

/// 挂载事实文件只在成功后生成；意图文件用于记录请求尚未执行或正在执行的阶段。
/// 两者分离后，管理端可以观察到挂载前状态，也不会把失败请求误当成可卸载挂载点。
pub fn mark_planned(
    package_name: &str,
    pid: i32,
    uid: i32,
    backend: StorageBackendMode,
    config_version: u64,
) {
    write_state(package_name, pid, uid, backend, config_version, "planned");
}

pub fn mark_state(
    package_name: &str,
    pid: i32,
    uid: i32,
    backend: StorageBackendMode,
    config_version: u64,
    state: &str,
) {
    write_state(package_name, pid, uid, backend, config_version, state);
}

fn write_state(
    package_name: &str,
    pid: i32,
    uid: i32,
    backend: StorageBackendMode,
    config_version: u64,
    state: &str,
) {
    if package_name.is_empty() || pid <= 0 || uid < 0 {
        return;
    }
    if fs::create_dir_all(module_paths::MOUNT_INTENT_DIR).is_err() {
        log::warn!("mount intent mkdir failed pkg={} pid={}", package_name, pid);
        return;
    }
    let safe_package = module_paths::sanitize_name(package_name);
    let path = PathBuf::from(module_paths::MOUNT_INTENT_DIR)
        .join(format!("{}_{}.intent", safe_package, pid));
    let temp = path.with_extension("intent.tmp");
    let start_time = crate::platform::process_start_time_ticks(pid)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let content = format!(
        "state={}\npackage={}\npid={}\nuid={}\napp_start_time={}\nbackend={}\nconfig_version={}\n",
        state,
        package_name,
        pid,
        uid,
        start_time,
        backend.as_str(),
        config_version
    );
    if fs::write(&temp, content).is_ok() && fs::rename(&temp, &path).is_ok() {
        log::debug!(
            "mount intent state={} pkg={} pid={} backend={}",
            state,
            package_name,
            pid,
            backend.as_str()
        );
    } else {
        let _ = fs::remove_file(temp);
        log::warn!(
            "mount intent write failed pkg={} pid={} state={}",
            package_name,
            pid,
            state
        );
    }
}

/// 清理已退出进程或 PID 复用后的挂载意图文件。
///
/// intent 文件用于观察挂载请求生命周期，不应因为 daemon 重启而永久累积。判断以
/// `/proc/<pid>` 的 starttime 为准，避免仅按 PID 清理时误删新进程的记录。
// quality-allow(lint-suppression): 该入口仅由 Android daemon reconcile 调用，cdylib 目标不会直接调用。
#[allow(dead_code)]
pub fn prune_stale() -> usize {
    let Ok(entries) = fs::read_dir(module_paths::MOUNT_INTENT_DIR) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("intent") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let pid = content
            .lines()
            .find_map(|line| line.strip_prefix("pid=")?.parse::<i32>().ok());
        let expected_start = content
            .lines()
            .find_map(|line| line.strip_prefix("app_start_time=")?.parse::<u64>().ok());
        let is_current = pid
            .zip(expected_start)
            .and_then(|(pid, expected)| {
                crate::platform::process_start_time_ticks(pid).map(|actual| actual == expected)
            })
            .unwrap_or(false);
        if !is_current && fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    if removed > 0 {
        log::info!("mount intent pruned stale count={}", removed);
    }
    removed
}
