use once_cell::sync::Lazy;
use std::collections::{HashMap, VecDeque};
use std::string::String;
use std::sync::Mutex;

const DATA_MEDIA_PREFIX: &str = "/data/media/";
const STORAGE_EMULATED_PREFIX: &str = "/storage/emulated/";

// 优化：路径规范化缓存（最多保留 256 条最近的路径）
const PATH_CACHE_MAX_SIZE: usize = 256;

struct PathNormalizeCache {
    entries: HashMap<String, String>,
    order: VecDeque<String>,
}

impl PathNormalizeCache {
    fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(PATH_CACHE_MAX_SIZE),
            order: VecDeque::with_capacity(PATH_CACHE_MAX_SIZE),
        }
    }

    fn insert(&mut self, path: String, normalized: String) {
        if self.entries.insert(path.clone(), normalized).is_some() {
            // 键已存在，仅原地更新值，不改变 LRU 顺序。
            return;
        }
        if self.entries.len() > PATH_CACHE_MAX_SIZE
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.order.push_back(path);
    }
}

static PATH_NORMALIZE_CACHE: Lazy<Mutex<PathNormalizeCache>> =
    Lazy::new(|| Mutex::new(PathNormalizeCache::new()));

// 合并斜杠、去尾斜杠，再逐层解析存储别名
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return path.to_string();
    }

    // 优化：对于常用路径先查缓存
    if let Ok(cache) = PATH_NORMALIZE_CACHE.try_lock()
        && let Some(cached) = cache.entries.get(path)
    {
        return cached.clone();
    }

    let mut result = String::with_capacity(path.len());
    let mut is_last_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if !is_last_slash {
                result.push('/');
                is_last_slash = true;
            }
        } else {
            result.push(ch);
            is_last_slash = false;
        }
    }

    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }

    let result = resolve_sdcard_alias(&result);
    let result = resolve_self_primary_alias(&result);
    let result = resolve_mnt_user_primary_alias(&result);
    let result = resolve_mnt_runtime_alias(&result);
    let result = resolve_mnt_emulated_alias(&result);
    let normalized = resolve_data_media_alias(&result);

    // 优化：缓存规范化结果
    if let Ok(mut cache) = PATH_NORMALIZE_CACHE.try_lock() {
        cache.insert(path.to_string(), normalized.clone());
    }

    normalized
}

pub fn resolve_user_path(path: &str, user_id: i32) -> String {
    if user_id == 0 {
        return path.to_string();
    }

    let mut resolved = path.to_string();
    let user_str = user_id.to_string();
    resolved = resolved.replace(
        "/storage/emulated/0/",
        &format!("/storage/emulated/{}/", user_str),
    );
    if resolved == "/storage/emulated/0" {
        resolved = format!("/storage/emulated/{}", user_str);
    }

    resolved = resolved.replace("/data/user/0/", &format!("/data/user/{}/", user_str));
    resolved
}

// 替换 ${APP_DATA_DIR} / ${REDIRECT_TARGET} 占位符
pub fn resolve_placeholders(path: &str, app_data_dir: &str, redirect_target: &str) -> String {
    let mut resolved = path.to_string();
    if !app_data_dir.is_empty() {
        resolved = resolved.replace("${APP_DATA_DIR}", app_data_dir);
        resolved = resolved.replace("$APP_DATA_DIR", app_data_dir);
    }

    if !redirect_target.is_empty() {
        resolved = resolved.replace("${REDIRECT_TARGET}", redirect_target);
        resolved = resolved.replace("$REDIRECT_TARGET", redirect_target);
    }

    resolved
}

pub fn starts_with(path: &str, prefix: &str) -> bool {
    path.starts_with(prefix)
}

pub fn match_key(path: &str) -> String {
    path.to_ascii_lowercase()
}

pub fn eq_ignore_case(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub fn sort_dedup_paths_case_insensitive(paths: &mut Vec<String>) {
    paths.sort_by(|a, b| match_key(a).cmp(&match_key(b)).then_with(|| a.cmp(b)));
    paths.dedup_by(|left, right| eq_ignore_case(left, right));
}

pub fn sort_dedup_paths_longest_first_case_insensitive(paths: &mut Vec<String>) {
    paths.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| match_key(a).cmp(&match_key(b)))
            .then_with(|| a.cmp(b))
    });
    paths.dedup_by(|left, right| eq_ignore_case(left, right));
}

fn starts_with_ignore_case(path: &str, prefix: &str) -> bool {
    let path_bytes = path.as_bytes();
    let prefix_bytes = prefix.as_bytes();
    path_bytes.len() >= prefix_bytes.len()
        && path_bytes[..prefix_bytes.len()].eq_ignore_ascii_case(prefix_bytes)
}

pub fn is_same_or_child(path: &str, root: &str) -> bool {
    if path.is_empty() || root.is_empty() {
        return false;
    }
    let root = root.trim_end_matches('/');
    path.eq_ignore_ascii_case(root)
        || (path.len() > root.len()
            && path.as_bytes().get(root.len()) == Some(&b'/')
            && starts_with_ignore_case(path, root))
}

pub fn is_child(path: &str, root: &str) -> bool {
    child_suffix(path, root)
        .map(|suffix| !suffix.is_empty())
        .unwrap_or(false)
}

pub fn child_suffix<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if path.eq_ignore_ascii_case(root) {
        return Some("");
    }
    if root.is_empty() || path.len() <= root.len() {
        return None;
    }
    if path.as_bytes().get(root.len()) != Some(&b'/') || !starts_with_ignore_case(path, root) {
        return None;
    }
    Some(&path[root.len()..])
}

pub fn relative_child_path<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    child_suffix(path, root).and_then(|suffix| suffix.strip_prefix('/'))
}

pub fn storage_user_root_for_user(user_id: i32) -> String {
    format!("{}{}", STORAGE_EMULATED_PREFIX, user_id)
}

pub fn data_media_user_root_for_user(user_id: i32) -> String {
    format!("{}{}", DATA_MEDIA_PREFIX, user_id)
}

pub fn default_redirect_target(package_name: &str, user_id: i32) -> String {
    format!(
        "{}/Android/data/{}/sdcard",
        storage_user_root_for_user(user_id),
        package_name
    )
}

pub fn is_default_redirect_backend_path(path: &str) -> bool {
    let Some(rest) = path.trim_end_matches('/').strip_prefix(DATA_MEDIA_PREFIX) else {
        return false;
    };

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let Some(user_id) = parts.next() else {
        return false;
    };
    if !user_id.chars().all(|ch| ch.is_ascii_digit())
        || parts.next() != Some("Android")
        || parts.next() != Some("data")
    {
        return false;
    }

    let Some(package_name) = parts.next() else {
        return false;
    };
    is_valid_package_name(package_name) && parts.next() == Some("sdcard") && parts.next().is_none()
}

pub fn data_media_to_storage_path(path: &str) -> String {
    if !starts_with(path, DATA_MEDIA_PREFIX) {
        return path.to_string();
    }
    format!(
        "{}{}",
        STORAGE_EMULATED_PREFIX,
        &path[DATA_MEDIA_PREFIX.len()..]
    )
}

pub fn storage_to_data_media_path(path: &str) -> String {
    if !starts_with(path, STORAGE_EMULATED_PREFIX) {
        return path.to_string();
    }
    format!(
        "{}{}",
        DATA_MEDIA_PREFIX,
        &path[STORAGE_EMULATED_PREFIX.len()..]
    )
}

#[allow(dead_code)]
pub fn storage_to_data_media_for_user(storage_path: &str, user_id: i32) -> Option<String> {
    let prefix = format!("{}/", storage_user_root_for_user(user_id));
    let normalized = normalize(storage_path);
    let suffix = normalized.strip_prefix(&prefix)?;
    if suffix.is_empty() {
        return None;
    }
    Some(join(&data_media_user_root_for_user(user_id), suffix))
}

#[allow(dead_code)]
pub fn storage_relative_path_for_user(storage_path: &str, user_id: i32) -> Option<String> {
    let prefix = format!("{}/", storage_user_root_for_user(user_id));
    let normalized = normalize(storage_path);
    let suffix = normalized.strip_prefix(&prefix)?;
    if suffix.is_empty() {
        return None;
    }
    Some(suffix.to_string())
}

pub fn storage_user_root(path: &str) -> Option<String> {
    let rest = path.strip_prefix(STORAGE_EMULATED_PREFIX)?;
    let mut parts = rest.split('/');
    let user_id = parts.next()?;
    if user_id.is_empty() || !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(format!("{}{}", STORAGE_EMULATED_PREFIX, user_id))
}

pub fn data_media_user_root(path: &str) -> Option<String> {
    let rest = path.strip_prefix(DATA_MEDIA_PREFIX)?;
    let mut parts = rest.split('/');
    let user_id = parts.next()?;
    if user_id.is_empty() || !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(format!("{}{}", DATA_MEDIA_PREFIX, user_id))
}

pub fn android_private_data_media_root(
    normalized_path: &str,
    owner_package: &str,
    user_id: i32,
) -> Option<String> {
    let storage_prefix = format!("{}/Android/", storage_user_root_for_user(user_id));
    let rest = normalized_path.strip_prefix(&storage_prefix)?;

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let category = parts.next()?;
    if category != "data" && category != "media" && category != "obb" {
        return None;
    }
    if parts.next() != Some(owner_package) {
        return None;
    }

    Some(format!(
        "{}/Android/{}/{}",
        data_media_user_root_for_user(user_id),
        category,
        owner_package
    ))
}

pub fn is_filtered_media_provider_path(path: &str) -> bool {
    path.contains("/.transforms/")
        || path.ends_with("/.transforms")
        || path.contains("/.picker_transcoded/")
        || path.ends_with("/.picker_transcoded")
}

#[allow(dead_code)]
pub fn is_android_data_path(path: &str) -> bool {
    path.contains("/Android/data/") || path.ends_with("/Android/data")
}

pub fn is_android_data_or_obb_path(path: &str) -> bool {
    let normalized = normalize(path);
    let Some(storage_root) = storage_user_root(&normalized) else {
        return false;
    };
    let Some(relative) = relative_child_path(&normalized, &storage_root) else {
        return false;
    };

    let mut segments = relative.split('/').filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return false;
    };
    if !first.eq_ignore_ascii_case("Android") {
        return false;
    }

    let Some(second) = segments.next() else {
        return false;
    };
    second.eq_ignore_ascii_case("data") || second.eq_ignore_ascii_case("obb")
}

// is_recursive 允许 target 深入 rule 之下任意层级
pub fn matches(rule_path: &str, target_path: &str, is_recursive: bool) -> bool {
    if rule_path.is_empty() || target_path.is_empty() {
        return false;
    }

    if !contains_wildcards(rule_path) && !rule_path.contains("//") && !target_path.contains("//") {
        let rule = trim_match_slashes(rule_path);
        let target = trim_match_slashes(target_path);
        if rule.is_empty() || target.is_empty() {
            return false;
        }
        if target.eq_ignore_ascii_case(rule) {
            return true;
        }
        return is_recursive
            && target.len() > rule.len()
            && target.as_bytes().get(rule.len()) == Some(&b'/')
            && starts_with_ignore_case(target, rule);
    }

    let rule_segments: Vec<&str> = rule_path.split('/').filter(|s| !s.is_empty()).collect();
    let target_segments: Vec<&str> = target_path.split('/').filter(|s| !s.is_empty()).collect();
    if rule_segments.is_empty() || target_segments.len() < rule_segments.len() {
        return false;
    }

    for (rule_segment, target_segment) in rule_segments.iter().zip(target_segments.iter()) {
        if !match_segment_pattern(rule_segment, target_segment) {
            return false;
        }
    }

    if target_segments.len() == rule_segments.len() {
        return true;
    }

    is_recursive
}

pub fn contains_wildcards(path: &str) -> bool {
    path.contains('*') || path.contains('?')
}

pub fn split_exclusion_rules(rules: &[String]) -> (Vec<String>, Vec<String>) {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix('!') {
            let stripped = stripped.trim_start();
            if !stripped.is_empty() {
                excludes.push(stripped.to_string());
            }
        } else {
            includes.push(trimmed.to_string());
        }
    }
    sort_dedup_paths_case_insensitive(&mut includes);
    sort_dedup_paths_case_insensitive(&mut excludes);
    (includes, excludes)
}

pub fn overlapping_exclusion_rules(includes: &[String], excludes: &[String]) -> Vec<String> {
    let mut effective: Vec<String> = excludes
        .iter()
        .filter(|excluded| {
            includes.iter().any(|included| {
                matches(included, excluded, true) || matches(excluded, included, true)
            })
        })
        .cloned()
        .collect();
    sort_dedup_paths_case_insensitive(&mut effective);
    effective
}

pub fn concrete_prefix_before_wildcard(path: &str) -> String {
    let normalized = normalize(path);
    if normalized.is_empty() || !contains_wildcards(&normalized) {
        return normalized;
    }

    let mut kept = Vec::new();
    for segment in normalized.split('/').filter(|segment| !segment.is_empty()) {
        if contains_wildcards(segment) {
            break;
        }
        kept.push(segment);
    }
    if kept.is_empty() {
        return String::new();
    }

    let prefix = kept.join("/");
    if normalized.starts_with('/') {
        normalize(&format!("/{prefix}"))
    } else {
        normalize(&prefix)
    }
}

pub fn wildcard_mount_fallback_parent(resolved_path: &str, storage_root: &str) -> Option<String> {
    let normalized = normalize(resolved_path);
    if normalized.is_empty() || !contains_wildcards(&normalized) {
        return None;
    }

    let prefix = concrete_prefix_before_wildcard(&normalized);
    if prefix.is_empty()
        || eq_ignore_case(&prefix, storage_root)
        || !is_child(&prefix, storage_root)
    {
        return None;
    }

    Some(prefix)
}

pub fn wildcard_policy_fallback_parent(resolved_path: &str, storage_root: &str) -> Option<String> {
    let normalized = normalize(resolved_path);
    if is_terminal_file_wildcard_rule(&normalized) {
        return None;
    }
    wildcard_mount_fallback_parent(&normalized, storage_root)
}

fn is_terminal_file_wildcard_rule(path: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return false;
    };
    contains_wildcards(last) && last.contains('.')
}

pub fn matches_xldownload_alias(rule_path: &str, target_path: &str) -> bool {
    if rule_path.is_empty() || target_path.is_empty() {
        return false;
    }

    let rule_segments: Vec<&str> = rule_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let target_segments: Vec<&str> = target_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if rule_segments.is_empty() || target_segments.len() < rule_segments.len() {
        return false;
    }

    for (rule_segment, target_segment) in rule_segments.iter().zip(target_segments.iter()) {
        if rule_segment.eq_ignore_ascii_case(".xldownload")
            && target_segment.eq_ignore_ascii_case(".xldownload")
        {
            continue;
        }
        if rule_segment != target_segment {
            return false;
        }
    }
    true
}

fn trim_match_slashes(path: &str) -> &str {
    path.trim_end_matches('/')
}

// 支持 * 和 ? 通配，单段匹配不跨 /
fn match_segment_pattern(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern.eq_ignore_ascii_case(text);
    }

    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut pattern_idx = 0usize;
    let mut text_idx = 0usize;
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;

    while text_idx < text_chars.len() {
        if pattern_idx < pattern_chars.len()
            && (pattern_chars[pattern_idx] == '?'
                || pattern_chars[pattern_idx].eq_ignore_ascii_case(&text_chars[text_idx]))
        {
            pattern_idx += 1;
            text_idx += 1;
            continue;
        }

        if pattern_idx < pattern_chars.len() && pattern_chars[pattern_idx] == '*' {
            star_idx = Some(pattern_idx);
            match_idx = text_idx;
            pattern_idx += 1;
            continue;
        }

        if let Some(star_pos) = star_idx {
            pattern_idx = star_pos + 1;
            match_idx += 1;
            text_idx = match_idx;
            continue;
        }

        return false;
    }

    while pattern_idx < pattern_chars.len() && pattern_chars[pattern_idx] == '*' {
        pattern_idx += 1;
    }

    pattern_idx == pattern_chars.len()
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

pub fn join(base: &str, relative: &str) -> String {
    if base.is_empty() {
        return relative.to_string();
    }
    if relative.is_empty() {
        return base.to_string();
    }
    if relative.starts_with('/') {
        return relative.to_string();
    }

    let mut result = base.to_string();
    if !result.ends_with('/') {
        result.push('/');
    }
    result.push_str(relative);
    result
}

pub fn parent(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return path.to_string();
    }

    let normalized = normalize(path);
    if let Some(pos) = normalized.rfind('/') {
        if pos == 0 {
            return "/".to_string();
        }
        return normalized[..pos].to_string();
    }

    String::new()
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

pub fn is_absolute(path: &str) -> bool {
    !path.is_empty() && path.starts_with('/')
}

// /sdcard -> /storage/emulated/0
fn resolve_sdcard_alias(path: &str) -> String {
    if let Some(suffix) = path.strip_prefix("/sdcard") {
        return format!("/storage/emulated/0{}", suffix);
    }
    path.to_string()
}

// /storage/self/primary -> /storage/emulated/0
fn resolve_self_primary_alias(path: &str) -> String {
    const PREFIX: &str = "/storage/self/primary";
    if let Some(suffix) = path.strip_prefix(PREFIX) {
        return format!("/storage/emulated/0{}", suffix);
    }
    path.to_string()
}

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

// 解析 /storage/emulated/<user_id>/... 中的 user_id，失败返回 -1
pub fn extract_user_id_from_storage_path(path: &str) -> i32 {
    const PREFIX: &str = "/storage/emulated/";
    if !path.starts_with(PREFIX) {
        return -1;
    }

    let user_start = PREFIX.len();
    let user_end = path[user_start..]
        .find('/')
        .map(|idx| user_start + idx)
        .unwrap_or(path.len());
    if user_start >= user_end {
        return -1;
    }

    if !path[user_start..user_end]
        .chars()
        .all(|ch| ch.is_ascii_digit())
    {
        return -1;
    }

    path[user_start..user_end].parse().unwrap_or(-1)
}

pub fn extract_android_private_path_owner(path: &str) -> String {
    const PREFIX: &str = "/storage/emulated/";
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return String::new();
    };

    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let Some(user_id) = parts.next() else {
        return String::new();
    };
    if !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return String::new();
    }
    if parts.next() != Some("Android") {
        return String::new();
    }

    match parts.next() {
        Some("data" | "media" | "obb") => {}
        _ => return String::new(),
    }

    let Some(package_name) = parts.next() else {
        return String::new();
    };
    if is_valid_package_name(package_name) {
        package_name.to_string()
    } else {
        String::new()
    }
}

fn is_valid_package_name(package_name: &str) -> bool {
    !package_name.is_empty()
        && package_name.contains('.')
        && !package_name.starts_with('.')
        && !package_name.ends_with('.')
        && package_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
        && package_name.split('.').all(|part| !part.is_empty())
}

#[cfg(target_os = "android")]
pub fn monotonic_ms() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts as *mut _);
    }
    ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
}

#[cfg(not(target_os = "android"))]
pub fn monotonic_ms() -> i64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
