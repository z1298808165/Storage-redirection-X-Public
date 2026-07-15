use super::{WatchNode, WatchRoot, WatchStart};
use crate::config::MonitorAppSpec;
use crate::platform::paths;

pub(super) fn build_watch_roots(spec: &MonitorAppSpec) -> Vec<WatchRoot> {
    let context = WatchRootBuildContext::new(spec);
    let mut roots = Vec::new();

    if let Some(root) = build_default_watch_root(spec, &context) {
        roots.push(root);
    }
    roots.extend(build_allowed_real_path_watch_roots(spec, &context));
    roots.extend(build_read_only_path_watch_roots(spec, &context));
    roots.extend(build_path_mapping_watch_roots(spec, &context));
    roots.extend(build_sandbox_watch_roots(spec, &context));

    roots
}

pub(super) fn build_private_owner_repair_roots(spec: &MonitorAppSpec) -> Vec<WatchRoot> {
    if !spec.is_enabled {
        return Vec::new();
    }

    let context = WatchRootBuildContext::new(spec);
    ["media", "data", "obb"]
        .into_iter()
        .filter_map(|category| {
            let display_root = paths::join(
                &paths::join(&paths::join(&context.storage_root, "Android"), category),
                &spec.package_name,
            );
            let backend_root = paths::storage_to_data_media_for_user(&display_root, spec.user_id)?;
            Some(WatchRoot {
                package_name: spec.package_name.clone(),
                backend_root,
                display_root: display_root.clone(),
                record_display_root: display_root,
                record_from_root: String::new(),
                excluded_roots: Vec::new(),
                source: "private_owner",
            })
        })
        .collect()
}

pub(super) fn build_public_owner_repair_root(spec: &MonitorAppSpec) -> Option<WatchRoot> {
    if !spec.is_enabled || spec.user_id < 0 {
        return None;
    }

    let display_root = paths::storage_user_root_for_user(spec.user_id);
    Some(WatchRoot {
        package_name: String::new(),
        backend_root: paths::data_media_user_root_for_user(spec.user_id),
        display_root: display_root.clone(),
        record_display_root: display_root,
        record_from_root: String::new(),
        excluded_roots: Vec::new(),
        source: "public_owner",
    })
}

struct WatchRootBuildContext {
    storage_root: String,
    redirect_storage_root: String,
    app_data_dir: String,
}

impl WatchRootBuildContext {
    fn new(spec: &MonitorAppSpec) -> Self {
        Self {
            storage_root: paths::storage_user_root_for_user(spec.user_id),
            redirect_storage_root: paths::default_redirect_target(&spec.package_name, spec.user_id),
            app_data_dir: format!("/data/user/{}/{}", spec.user_id, spec.package_name),
        }
    }
}

fn build_default_watch_root(
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Option<WatchRoot> {
    if spec.is_enabled && spec.is_mapping_mode_only {
        return None;
    }

    let (backend_root, source) = if spec.is_enabled {
        (
            paths::storage_to_data_media_for_user(&context.redirect_storage_root, spec.user_id)?,
            "redirect_root",
        )
    } else {
        (
            paths::data_media_user_root_for_user(spec.user_id),
            "public_root",
        )
    };
    Some(WatchRoot {
        package_name: spec.package_name.clone(),
        backend_root,
        display_root: context.storage_root.clone(),
        record_display_root: context.storage_root.clone(),
        record_from_root: String::new(),
        excluded_roots: Vec::new(),
        source,
    })
}

fn build_allowed_real_path_watch_roots(
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Vec<WatchRoot> {
    let mut roots = Vec::new();
    if !spec.is_mapping_mode_only {
        let excluded_roots = resolved_excluded_roots(&spec.excluded_real_paths, spec, context);
        for allowed_path in &spec.allowed_real_paths {
            let Some(display_root) = resolve_profile_storage_path(
                allowed_path,
                spec.user_id,
                &context.storage_root,
                &context.app_data_dir,
                &context.redirect_storage_root,
            ) else {
                continue;
            };
            if paths::contains_wildcards(&display_root) {
                continue;
            }
            let Some(backend_root) =
                paths::storage_to_data_media_for_user(&display_root, spec.user_id)
            else {
                continue;
            };
            roots.push(WatchRoot {
                package_name: spec.package_name.clone(),
                backend_root,
                display_root: display_root.clone(),
                record_display_root: display_root,
                record_from_root: String::new(),
                excluded_roots: excluded_roots.clone(),
                source: "allowed_real_path",
            });
        }
    }
    roots
}

fn build_read_only_path_watch_roots(
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Vec<WatchRoot> {
    let mut roots = Vec::new();
    let (included_read_only_paths, excluded_read_only_paths) =
        paths::split_exclusion_rules(&spec.read_only_paths);
    let mut excluded_roots = resolved_excluded_roots(&spec.excluded_real_paths, spec, context);
    excluded_roots.extend(resolved_excluded_roots(
        &paths::overlapping_exclusion_rules(&included_read_only_paths, &excluded_read_only_paths),
        spec,
        context,
    ));
    paths::sort_dedup_paths_case_insensitive(&mut excluded_roots);
    for read_only_path in &included_read_only_paths {
        let Some(display_root) = resolve_profile_storage_path(
            read_only_path,
            spec.user_id,
            &context.storage_root,
            &context.app_data_dir,
            &context.redirect_storage_root,
        ) else {
            continue;
        };
        if paths::contains_wildcards(&display_root) {
            continue;
        }
        let Some(backend_root) = paths::storage_to_data_media_for_user(&display_root, spec.user_id)
        else {
            continue;
        };
        roots.push(WatchRoot {
            package_name: spec.package_name.clone(),
            backend_root,
            display_root: display_root.clone(),
            record_display_root: display_root,
            record_from_root: String::new(),
            excluded_roots: excluded_roots.clone(),
            source: "read_only_path",
        });
    }
    roots
}

fn build_path_mapping_watch_roots(
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Vec<WatchRoot> {
    let mut roots = Vec::new();
    for mapping in &spec.path_mappings {
        let Some(request_path) = resolve_profile_storage_path(
            &mapping.request_path,
            spec.user_id,
            &context.storage_root,
            &context.app_data_dir,
            &context.redirect_storage_root,
        ) else {
            continue;
        };
        let Some(final_path) = resolve_profile_storage_path(
            &mapping.final_path,
            spec.user_id,
            &context.storage_root,
            &context.app_data_dir,
            &context.redirect_storage_root,
        ) else {
            continue;
        };
        if paths::eq_ignore_case(&request_path, &final_path) {
            continue;
        }
        if paths::is_android_data_or_obb_path(&final_path) {
            continue;
        }
        let Some(backend_root) = paths::storage_to_data_media_for_user(&final_path, spec.user_id)
        else {
            continue;
        };
        roots.push(WatchRoot {
            package_name: spec.package_name.clone(),
            backend_root,
            display_root: final_path.clone(),
            record_display_root: final_path,
            record_from_root: request_path,
            excluded_roots: Vec::new(),
            source: "path_mapping",
        });
    }
    roots
}

fn build_sandbox_watch_roots(
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Vec<WatchRoot> {
    let mut roots = Vec::new();
    if !spec.is_mapping_mode_only {
        return roots;
    }

    let Some(redirect_backend_root) =
        paths::storage_to_data_media_for_user(&context.redirect_storage_root, spec.user_id)
    else {
        return roots;
    };
    for sandboxed_path in &spec.sandboxed_paths {
        let Some(display_root) = resolve_profile_storage_path(
            sandboxed_path,
            spec.user_id,
            &context.storage_root,
            &context.app_data_dir,
            &context.redirect_storage_root,
        ) else {
            continue;
        };
        let Some(relative) = paths::storage_relative_path_for_user(&display_root, spec.user_id)
        else {
            continue;
        };
        let landing_display_root = paths::join(&context.redirect_storage_root, &relative);
        roots.push(WatchRoot {
            package_name: spec.package_name.clone(),
            backend_root: paths::join(&redirect_backend_root, &relative),
            display_root: landing_display_root.clone(),
            record_display_root: landing_display_root,
            record_from_root: display_root,
            excluded_roots: Vec::new(),
            source: "sandbox_path",
        });
    }
    roots
}

fn resolve_profile_storage_path(
    path: &str,
    user_id: i32,
    storage_root: &str,
    app_data_dir: &str,
    redirect_storage_root: &str,
) -> Option<String> {
    if path.is_empty() || paths::contains_wildcards(path) {
        return None;
    }

    let mut resolved = paths::normalize(path);
    resolved = paths::resolve_placeholders(&resolved, app_data_dir, redirect_storage_root);
    resolved = paths::resolve_user_path(&resolved, user_id);
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    } else {
        resolved = paths::normalize(&resolved);
    }

    if paths::has_unsafe_segments(&resolved) || paths::eq_ignore_case(&resolved, storage_root) {
        return None;
    }
    if !paths::is_child(&resolved, storage_root) {
        return None;
    }
    Some(resolved)
}

pub(super) fn dedup_roots(roots: &mut Vec<WatchRoot>) {
    roots.sort_by(|left, right| {
        left.package_name
            .cmp(&right.package_name)
            .then_with(|| {
                paths::match_key(&left.backend_root).cmp(&paths::match_key(&right.backend_root))
            })
            .then_with(|| {
                paths::match_key(&left.display_root).cmp(&paths::match_key(&right.display_root))
            })
            .then_with(|| {
                paths::match_key(&left.record_display_root)
                    .cmp(&paths::match_key(&right.record_display_root))
            })
            .then_with(|| {
                paths::match_key(&left.record_from_root)
                    .cmp(&paths::match_key(&right.record_from_root))
            })
            .then_with(|| {
                monitor_source_priority(left.source).cmp(&monitor_source_priority(right.source))
            })
    });
    roots.dedup_by(|left, right| {
        left.package_name == right.package_name
            && paths::eq_ignore_case(&left.backend_root, &right.backend_root)
            && paths::eq_ignore_case(&left.display_root, &right.display_root)
            && paths::eq_ignore_case(&left.record_display_root, &right.record_display_root)
            && paths::eq_ignore_case(&left.record_from_root, &right.record_from_root)
    });
}

pub(super) fn sort_roots_by_monitor_priority(roots: &mut [WatchRoot]) {
    roots.sort_by(|left, right| {
        monitor_source_priority(left.source)
            .cmp(&monitor_source_priority(right.source))
            .then_with(|| {
                private_owner_category_priority(left).cmp(&private_owner_category_priority(right))
            })
            .then_with(|| left.package_name.cmp(&right.package_name))
            .then_with(|| left.backend_root.cmp(&right.backend_root))
            .then_with(|| left.display_root.cmp(&right.display_root))
            .then_with(|| left.record_display_root.cmp(&right.record_display_root))
            .then_with(|| left.record_from_root.cmp(&right.record_from_root))
    });
}

fn monitor_source_priority(source: &str) -> u8 {
    match source {
        "public_owner" => 0,
        "private_owner" => 1,
        "path_mapping" => 2,
        "sandbox_path" => 3,
        "read_only_path" => 4,
        "allowed_real_path" => 5,
        "redirect_root" => 6,
        "public_root" => 7,
        _ => 8,
    }
}

fn private_owner_category_priority(root: &WatchRoot) -> u8 {
    if root.source != "private_owner" {
        return 0;
    }
    let normalized = paths::normalize(&root.display_root);
    if normalized.contains("/Android/media/") {
        0
    } else if normalized.contains("/Android/data/") {
        1
    } else if normalized.contains("/Android/obb/") {
        2
    } else {
        3
    }
}

pub(super) fn is_high_value_monitor_source(source: &str) -> bool {
    matches!(source, "path_mapping" | "sandbox_path" | "private_owner")
}

pub(super) fn select_watch_start(root: &WatchRoot) -> Option<WatchStart> {
    match directory_status(&root.backend_root) {
        Ok(true) => Some(WatchStart {
            backend_dir: root.backend_root.clone(),
            display_dir: root.display_root.clone(),
        }),
        Ok(false) => None,
        Err(errno) => {
            if is_high_value_monitor_source(root.source)
                && root.display_root != root.backend_root
                && matches!(errno, libc::EACCES | libc::EPERM)
            {
                match directory_status(&root.display_root) {
                    Ok(true) => {
                        return Some(WatchStart {
                            backend_dir: root.display_root.clone(),
                            display_dir: root.display_root.clone(),
                        });
                    }
                    Ok(false) => return None,
                    Err(_) => {}
                }
            }
            if is_high_value_monitor_source(root.source) && errno == libc::ENOENT {
                return select_existing_ancestor_watch_start(root);
            }
            None
        }
    }
}

fn select_existing_ancestor_watch_start(root: &WatchRoot) -> Option<WatchStart> {
    let mut backend_dir = paths::parent(&root.backend_root);
    while !backend_dir.is_empty() && backend_dir != "/" {
        match directory_status(&backend_dir) {
            Ok(true) => {
                if let Some(display_dir) = align_display_dir_to_backend_ancestor(root, &backend_dir)
                {
                    return Some(WatchStart {
                        backend_dir,
                        display_dir,
                    });
                }
                backend_dir = paths::parent(&backend_dir);
            }
            Ok(false) => return None,
            Err(_) => {
                backend_dir = paths::parent(&backend_dir);
            }
        }
    }
    None
}

pub(super) fn align_display_dir_to_backend_ancestor(
    root: &WatchRoot,
    backend_dir: &str,
) -> Option<String> {
    let suffix = paths::child_suffix(&root.backend_root, backend_dir)?;
    strip_path_suffix(&root.display_root, suffix)
}

fn strip_path_suffix(path: &str, suffix: &str) -> Option<String> {
    if suffix.is_empty() {
        return Some(paths::normalize(path));
    }
    let normalized = paths::normalize(path);
    if !normalized.ends_with(suffix) {
        return None;
    }
    let keep_len = normalized.len().saturating_sub(suffix.len());
    if keep_len == 0 {
        return Some("/".to_string());
    }
    Some(normalized[..keep_len].trim_end_matches('/').to_string())
}

pub(super) fn should_descend_into_child(node: &WatchNode, child_display_dir: &str) -> bool {
    if node.source == "public_owner" && is_android_app_private_path(child_display_dir) {
        return false;
    }
    paths::is_same_or_child(child_display_dir, &node.record_display_root)
        || paths::is_same_or_child(&node.record_display_root, child_display_dir)
}

fn is_android_app_private_path(path: &str) -> bool {
    let normalized = paths::normalize(path);
    let user_id = paths::extract_user_id_from_storage_path(&normalized);
    if user_id < 0 {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(&normalized, &storage_root) else {
        return false;
    };
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    parts.next() == Some("Android") && matches!(parts.next(), Some("data" | "media" | "obb"))
}

pub(super) fn should_record_display_path(display_path: &str, record_display_root: &str) -> bool {
    paths::is_same_or_child(display_path, record_display_root)
}

pub(super) fn map_record_from_path(
    display_path: &str,
    record_display_root: &str,
    record_from_root: &str,
) -> String {
    if record_from_root.is_empty() {
        return String::new();
    }
    paths::child_suffix(display_path, record_display_root)
        .map(|suffix| paths::normalize(&format!("{}{}", record_from_root, suffix)))
        .unwrap_or_default()
}

fn resolved_excluded_roots(
    paths_in: &[String],
    spec: &MonitorAppSpec,
    context: &WatchRootBuildContext,
) -> Vec<String> {
    let mut roots: Vec<String> = paths_in
        .iter()
        .filter_map(|path| {
            resolve_profile_storage_path(
                path,
                spec.user_id,
                &context.storage_root,
                &context.app_data_dir,
                &context.redirect_storage_root,
            )
        })
        .collect();
    paths::sort_dedup_paths_longest_first_case_insensitive(&mut roots);
    roots
}

pub(super) fn is_under_any_root(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| paths::is_same_or_child(path, root))
}

fn directory_status(path: &str) -> Result<bool, i32> {
    std::fs::metadata(path)
        .map(|meta| meta.is_dir())
        .map_err(|error| raw_os_error(&error))
}

fn raw_os_error(error: &std::io::Error) -> i32 {
    error.raw_os_error().unwrap_or(libc::EIO)
}
