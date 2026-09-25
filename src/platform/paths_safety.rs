// 路径安全性与特殊文件判定。
//
// 这些判定用于拒绝越界规则、保护应用私有目录和过滤 MediaProvider 的中间路径；
// 判定口径必须与配置校验阶段一致，否则会出现「配置通过但运行时拒绝」的偏差。

use crate::platform::paths::{STORAGE_EMULATED_PREFIX, is_absolute, normalize};
use crate::platform::paths_roots::is_valid_package_name;

pub fn is_filtered_media_provider_path(path: &str) -> bool {
    path.contains("/.transforms/")
        || path.ends_with("/.transforms")
        || path.contains("/.picker_transcoded/")
        || path.ends_with("/.picker_transcoded")
}

/// 把 MediaStore 未提交文件的 `.pending-<id>-<显示名>` 路径还原为显示名路径。
///
/// MediaProvider 在写入完成前使用该临时命名，只有还原后的路径才是用户在相册或
/// 文件管理器里看到的位置。文件监视记录、FUSE 策略判定和写入归因都依赖同一份
/// 解析结果，因此必须共用实现：命名格式随 Android 版本变化时只需改这一处，
/// 否则同一次写入会在不同记录里显示成不同路径。
///
/// 传入路径需已规范化；不符合 pending 命名时返回 `None`。
pub fn media_store_pending_display_path(path: &str) -> Option<String> {
    let slash = path.rfind('/')?;
    let file_name = &path[slash + 1..];
    let pending_tail = file_name.strip_prefix(".pending-")?;
    let display_name_start = pending_tail.find('-')? + 1;
    if display_name_start >= pending_tail.len() {
        return None;
    }

    Some(format!(
        "{}/{}",
        path[..slash].trim_end_matches('/'),
        &pending_tail[display_name_start..]
    ))
}

// 存在 . 或 .. 段视为不安全
pub fn has_unsafe_segments(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }

    let mut start = 0usize;
    let bytes = path.as_bytes();
    while start <= bytes.len() {
        let end = match path[start..].find('/') {
            Some(idx) => start + idx,
            None => path.len(),
        };
        let segment = &path[start..end];
        if segment == "." || segment == ".." {
            return true;
        }
        if end == path.len() {
            break;
        }
        start = end + 1;
    }

    false
}

pub fn is_safe_namespace_path(path: &str) -> bool {
    if path.is_empty() || path == "/" || !is_absolute(path) || has_unsafe_segments(path) {
        return false;
    }
    ["/proc", "/sys", "/dev", "/data/adb"]
        .iter()
        .all(|prefix| path != *prefix && !path.starts_with(&format!("{prefix}/")))
}

/// 判断路径是否正好是应用私有数据目录的根目录。
///
/// 应用私有目录下的子路径可以作为映射边界，但私有根目录本身不能被重定向：
/// 对它创建挂载点或修复权限会改变应用自身数据目录的所有者、权限或可见性。
/// 调用方应先完成用户路径与存储别名解析，再使用该检查；这里仍保留主要别名和
/// `/data/data` 历史写法，作为跨入口的最后一道防线。
pub fn is_application_private_root(path: &str) -> bool {
    let normalized_path = normalize(path);
    let normalized = normalized_path.trim_end_matches('/');
    if normalized.is_empty() {
        return false;
    }

    let storage_root = STORAGE_EMULATED_PREFIX;
    if let Some(rest) = normalized.strip_prefix(storage_root) {
        let mut parts = rest.split('/').filter(|part| !part.is_empty());
        let Some(user_id) = parts.next() else {
            return false;
        };
        if user_id != "legacy" && !user_id.chars().all(|ch| ch.is_ascii_digit()) {
            return false;
        }
        return parts.next() == Some("Android")
            && matches!(parts.next(), Some("data" | "media" | "obb"))
            && parts.next().is_some_and(is_valid_package_name)
            && parts.next().is_none();
    }

    let mut parts = normalized
        .strip_prefix('/')
        .unwrap_or(normalized)
        .split('/')
        .filter(|part| !part.is_empty());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("data"), Some("data"), Some(package_name), None) => {
            is_valid_package_name(package_name)
        }
        (Some("data"), Some("user" | "user_de"), Some(user_id), Some(package_name)) => {
            user_id.chars().all(|ch| ch.is_ascii_digit())
                && is_valid_package_name(package_name)
                && parts.next().is_none()
        }
        _ => false,
    }
}

pub fn is_sqlite_database_or_sidecar_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    [
        ".db",
        ".db-shm",
        ".db-wal",
        ".db-journal",
        ".sqlite",
        ".sqlite-shm",
        ".sqlite-wal",
        ".sqlite-journal",
        ".sqlite3",
        ".sqlite3-shm",
        ".sqlite3-wal",
        ".sqlite3-journal",
    ]
    .iter()
    .any(|suffix| file_name.ends_with(suffix))
}
