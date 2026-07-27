//! 来源提示（hint）文件的读写、解析与缓存。
//!
//! 提示文件是跨进程共享调用方归因线索的载体：写入方在 hook 侧记录，读取方在归因时
//! 取用。这里集中放置落盘格式、行解析、指纹缓存与权限修正，把「文件长什么样」与
//! 「怎么用提示做归因」分开——`source_hint.rs` 只保留归因判定逻辑。
//!
//! 提示文件内容由其它进程写入，解析时按字段逐项校验，不信任其中的包名与路径。

use super::source_hint::{PathCallerHint, PrivateOwnerHint, is_public_storage_hint_path};
use crate::platform::module_paths;
use once_cell::sync::Lazy;
use std::io::Write;
use std::sync::{Arc, Mutex};

// 提示文件格式版本，解析时用于拒绝旧格式行。
const HINT_VERSION: &str = "3";
const RECENT_PATH_CALLER_HINT_VERSION: &str = "2";

pub(super) fn write_hint_file(hints: &[PrivateOwnerHint]) {
    let path = std::path::Path::new(module_paths::RECENT_SOURCE_HINT_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::File::create(path) else {
        return;
    };
    for hint in hints {
        let _ = writeln!(
            file,
            "{}|{}|{}|{}|{}|{}|{}|{}",
            HINT_VERSION,
            hint.user_id,
            hint.updated_ms,
            hint.owner_package,
            hint.package_name,
            hint.tokens.join(","),
            hint.source,
            hint.confidence
        );
    }
    chmod_hint_file(path);
    invalidate_cached_hint_file(&RECENT_SOURCE_HINT_FILE_CACHE);
}

pub(super) fn write_path_hint_file(hints: &[PathCallerHint]) {
    let path = std::path::Path::new(module_paths::RECENT_PATH_CALLER_HINT_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::File::create(path) else {
        return;
    };
    for hint in hints {
        let _ = writeln!(
            file,
            "{}|{}|{}|{}|{}|{}|{}|{}",
            RECENT_PATH_CALLER_HINT_VERSION,
            hint.user_id,
            hint.updated_ms,
            hint.package_name,
            hint.source,
            hint.confidence,
            hint.op_filter,
            hint.path
        );
    }
    chmod_hint_file(path);
    invalidate_cached_hint_file(&RECENT_PATH_CALLER_HINT_FILE_CACHE);
}

pub(super) fn read_hint_file() -> Arc<Vec<PrivateOwnerHint>> {
    read_cached_hint_file(
        std::path::Path::new(module_paths::RECENT_SOURCE_HINT_FILE),
        &RECENT_SOURCE_HINT_FILE_CACHE,
        |content| {
            content
                .lines()
                .filter_map(|line| parse_hint_line(line.trim()))
                .collect()
        },
    )
}

pub(super) fn read_path_hint_file() -> Arc<Vec<PathCallerHint>> {
    read_cached_hint_file(
        std::path::Path::new(module_paths::RECENT_PATH_CALLER_HINT_FILE),
        &RECENT_PATH_CALLER_HINT_FILE_CACHE,
        |content| {
            content
                .lines()
                .filter_map(|line| parse_path_hint_line(line.trim()))
                .collect()
        },
    )
}

pub(super) fn read_cached_hint_file<T>(
    path: &std::path::Path,
    cache: &Mutex<CachedHintFile<T>>,
    parse: impl FnOnce(&str) -> Vec<T>,
) -> Arc<Vec<T>> {
    let fingerprint = hint_file_fingerprint(path);
    let Ok(mut cached) = cache.lock() else {
        return std::fs::read_to_string(path)
            .ok()
            .map(|content| Arc::new(parse(&content)))
            .unwrap_or_else(|| Arc::new(Vec::new()));
    };

    if cached.fingerprint == fingerprint {
        return Arc::clone(&cached.values);
    }

    let values = std::fs::read_to_string(path)
        .ok()
        .map(|content| parse(&content))
        .unwrap_or_default();
    cached.fingerprint = fingerprint;
    cached.values = Arc::new(values);
    Arc::clone(&cached.values)
}

pub(super) fn invalidate_cached_hint_file<T>(cache: &Mutex<CachedHintFile<T>>) {
    if let Ok(mut cached) = cache.lock() {
        cached.fingerprint = None;
        cached.values = Arc::new(Vec::new());
    }
}

pub(super) fn hint_file_fingerprint(path: &std::path::Path) -> Option<HintFileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(HintFileFingerprint {
        length: metadata.len(),
        modified_ns,
    })
}

pub(super) fn parse_hint_line(line: &str) -> Option<PrivateOwnerHint> {
    let parts: Vec<&str> = line.split('|').collect();
    let (
        user_id_part,
        updated_ms_part,
        owner_package_part,
        package_name_part,
        tokens_part,
        source,
        confidence,
    ) = match parts.as_slice() {
        ["1", user_id, updated_ms, package_name, tokens] => (
            *user_id,
            *updated_ms,
            *package_name,
            *package_name,
            *tokens,
            "recent_private_owner",
            "medium",
        ),
        [
            "2",
            user_id,
            updated_ms,
            package_name,
            tokens,
            source,
            confidence,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *package_name,
            *tokens,
            normalize_hint_source(source)?,
            normalize_hint_confidence(confidence)?,
        ),
        [
            "3",
            user_id,
            updated_ms,
            owner_package,
            package_name,
            tokens,
            source,
            confidence,
        ] => (
            *user_id,
            *updated_ms,
            *owner_package,
            *package_name,
            *tokens,
            normalize_hint_source(source)?,
            normalize_hint_confidence(confidence)?,
        ),
        _ => return None,
    };
    let user_id = user_id_part.parse().ok()?;
    let updated_ms = updated_ms_part.parse().ok()?;
    let owner_package = owner_package_part.to_string();
    let package_name = package_name_part.to_string();
    if !is_valid_package_name(&owner_package) || !is_valid_package_name(&package_name) {
        return None;
    }
    let tokens = tokens_part
        .split(',')
        .filter(|token| !token.is_empty() && token.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    Some(PrivateOwnerHint {
        user_id,
        updated_ms,
        owner_package,
        package_name,
        caller_uid: -1,
        tokens,
        source,
        confidence,
    })
}

pub(super) fn parse_path_hint_line(line: &str) -> Option<PathCallerHint> {
    let parts: Vec<&str> = line.split('|').collect();
    let (
        user_id_part,
        updated_ms_part,
        package_name_part,
        source_part,
        confidence_part,
        op_filter_part,
        path_part,
    ) = match parts.as_slice() {
        [
            "1",
            user_id,
            updated_ms,
            package_name,
            source,
            confidence,
            path,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *source,
            *confidence,
            "provider_open",
            *path,
        ),
        [
            "2",
            user_id,
            updated_ms,
            package_name,
            source,
            confidence,
            op_filter,
            path,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *source,
            *confidence,
            *op_filter,
            *path,
        ),
        _ => return None,
    };
    let user_id = user_id_part.parse().ok()?;
    let updated_ms = updated_ms_part.parse().ok()?;
    let package_name = package_name_part.to_string();
    let source = normalize_path_hint_source(source_part)?;
    let confidence = normalize_hint_confidence(confidence_part)?;
    let op_filter = normalize_path_hint_op_filter(op_filter_part)?;
    let path = path_part.to_string();
    if !is_valid_package_name(&package_name) || !is_public_storage_hint_path(&path, user_id) {
        return None;
    }
    Some(PathCallerHint {
        user_id,
        updated_ms,
        package_name,
        path,
        source,
        confidence,
        op_filter,
    })
}

pub(super) fn normalize_hint_source(value: &str) -> Option<&'static str> {
    match value {
        "recent_private_owner" => Some("recent_private_owner"),
        "recent_private_caller" => Some("recent_private_caller"),
        "recent_private_token" => Some("recent_private_token"),
        _ => None,
    }
}

pub(super) fn normalize_path_hint_source(value: &str) -> Option<&'static str> {
    match value {
        "provider_open" => Some("provider_open"),
        "saf_provider" => Some("saf_provider"),
        "query_access" => Some("query_access"),
        _ => None,
    }
}

pub(super) fn normalize_path_hint_op_filter(value: &str) -> Option<&'static str> {
    match value {
        "provider_open" => Some("provider_open"),
        "provider_open:create" => Some("provider_open:create"),
        "provider_open:read" => Some("provider_open:read"),
        "provider_open:write" => Some("provider_open:write"),
        _ => None,
    }
}

pub(super) fn normalize_hint_confidence(value: &str) -> Option<&'static str> {
    match value {
        "high" => Some("high"),
        "medium" => Some("medium"),
        "fallback" => Some("fallback"),
        _ => None,
    }
}

pub(super) fn is_valid_package_name(value: &str) -> bool {
    !value.is_empty()
        && value.contains('.')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
}

pub(super) fn chmod_hint_file(path: &std::path::Path) {
    let Some(path_text) = path.to_str() else {
        return;
    };
    let Ok(c_path) = std::ffi::CString::new(path_text) else {
        return;
    };
    // SAFETY: c_path 是上方构造的 NUL 结尾 CString，在本次调用期间保持存活；
    // chmod 只读取该指针且不保留它。提示文件需要对其它进程可写，因此放开权限。
    unsafe {
        libc::chmod(c_path.as_ptr(), 0o666);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HintFileFingerprint {
    length: u64,
    modified_ns: u128,
}

pub(super) struct CachedHintFile<T> {
    fingerprint: Option<HintFileFingerprint>,
    values: Arc<Vec<T>>,
}

impl<T> Default for CachedHintFile<T> {
    fn default() -> Self {
        Self {
            fingerprint: None,
            values: Arc::new(Vec::new()),
        }
    }
}

static RECENT_SOURCE_HINT_FILE_CACHE: Lazy<Mutex<CachedHintFile<PrivateOwnerHint>>> =
    Lazy::new(|| Mutex::new(CachedHintFile::default()));

static RECENT_PATH_CALLER_HINT_FILE_CACHE: Lazy<Mutex<CachedHintFile<PathCallerHint>>> =
    Lazy::new(|| Mutex::new(CachedHintFile::default()));
