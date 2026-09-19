//! 历史挂载状态标记文件的清理。
//!
//! 早期版本用「daemon 在应用数据目录写 `.srx_mount_status_<pid>`，应用在
//! `specialize_post` 里轮询读取」的方式传递挂载结果。该设计有两个缺陷：
//!
//! 1. **文件名带 PID**：应用每次重启都是新 PID，于是每轮都会产生一个新文件；而清理逻辑
//!    只删「当前 PID」那一个（`clear_mount_status_marker`），历史 PID 的文件永远无人清理，
//!    在 `/data/user/0/<包名>/` 下无限累积；
//! 2. **在应用数据目录里留垃圾**：这是应用自己的目录，模块没有理由往里塞私有状态文件。
//!
//! 现在挂载结果改由应用直接读 `/proc/self/mountinfo` 判定（见
//! [`crate::module_mount_source::app_redirect_mounts_in`]）：挂载源是内核记录的事实，不需要
//! 任何组件额外写文件，进程退出也不会留下残骸。
//!
//! 本模块只承担一件事：把升级前遗留的标记文件删掉，让已经累积的垃圾自愈。**不要**再往
//! 这里补写文件的逻辑——一旦需要重新引入落盘的状态通道，先回到设计层讨论，不要复活本机制。

use std::fs;

/// 历史标记文件的固定前缀。
const LEGACY_MARKER_PREFIX: &str = ".srx_mount_status_";

/// 删除应用数据目录下全部历史挂载状态标记，返回删除数量。
///
/// 不做 PID 过滤：新版本不再产生这类文件，因此目录里存在的任何一个都是遗留垃圾。
/// 目录不可读、或某个文件删不掉都不算失败——清理是尽力而为的自愈动作，不应影响应用启动。
pub(crate) fn sweep_legacy_markers(app_data_dir: &str) -> usize {
    if app_data_dir.is_empty() {
        return 0;
    }
    let Ok(entries) = fs::read_dir(app_data_dir) else {
        // 目录不可读不算失败，但要留下可诊断的记录：标记长期不减少时，这里是第一现场。
        log::debug!(
            "legacy marker sweep skipped dir={} reason=read_dir",
            app_data_dir
        );
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with(LEGACY_MARKER_PREFIX) {
            continue;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        } else {
            log::debug!(
                "legacy marker unlink failed dir={} name={}",
                app_data_dir,
                file_name
            );
        }
    }
    if removed > 0 {
        log::info!(
            "legacy mount status markers swept dir={} count={}",
            app_data_dir,
            removed
        );
    } else {
        log::debug!("legacy marker sweep found none dir={}", app_data_dir);
    }
    removed
}
