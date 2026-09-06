use crate::platform::paths;

/// 将 FUSE 配置规则解析为当前用户的绝对路径，并保留排除规则语义。
pub(crate) fn normalize_rule_list(paths_in: Vec<String>, user_id: i32) -> Vec<String> {
    let mut out = Vec::with_capacity(paths_in.len());
    let storage_root = paths::storage_user_root_for_user(user_id);
    for path in paths_in {
        let path = path.trim_start();
        let (excluded, body) = path
            .strip_prefix('!')
            .map_or((false, path), |value| (true, value.trim_start()));
        let mut resolved = paths::resolve_user_path(&paths::normalize(body), user_id);
        if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if paths::is_child(&resolved, &storage_root) {
            out.push(if excluded {
                format!("!{resolved}")
            } else {
                resolved
            });
        }
    }
    paths::sort_dedup_paths_case_insensitive(&mut out);
    out
}
