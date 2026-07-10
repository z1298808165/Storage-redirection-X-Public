use crate::domain::PathMapping;
use crate::platform::module_paths;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "config/consensus.rs"]
mod consensus;
#[path = "config/fingerprint.rs"]
mod fingerprint;
#[path = "config/ingest.rs"]
mod ingest;
#[path = "config/inspect.rs"]
mod inspect;
#[path = "config/merge.rs"]
mod merge;
#[path = "config/raw_scan.rs"]
mod raw_scan;
#[path = "config/source.rs"]
mod source;
#[path = "config/watcher.rs"]
pub mod watcher;

#[derive(Clone)]
pub struct UserProfile {
    pub is_enabled: bool,
    pub is_mapping_mode_only: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
}

#[derive(Clone)]
pub struct AppProfile {
    pub user_profiles: HashMap<i32, UserProfile>,
}

#[derive(Clone)]
pub struct ResolvedUserProfile {
    pub user_id: i32,
    pub redirect_target: String,
    pub is_mapping_mode_only: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
}

#[derive(Clone, Copy)]
pub struct ResolvedUserProfileFlags {
    pub is_mapping_mode_only: bool,
}

#[derive(Clone, Copy, Default)]
pub struct RawUserProfileFlags {
    pub has_config: bool,
    pub is_enabled: bool,
    pub is_mapping_mode_only: bool,
}

#[derive(Clone, Copy)]
pub struct UserRedirectEnablement {
    pub enabled_in_memory: bool,
    pub has_raw_config: bool,
    pub enabled_in_raw: bool,
    pub is_mapping_mode_only: bool,
}

impl UserRedirectEnablement {
    pub fn is_enabled(&self) -> bool {
        if self.has_raw_config {
            self.enabled_in_raw
        } else {
            self.enabled_in_memory
        }
    }
}

#[derive(Clone)]
pub struct MonitorFilterConfig {
    pub excluded_paths: Vec<String>,
    pub excluded_operations: Vec<String>,
}

impl MonitorFilterConfig {
    fn default() -> Self {
        Self {
            excluded_paths: vec!["Android/data".to_string()],
            excluded_operations: vec![
                "open:read".to_string(),
                "open*:read".to_string(),
                "provider_open:read".to_string(),
                "unlink*".to_string(),
                "delete*".to_string(),
                "rmdir*".to_string(),
                "link*".to_string(),
                "symlink*".to_string(),
                "truncate*".to_string(),
                "ftruncate*".to_string(),
                "chmod*".to_string(),
                "fchmod*".to_string(),
                "utimens*".to_string(),
                "futimens*".to_string(),
                "attrib*".to_string(),
            ],
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct MonitorAppSpec {
    pub package_name: String,
    pub user_id: i32,
    pub is_enabled: bool,
    pub is_mapping_mode_only: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
}

struct SettingsState {
    config_dir: String,
    is_file_monitor_enabled: bool,
    is_fuse_fix_enabled: bool,
    is_fuse_daemon_redirect_enabled: bool,
    is_verbose_logging_enabled: bool,
    monitor_filters: MonitorFilterConfig,
    apps: HashMap<String, AppProfile>,
    is_loaded: bool,
    should_log_summary: bool,
    last_fingerprint: u64,
    invalid_packages: HashSet<String>,
}

impl SettingsState {
    fn new() -> Self {
        Self {
            config_dir: module_paths::CONFIG_DIR.to_string(),
            is_file_monitor_enabled: false,
            is_fuse_fix_enabled: true,
            is_fuse_daemon_redirect_enabled: false,
            is_verbose_logging_enabled: false,
            monitor_filters: MonitorFilterConfig::default(),
            apps: HashMap::new(),
            is_loaded: false,
            should_log_summary: true,
            last_fingerprint: 0,
            invalid_packages: HashSet::new(),
        }
    }
}

pub struct SettingsHub {
    state: Mutex<SettingsState>,
    // 单调递增的配置代际号，用于缓存失效；磁盘指纹只用于判断是否需要重载。
    config_version: AtomicU64,
}

impl SettingsHub {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(SettingsState::new()),
            config_version: AtomicU64::new(0),
        }
    }

    pub fn instance() -> &'static SettingsHub {
        &SETTINGS_HUB
    }

    pub fn config_version(&self) -> u64 {
        self.config_version.load(Ordering::Relaxed)
    }

    fn bump_config_version(&self) {
        self.config_version.fetch_add(1, Ordering::Relaxed);
    }
}

static SETTINGS_HUB: Lazy<SettingsHub> = Lazy::new(SettingsHub::new);
