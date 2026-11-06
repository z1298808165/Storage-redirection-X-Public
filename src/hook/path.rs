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

/// 把路径折叠为“决策用绝对形态”：先按内核语义折叠 `.` / `..` 与重复斜杠，
/// 再解析存储别名。所有基于字符串的前缀/规则判断都应使用这一形态，避免
/// `//storage/...`、`/storage/../storage/...` 这类写法绕过快速筛选。
pub fn normalize_decision_path(pathname: &str) -> String {
    if pathname.is_empty() {
        return String::new();
    }
    // 含点段/重复斜杠时，内核会先解析符号链接再处理 `..`。纯词法折叠在
    // `/storage/.../link/../file` 这类形态下会得到错误目标，因此优先用
    // realpath 语义（canonicalize）；目标不存在时只 canonicalize 父目录，
    // 保留最后一段不跟随（与创建型 open 的语义一致）。
    // magic link 的前缀本身不是存储路径，但内核会解析到 fd/root 的真实目标；
    // 这里先 canonicalize，让后续策略看到真实绝对路径，而不是把 /proc 当非存储。
    let is_magic_link = pathname.starts_with("/proc/self/fd/")
        || pathname.starts_with("/proc/thread-self/fd/")
        || pathname.starts_with("/dev/fd/")
        || pathname.starts_with("/proc/self/root")
        || pathname.starts_with("/proc/thread-self/root");
    if is_magic_link && let Ok(real) = std::fs::canonicalize(pathname) {
        return paths::normalize(&real.to_string_lossy());
    }
    if paths::needs_dot_segment_collapse(pathname)
        || pathname.starts_with("//")
        || pathname.contains("//")
    {
        if let Ok(real) = std::fs::canonicalize(pathname) {
            return paths::normalize(&real.to_string_lossy());
        }
        let path = std::path::Path::new(pathname);
        if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
            && let Ok(real_parent) = std::fs::canonicalize(parent)
        {
            return paths::normalize(&real_parent.join(name).to_string_lossy());
        }
        // 整条链都不存在时退回词法折叠，保证 create 场景仍能进入策略判断。
        let collapsed = paths::collapse_dot_segments(pathname);
        return paths::normalize(&collapsed);
    }
    paths::normalize(pathname)
}

/// 解析 dirfd 当前指向的真实绝对路径；用于 `AT_EMPTY_PATH` 这类没有
/// pathname、但操作对象由 fd 决定的调用。失败返回空串。
pub fn resolve_fd_target(dirfd: i32) -> String {
    if dirfd == AT_FDCWD {
        return String::new();
    }
    let raw = resolve_dirfd_path(dirfd);
    if raw.starts_with('/') {
        normalize_decision_path(&raw)
    } else {
        String::new()
    }
}

pub fn resolve_path_for_dirfd(dirfd: i32, pathname: &str) -> String {
    if pathname.is_empty() {
        return String::new();
    }

    if pathname.starts_with('/') {
        return normalize_decision_path(pathname);
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
    normalize_decision_path(&merged)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    macro_rules! test_require {
        ($result:expr, $context:literal) => {
            match $result {
                Ok(value) => value,
                Err(error) => panic!("{}: {:?}", $context, error),
            }
        };
    }

    fn test_dir(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "srx-path-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        test_require!(std::fs::create_dir_all(base.join("a")), "create a");
        test_require!(std::fs::create_dir_all(base.join("b")), "create b");
        base
    }

    #[test]
    fn dot_segments_resolve_against_existing_parent() {
        let base = test_dir("dot");
        let raw = format!("{}/a/../b/missing", base.display());
        let expected = format!(
            "{}/b/missing",
            test_require!(std::fs::canonicalize(&base), "base").display()
        );
        assert_eq!(normalize_decision_path(&raw), expected);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn duplicate_slashes_are_collapsed() {
        let base = test_dir("slash");
        let raw = format!("{}//a//missing", base.display());
        let expected = format!(
            "{}/a/missing",
            test_require!(std::fs::canonicalize(&base), "base").display()
        );
        assert_eq!(normalize_decision_path(&raw), expected);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn relative_dirfd_resolves_dot_segments() {
        let base = test_dir("dirfd");
        let c_path = test_require!(
            std::ffi::CString::new(base.to_string_lossy().as_bytes()),
            "c path"
        );
        // SAFETY: c_path 指向有效的 NUL 结尾路径，open 不保留指针。
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
        assert!(fd >= 0);
        let resolved = resolve_path_for_dirfd(fd, "a/../b/missing");
        let expected = format!(
            "{}/b/missing",
            test_require!(std::fs::canonicalize(&base), "base").display()
        );
        assert_eq!(resolved, expected);
        // SAFETY: fd 由上面的 open 返回且未被其它对象接管。
        drop(unsafe { std::fs::File::from_raw_fd(fd) });
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn magic_fd_link_resolves_to_target() {
        let base = test_dir("magic");
        let c_path = test_require!(
            std::ffi::CString::new(base.to_string_lossy().as_bytes()),
            "c path"
        );
        // SAFETY: c_path 指向有效的 NUL 结尾路径，open 不保留指针。
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
        assert!(fd >= 0);
        let raw = format!("/proc/self/fd/{fd}");
        let resolved = normalize_decision_path(&raw);
        assert_eq!(
            resolved,
            test_require!(std::fs::canonicalize(&base), "base")
                .to_string_lossy()
                .to_string()
        );
        // SAFETY: fd 由上面的 open 返回且未被其它对象接管。
        drop(unsafe { std::fs::File::from_raw_fd(fd) });
        let _ = std::fs::remove_dir_all(base);
    }
}
