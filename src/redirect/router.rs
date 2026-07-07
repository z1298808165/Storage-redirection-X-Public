use super::writer;
use crate::domain::{
    PathMapping, dedup_path_mappings_by_request_case_insensitive, filter_valid_path_mapping_chains,
    sort_path_mappings_longest_request_first_case_insensitive,
};
use crate::platform::{self, paths};
use once_cell::sync::Lazy;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// true when the redirect was triggered by an explicit path_mapping entry
    /// (as opposed to a fallback/excluded redirect to the app's sandbox).
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
    allowed_real_paths: Vec<String>,
    excluded_real_paths: Vec<String>,
    sandboxed_paths: Vec<String>,
    read_only_paths: Vec<String>,
    read_only_excluded_paths: Vec<String>,
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
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            read_only_excluded_paths: Vec::new(),
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        }
    }
}

pub struct PathRouter {
    state: RwLock<RouterState>,
    initialized: AtomicBool,
}

impl PathRouter {
    fn new() -> Self {
        Self {
            state: RwLock::new(RouterState::new()),
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
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.current_package = package_name.to_string();
        state.user_id = platform::user_id_from_uid(app_uid);
        state.storage_root = paths::storage_user_root_for_user(state.user_id);
        state.redirect_target =
            paths::resolve_user_path(&paths::normalize(redirect_target), state.user_id);
        state.is_mapping_mode_only = is_mapping_mode_only;

        let expand_mount_fallbacks =
            !crate::config::SettingsHub::instance().is_fuse_daemon_redirect_enabled();
        state.allowed_real_paths = resolve_router_path_list(
            allowed_real_paths,
            state.user_id,
            &state.storage_root,
            expand_mount_fallbacks,
        );
        state.excluded_real_paths = resolve_router_path_list(
            excluded_real_paths,
            state.user_id,
            &state.storage_root,
            false,
        );
        state.sandboxed_paths =
            resolve_router_sandboxed_paths(sandboxed_paths, state.user_id, &state.storage_root);
        let (read_only_includes, read_only_excludes) =
            paths::split_exclusion_rules(read_only_paths);
        state.read_only_paths = resolve_router_read_only_paths(
            &read_only_includes,
            state.user_id,
            &state.storage_root,
            expand_mount_fallbacks,
        );
        let excluded_read_only_paths = resolve_router_read_only_paths(
            &read_only_excludes,
            state.user_id,
            &state.storage_root,
            false,
        );
        state.read_only_excluded_paths =
            paths::overlapping_exclusion_rules(&state.read_only_paths, &excluded_read_only_paths);
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
    }

    pub fn redirect_target(&self) -> String {
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        state.redirect_target.clone()
    }

    pub fn is_path_excluded(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        is_path_excluded_locked(&state, resolved_path)
    }

    pub fn is_path_allowed_real(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        is_path_allowed_real_locked(&state, resolved_path)
    }

    pub fn is_path_sandboxed(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        state.is_mapping_mode_only
            && router_path_list_matches(&state.sandboxed_paths, resolved_path, true)
    }

    pub fn map_path(&self, resolved_path: &str) -> String {
        if resolved_path.is_empty() {
            return String::new();
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        writer::map_path_by_caller_mappings(resolved_path, &state.path_mappings)
    }

    pub fn read_only_check_path(&self, resolved_path: &str) -> String {
        if resolved_path.is_empty() {
            return String::new();
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        read_only_check_path_locked(&state, resolved_path)
    }

    pub fn is_path_readable_by_read_only_rule(&self, resolved_path: &str) -> bool {
        if resolved_path.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        is_path_read_only_locked(&state, resolved_path)
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
    router_path_list_matches(&state.excluded_real_paths, resolved_path, false)
}

fn is_path_allowed_real_locked(state: &RouterState, resolved_path: &str) -> bool {
    !is_path_excluded_locked(state, resolved_path)
        && router_path_list_matches(&state.allowed_real_paths, resolved_path, false)
}

fn is_path_read_only_locked(state: &RouterState, resolved_path: &str) -> bool {
    !is_path_excluded_locked(state, resolved_path)
        && !router_path_list_matches(&state.read_only_excluded_paths, resolved_path, true)
        && router_path_list_matches(&state.read_only_paths, resolved_path, true)
}

fn router_path_list_matches(
    configured_paths: &[String],
    resolved_path: &str,
    include_xldownload_alias: bool,
) -> bool {
    configured_paths.iter().any(|configured| {
        !configured.is_empty()
            && (paths::matches(configured, resolved_path, true)
                || (include_xldownload_alias
                    && paths::matches_xldownload_alias(configured, resolved_path)))
    })
}

#[allow(dead_code)]
fn is_path_or_mapped_target_read_only_locked(state: &RouterState, resolved_path: &str) -> bool {
    !read_only_check_path_locked(state, resolved_path).is_empty()
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
        let Some(target_path) =
            resolve_router_storage_path(&mapping.final_path, user_id, storage_root)
        else {
            continue;
        };
        if paths::eq_ignore_case(&current_path, &target_path) {
            continue;
        }
        if paths::is_android_data_or_obb_path(&target_path) {
            continue;
        }
        mappings.push(PathMapping::new(current_path, target_path));
    }

    sort_path_mappings_longest_request_first_case_insensitive(&mut mappings);
    dedup_path_mappings_by_request_case_insensitive(&mut mappings);
    filter_valid_path_mapping_chains(mappings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xldownload_sandbox_alias_matches_case_variants() {
        assert!(paths::matches_xldownload_alias(
            "/storage/emulated/0/.xlDownload",
            "/storage/emulated/0/.xldownload"
        ));
        assert!(paths::matches_xldownload_alias(
            "/storage/emulated/0/.xlDownload",
            "/storage/emulated/0/.xldownload/tmp"
        ));
        assert!(!paths::matches_xldownload_alias(
            "/storage/emulated/0/Download",
            "/storage/emulated/0/download"
        ));
    }

    #[test]
    fn router_config_filters_invalid_paths_and_mappings() {
        let storage_root = paths::storage_user_root_for_user(0);
        let paths = resolve_router_path_list(
            &[
                "/storage/emulated/0/Download".to_string(),
                "/storage/emulated/0".to_string(),
                "/data/user/0/org.srx.demo/files".to_string(),
                "/storage/emulated/0/Download".to_string(),
            ],
            0,
            &storage_root,
            false,
        );
        assert_eq!(paths, vec!["/storage/emulated/0/Download".to_string()]);

        let mappings = resolve_router_mappings(
            &[
                PathMapping::new(
                    "/storage/emulated/0/DCIM".to_string(),
                    "/storage/emulated/0/Pictures".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/DCIM".to_string(),
                    "/storage/emulated/0/Movies".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Download".to_string(),
                    "/storage/emulated/0/Download".to_string(),
                ),
            ],
            0,
            &storage_root,
        );
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].request_path, "/storage/emulated/0/DCIM");
        assert_eq!(mappings[0].final_path, "/storage/emulated/0/Pictures");
    }

    #[test]
    fn router_path_list_can_expand_wildcards_to_mount_fallback_parent() {
        let storage_root = paths::storage_user_root_for_user(0);
        let paths = resolve_router_path_list(
            &[
                "/storage/emulated/0/Download/SrtMountNsAllow/Team*/Deep".to_string(),
                "/storage/emulated/0/Download/Plain".to_string(),
            ],
            0,
            &storage_root,
            true,
        );

        assert_eq!(
            paths,
            vec![
                "/storage/emulated/0/Download/Plain".to_string(),
                "/storage/emulated/0/Download/SrtMountNsAllow".to_string(),
            ]
        );
    }

    #[test]
    fn router_sandbox_paths_sort_deepest_first_after_dedup() {
        let storage_root = paths::storage_user_root_for_user(0);
        let paths = resolve_router_sandboxed_paths(
            &[
                "Download".to_string(),
                "/storage/emulated/0/Download/Nested".to_string(),
                "/storage/emulated/0/Download".to_string(),
                "/storage/emulated/10/Download/OtherUser".to_string(),
                "/storage/emulated/0".to_string(),
            ],
            0,
            &storage_root,
        );

        assert_eq!(
            paths,
            vec![
                "/storage/emulated/0/Download/Nested".to_string(),
                "/storage/emulated/0/Download".to_string(),
            ]
        );
    }

    #[test]
    fn router_read_only_respects_excludes_and_mapping_targets() {
        let mut state = RouterState::new();
        state.excluded_real_paths = vec!["/storage/emulated/0/DCIM/Open".to_string()];
        state.read_only_paths = vec![
            "/storage/emulated/0/DCIM".to_string(),
            "/storage/emulated/0/Pictures/Locked".to_string(),
        ];
        state.path_mappings = vec![PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Pictures/Locked".to_string(),
        )];

        assert!(is_path_read_only_locked(
            &state,
            "/storage/emulated/0/DCIM/Locked/a.jpg"
        ));
        assert!(!is_path_read_only_locked(
            &state,
            "/storage/emulated/0/DCIM/Open/a.jpg"
        ));

        assert!(is_path_or_mapped_target_read_only_locked(
            &state,
            "/storage/emulated/0/Download/QQ/a.jpg"
        ));
        assert_eq!(
            read_only_check_path_locked(&state, "/storage/emulated/0/Download/QQ/a.jpg"),
            "/storage/emulated/0/Pictures/Locked/a.jpg"
        );
    }

    #[test]
    fn router_mapping_read_only_uses_target_not_request_parent() {
        let mut state = RouterState::new();
        state.read_only_paths = vec!["/storage/emulated/0/Download".to_string()];
        state.read_only_excluded_paths =
            vec!["/storage/emulated/0/Download/ThirdParty/QQ".to_string()];
        state.path_mappings = vec![PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        )];

        assert!(!is_path_or_mapped_target_read_only_locked(
            &state,
            "/storage/emulated/0/Download/QQ/a.jpg"
        ));
    }

    #[test]
    fn router_mapping_target_inherits_parent_read_only_without_exclusion() {
        let mut state = RouterState::new();
        state.read_only_paths = vec!["/storage/emulated/0/Download".to_string()];
        state.path_mappings = vec![PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        )];

        assert!(is_path_or_mapped_target_read_only_locked(
            &state,
            "/storage/emulated/0/Download/QQ/a.jpg"
        ));
        assert_eq!(
            read_only_check_path_locked(&state, "/storage/emulated/0/Download/QQ/a.jpg"),
            "/storage/emulated/0/Download/ThirdParty/QQ/a.jpg"
        );
    }

    #[test]
    fn router_mapping_request_read_only_rule_does_not_make_target_read_only() {
        let mut state = RouterState::new();
        state.read_only_paths = vec!["/storage/emulated/0/Download/QQ".to_string()];
        state.path_mappings = vec![PathMapping::new(
            "/storage/emulated/0/Download/QQ".to_string(),
            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
        )];

        assert!(!is_path_or_mapped_target_read_only_locked(
            &state,
            "/storage/emulated/0/Download/QQ/a.jpg"
        ));
    }

    #[test]
    fn router_read_only_respects_read_only_exclusion_rules() {
        let mut state = RouterState::new();
        state.read_only_paths = vec!["/storage/emulated/0/Documents".to_string()];
        state.read_only_excluded_paths = vec!["/storage/emulated/0/Documents/tmp".to_string()];

        assert!(is_path_read_only_locked(
            &state,
            "/storage/emulated/0/Documents/report.txt"
        ));
        assert!(!is_path_read_only_locked(
            &state,
            "/storage/emulated/0/Documents/tmp/report.txt"
        ));
    }

    #[test]
    fn router_mappings_filter_cross_user_and_sort_longest_request_first() {
        let storage_root = paths::storage_user_root_for_user(0);
        let mappings = resolve_router_mappings(
            &[
                PathMapping::new(
                    "/storage/emulated/0/DCIM".to_string(),
                    "/storage/emulated/0/Pictures".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/DCIM/Nested".to_string(),
                    "/storage/emulated/0/Pictures/Nested".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/10/DCIM".to_string(),
                    "/storage/emulated/0/Pictures/OtherUser".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Movies".to_string(),
                    "/storage/emulated/10/Movies".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Download/Game".to_string(),
                    "/storage/emulated/0/Android/data/com.example.game/files".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Download/Obb".to_string(),
                    "/storage/emulated/0/Android/obb/com.example.game".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/Download/Media".to_string(),
                    "/storage/emulated/0/Android/media/com.example.game/cache".to_string(),
                ),
            ],
            0,
            &storage_root,
        );

        assert_eq!(mappings.len(), 3);
        assert_eq!(mappings[0].request_path, "/storage/emulated/0/DCIM/Nested");
        assert_eq!(mappings[1].request_path, "/storage/emulated/0/DCIM");
        assert_eq!(
            mappings[2].request_path,
            "/storage/emulated/0/Download/Media"
        );
    }

    #[test]
    fn router_mappings_dedup_case_variant_request_paths() {
        let storage_root = paths::storage_user_root_for_user(0);
        let mappings = resolve_router_mappings(
            &[
                PathMapping::new(
                    "/storage/emulated/0/Download/AppBucket".to_string(),
                    "/storage/emulated/0/Documents/First".to_string(),
                ),
                PathMapping::new(
                    "/storage/emulated/0/download/appbucket".to_string(),
                    "/storage/emulated/0/Documents/Second".to_string(),
                ),
            ],
            0,
            &storage_root,
        );

        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].final_path,
            "/storage/emulated/0/Documents/First"
        );
    }

    #[test]
    fn router_mappings_drop_cycles_and_overly_deep_chains() {
        let storage_root = paths::storage_user_root_for_user(0);
        let mappings = resolve_router_mappings(
            &[
                PathMapping::new("A".to_string(), "B".to_string()),
                PathMapping::new("B".to_string(), "C".to_string()),
                PathMapping::new("C".to_string(), "A".to_string()),
                PathMapping::new("Depth00".to_string(), "Depth01".to_string()),
                PathMapping::new("Depth01".to_string(), "Depth02".to_string()),
                PathMapping::new("Depth02".to_string(), "Depth03".to_string()),
                PathMapping::new("Depth03".to_string(), "Depth04".to_string()),
                PathMapping::new("Depth04".to_string(), "Depth05".to_string()),
                PathMapping::new("Depth05".to_string(), "Depth06".to_string()),
                PathMapping::new("Depth06".to_string(), "Depth07".to_string()),
                PathMapping::new("Depth07".to_string(), "Depth08".to_string()),
                PathMapping::new("Depth08".to_string(), "Depth09".to_string()),
                PathMapping::new("Depth09".to_string(), "Depth10".to_string()),
                PathMapping::new("Depth10".to_string(), "Depth11".to_string()),
                PathMapping::new("Keep".to_string(), "Target".to_string()),
            ],
            0,
            &storage_root,
        );

        let request_paths: Vec<&str> = mappings
            .iter()
            .map(|mapping| mapping.request_path.as_str())
            .collect();
        assert!(!request_paths.contains(&"/storage/emulated/0/A"));
        assert!(!request_paths.contains(&"/storage/emulated/0/B"));
        assert!(!request_paths.contains(&"/storage/emulated/0/C"));
        assert!(!request_paths.contains(&"/storage/emulated/0/Depth00"));
        assert!(request_paths.contains(&"/storage/emulated/0/Depth01"));
        assert!(request_paths.contains(&"/storage/emulated/0/Keep"));
    }
}
