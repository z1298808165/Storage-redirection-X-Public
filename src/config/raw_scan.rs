use super::SettingsHub;
use crate::config::RawUserProfileFlags;
use crate::platform::module_paths;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

const SELF_PACKAGE_NAME: &str = "com.storage.redirect.x";
const RAW_PROBE_LOG_STEP: u64 = 256;
// 缓存条目存活时间，命中即跳过 fs::metadata，避开同一 caller 的高频探测
const RAW_CACHE_TTL_MS: i64 = 1000;
// 单线程缓存项数上限，防止瞬时大量唯一 caller 撑爆缓存
const RAW_CACHE_CAP: usize = 128;

static RAW_PROBE_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct CacheEntry {
    package_name: String,
    user_id: i32,
    config_version: u64,
    cached_at_ms: i64,
    is_enabled: bool,
    is_mapping_mode_only: bool,
    sandboxed_paths: Vec<String>,
    read_only_paths: Vec<String>,
    has_config: bool,
}

impl CacheEntry {
    fn missing(package_name: &str, user_id: i32, config_version: u64, cached_at_ms: i64) -> Self {
        Self {
            package_name: package_name.to_string(),
            user_id,
            config_version,
            cached_at_ms,
            is_enabled: false,
            is_mapping_mode_only: false,
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            has_config: false,
        }
    }
}

thread_local! {
    static RAW_CACHE: RefCell<Vec<CacheEntry>> = const { RefCell::new(Vec::new()) };
    static RAW_CACHE_VERSION: Cell<u64> = const { Cell::new(0) };
}

impl SettingsHub {
    // Bypasses merged in-memory state and probes disk JSON directly. This is used by
    // early zygote/writer paths before the normal config snapshot may be available.
    pub fn has_enabled_user_profile_in_raw_config(&self, package_name: &str, user_id: i32) -> bool {
        self.get_user_flags_in_raw_config(package_name, user_id)
            .is_enabled
    }

    pub fn has_any_enabled_user_profile_in_raw_config(&self, user_id: i32) -> bool {
        if user_id < 0 {
            return false;
        }

        let config_dir = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            state.config_dir.clone()
        };
        if config_dir.is_empty() {
            return false;
        }

        let apps_dir = crate::platform::paths::join(&config_dir, "apps");
        if fs::metadata(&apps_dir)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            return scan_raw_apps_dir_for_enabled_user(&apps_dir, user_id);
        }

        config_dir != module_paths::CONFIG_DIR
            && scan_raw_apps_dir_for_enabled_user(
                &crate::platform::paths::join(module_paths::CONFIG_DIR, "apps"),
                user_id,
            )
    }

    pub fn get_user_flags_in_raw_config(
        &self,
        package_name: &str,
        user_id: i32,
    ) -> RawUserProfileFlags {
        self.ensure_raw_config_cache_entry(package_name, user_id)
            .map(|entry| raw_flags_from_cache_entry(&entry))
            .unwrap_or_default()
    }

    pub fn get_user_sandboxed_paths_in_raw_config(
        &self,
        package_name: &str,
        user_id: i32,
    ) -> Vec<String> {
        self.ensure_raw_config_cache_entry(package_name, user_id)
            .filter(|entry| entry.has_config && entry.is_enabled)
            .map(|entry| entry.sandboxed_paths)
            .unwrap_or_default()
    }

    pub fn get_user_read_only_paths_in_raw_config(
        &self,
        package_name: &str,
        user_id: i32,
    ) -> Vec<String> {
        self.ensure_raw_config_cache_entry(package_name, user_id)
            .filter(|entry| entry.has_config && entry.is_enabled)
            .map(|entry| entry.read_only_paths)
            .unwrap_or_default()
    }

    fn ensure_raw_config_cache_entry(
        &self,
        package_name: &str,
        user_id: i32,
    ) -> Option<CacheEntry> {
        if package_name.is_empty() || user_id < 0 || package_name == SELF_PACKAGE_NAME {
            return None;
        }

        let config_dir = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            state.config_dir.clone()
        };
        if config_dir.is_empty() {
            return None;
        }

        let config_version = self.config_version();
        let now_ms = current_time_ms();
        invalidate_cache_on_version_change(config_version);

        // TTL 内的命中直接返回，跳过 fs::metadata
        if let Some(cached) = lookup_cached_entry(package_name, user_id, config_version, now_ms) {
            return Some(cached);
        }

        let entry =
            load_raw_config_entry(package_name, user_id, &config_dir, config_version, now_ms);
        upsert_entry(package_name, user_id, entry.clone());
        Some(entry)
    }
}

fn scan_raw_apps_dir_for_enabled_user(apps_dir: &str, user_id: i32) -> bool {
    let Ok(dir_entries) = fs::read_dir(apps_dir) else {
        return false;
    };

    for entry in dir_entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Some(package_name) = name_str
            .strip_suffix(".json")
            .filter(|value| !value.is_empty() && *value != SELF_PACKAGE_NAME)
        else {
            continue;
        };

        let path = entry.path();
        let path = path.to_string_lossy();
        let entry = parse_raw_config_entry(package_name, user_id, &path, 0, current_time_ms());
        if entry.has_config && entry.is_enabled {
            return true;
        }
    }

    false
}

fn raw_flags_from_cache_entry(entry: &CacheEntry) -> RawUserProfileFlags {
    RawUserProfileFlags {
        has_config: entry.has_config,
        is_enabled: entry.has_config && entry.is_enabled,
        is_mapping_mode_only: entry.has_config && entry.is_enabled && entry.is_mapping_mode_only,
    }
}

fn load_raw_config_entry(
    package_name: &str,
    user_id: i32,
    config_dir: &str,
    config_version: u64,
    now_ms: i64,
) -> CacheEntry {
    let config_path = crate::platform::paths::join(
        &crate::platform::paths::join(config_dir, "apps"),
        &format!("{}.json", package_name),
    );
    let fallback_path = crate::platform::paths::join(
        &crate::platform::paths::join(module_paths::CONFIG_DIR, "apps"),
        &format!("{}.json", package_name),
    );

    let resolved_path = match fs::metadata(&config_path) {
        Ok(_) => config_path.clone(),
        Err(_) => match fs::metadata(&fallback_path) {
            Ok(_) => {
                log_probe(&format!(
                    "raw config fallback pkg={} shared={} module={}",
                    package_name, config_path, fallback_path
                ));
                fallback_path.clone()
            }
            Err(_) => {
                log_probe(&format!(
                    "raw config stat failed pkg={} shared={} module={}",
                    package_name, config_path, fallback_path
                ));
                return CacheEntry::missing(package_name, user_id, config_version, now_ms);
            }
        },
    };

    parse_raw_config_entry(
        package_name,
        user_id,
        &resolved_path,
        config_version,
        now_ms,
    )
}

fn parse_raw_config_entry(
    package_name: &str,
    user_id: i32,
    resolved_path: &str,
    config_version: u64,
    now_ms: i64,
) -> CacheEntry {
    let mut has_config = false;
    let mut is_enabled = false;
    let mut is_mapping_mode_only = false;
    let mut sandboxed_paths = Vec::new();
    let mut read_only_paths = Vec::new();
    let content = fs::read_to_string(resolved_path).unwrap_or_default();
    if !content.is_empty() {
        match serde_json::from_str::<Value>(&content) {
            Ok(json) => {
                if let Some(users) = json.get("users").and_then(|v| v.as_object())
                    && let Some(user) = users.get(&user_id.to_string()).and_then(|v| v.as_object())
                {
                    let enabled = user.get("enabled");
                    is_enabled = match enabled {
                        None => true,
                        Some(value) => value.as_bool().unwrap_or(false),
                    };
                    is_mapping_mode_only = user
                        .get("mapping_mode_only")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false);
                    sandboxed_paths =
                        parse_raw_sandboxed_paths(user.get("sandboxed_paths"), user_id);
                    read_only_paths =
                        parse_raw_read_only_paths(user.get("read_only_paths"), user_id);
                    has_config = true;
                }
            }
            Err(error) => {
                log_probe(&format!(
                    "raw config parse failed pkg={} path={} err={}",
                    package_name, resolved_path, error
                ));
                is_enabled = false;
            }
        }
    }

    CacheEntry {
        package_name: package_name.to_string(),
        user_id,
        config_version,
        cached_at_ms: now_ms,
        is_enabled,
        is_mapping_mode_only,
        sandboxed_paths,
        read_only_paths,
        has_config,
    }
}

fn parse_raw_sandboxed_paths(value: Option<&Value>, user_id: i32) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    if let Some(path) = value.as_str() {
        push_raw_sandboxed_path(&mut paths, path, user_id);
    } else if let Some(list) = value.as_array() {
        for item in list {
            if let Some(path) = item.as_str() {
                push_raw_sandboxed_path(&mut paths, path, user_id);
            }
        }
    }
    crate::platform::paths::sort_dedup_paths_case_insensitive(&mut paths);
    paths
}

fn push_raw_sandboxed_path(paths: &mut Vec<String>, raw_path: &str, user_id: i32) {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return;
    }

    let normalized = crate::platform::paths::normalize(raw_path);
    if normalized.is_empty()
        || normalized.starts_with('/')
        || crate::platform::paths::has_unsafe_segments(&normalized)
    {
        return;
    }

    let storage_root = crate::platform::paths::storage_user_root_for_user(user_id);
    let resolved = crate::platform::paths::normalize(&crate::platform::paths::join(
        &storage_root,
        &normalized,
    ));
    if !crate::platform::paths::is_child(&resolved, &storage_root) {
        return;
    }

    paths.push(resolved);
}

fn parse_raw_read_only_paths(value: Option<&Value>, user_id: i32) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    if let Some(path) = value.as_str() {
        push_raw_read_only_path(&mut paths, path, user_id);
    } else if let Some(list) = value.as_array() {
        for item in list {
            if let Some(path) = item.as_str() {
                push_raw_read_only_path(&mut paths, path, user_id);
            }
        }
    }
    crate::platform::paths::sort_dedup_paths_case_insensitive(&mut paths);
    paths
}

fn push_raw_read_only_path(paths: &mut Vec<String>, raw_path: &str, user_id: i32) {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return;
    }

    let (excluded, body) = if let Some(stripped) = raw_path.strip_prefix('!') {
        (true, stripped.trim_start())
    } else {
        (false, raw_path)
    };
    let normalized = crate::platform::paths::normalize(body);
    if normalized.is_empty()
        || normalized.starts_with('/')
        || crate::platform::paths::has_unsafe_segments(&normalized)
    {
        return;
    }

    let storage_root = crate::platform::paths::storage_user_root_for_user(user_id);
    let resolved = crate::platform::paths::normalize(&crate::platform::paths::join(
        &storage_root,
        &normalized,
    ));
    if !crate::platform::paths::is_child(&resolved, &storage_root) {
        return;
    }

    paths.push(if excluded {
        format!("!{resolved}")
    } else {
        resolved
    });
}

// config_version 变化即清空缓存，保证用户改配置后立即可见
fn invalidate_cache_on_version_change(current_version: u64) {
    RAW_CACHE_VERSION.with(|cell| {
        if cell.get() != current_version {
            cell.set(current_version);
            RAW_CACHE.with(|cache| {
                cache.borrow_mut().clear();
            });
        }
    });
}

fn lookup_cached_entry(
    package_name: &str,
    user_id: i32,
    config_version: u64,
    now_ms: i64,
) -> Option<CacheEntry> {
    RAW_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache
            .iter()
            .find(|entry| entry.user_id == user_id && entry.package_name == package_name)
            .and_then(|entry| {
                if entry.config_version != config_version {
                    return None;
                }
                if !entry.has_config {
                    return None;
                }
                if now_ms.saturating_sub(entry.cached_at_ms) > RAW_CACHE_TTL_MS {
                    return None;
                }
                Some(entry.clone())
            })
    })
}

fn upsert_entry(package_name: &str, user_id: i32, entry: CacheEntry) {
    RAW_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(slot) = cache
            .iter_mut()
            .find(|e| e.user_id == user_id && e.package_name == package_name)
        {
            *slot = entry;
            return;
        }
        if cache.len() >= RAW_CACHE_CAP {
            cache.clear();
        }
        cache.push(entry);
    });
}

fn current_time_ms() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    ts.tv_sec.saturating_mul(1000) + ts.tv_nsec / 1_000_000
}

fn log_probe(message: &str) {
    let count = RAW_PROBE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count == 1 || count.is_multiple_of(RAW_PROBE_LOG_STEP) {
        log::debug!("{}", message);
    }
}
