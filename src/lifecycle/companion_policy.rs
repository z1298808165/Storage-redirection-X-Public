use crate::platform::paths;

pub fn package_uid_matches(uid: i32, package_uid: i32) -> bool {
    let app_id = uid % 100000;
    uid >= 0 && package_uid >= 0 && app_id == package_uid % 100000
}

/// fchown 只允许修复对应 Android 用户的应用私有树，设备、管道和已删除对象均拒绝。
pub fn chown_target_package(path: &str, uid: i32, mode: libc::mode_t) -> Option<String> {
    if !matches!(mode & libc::S_IFMT, libc::S_IFREG | libc::S_IFDIR)
        || path.ends_with(" (deleted)")
        || path.split('/').any(|part| matches!(part, "." | ".."))
        || uid < 0
    {
        return None;
    }
    let path = paths::normalize(path);
    if paths::extract_user_id_from_storage_path(&path) != uid / 100000 {
        return None;
    }
    let package = paths::extract_android_private_path_owner(&path);
    (!package.is_empty()).then_some(package)
}

pub fn mount_caller_matches(caller: &libc::ucred, target_pid: i32) -> bool {
    caller.uid == 0 && caller.pid > 0 && caller.pid == target_pid
}
