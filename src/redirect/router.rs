use super::writer;
use crate::domain::{
    PathMapping, dedup_path_mappings_by_request_case_insensitive, filter_valid_path_mapping_chains,
    sort_path_mappings_longest_request_first_case_insensitive,
};
use crate::platform::{self, paths};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RedirectAction {
    Allow,
    Redirect,
    DenyReadOnly,
}

#[derive(Clone)]
pub struct RedirectDecision {
    pub action: RedirectAction,
    pub new_path: String,
    /// 显式 path_mapping 条目触发重定向时为 true，区别于 fallback/excluded
    /// 到应用沙箱的重定向。
    pub is_mapping: bool,
}

impl RedirectDecision {
    pub fn is_redirect(&self) -> bool {
        matches!(self.action, RedirectAction::Redirect)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self.action, RedirectAction::DenyReadOnly)
    }
}

struct RouterState {
    current_package: String,
    user_id: i32,
    storage_root: String,
    redirect_target: String,
    allowed_real_paths: RouterPathIndex,
    excluded_real_paths: RouterPathIndex,
    sandboxed_paths: RouterPathIndex,
    sandboxed_excluded_paths: RouterPathIndex,
    read_only_paths: RouterPathIndex,
    read_only_excluded_paths: RouterPathIndex,
    path_mappings: Vec<PathMapping>,
    is_mapping_mode_only: bool,
}

impl RouterState {
    fn new() -> Self {
        Self {
            current_package: String::new(),
            user_id: 0,
            storage_root: String::new(),
            redirect_target: String::new(),
            allowed_real_paths: RouterPathIndex::default(),
            excluded_real_paths: RouterPathIndex::default(),
            sandboxed_paths: RouterPathIndex::default(),
            sandboxed_excluded_paths: RouterPathIndex::default(),
            read_only_paths: RouterPathIndex::default(),
            read_only_excluded_paths: RouterPathIndex::default(),
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        }
    }
}

/// 预编译路径规则索引，按首段路径缩小匹配候选，避免每次查询线性扫描全部规则。
#[derive(Clone, Default)]
struct RouterPathIndex {
    entries: Vec<String>,
    buckets: HashMap<String, Vec<usize>>,
    fallback: Vec<usize>,
}

impl RouterPathIndex {
    fn new(entries: Vec<String>) -> Self {
        let mut index = Self {
            entries,
            buckets: HashMap::new(),
            fallback: Vec::new(),
        };
        for (idx, pattern) in index.entries.iter().enumerate() {
            if let Some(key) = router_path_first_segment(pattern) {
                index.buckets.entry(key).or_default().push(idx);
            } else {
                index.fallback.push(idx);
            }
        }
        index
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn matches(&self, resolved_path: &str, include_xldownload_alias: bool) -> bool {
        let first_segment = router_path_first_segment_ref(resolved_path);
        let mut candidates = self
            .buckets
            .iter()
            .filter(|(key, _)| {
                first_segment.is_some_and(|segment| key.eq_ignore_ascii_case(segment))
            })
            .flat_map(|(_, indices)| indices.iter().copied())
            .chain(self.fallback.iter().copied());
        candidates.any(|idx| {
            let configured = &self.entries[idx];
            !configured.is_empty()
                && (paths::matches(configured, resolved_path, true)
                    || (include_xldownload_alias
                        && paths::matches_xldownload_alias(configured, resolved_path)))
        })
    }
}

fn router_path_first_segment(path: &str) -> Option<String> {
    let segment = router_path_first_segment_ref(path)?;
    if paths::contains_wildcards(segment) {
        return None;
    }
    Some(segment.to_ascii_lowercase())
}

fn router_path_first_segment_ref(path: &str) -> Option<&str> {
    let trimmed = path.strip_prefix('/')?;
    let segment = trimmed.split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

pub struct PathRouter {
    // 配置更新很少、路径判定极其频繁。读锁只用来取得不可变快照，随后在锁外完成
    // 规则匹配，避免通配符判断和映射解析阻塞配置刷新。
    state: RwLock<Arc<RouterState>>,
    initialized: AtomicBool,
}

impl PathRouter {
    fn new() -> Self {
        Self {
            state: RwLock::new(Arc::new(RouterState::new())),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn instance() -> &'static PathRouter {
        &PATH_ROUTER
    }

    pub fn init(&self) -> bool {
        if self.initialized.load(Ordering::Relaxed) {
            return true;
        }
        log::info!("path router init");
        self.initialized.store(true, Ordering::Relaxed);
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn configure(
        &self,
        package_name: &str,
        app_uid: i32,
        redirect_target: &str,
        allowed_real_paths: &[String],
        excluded_real_paths: &[String],
        sandboxed_paths: &[String],
        read_only_paths: &[String],
        path_mappings: &[PathMapping],
        is_mapping_mode_only: bool,
    ) {
        let mut state = RouterState::new();
        state.current_package = package_name.to_string();
        state.user_id = platform::user_id_from_uid(app_uid);
        state.storage_root = paths::storage_user_root_for_user(state.user_id);
        state.redirect_target =
            paths::resolve_user_path(&paths::normalize(redirect_target), state.user_id);
        state.is_mapping_mode_only = is_mapping_mode_only;

        let config = crate::config::SettingsHub::instance();
        let expand_mount_fallbacks = crate::fuse_redirect::config::expand_mount_fallbacks_for_mode(
            config.storage_backend_mode(),
        );
        state.allowed_real_paths = RouterPathIndex::new(resolve_router_path_list(
            allowed_real_paths,
            state.user_id,
            &state.storage_root,
            expand_mount_fallbacks,
        ));
        state.excluded_real_paths = RouterPathIndex::new(resolve_router_path_list(
            excluded_real_paths,
            state.user_id,
            &state.storage_root,
            false,
        ));
        let (sandbox_includes, sandbox_excludes) = paths::split_exclusion_rules(sandboxed_paths);
        state.sandboxed_paths = RouterPathIndex::new(resolve_router_sandboxed_paths(
            &sandbox_includes,
            state.user_id,
            &state.storage_root,
        ));
        state.sandboxed_excluded_paths = RouterPathIndex::new(resolve_router_sandboxed_paths(
            &sandbox_excludes,
            state.user_id,
            &state.storage_root,
        ));
        let (read_only_includes, read_only_excludes) =
            paths::split_exclusion_rules(read_only_paths);
        state.read_only_paths = RouterPathIndex::new(resolve_router_read_only_paths(
            &read_only_includes,
            state.user_id,
            &state.storage_root,
            expand_mount_fallbacks,
        ));
        let excluded_read_only_paths = resolve_router_read_only_paths(
            &read_only_excludes,
            state.user_id,
            &state.storage_root,
            false,
        );
        state.read_only_excluded_paths = RouterPathIndex::new(paths::overlapping_exclusion_rules(
            &state.read_only_paths.entries,
            &excluded_read_only_paths,
        ));
        state.path_mappings =
            resolve_router_mappings(path_mappings, state.user_id, &state.storage_root);

        log::info!(
            "router cfg pkg={} user={} allow={} excl={} sandbox={} ro={} map={} map_only={}",
            state.current_package,
            state.user_id,
            state.allowed_real_paths.len(),
            state.excluded_real_paths.len(),
            state.sandboxed_paths.len(),
            state.read_only_paths.len(),
            state.path_mappings.len(),
            state.is_mapping_mode_only
        );
        // quality-allow(chinese-language): 此处是 Rust 解引用赋值，原子替换不可变配置快照。
        *self.state.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(state);
    }

    pub fn redirect_target(&self) -> String {
        let state = self.router_state_snapshot();
        state.redirect_target.clone()
    }

    pub fn is_path_excluded(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.router_state_snapshot();
        is_path_excluded_locked(&state, resolved_path)
    }

    pub fn is_path_allowed_real(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.router_state_snapshot();
        is_path_allowed_real_locked(&state, resolved_path)
    }

    pub fn is_path_sandboxed(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.router_state_snapshot();
        state.is_mapping_mode_only
            && state.sandboxed_paths.matches(resolved_path, true)
            && !state.sandboxed_excluded_paths.matches(resolved_path, true)
    }

    pub fn map_path(&self, resolved_path: &str) -> String {
        if resolved_path.is_empty() {
            return String::new();
        }
        let state = self.router_state_snapshot();
        writer::map_path_by_caller_mappings(resolved_path, &state.path_mappings)
    }

    pub fn is_path_mapping_target(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.router_state_snapshot();
        state
            .path_mappings
            .iter()
            .any(|mapping| paths::is_same_or_child(resolved_path, &mapping.final_path))
    }

    pub fn read_only_check_path(&self, resolved_path: &str) -> String {
        if resolved_path.is_empty() {
            return String::new();
        }
        let state = self.router_state_snapshot();
        read_only_check_path_locked(&state, resolved_path)
    }

    pub fn is_path_readable_by_read_only_rule(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.router_state_snapshot();
        is_path_read_only_locked(&state, resolved_path)
    }

    fn router_state_snapshot(&self) -> Arc<RouterState> {
        self.state
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

static PATH_ROUTER: Lazy<PathRouter> = Lazy::new(PathRouter::new);

fn resolve_router_path_list(
    paths_in: &[String],
    user_id: i32,
    storage_root: &str,
    expand_mount_fallbacks: bool,
) -> Vec<String> {
    let mut resolved = Vec::with_capacity(paths_in.len());
    for path in paths_in {
        let Some(path) = resolve_router_storage_path(path, user_id, storage_root) else {
            continue;
        };
        append_router_path(&mut resolved, path, storage_root, expand_mount_fallbacks);
    }
    paths::sort_dedup_paths_case_insensitive(&mut resolved);
    resolved
}

fn resolve_router_sandboxed_paths(
    paths_in: &[String],
    user_id: i32,
    storage_root: &str,
) -> Vec<String> {
    let mut resolved = resolve_router_path_list(paths_in, user_id, storage_root, false);
    resolved.sort_by(|a, b| {
        if a.len() != b.len() {
            b.len().cmp(&a.len())
        } else {
            a.cmp(b)
        }
    });
    resolved
}

fn resolve_router_read_only_paths(
    paths_in: &[String],
    user_id: i32,
    storage_root: &str,
    expand_mount_fallbacks: bool,
) -> Vec<String> {
    let mut resolved =
        resolve_router_path_list(paths_in, user_id, storage_root, expand_mount_fallbacks);
    resolved.sort_by(|a, b| {
        if a.len() != b.len() {
            b.len().cmp(&a.len())
        } else {
            a.cmp(b)
        }
    });
    resolved
}

fn append_router_path(
    resolved: &mut Vec<String>,
    path: String,
    storage_root: &str,
    expand_mount_fallbacks: bool,
) {
    if !expand_mount_fallbacks || !paths::contains_wildcards(&path) {
        resolved.push(path);
        return;
    }

    if let Some(fallback) = paths::wildcard_policy_fallback_parent(&path, storage_root) {
        log::warn!(
            "router fallback wildcard to mount parent: {} -> {}",
            path,
            fallback
        );
        resolved.push(fallback);
    } else {
        resolved.push(path);
    }
}

fn resolve_router_storage_path(path: &str, user_id: i32, storage_root: &str) -> Option<String> {
    let mut resolved = paths::normalize(path);
    if paths::has_unsafe_segments(&resolved) {
        return None;
    }
    resolved = paths::resolve_user_path(&resolved, user_id);
    if resolved.is_empty() || !paths::is_child(&resolved, storage_root) {
        return None;
    }
    Some(resolved)
}

fn is_path_excluded_locked(state: &RouterState, resolved_path: &str) -> bool {
    state.excluded_real_paths.matches(resolved_path, true)
}

fn is_path_allowed_real_locked(state: &RouterState, resolved_path: &str) -> bool {
    !is_path_excluded_locked(state, resolved_path)
        && state.allowed_real_paths.matches(resolved_path, true)
}

fn is_path_read_only_locked(state: &RouterState, resolved_path: &str) -> bool {
    !is_path_excluded_locked(state, resolved_path)
        && !state.read_only_excluded_paths.matches(resolved_path, true)
        && state.read_only_paths.matches(resolved_path, true)
}

fn read_only_check_path_locked(state: &RouterState, resolved_path: &str) -> String {
    let mapped_path = writer::map_path_by_caller_mappings(resolved_path, &state.path_mappings);
    if !mapped_path.is_empty() && mapped_path != resolved_path {
        if is_path_read_only_locked(state, &mapped_path) {
            return mapped_path;
        }
        return String::new();
    }
    if is_path_read_only_locked(state, resolved_path) {
        return resolved_path.to_string();
    }
    String::new()
}

fn resolve_router_mappings(
    path_mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
) -> Vec<PathMapping> {
    let mut mappings = Vec::with_capacity(path_mappings.len());
    for mapping in path_mappings {
        let Some(current_path) =
            resolve_router_storage_path(&mapping.request_path, user_id, storage_root)
        else {
            continue;
        };
        if paths::is_application_private_root(&current_path) {
            continue;
        }
        let Some(target_path) =
            resolve_router_storage_path(&mapping.final_path, user_id, storage_root)
        else {
            continue;
        };
        if paths::eq_ignore_case(&current_path, &target_path) {
            continue;
        }
        mappings.push(PathMapping::new(current_path, target_path));
    }

    sort_path_mappings_longest_request_first_case_insensitive(&mut mappings);
    dedup_path_mappings_by_request_case_insensitive(&mut mappings);
    filter_valid_path_mapping_chains(mappings)
}
