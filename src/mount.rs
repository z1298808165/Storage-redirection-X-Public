use crate::platform::{self, paths};
use std::cell::RefCell;
#[path = "mount/alias.rs"]
mod alias;
#[path = "mount/apply.rs"]
mod apply;
#[path = "mount/core.rs"]
mod core;
#[path = "mount/map.rs"]
mod map;

pub struct MountPlanner {
    should_unshare: bool,
    is_namespace_ready: bool,
    is_storage_root_redirected: bool,
    package_name: String,
    app_uid: i32,
    user_id: i32,
    app_data_dir: String,
    redirect_target: String,
    mounted_targets: RefCell<Vec<String>>,
    is_file_monitor_enabled: bool,
    real_storage_anchor: Option<String>,
}

#[derive(Copy, Clone)]
enum PrimaryMountFailure {
    AbortAll,
    StopCurrentTarget,
    ContinueAliases,
}

const MAX_WILDCARD_MOUNT_MATCHES: usize = 128;
const MAX_WILDCARD_SCAN_DIRS: usize = 512;

pub(super) fn concrete_mount_fallback_parent(
    resolved_path: &str,
    storage_path: &str,
) -> Option<String> {
    paths::wildcard_mount_fallback_parent(resolved_path, storage_path)
}

fn concrete_wildcard_mount_matches_for_roots(
    resolved_path: &str,
    storage_path: &str,
    scan_roots: &[String],
) -> Vec<String> {
    let normalized = paths::normalize(resolved_path);
    if normalized.is_empty() || !paths::contains_wildcards(&normalized) {
        return Vec::new();
    }

    let Some(relative_rule) = paths::relative_child_path(&normalized, storage_path) else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for scan_root in scan_roots {
        let source_rule = paths::join(scan_root, relative_rule);
        append_concrete_wildcard_mount_matches_from_root(
            &source_rule,
            storage_path,
            scan_root,
            &mut matches,
        );
    }

    paths::sort_dedup_paths_case_insensitive(&mut matches);
    matches
}

fn append_concrete_wildcard_mount_matches_from_root(
    source_rule: &str,
    storage_path: &str,
    scan_storage_root: &str,
    matches: &mut Vec<String>,
) {
    let scan_root = concrete_prefix_before_wildcard_preserving_alias(source_rule);
    if scan_root.is_empty()
        || (!paths::eq_ignore_case(&scan_root, scan_storage_root)
            && !paths::is_child(&scan_root, scan_storage_root))
    {
        log::debug!(
            "wildcard scan skip root outside storage source={} scan_root={} storage={}",
            source_rule,
            scan_root,
            scan_storage_root
        );
        return;
    }
    if !std::path::Path::new(&scan_root).is_dir() {
        log::debug!(
            "wildcard scan skip missing root source={} scan_root={} storage={}",
            source_rule,
            scan_root,
            scan_storage_root
        );
        return;
    }

    let root_depth = path_segment_count(&scan_root);
    let rule_depth = path_segment_count(source_rule);
    if rule_depth <= root_depth {
        return;
    }

    let mut source_matches = Vec::new();
    let mut scanned_dirs = 0usize;
    scan_wildcard_mount_matches(
        &scan_root,
        rule_depth - root_depth,
        source_rule,
        scan_storage_root,
        &mut source_matches,
        &mut scanned_dirs,
    );
    log::debug!(
        "wildcard scan root done source={} scan_root={} storage={} matches={} scanned={}",
        source_rule,
        scan_root,
        scan_storage_root,
        source_matches.len(),
        scanned_dirs
    );
    for source_match in source_matches {
        let Some(relative) = paths::relative_child_path(&source_match, scan_storage_root) else {
            continue;
        };
        matches.push(paths::join(storage_path, relative));
    }
}

fn scan_wildcard_mount_matches(
    current_dir: &str,
    remaining_depth: usize,
    rule_path: &str,
    storage_path: &str,
    matches: &mut Vec<String>,
    scanned_dirs: &mut usize,
) {
    if remaining_depth == 0
        || matches.len() >= MAX_WILDCARD_MOUNT_MATCHES
        || *scanned_dirs >= MAX_WILDCARD_SCAN_DIRS
    {
        return;
    }

    let Ok(entries) = std::fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if matches.len() >= MAX_WILDCARD_MOUNT_MATCHES || *scanned_dirs >= MAX_WILDCARD_SCAN_DIRS {
            break;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        *scanned_dirs += 1;
        let candidate = normalize_host_path(entry.path());
        if !paths::is_child(&candidate, storage_path)
            || !wildcard_prefix_matches(rule_path, &candidate)
        {
            continue;
        }

        if remaining_depth == 1 {
            if paths::matches(rule_path, &candidate, true) {
                matches.push(candidate);
            }
            continue;
        }

        scan_wildcard_mount_matches(
            &candidate,
            remaining_depth - 1,
            rule_path,
            storage_path,
            matches,
            scanned_dirs,
        );
    }
}

fn wildcard_prefix_matches(rule_path: &str, candidate_path: &str) -> bool {
    let rule_segments: Vec<&str> = rule_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let candidate_depth = path_segment_count(candidate_path);
    if candidate_depth == 0 || candidate_depth > rule_segments.len() {
        return false;
    }

    let prefix = rule_segments[..candidate_depth].join("/");
    let prefix_rule = if rule_path.starts_with('/') {
        format!("/{prefix}")
    } else {
        prefix
    };
    paths::matches(&prefix_rule, candidate_path, false)
}

fn normalize_host_path(path: impl AsRef<std::path::Path>) -> String {
    normalize_mount_scan_path(&path.as_ref().to_string_lossy().replace('\\', "/"))
}

fn concrete_prefix_before_wildcard_preserving_alias(path: &str) -> String {
    let normalized = normalize_mount_scan_path(path);
    if normalized.is_empty() || !paths::contains_wildcards(&normalized) {
        return normalized;
    }

    let mut kept = Vec::new();
    for segment in normalized.split('/').filter(|segment| !segment.is_empty()) {
        if paths::contains_wildcards(segment) {
            break;
        }
        kept.push(segment);
    }
    if kept.is_empty() {
        return String::new();
    }

    let prefix = kept.join("/");
    if normalized.starts_with('/') {
        format!("/{prefix}")
    } else {
        prefix
    }
}

fn normalize_mount_scan_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
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
    result
}

fn path_segment_count(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

impl MountPlanner {
    pub fn new(
        package_name: &str,
        app_uid: i32,
        app_data_dir: &str,
        redirect_target: &str,
        should_unshare: bool,
    ) -> Self {
        let user_id = platform::user_id_from_uid(app_uid);
        log::debug!(
            "mount redirect pkg={} uid={} user={}",
            package_name,
            app_uid,
            user_id
        );
        Self {
            should_unshare,
            is_namespace_ready: false,
            is_storage_root_redirected: false,
            package_name: package_name.to_string(),
            app_uid,
            user_id,
            app_data_dir: app_data_dir.to_string(),
            redirect_target: paths::normalize(redirect_target),
            mounted_targets: RefCell::new(Vec::new()),
            is_file_monitor_enabled: false,
            real_storage_anchor: None,
        }
    }

    pub fn set_file_monitor_enabled(&mut self, enabled: bool) {
        self.is_file_monitor_enabled = enabled;
    }

    pub fn take_mounted_targets(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.mounted_targets.borrow_mut())
    }

    pub(super) fn concrete_wildcard_mount_matches(
        &self,
        resolved_path: &str,
        storage_path: &str,
    ) -> Vec<String> {
        let mut scan_roots = Vec::with_capacity(4);
        append_unique_scan_root(&mut scan_roots, storage_path.to_string());
        append_unique_scan_root(
            &mut scan_roots,
            paths::data_media_user_root_for_user(self.user_id),
        );
        append_unique_scan_root(
            &mut scan_roots,
            self.to_data_media_backend_path(&self.redirect_target),
        );
        if let Some(anchor) = &self.real_storage_anchor {
            append_unique_scan_root(&mut scan_roots, anchor.clone());
        }
        concrete_wildcard_mount_matches_for_roots(resolved_path, storage_path, &scan_roots)
    }
}

fn append_unique_scan_root(scan_roots: &mut Vec<String>, root: String) {
    if root.is_empty()
        || scan_roots
            .iter()
            .any(|existing| paths::eq_ignore_case(existing, &root))
    {
        return;
    }
    scan_roots.push(root);
}
