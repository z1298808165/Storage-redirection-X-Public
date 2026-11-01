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

/// 应用私有树（Android/data|media|obb）新建条目使用的扩展数据组。
const EXT_DATA_RW_GID: u32 = 1078;

pub(super) fn cstring_path(path: &Path) -> Result<CString, Errno> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CreateMetadata {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub rewrite_owner: bool,
    pub rewrite_mode: bool,
}

pub(super) fn create_metadata(
    request_uid: u32,
    request_gid: u32,
    owner_uid: u32,
    mode: u32,
    is_shared_public_backend: bool,
    is_app_private_backend: bool,
    is_dir: bool,
) -> CreateMetadata {
    if !crate::metadata_repair::enabled() {
        return CreateMetadata {
            uid: request_uid,
            gid: request_gid,
            mode: mode & 0o7777,
            rewrite_owner: false,
            rewrite_mode: false,
        };
    }
    if is_shared_public_backend {
        return CreateMetadata {
            uid: MEDIA_RW_UID,
            gid: MEDIA_RW_GID,
            // 公共后端的属主被改写成 media_rw，mode 必须同步派生（owner 位镜像到
            // group、清 others）：保留调用方原始 0600 会让应用连自己刚写的文件都读
            // 不到（真机 2026-11-01 duchamp：场景 5 写入后 1023:1023 0600，场景 6
            // 同路径 stat 得 EACCES、createNewFile 报 EEXIST）。目录固定 02770。
            mode: adjust_metadata_mode(mode, true, false, is_dir),
            rewrite_owner: true,
            rewrite_mode: true,
        };
    }
    if is_app_private_backend {
        return CreateMetadata {
            uid: owner_uid,
            gid: EXT_DATA_RW_GID,
            mode: mode & 0o7777,
            rewrite_owner: true,
            rewrite_mode: false,
        };
    }
    CreateMetadata {
        uid: request_uid,
        gid: request_gid,
        mode: mode & 0o7777,
        rewrite_owner: false,
        rewrite_mode: false,
    }
}

pub(super) fn apply_create_metadata(path: &Path, metadata: CreateMetadata) -> Result<(), Errno> {
    let mut failed = None;
    if metadata.rewrite_owner {
        let c_path = cstring_path(path)?;
        // SAFETY: c_path 是 NUL 结尾的合法路径；uid/gid 来自当前 FUSE 策略
        // （共享媒体常量或 app-private 的属主），不来自不可信输入。
        if unsafe { libc::chown(c_path.as_ptr(), metadata.uid, metadata.gid) } != 0 {
            failed = Some(errno_from_code(last_errno()));
        }
    }
    if metadata.rewrite_mode
        && let Err(error) =
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(metadata.mode))
    {
        failed.get_or_insert_with(|| errno_from_io(error));
    }
    failed.map_or(Ok(()), Err)
}

pub(super) fn fix_existing_path_metadata(
    path: &Path,
    owner_uid: i32,
    is_shared_public_backend: bool,
) {
    if !crate::metadata_repair::enabled() || !is_shared_public_backend {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o7777;
    let metadata = create_metadata(
        MEDIA_RW_UID,
        MEDIA_RW_GID,
        owner_uid as u32,
        mode,
        is_shared_public_backend,
        false,
        metadata.is_dir(),
    );
    let _ = apply_create_metadata(path, metadata);
}

pub(super) fn adjust_metadata_mode(
    mode: u32,
    is_shared_public_backend: bool,
    is_app_private_backend: bool,
    is_dir: bool,
) -> u32 {
    let mode = mode & 0o7777;
    if is_shared_public_backend && is_dir {
        return SHARED_PUBLIC_DIR_MODE;
    }
    if is_app_private_backend {
        // Android/data|media|obb 的 app-private 条目沿用原生 mode/umask：既不做
        // owner→group 位镜像，也不强加 media_rw 那套 0770/0660 语义。私有树的属主
        // 归位由 owner_migration 与运行时代写 hook 负责，FUSE 新建入口只负责
        // 「新建时用对的身份与组」。
        return mode;
    }
    // 公共后端的文件统一做 owner→group 位镜像并清 others：新建时属主被改写为
    // media_rw(1023)，若保留调用方原始 mode（应用常见 0600），应用自己就再也读不到
    // 自己刚写的文件——真机 2026-11-01 duchamp 实证：场景 5 放行路径写入后文件为
    // `1023:1023 0600`，紧接着场景 6 写同名时 `stat` 得 EACCES、`createNewFile`
    // 报 EEXIST（内核认为条目存在但打不开）。原生 MP FUSE 同样不透传请求 mode，
    // 而是按 media_rw 可管理的语义派生（同一 app 显式 0600 创建录制文件，原生结果
    // 是 0660/目录 02770），这里与之对齐。
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
