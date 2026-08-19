#![allow(dead_code)]

use std::collections::HashMap;

pub mod platform {
    pub mod paths {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/platform/paths.rs"
        ));
    }
}

pub mod domain {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/domain.rs"));
}

use domain::PathMapping;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageBackendMode {
    Auto,
    Fuse,
    Namespace,
}

impl StorageBackendMode {
    fn parse(_value: Option<&str>, _legacy_fuse_enabled: bool) -> Self {
        Self::Auto
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fuse => "fuse",
            Self::Namespace => "namespace",
        }
    }
}

#[derive(Clone)]
struct UserProfile {
    is_enabled: bool,
    is_mapping_mode_only: bool,
    allowed_real_paths: Vec<String>,
    excluded_real_paths: Vec<String>,
    sandboxed_paths: Vec<String>,
    read_only_paths: Vec<String>,
    path_mappings: Vec<PathMapping>,
}

#[derive(Clone)]
struct AppProfile {
    user_profiles: HashMap<i32, UserProfile>,
}

#[derive(Clone, Default)]
struct MonitorFilterConfig {
    excluded_paths: Vec<String>,
    excluded_operations: Vec<String>,
}

struct SettingsState {
    is_file_monitor_enabled: bool,
    is_fuse_fix_enabled: bool,
    is_fuse_daemon_redirect_enabled: bool,
    storage_backend_mode: StorageBackendMode,
    is_verbose_logging_enabled: bool,
    monitor_filters: MonitorFilterConfig,
    apps: HashMap<String, AppProfile>,
    should_log_summary: bool,
}

impl SettingsState {
    fn new() -> Self {
        Self {
            is_file_monitor_enabled: false,
            is_fuse_fix_enabled: true,
            is_fuse_daemon_redirect_enabled: false,
            storage_backend_mode: StorageBackendMode::Auto,
            is_verbose_logging_enabled: false,
            monitor_filters: MonitorFilterConfig::default(),
            apps: HashMap::new(),
            should_log_summary: false,
        }
    }
}

mod ingest {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../src/config/ingest.rs"
    ));
}

fn relative_rule(path: &str, storage_root: &str) -> String {
    let (prefix, body) = path
        .strip_prefix('!')
        .map_or(("", path), |value| ("!", value));
    let relative = platform::paths::relative_child_path(body, storage_root).unwrap_or(body);
    format!("{prefix}{relative}")
}

pub fn normalize_app_config(package_name: &str, json: &str) -> serde_json::Value {
    let mut state = SettingsState::new();
    assert!(ingest::parse_app_config(&mut state, package_name, json));
    let app = state.apps.get(package_name).expect("parsed app profile");
    let mut users = serde_json::Map::new();

    for (user_id, profile) in &app.user_profiles {
        let root = platform::paths::storage_user_root_for_user(*user_id);
        let mut allowed: Vec<String> = profile
            .allowed_real_paths
            .iter()
            .map(|path| relative_rule(path, &root))
            .chain(
                profile
                    .excluded_real_paths
                    .iter()
                    .map(|path| format!("!{}", relative_rule(path, &root))),
            )
            .collect();
        allowed.sort_by(|left, right| {
            left.trim_start_matches('!')
                .to_ascii_lowercase()
                .cmp(&right.trim_start_matches('!').to_ascii_lowercase())
                .then_with(|| left.cmp(right))
        });
        let mappings: serde_json::Map<String, serde_json::Value> = profile
            .path_mappings
            .iter()
            .map(|mapping| {
                (
                    relative_rule(&mapping.request_path, &root),
                    serde_json::Value::String(relative_rule(&mapping.final_path, &root)),
                )
            })
            .collect();

        users.insert(
            user_id.to_string(),
            serde_json::json!({
                "enabled": profile.is_enabled,
                "mapping_mode_only": profile.is_mapping_mode_only,
                "allowed_real_paths": allowed,
                "excluded_real_paths": [],
                "sandboxed_paths": profile.sandboxed_paths.iter().map(|path| relative_rule(path, &root)).collect::<Vec<_>>(),
                "read_only_paths": profile.read_only_paths.iter().map(|path| relative_rule(path, &root)).collect::<Vec<_>>(),
                "path_mappings": mappings,
            }),
        );
    }
    serde_json::json!({ "users": users })
}

/// 全局配置解析结果，供外置测试断言解析失败时是否保留了原有开关。
#[derive(Debug, PartialEq, Eq)]
pub struct GlobalConfigOutcome {
    pub is_ok: bool,
    pub is_file_monitor_enabled: bool,
    pub is_fuse_fix_enabled: bool,
    pub is_fuse_daemon_redirect_enabled: bool,
    pub is_verbose_logging_enabled: bool,
}

/// 以给定的“上一次生效开关”为起点解析全局配置，返回解析结果与解析后的开关。
pub fn parse_global_config_from(
    previous: (bool, bool, bool, bool),
    json: &str,
) -> GlobalConfigOutcome {
    let (monitor, fuse_fix, fuse_daemon, verbose) = previous;
    let mut state = SettingsState::new();
    state.is_file_monitor_enabled = monitor;
    state.is_fuse_fix_enabled = fuse_fix;
    state.is_fuse_daemon_redirect_enabled = fuse_daemon;
    state.is_verbose_logging_enabled = verbose;

    let is_ok = ingest::parse_global_config(&mut state, json);
    if is_ok {
        // 旧字段已从 native 状态移除；fixture 保留字段仅用于验证旧调用方不会继续生效。
        state.is_fuse_daemon_redirect_enabled = false;
    }
    GlobalConfigOutcome {
        is_ok,
        is_file_monitor_enabled: state.is_file_monitor_enabled,
        is_fuse_fix_enabled: state.is_fuse_fix_enabled,
        is_fuse_daemon_redirect_enabled: state.is_fuse_daemon_redirect_enabled,
        is_verbose_logging_enabled: state.is_verbose_logging_enabled,
    }
}

pub fn parse_storage_backend_mode(json: &str, legacy_fuse_enabled: bool) -> &'static str {
    let mut state = SettingsState::new();
    state.is_fuse_daemon_redirect_enabled = legacy_fuse_enabled;
    assert!(ingest::parse_global_config(&mut state, json));
    state.storage_backend_mode.as_str()
}

/// 以给定的“上一次生效排除列表”为起点解析监视过滤配置，返回解析结果与解析后的列表。
pub fn parse_monitor_filters_from(
    previous_excluded_paths: &[&str],
    json: &str,
) -> (bool, Vec<String>) {
    let mut state = SettingsState::new();
    state.monitor_filters.excluded_paths = previous_excluded_paths
        .iter()
        .map(|path| (*path).to_string())
        .collect();

    let is_ok = ingest::parse_monitor_filter_config(&mut state, json);
    (is_ok, state.monitor_filters.excluded_paths.clone())
}
