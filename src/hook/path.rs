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
    if pathname.starts_with("/storage/emulated/") {
        return true;
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

    if !pathname.starts_with("/data/") {
        return false;
    }

    // 结果要求 normalize 后仍以 /data/media/ 开头，而 normalize 对 /data/ 路径只会
    // 折叠重复斜杠、去尾斜杠并把 /data/media/<数字用户> 改写为 /storage/emulated/。
    // 因此不含重复斜杠且前缀不是 /data/media/ 的路径必然不命中，可以在分配之前排除。
    // 应用进程访问 /data/user/0/<包名>/、/data/app/ 等路径极其频繁，这里省掉的是每次
    // syscall 一次 String 分配；含重复斜杠的少见形态仍交给 normalize 判定，行为不变。
    if !pathname.contains("//") && !pathname.starts_with("/data/media/") {
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

pub fn is_normalized_relevant_storage_path(hub: &InterceptHub, pathname: &str) -> bool {
    paths::starts_with(pathname, "/storage/emulated/")
        || (hub.is_redirect_enabled() && paths::starts_with(pathname, "/data/media/"))
}
