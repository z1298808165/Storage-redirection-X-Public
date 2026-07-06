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
}

const MAX_WILDCARD_MOUNT_MATCHES: usize = 128;
const MAX_WILDCARD_SCAN_DIRS: usize = 512;

pub(super) fn concrete_mount_fallback_parent(
    resolved_path: &str,
    storage_path: &str,
) -> Option<String> {
    paths::wildcard_mount_fallback_parent(resolved_path, storage_path)
}

pub(super) fn concrete_wildcard_mount_matches(
    resolved_path: &str,
    storage_path: &str,
) -> Vec<String> {
    let normalized = paths::normalize(resolved_path);
    if normalized.is_empty() || !paths::contains_wildcards(&normalized) {
        return Vec::new();
    }

    let scan_root = paths::concrete_prefix_before_wildcard(&normalized);
    if scan_root.is_empty()
        || (!paths::eq_ignore_case(&scan_root, storage_path)
            && !paths::is_child(&scan_root, storage_path))
    {
        return Vec::new();
    }
    if !std::path::Path::new(&scan_root).is_dir() {
        return Vec::new();
    }

    let root_depth = path_segment_count(&scan_root);
    let rule_depth = path_segment_count(&normalized);
    if rule_depth <= root_depth {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut scanned_dirs = 0usize;
    scan_wildcard_mount_matches(
        &scan_root,
        rule_depth - root_depth,
        &normalized,
        storage_path,
        &mut matches,
        &mut scanned_dirs,
    );
    paths::sort_dedup_paths_case_insensitive(&mut matches);
    matches
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
    paths::normalize(&path.as_ref().to_string_lossy().replace('\\', "/"))
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
}

#[cfg(test)]
mod tests {
    use super::{concrete_mount_fallback_parent, concrete_wildcard_mount_matches};
    use std::fs;

    #[test]
    fn wildcard_parent_fallback_uses_nearest_concrete_parent() {
        assert_eq!(
            concrete_mount_fallback_parent(
                "/storage/emulated/0/Download/A*",
                "/storage/emulated/0",
            ),
            Some("/storage/emulated/0/Download".to_string())
        );
        assert_eq!(
            concrete_mount_fallback_parent(
                "/storage/emulated/0/Download/A/*",
                "/storage/emulated/0",
            ),
            Some("/storage/emulated/0/Download/A".to_string())
        );
        assert_eq!(
            concrete_mount_fallback_parent("/storage/emulated/0/*", "/storage/emulated/0"),
            None
        );
    }

    #[test]
    fn wildcard_mount_matches_expand_existing_directories() {
        let root = temp_storage_root("wildcard_matches");
        let download = root.join("Download");
        fs::create_dir_all(download.join("Alpha")).expect("create Alpha");
        fs::create_dir_all(download.join("Beta")).expect("create Beta");
        fs::write(download.join("Archive.tmp"), b"file").expect("create file");

        let storage = root.to_string_lossy().replace('\\', "/");
        let matches = concrete_wildcard_mount_matches(&format!("{storage}/Download/A*"), &storage);

        assert_eq!(matches, vec![format!("{storage}/Download/Alpha")]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wildcard_mount_matches_can_scan_storage_root_without_parent_fallback() {
        let root = temp_storage_root("wildcard_root_matches");
        fs::create_dir_all(root.join("Download").join("A")).expect("create Download/A");
        fs::create_dir_all(root.join("Pictures").join("A")).expect("create Pictures/A");

        let storage = root.to_string_lossy().replace('\\', "/");
        let matches = concrete_wildcard_mount_matches(&format!("{storage}/*/A"), &storage);

        assert_eq!(
            matches,
            vec![
                format!("{storage}/Download/A"),
                format!("{storage}/Pictures/A"),
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    fn temp_storage_root(name: &str) -> std::path::PathBuf {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "srx_mount_{name}_{}_{}",
            std::process::id(),
            millis
        ));
        fs::create_dir_all(&root).expect("create temp storage");
        root
    }
}
