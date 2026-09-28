use super::lru_cache::LruCache;
use once_cell::sync::Lazy;
use std::cell::RefCell;
use std::string::String;
use std::sync::Mutex;

pub(crate) const DATA_MEDIA_PREFIX: &str = "/data/media/";
pub(crate) const STORAGE_EMULATED_PREFIX: &str = "/storage/emulated/";

// 优化：路径规范化缓存（最多保留 256 条最近的路径）
const PATH_CACHE_MAX_SIZE: usize = 256;

struct PathNormalizeCache {
    cache: LruCache<String, String>,
}

impl PathNormalizeCache {
    fn new() -> Self {
        Self {
            cache: LruCache::new(PATH_CACHE_MAX_SIZE),
        }
    }

    fn insert(&mut self, path: String, normalized: String) {
        self.cache.insert(path, normalized);
    }

    fn get(&mut self, path: &str) -> Option<String> {
        self.cache.get(&path.to_string())
    }
}

static PATH_NORMALIZE_CACHE: Lazy<Mutex<PathNormalizeCache>> =
    Lazy::new(|| Mutex::new(PathNormalizeCache::new()));

// 同一线程连续处理相同别名路径时，先走这一项缓存，避免重复竞争全局锁。
struct LastNormalizedPath {
    path: String,
    normalized: String,
}

thread_local! {
    static LAST_NORMALIZED_PATH: RefCell<Option<LastNormalizedPath>> = const { RefCell::new(None) };
}

// paths 只保留通用路径工具与规范化入口；别名、存储根、规则与安全判定按职责放在
// 同级模块，重导出后调用方仍然只面对 `paths::*`。
pub use crate::platform::paths_roots::*;
pub use crate::platform::paths_rules::*;
pub use crate::platform::paths_safety::*;

use crate::platform::paths_alias::{has_potential_storage_alias, resolve_storage_alias};

// 合并斜杠、去尾斜杠，再逐层解析存储别名
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return path.to_string();
    }
    if !path.ends_with('/') && !path.contains("//") && !has_potential_storage_alias(path) {
        return path.to_string();
    }

    if let Some(normalized) = LAST_NORMALIZED_PATH.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|cached| cached.path == path)
            .map(|cached| cached.normalized.clone())
    }) {
        return normalized;
    }

    // 优化：对于常用路径先查缓存
    if let Ok(mut cache) = PATH_NORMALIZE_CACHE.try_lock()
        && let Some(normalized) = cache.get(path)
    {
        LAST_NORMALIZED_PATH.with(|slot| {
            // quality-allow(chinese-language): path 与 normalized 是必要的 Rust 字段名。
            *slot.borrow_mut() = Some(LastNormalizedPath {
                path: path.to_string(),
                normalized: normalized.clone(),
            });
        });
        return normalized;
    }

    // 只有确实需要折叠重复斜杠或去掉尾斜杠时才重建字符串。别名路径（如
    // /data/media/0/... 与 /sdcard/...）通常本身已经规整，此前仍会逐字符复制一遍
    // 产生完全相同的内容，再交给别名解析；这里先判断是否需要改写，避免这次多余分配。
    let normalized = resolve_storage_alias(&normalize_syntax(path));

    // 优化：缓存规范化结果
    if let Ok(mut cache) = PATH_NORMALIZE_CACHE.try_lock() {
        cache.insert(path.to_string(), normalized.clone());
    }

    normalized
}

/// 仅执行斜杠折叠和尾斜杠清理，不解析 Android 存储别名。
pub fn normalize_syntax(path: &str) -> String {
    if !path.contains("//") && (path.len() <= 1 || !path.ends_with('/')) {
        return path.to_string();
    }
    collapse_redundant_slashes(path)
}

fn collapse_redundant_slashes(path: &str) -> String {
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
    result
}

/// 折叠路径中的 `.` 与 `..` 段，返回内核实际会解析到的等价路径。
///
/// 仅供 hook 决策入口使用，不能并入 [`normalize`]：配置校验依赖
/// `normalize` 之后 [`has_unsafe_segments`] 仍能看到 `..` 来拒绝越界规则，
/// 若在那里提前折叠，`DCIM/../../etc` 这类配置会被折叠成合法路径而通过校验。
///
/// hook 侧必须折叠的原因相反：`Pictures/../ReadOnlyDir/a.jpg` 与任何规则都不
/// 匹配，但内核解析后仍落在 `ReadOnlyDir` 内。对于跳过 companion mount、没有
/// `MS_RDONLY` 兜底的系统代写进程，不折叠就等于只读保护与路径映射被绕过。
///
/// 绝对路径在根目录处的 `..` 直接丢弃，与内核一致；相对路径的前导 `..` 无法在
/// 词法层解析，原样保留交由调用方按当前目录处理。符号链接不在词法层处理范围内。
pub fn collapse_dot_segments(path: &str) -> String {
    let is_absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => match segments.last() {
                // 前一段可回退：抵消这一对。
                Some(&last) if last != ".." => {
                    segments.pop();
                }
                // 绝对路径已在根目录，`..` 无处可退，按内核语义丢弃。
                _ if is_absolute => {}
                // 相对路径的前导 `..` 保留。
                _ => segments.push(".."),
            },
            other => segments.push(other),
        }
    }

    let mut result = String::with_capacity(path.len());
    if is_absolute {
        result.push('/');
    }
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            result.push('/');
        }
        result.push_str(segment);
    }
    result
}

/// 判断路径是否含需要折叠的 `.` 或 `..` 段。廉价预筛，避免在热路径上对绝大多数
/// 不含点段的路径做逐段扫描。
pub fn needs_dot_segment_collapse(path: &str) -> bool {
    if !path.contains('.') {
        return false;
    }
    crate::platform::paths_safety::has_unsafe_segments(path)
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

pub(crate) fn starts_with_ignore_case(path: &str, prefix: &str) -> bool {
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

pub fn is_absolute(path: &str) -> bool {
    !path.is_empty() && path.starts_with('/')
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
