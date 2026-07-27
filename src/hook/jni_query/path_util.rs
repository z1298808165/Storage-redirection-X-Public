//! MediaStore 查询改写用到的纯路径与字符串工具。
//!
//! 这些函数只做公共存储路径、相对路径和 MediaStore 取值字符串的解析与判定，不涉及
//! JNI、配置或重定向决策。从 `rewrite.rs` 分出来是因为它们与游标改写、值改写、
//! Download 占位符解析这三类逻辑没有交叉依赖，混在一个文件里会让主流程难以定位。

use super::types::{FILE_SCHEME_PREFIX, STORAGE_PREFIXES};
use crate::platform::{self, paths};

const MEDIASTORE_RELATIVE_ROOTS: [&str; 12] = [
    "Alarms",
    "Audiobooks",
    "DCIM",
    "Documents",
    "Download",
    "Movies",
    "Music",
    "Notifications",
    "Pictures",
    "Podcasts",
    "Recordings",
    "Ringtones",
];

pub(super) struct NormalizedPublicPath {
    pub(super) path: String,
    pub(super) relative: String,
}

pub(super) fn to_public_storage_path(path: &str) -> String {
    const PREFIX: &str = "/data/media/";
    if !path.starts_with(PREFIX) {
        return path.to_string();
    }
    let suffix = &path[PREFIX.len()..];
    format!("/storage/emulated/{}", suffix)
}

pub(super) fn normalize_public_storage_path(
    path: &str,
    user_id: i32,
) -> Option<NormalizedPublicPath> {
    if path.is_empty() || user_id < 0 {
        return None;
    }
    let path_text = path.strip_prefix(FILE_SCHEME_PREFIX).unwrap_or(path);
    let normalized = paths::resolve_user_path(&paths::normalize(path_text), user_id);
    if normalized.is_empty() || paths::has_unsafe_segments(&normalized) {
        return None;
    }
    let public = to_public_storage_path(&normalized);
    let storage_root = format!("/storage/emulated/{}/", user_id);
    let relative = normalize_relative_path(public.strip_prefix(&storage_root)?);
    Some(NormalizedPublicPath {
        path: public,
        relative,
    })
}

pub(super) fn relative_path_from_public_storage_path(path: &str, user_id: i32) -> String {
    if path.is_empty() || user_id < 0 {
        return String::new();
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path), user_id);
    if normalized.is_empty() || paths::has_unsafe_segments(&normalized) {
        return String::new();
    }
    let public = to_public_storage_path(&normalized);
    let storage_root = format!("/storage/emulated/{}/", user_id);
    public
        .strip_prefix(&storage_root)
        .map(normalize_relative_path)
        .unwrap_or_default()
}

pub(super) fn normalize_relative_path(path: &str) -> String {
    path.trim().replace('\\', "/").trim_matches('/').to_string()
}

pub(super) fn relative_path_is_under(relative_path: &str, root: &str) -> bool {
    let relative = normalize_relative_path(relative_path);
    relative == root
        || (relative.len() > root.len()
            && relative.starts_with(root)
            && relative.as_bytes().get(root.len()) == Some(&b'/'))
}

pub(super) fn download_relative_suffix(relative_path: &str) -> &str {
    let relative = relative_path.trim_matches('/');
    if relative == "Download" {
        ""
    } else {
        relative.strip_prefix("Download/").unwrap_or("")
    }
}

pub(super) fn relative_ends_with_suffix(relative_path: &str, suffix: &str) -> bool {
    let relative = relative_path.trim_matches('/');
    let suffix = suffix.trim_matches('/');
    !suffix.is_empty()
        && (relative == suffix
            || (relative.len() > suffix.len()
                && relative.ends_with(suffix)
                && relative.as_bytes().get(relative.len() - suffix.len() - 1) == Some(&b'/')))
}

pub(super) fn split_relative_parent_and_name(relative_path: &str) -> Option<(String, String)> {
    let relative = normalize_relative_path(relative_path);
    let slash = relative.rfind('/')?;
    if slash == 0 || slash >= relative.len() - 1 {
        return None;
    }
    Some((
        relative[..slash].to_string(),
        relative[slash + 1..].to_string(),
    ))
}

pub(super) fn has_unsafe_relative_path_segment(relative_path: &str) -> bool {
    let relative = normalize_relative_path(relative_path);
    relative
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
}

pub(super) fn normalize_bucket_path_for_user(path: &str, user_id: i32) -> String {
    if path.is_empty() || user_id < 0 {
        return String::new();
    }
    let normalized = paths::resolve_user_path(&paths::normalize(path), user_id);
    if normalized.is_empty() {
        return String::new();
    }
    let public_path = to_public_storage_path(&normalized);
    public_path.trim_end_matches('/').to_string()
}

pub(super) fn java_bucket_id(path: &str) -> i32 {
    let lower = path.to_lowercase();
    let mut hash = 0i32;
    for unit in lower.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
    }
    hash
}

pub(super) fn split_storage_path(text: &str) -> Option<(&str, bool)> {
    if text.is_empty() {
        return None;
    }

    if let Some(path_text) = text.strip_prefix(FILE_SCHEME_PREFIX)
        && STORAGE_PREFIXES
            .iter()
            .any(|prefix| path_text.starts_with(prefix))
    {
        return Some((path_text, true));
    }

    if STORAGE_PREFIXES
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return Some((text, false));
    }
    None
}

pub(super) fn split_media_store_value_path(text: &str) -> Option<(&str, bool)> {
    if text.is_empty() {
        return None;
    }

    if let Some(path_text) = text.strip_prefix(FILE_SCHEME_PREFIX)
        && is_media_store_value_path(path_text)
    {
        return Some((path_text, true));
    }

    if is_media_store_value_path(text) {
        return Some((text, false));
    }
    None
}

pub(super) fn is_media_store_value_path(path: &str) -> bool {
    STORAGE_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || path.starts_with("/data/media/")
}

pub(super) fn is_probe_path(path: &str) -> bool {
    path.ends_with("/.srx_probe") || path.ends_with("/.srx_probe/")
}

pub(super) fn is_media_store_pending_path(path: &str) -> bool {
    let path_text = path.strip_prefix(FILE_SCHEME_PREFIX).unwrap_or(path);
    path_text
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with(".pending-"))
}

pub(super) fn is_visibility_log_path(path: &str) -> bool {
    path.contains("/SRXTest/")
        || path.contains("/srx_pathowner_verify")
        || path.contains("/srx_photosgo")
}

pub(super) fn normalize_media_store_relative_value_path(
    text: &str,
    caller_uid: i32,
) -> Option<String> {
    if text.is_empty() || text.starts_with(FILE_SCHEME_PREFIX) || text.contains('\\') {
        return None;
    }
    let relative = text.strip_prefix('/').unwrap_or(text);
    if relative.is_empty() || relative.starts_with('/') {
        return None;
    }
    let mut segments = relative.split('/');
    let root = segments.next()?;
    if !MEDIASTORE_RELATIVE_ROOTS.contains(&root) {
        return None;
    }
    segments.clone().next()?;
    if segments
        .clone()
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    let user_id = platform::user_id_from_uid(caller_uid);
    if user_id < 0 {
        return None;
    }
    Some(format!("/storage/emulated/{}/{}", user_id, relative))
}
