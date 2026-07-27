//! FUSE 重定向后端的元数据与底层系统调用辅助函数。
//!
//! 这里集中放置属主、权限、时间戳修正以及 chmod/chown/truncate/utimens/renameat2
//! 等直接调用 libc 的封装，避免 `mod.rs` 同时承载协议实现和系统调用细节。

use super::{MEDIA_RW_GID, MEDIA_RW_UID, SHARED_PUBLIC_DIR_MODE};
use fuser::{Errno, TimeOrNow};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(super) fn cstring_path(path: &Path) -> Result<CString, Errno> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)
}

pub(super) fn fix_path_metadata(
    path: &Path,
    owner_uid: i32,
    mode: u32,
    is_shared_public_backend: bool,
    is_dir: bool,
) {
    let mode = adjust_metadata_mode(mode, is_shared_public_backend, is_dir);
    let effective_uid = if is_shared_public_backend {
        MEDIA_RW_UID
    } else {
        owner_uid as u32
    };
    if let Ok(c_path) = cstring_path(path) {
        // SAFETY: c_path 以 NUL 结尾，并在 chown 调用期间保持有效。
        let _ = unsafe { libc::chown(c_path.as_ptr(), effective_uid, MEDIA_RW_GID) };
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

pub(super) fn fix_existing_path_metadata(
    path: &Path,
    owner_uid: i32,
    is_shared_public_backend: bool,
) {
    if !is_shared_public_backend {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o7777;
    fix_path_metadata(
        path,
        owner_uid,
        mode,
        is_shared_public_backend,
        metadata.is_dir(),
    );
}

pub(super) fn adjust_metadata_mode(mode: u32, is_shared_public_backend: bool, is_dir: bool) -> u32 {
    let mode = mode & 0o7777;
    if !is_shared_public_backend {
        return mode;
    }
    if is_dir {
        return SHARED_PUBLIC_DIR_MODE;
    }
    let owner_bits_for_group = (mode & 0o700) >> 3;
    (mode | owner_bits_for_group) & !0o007
}

pub(super) fn chmod_path(path: &Path, mode: u32) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    // SAFETY: c_path 以 NUL 结尾，并在 chmod 调用期间保持有效。
    if unsafe { libc::chmod(c_path.as_ptr(), mode as libc::mode_t) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

pub(super) fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    let uid = if uid == u32::MAX { !0 } else { uid };
    let gid = if gid == u32::MAX { !0 } else { gid };
    // SAFETY: c_path 以 NUL 结尾，并在 chown 调用期间保持有效。
    if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

pub(super) fn truncate_path(path: &Path, size: u64) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    // SAFETY: c_path 以 NUL 结尾，并在 truncate 调用期间保持有效。
    if unsafe { libc::truncate(c_path.as_ptr(), size as libc::off_t) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

pub(super) fn utimens_path(
    path: &Path,
    atime: Option<TimeOrNow>,
    mtime: Option<TimeOrNow>,
) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    let times = [
        time_or_now_to_timespec(atime),
        time_or_now_to_timespec(mtime),
    ];
    // SAFETY: c_path 以 NUL 结尾，times 是长度为 2 的数组，均在调用期间保持有效。
    if unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

fn time_or_now_to_timespec(value: Option<TimeOrNow>) -> libc::timespec {
    match value {
        Some(TimeOrNow::SpecificTime(time)) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => libc::timespec {
                tv_sec: duration.as_secs() as libc::time_t,
                tv_nsec: duration.subsec_nanos() as libc::c_long,
            },
            Err(_) => libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
        },
        Some(TimeOrNow::Now) => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW as libc::c_long,
        },
        None => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT as libc::c_long,
        },
    }
}

pub(super) fn rename_noreplace(old_path: &Path, new_path: &Path) -> Result<(), Errno> {
    let old_path = cstring_path(old_path)?;
    let new_path = cstring_path(new_path)?;
    // SAFETY: 两个指针均指向有效且以 NUL 结尾的 C 字符串，并在 syscall 调用期间保持有效。
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            old_path.as_ptr(),
            libc::AT_FDCWD,
            new_path.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

pub(super) fn errno_from_io(error: std::io::Error) -> Errno {
    errno_from_code(error.raw_os_error().unwrap_or(libc::EIO))
}

pub(super) fn errno_from_code(code: i32) -> Errno {
    Errno::from_i32(code)
}

pub(super) use crate::platform::errno::last as last_errno;
