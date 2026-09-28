// Android 共享存储路径别名的解析。
//
// 同一份文件在 Android 上有多种可见路径（`/sdcard`、`/storage/self/primary`、
// `/mnt/runtime/*/emulated/<user>`、`/data/media/<user>` 等）。挂载、策略匹配和
// 文件监视必须把所有别名解析到同一个规范前缀，否则同一份文件会被当成多份。

use crate::platform::paths::DATA_MEDIA_PREFIX;

pub(crate) fn has_potential_storage_alias(path: &str) -> bool {
    path.starts_with("/sdcard")
        || path.starts_with("/storage/self/primary")
        || path.starts_with("/mnt/")
        || path.starts_with(DATA_MEDIA_PREFIX)
}

// 各别名解析分支内部只需借用，因此这里收 &str：调用方在无需折叠斜杠时可以直接
// 传入原始路径，不必先复制一份仅为满足所有权。
pub(crate) fn resolve_storage_alias(path: &str) -> String {
    if path.starts_with("/sdcard") {
        return resolve_sdcard_alias(path);
    }
    if path.starts_with("/storage/self/primary") {
        return resolve_self_primary_alias(path);
    }
    if path.starts_with("/mnt/runtime/") {
        return resolve_mnt_runtime_alias(path);
    }
    if path.starts_with("/mnt/user/") {
        let primary = resolve_mnt_user_primary_alias(path);
        if primary != path {
            return primary;
        }
        return resolve_mnt_emulated_alias(path);
    }
    if path.starts_with("/mnt/installer/")
        || path.starts_with("/mnt/androidwritable/")
        || path.starts_with("/mnt/pass_through/")
    {
        return resolve_mnt_emulated_alias(path);
    }
    if path.starts_with(DATA_MEDIA_PREFIX) {
        return resolve_data_media_alias(path);
    }
    path.to_string()
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /sdcard -> /storage/emulated/0
fn resolve_sdcard_alias(path: &str) -> String {
    if let Some(suffix) = path.strip_prefix("/sdcard") {
        return format!("/storage/emulated/0{}", suffix);
    }
    path.to_string()
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /storage/self/primary -> /storage/emulated/0
fn resolve_self_primary_alias(path: &str) -> String {
    const PREFIX: &str = "/storage/self/primary";
    if let Some(suffix) = path.strip_prefix(PREFIX) {
        return format!("/storage/emulated/0{}", suffix);
    }
    path.to_string()
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /mnt/user/N/primary -> /storage/emulated/N
fn resolve_mnt_user_primary_alias(path: &str) -> String {
    const PREFIX: &str = "/mnt/user/";
    if !path.starts_with(PREFIX) {
        return path.to_string();
    }

    let user_start = PREFIX.len();
    let user_end = match path[user_start..].find('/') {
        Some(idx) => user_start + idx,
        None => return path.to_string(),
    };
    if user_start >= user_end {
        return path.to_string();
    }

    if !path[user_start..user_end]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        return path.to_string();
    }

    const PRIMARY_SEGMENT: &str = "/primary";
    if !path[user_end..].starts_with(PRIMARY_SEGMENT) {
        return path.to_string();
    }

    let user_id = &path[user_start..user_end];
    format!(
        "/storage/emulated/{}{}",
        user_id,
        &path[user_end + PRIMARY_SEGMENT.len()..]
    )
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /mnt/runtime/{default,read,write,full}/emulated/N -> /storage/emulated/N
fn resolve_mnt_runtime_alias(path: &str) -> String {
    const PREFIX: &str = "/mnt/runtime/";
    if !path.starts_with(PREFIX) {
        return path.to_string();
    }

    let tier_start = PREFIX.len();
    let tier_end = match path[tier_start..].find('/') {
        Some(idx) => tier_start + idx,
        None => return path.to_string(),
    };
    let tier = &path[tier_start..tier_end];
    if tier != "default" && tier != "read" && tier != "write" && tier != "full" {
        return path.to_string();
    }

    const EMULATED_SEGMENT: &str = "/emulated/";
    if !path[tier_end..].starts_with(EMULATED_SEGMENT) {
        return path.to_string();
    }

    let user_start = tier_end + EMULATED_SEGMENT.len();
    let user_end = match path[user_start..].find('/') {
        Some(idx) => user_start + idx,
        None => path.len(),
    };
    if user_start >= user_end {
        return path.to_string();
    }
    if !path[user_start..user_end]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        return path.to_string();
    }

    let user_id = &path[user_start..user_end];
    if user_end == path.len() {
        return format!("/storage/emulated/{}", user_id);
    }
    format!("/storage/emulated/{}{}", user_id, &path[user_end..])
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /mnt/{user,installer,androidwritable,pass_through}/OWNER/emulated/N -> /storage/emulated/N
fn resolve_mnt_emulated_alias(path: &str) -> String {
    const PREFIXES: [&str; 4] = [
        "/mnt/user/",
        "/mnt/installer/",
        "/mnt/androidwritable/",
        "/mnt/pass_through/",
    ];

    let matched_prefix = PREFIXES.iter().find(|prefix| path.starts_with(*prefix));
    let Some(prefix) = matched_prefix else {
        return path.to_string();
    };

    let prefix_len = prefix.len();
    let owner_start = prefix_len;
    let owner_end = match path[owner_start..].find('/') {
        Some(idx) => owner_start + idx,
        None => return path.to_string(),
    };
    if owner_start >= owner_end {
        return path.to_string();
    }
    if !path[owner_start..owner_end]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        return path.to_string();
    }

    const EMULATED_SEGMENT: &str = "/emulated/";
    if !path[owner_end..].starts_with(EMULATED_SEGMENT) {
        return path.to_string();
    }

    let user_start = owner_end + EMULATED_SEGMENT.len();
    let user_end = match path[user_start..].find('/') {
        Some(idx) => user_start + idx,
        None => path.len(),
    };
    if user_start >= user_end {
        return path.to_string();
    }
    if !path[user_start..user_end]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        return path.to_string();
    }

    let user_id = &path[user_start..user_end];
    if user_end == path.len() {
        return format!("/storage/emulated/{}", user_id);
    }
    format!("/storage/emulated/{}{}", user_id, &path[user_end..])
}

// quality-allow(chinese-language): 该行是 Android 存储别名的实际路径形态，改写会失去可比对的原样。
// /data/media/N -> /storage/emulated/N
fn resolve_data_media_alias(path: &str) -> String {
    const PREFIX: &str = "/data/media/";
    if !path.starts_with(PREFIX) {
        return path.to_string();
    }

    let user_start = PREFIX.len();
    let user_end = match path[user_start..].find('/') {
        Some(idx) => user_start + idx,
        None => path.len(),
    };
    if user_start >= user_end {
        return path.to_string();
    }
    if !path[user_start..user_end]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        return path.to_string();
    }

    let user_id = &path[user_start..user_end];
    if user_end == path.len() {
        return format!("/storage/emulated/{}", user_id);
    }
    format!("/storage/emulated/{}{}", user_id, &path[user_end..])
}
