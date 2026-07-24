use crate::domain::PathMapping;
use crate::platform::{fs, paths};
use fuser::{MountOption, SessionACL};

#[derive(Clone)]
pub struct FuseRedirectConfig {
    pub package_name: String,
    pub uid: i32,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub mount_root: Option<String>,
    pub real_root_override: Option<String>,
    pub is_file_monitor_enabled: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub is_mapping_mode_only: bool,
}

impl FuseRedirectConfig {
    pub(super) fn user_id(&self) -> i32 {
        crate::platform::user_id_from_uid(self.uid)
    }
}

pub fn mount_blocking_with_ready(
    config: FuseRedirectConfig,
    ready_sock: Option<libc::c_int>,
) -> bool {
    let user_id = config.user_id();
    let mount_point = fuse_mount_point(&config, user_id);
    let metadata_dir = mount_point_metadata_dir(&mount_point, user_id);
    if !fs::create_directory(&metadata_dir, config.uid) {
        log::error!(
            "fuse redirect mount point missing: {} metadata={}",
            mount_point,
            metadata_dir
        );
        send_ready_result(ready_sock, -1);
        return false;
    }

    let fs = match super::FuseRedirectFs::new(config) {
        Some(fs) => fs,
        None => {
            send_ready_result(ready_sock, -1);
            return false;
        }
    };
    let mut mount_options = fuser::Config::default();
    mount_options.mount_options = vec![
        MountOption::FSName("srx_fuse_redirect".to_string()),
        MountOption::Subtype("srx".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoAtime,
        MountOption::Async,
    ];
    mount_options.acl = SessionACL::All;
    mount_options.n_threads = Some(4);
    mount_options.clone_fd = true;

    log::info!(
        "fuse redirect mount start pkg={} uid={} user={} mp={} rel={} real={} map_only={} allow={} excl={} sandbox={} ro={} map={}",
        fs.policy.package_name,
        fs.policy.uid,
        user_id,
        mount_point,
        fs.policy.mount_rel,
        fs.policy.real_root.display(),
        fs.policy.is_mapping_mode_only,
        fs.policy.allowed_real_paths.len(),
        fs.policy.excluded_real_paths.len(),
        fs.policy.sandboxed_paths.len(),
        fs.policy.read_only_paths.len(),
        fs.policy.path_mappings.len()
    );

    match fuser::mount2_with_ready(fs, &mount_point, &mount_options, |ready| {
        send_ready_result(ready_sock, if ready { 0 } else { -1 });
    }) {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "fuse redirect session ended with error mp={} err={}",
                mount_point,
                error
            );
            false
        }
    }
}

pub(super) fn fuse_mount_point(config: &FuseRedirectConfig, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(raw_mount_root) = config.mount_root.as_deref() else {
        return storage_root;
    };
    let mut mount_root = paths::resolve_user_path(&paths::normalize(raw_mount_root), user_id);
    if !paths::is_absolute(&mount_root) {
        mount_root = paths::normalize(&paths::join(&storage_root, &mount_root));
    }
    if paths::eq_ignore_case(&mount_root, &storage_root)
        || paths::is_child(&mount_root, &storage_root)
    {
        mount_root
    } else {
        storage_root
    }
}

fn mount_point_metadata_dir(mount_point: &str, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(mount_point, &storage_root) {
        return paths::data_media_user_root_for_user(user_id);
    }
    paths::storage_to_data_media_for_user(mount_point, user_id)
        .unwrap_or_else(|| mount_point.to_string())
}

pub fn scoped_mount_roots_for_wildcard_rules<'a>(
    uid: i32,
    rules: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut roots = Vec::new();
    for raw in rules {
        let raw = raw.trim_start();
        let raw = raw.strip_prefix('!').unwrap_or(raw).trim_start();
        let mut resolved = paths::resolve_user_path(&paths::normalize(raw), user_id);
        if resolved.is_empty()
            || paths::has_unsafe_segments(&resolved)
            || !paths::contains_wildcards(&resolved)
        {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if !paths::is_child(&resolved, &storage_root)
            && !paths::eq_ignore_case(&resolved, &storage_root)
        {
            continue;
        }
        let prefix = paths::concrete_prefix_before_wildcard(&resolved);
        if let Some(root) = scoped_mount_root_for_wildcard_prefix(&prefix, &storage_root) {
            roots.push(root);
        }
    }
    compact_scoped_mount_roots(roots, &storage_root)
}

fn scoped_mount_root_for_wildcard_prefix(prefix: &str, storage_root: &str) -> Option<String> {
    if prefix.is_empty() || !paths::is_child(prefix, storage_root) {
        return Some(storage_root.to_string());
    }
    if let Some(root) = public_collection_mount_root(prefix, storage_root) {
        return Some(root);
    }
    Some(prefix.to_string())
}

fn public_collection_mount_root(prefix: &str, storage_root: &str) -> Option<String> {
    public_collection_name(prefix, storage_root).map(|first| paths::join(storage_root, first))
}

fn public_collection_name<'a>(prefix: &'a str, storage_root: &str) -> Option<&'a str> {
    let rel = paths::relative_child_path(prefix, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    match first {
        "Alarms" | "Audiobooks" | "DCIM" | "Documents" | "Download" | "Movies" | "Music"
        | "Notifications" | "Pictures" | "Podcasts" | "Recordings" | "Ringtones" => Some(first),
        _ => None,
    }
}

pub fn scoped_mount_roots_for_hybrid_rules(
    uid: i32,
    allowed_real_paths: &[String],
    excluded_real_paths: &[String],
    sandboxed_paths: &[String],
    read_only_paths: &[String],
    path_mappings: &[crate::domain::PathMapping],
    is_mapping_mode_only: bool,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let scoped_allowed_rules = allowed_real_paths.iter().map(String::as_str);
    let mut roots = scoped_mount_roots_for_wildcard_rules(
        uid,
        scoped_allowed_rules
            .chain(excluded_real_paths.iter().map(String::as_str))
            .chain(sandboxed_paths.iter().map(String::as_str))
            .chain(read_only_paths.iter().map(String::as_str)),
    );

    if is_mapping_mode_only {
        for sandboxed_path in sandboxed_paths {
            let sandboxed_root =
                resolve_concrete_scoped_rule_parent(sandboxed_path, user_id, &storage_root);
            if !sandboxed_root.is_empty() {
                roots.push(sandboxed_root);
            }
        }
    }

    for allowed_path in allowed_real_paths {
        let allowed_root =
            resolve_concrete_scoped_rule_parent(allowed_path, user_id, &storage_root);
        if !allowed_root.is_empty() {
            roots.push(allowed_root);
        }
    }

    let normalized_read_only_paths = super::normalize_rule_list(read_only_paths.to_vec(), user_id);
    let (read_only_includes, read_only_excludes) =
        paths::split_exclusion_rules(&normalized_read_only_paths);
    let read_only_excludes =
        paths::overlapping_exclusion_rules(&read_only_includes, &read_only_excludes);
    let scoped_path_mappings = resolve_scoped_path_mappings(path_mappings, user_id, &storage_root);
    for read_only_root in &read_only_includes {
        if paths::contains_wildcards(read_only_root) {
            continue;
        }
        if read_only_excludes.iter().any(|excluded| {
            !paths::contains_wildcards(excluded) && paths::is_child(excluded, read_only_root)
        }) || scoped_path_mappings
            .iter()
            .any(|(request_path, final_path)| {
                (!paths::contains_wildcards(request_path)
                    && paths::is_child(request_path, read_only_root))
                    || (!paths::contains_wildcards(final_path)
                        && paths::is_child(final_path, read_only_root))
            })
        {
            roots.push(read_only_root.clone());
        }
    }

    compact_scoped_mount_roots(roots, &storage_root)
}

fn resolve_scoped_path_mappings(
    path_mappings: &[crate::domain::PathMapping],
    user_id: i32,
    storage_root: &str,
) -> Vec<(String, String)> {
    let mut resolved = Vec::with_capacity(path_mappings.len());
    for mapping in path_mappings {
        let request_path = resolve_scoped_rule_path(&mapping.request_path, user_id, storage_root);
        let final_path = resolve_scoped_rule_path(&mapping.final_path, user_id, storage_root);
        if request_path.is_empty()
            || final_path.is_empty()
            || paths::eq_ignore_case(&request_path, &final_path)
            || paths::is_android_data_or_obb_path(&final_path)
        {
            continue;
        }
        resolved.push((request_path, final_path));
    }
    resolved
}

fn resolve_concrete_scoped_rule_parent(path: &str, user_id: i32, storage_root: &str) -> String {
    let resolved = resolve_scoped_rule_path(path, user_id, storage_root);
    if resolved.is_empty()
        || paths::contains_wildcards(&resolved)
        || paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }

    let parent = paths::parent(&resolved);
    if paths::eq_ignore_case(&parent, storage_root) || paths::is_child(&parent, storage_root) {
        parent
    } else {
        String::new()
    }
}

fn resolve_scoped_rule_path(path: &str, user_id: i32, storage_root: &str) -> String {
    let mut resolved = paths::resolve_user_path(&paths::normalize(path), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return String::new();
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }
    resolved
}

fn compact_scoped_mount_roots(mut roots: Vec<String>, storage_root: &str) -> Vec<String> {
    paths::sort_dedup_paths_case_insensitive(&mut roots);
    roots.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    let mut effective: Vec<String> = Vec::new();
    for root in roots {
        if effective
            .iter()
            .any(|kept| paths::eq_ignore_case(kept, &root) || paths::is_child(&root, kept))
        {
            continue;
        }
        effective.push(root);
    }

    if effective.len() <= super::MAX_SCOPED_FUSE_ROOTS {
        return effective;
    }

    let mut top_level: Vec<String> = effective
        .iter()
        .map(|root| {
            top_level_storage_child(root, storage_root).unwrap_or_else(|| storage_root.to_string())
        })
        .collect();
    paths::sort_dedup_paths_case_insensitive(&mut top_level);
    if top_level.len() <= super::MAX_SCOPED_FUSE_ROOTS {
        return top_level;
    }

    vec![storage_root.to_string()]
}

fn top_level_storage_child(path: &str, storage_root: &str) -> Option<String> {
    if paths::eq_ignore_case(path, storage_root) {
        return None;
    }
    let rel = paths::relative_child_path(path, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    Some(paths::join(storage_root, first))
}

fn send_ready_result(sock: Option<libc::c_int>, result: i32) {
    let Some(sock) = sock else {
        return;
    };
    // SAFETY: sock 是有效的 socket fd，buffer 指针指向栈上有效数据，size 与类型匹配，调用期间保持有效。
    let _ = unsafe {
        libc::send(
            sock,
            &result as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    // SAFETY: sock 是有效的 socket fd，此处是唯一的关闭点，调用后不再使用。
    unsafe { libc::close(sock) };
}
