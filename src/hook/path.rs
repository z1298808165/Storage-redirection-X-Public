use super::stats::InterceptHub;
use crate::platform::paths;
use libc::{AT_FDCWD, readlink};
use std::ffi::CString;

// /proc/self/fd/<N> symlink 解析
pub fn resolve_dirfd_path(dirfd: i32) -> String {
    if dirfd == AT_FDCWD {
        return "AT_FDCWD".to_string();
    }

    let link_path = format!("/proc/self/fd/{}", dirfd);
    let Ok(c_path) = CString::new(link_path) else {
        return "<bad-dirfd>".to_string();
    };

    let mut buffer = [0u8; libc::PATH_MAX as usize + 1];
    let len = unsafe {
        readlink(
            c_path.as_ptr(),
            buffer.as_mut_ptr() as *mut _,
            buffer.len() - 1,
        )
    };
    if len <= 0 {
        return "<unresolved>".to_string();
    }
    buffer[len as usize] = 0;
    String::from_utf8_lossy(&buffer[..len as usize]).to_string()
}

pub fn resolve_current_working_directory() -> String {
    let Ok(c_path) = CString::new("/proc/self/cwd") else {
        return String::new();
    };

    let mut buffer = [0u8; libc::PATH_MAX as usize + 1];
    let len = unsafe {
        readlink(
            c_path.as_ptr(),
            buffer.as_mut_ptr() as *mut _,
            buffer.len() - 1,
        )
    };
    if len <= 0 {
        return String::new();
    }
    buffer[len as usize] = 0;
    String::from_utf8_lossy(&buffer[..len as usize]).to_string()
}

pub fn resolve_path_for_dirfd(dirfd: i32, pathname: &str) -> String {
    if pathname.is_empty() {
        return String::new();
    }

    if pathname.starts_with('/') {
        return paths::normalize(pathname);
    }

    let dirfd_path = if dirfd == AT_FDCWD {
        resolve_current_working_directory()
    } else {
        resolve_dirfd_path(dirfd)
    };
    if dirfd_path.is_empty() || !dirfd_path.starts_with('/') {
        return String::new();
    }

    let mut merged = dirfd_path;
    if !merged.ends_with('/') {
        merged.push('/');
    }
    merged.push_str(pathname);
    paths::normalize(&merged)
}

pub fn is_storage_path_fast(pathname: &str) -> bool {
    if pathname.is_empty() || !pathname.starts_with('/') {
        return false;
    }

    // 热路径：先以前缀快速排除大量非存储路径（如 /system, /data/app, /proc, /dev 等），
    // 只在可能命中存储别名时才做完整 normalize；避免每次 syscall hook 上分配 String。
    if has_potential_storage_prefix(pathname) {
        let normalized = paths::normalize(pathname);
        return paths::starts_with(&normalized, "/storage/emulated/");
    }
    false
}

pub fn is_data_media_path_fast(pathname: &str) -> bool {
    if pathname.is_empty() || !pathname.starts_with('/') {
        return false;
    }

    // /data/media/ 已经是规范化形态，直接比较前缀；其他 /data/... 路径快速短路
    if !pathname.starts_with("/data/") {
        return false;
    }
    let normalized = paths::normalize(pathname);
    paths::starts_with(&normalized, "/data/media/")
}

// 仅用于快路径筛选：路径以下列前缀开头才有可能在 normalize 后变成 /storage/emulated/...
fn has_potential_storage_prefix(pathname: &str) -> bool {
    pathname.starts_with("/storage/")
        || pathname.starts_with("/sdcard")
        || pathname.starts_with("/mnt/runtime/")
        || pathname.starts_with("/mnt/user/")
        || pathname.starts_with("/mnt/installer/")
        || pathname.starts_with("/mnt/androidwritable/")
        || pathname.starts_with("/mnt/pass_through/")
        || pathname.starts_with("/data/media/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_storage_path_check_handles_aliases() {
        assert!(is_storage_path_fast("/storage/emulated/0/DCIM/x.jpg"));
        assert!(is_storage_path_fast("/sdcard/Download/foo"));
        assert!(is_storage_path_fast("/storage/self/primary/Movies/foo.mp4"));
        assert!(is_storage_path_fast("/mnt/runtime/default/emulated/10/foo"));
        assert!(is_storage_path_fast("/mnt/user/0/primary/foo"));
        assert!(is_storage_path_fast("/data/media/0/Download/foo"));
    }

    #[test]
    fn fast_storage_path_check_rejects_non_storage() {
        assert!(!is_storage_path_fast("/system/bin/sh"));
        assert!(!is_storage_path_fast("/data/app/com.demo/base.apk"));
        assert!(!is_storage_path_fast("/data/user/0/com.demo/files/x"));
        assert!(!is_storage_path_fast("/proc/self/maps"));
        assert!(!is_storage_path_fast("/dev/binder"));
        assert!(!is_storage_path_fast(""));
        assert!(!is_storage_path_fast("relative/path"));
    }

    #[test]
    fn fast_data_media_path_check_does_not_double_match_storage_alias() {
        // normalize 会把 /data/media/N 转成 /storage/emulated/N，
        // 因此 is_data_media_path_fast 对 /data/media 路径返回 false 是预期行为；
        // 这里仅锁定该不变量，避免后续修改无意中改变语义。
        assert!(!is_data_media_path_fast("/data/media/0/Download/foo"));
        assert!(!is_data_media_path_fast("/data/app/com.demo"));
        assert!(!is_data_media_path_fast("/data/user/0/com.demo/files"));
        assert!(!is_data_media_path_fast("/storage/emulated/0/DCIM/x.jpg"));
        assert!(!is_data_media_path_fast("/system/bin"));
        assert!(!is_data_media_path_fast(""));
    }
}

// 启用重定向时 /data/media 也视为相关
pub fn is_relevant_storage_path(hub: &InterceptHub, pathname: &str) -> bool {
    if is_storage_path_fast(pathname) {
        return true;
    }

    if hub.is_redirect_enabled() && is_data_media_path_fast(pathname) {
        return true;
    }
    false
}
