// 挂载目标的路径归类与元数据修复。
//
// 这里只回答「这个路径属于哪一类」以及「挂载后要不要补权限/属主」：
// Android 私有目录判定、共享公共目录判定、只读与 allowed-real 的元数据修复。
// 挂载动作本身留在 planner，改归类口径不会碰到挂载顺序。

use super::apply_targets::{mountinfo_has_target, read_mountinfo};
use super::planner::{ALLOWED_REAL_DIR_MODE, MEDIA_RW_GID, MEDIA_RW_UID, REAL_PUBLIC_DIR_MODE};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::{mountinfo, paths};
use libc::{MS_BIND, MS_RDONLY, MS_REC, MS_REMOUNT, chmod, chown, mount, stat as c_stat};
use std::ffi::CString;

pub(super) fn shared_storage_root(path: &str) -> Option<String> {
    paths::storage_user_root(path).or_else(|| paths::data_media_user_root(path))
}

pub(super) fn is_android_private_storage_path(path: &str, storage_root: &str) -> bool {
    let Some(relative) = paths::relative_child_path(path, storage_root) else {
        return false;
    };
    relative == "Android/data"
        || relative == "Android/media"
        || relative == "Android/obb"
        || relative.starts_with("Android/data/")
        || relative.starts_with("Android/media/")
        || relative.starts_with("Android/obb/")
}

pub(super) fn is_android_private_backend_path(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    parts.next() == Some("Android") && matches!(parts.next(), Some("data" | "media" | "obb"))
}

pub(super) fn is_android_private_package_root(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    relative.split('/').filter(|part| !part.is_empty()).count() == 3
        && is_android_private_backend_path(path, user_id)
}

pub(super) fn should_apply_app_writable_metadata(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        if path.starts_with("/data/media/") || path == "/data/media" {
            return false;
        }
        return true;
    };

    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return true;
    }

    match parts.next() {
        Some("data" | "media" | "obb") => parts.next().is_some(),
        Some(_) => true,
        None => false,
    }
}

pub(super) fn is_data_media_shared_public_directory(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    !is_android_app_private_relative_path(relative)
}

pub(super) fn is_shared_public_storage_directory(path: &str, user_id: i32) -> bool {
    if is_data_media_shared_public_directory(path, user_id) {
        return true;
    }
    paths::storage_to_data_media_for_user(path, user_id)
        .is_some_and(|backend| is_data_media_shared_public_directory(&backend, user_id))
}

pub(super) fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    matches!(parts.next(), Some("data" | "media" | "obb"))
}

pub(super) fn fix_real_public_directory_metadata(path: &str) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    // SAFETY: st 为本作用域独占的 MaybeUninit，c_stat 成功时才会初始化它。
    let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        log_metadata_fix_failure("real public stat", path);
        return;
    }
    // SAFETY: 上面 c_stat 返回 0，st 已被完整初始化。
    let st = unsafe { st.assume_init() };

    if st.st_uid != MEDIA_RW_UID || st.st_gid != MEDIA_RW_GID {
        // SAFETY: c_path 指向本作用域内以 NUL 结尾的合法路径，只改属主不改内容。
        let ret = unsafe { chown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
        if ret != 0 {
            log_metadata_fix_failure("real public chown", path);
        }
    }

    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    if mode != REAL_PUBLIC_DIR_MODE {
        // SAFETY: c_path 指向本作用域内以 NUL 结尾的合法路径，chmod 只改权限位。
        let ret = unsafe { chmod(c_path.as_ptr(), REAL_PUBLIC_DIR_MODE) };
        if ret != 0 {
            log_metadata_fix_failure("real public chmod", path);
        }
    }
}

pub(super) fn fix_allowed_real_directory_metadata(path: &str) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    // SAFETY: c_path 在调用期间是有效且以 NUL 结尾的 CString。
    let ret = unsafe { chown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
    if ret != 0 {
        log_metadata_fix_failure("allowed real chown", path);
    }

    // SAFETY: c_path 指向本作用域内以 NUL 结尾的合法路径，chmod 只改权限位。
    let ret = unsafe { chmod(c_path.as_ptr(), ALLOWED_REAL_DIR_MODE) };
    if ret != 0 {
        log_metadata_fix_failure("allowed real chmod", path);
    }
}

pub(super) fn fix_read_only_public_metadata(path: &str) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    // SAFETY: st 为本作用域独占的 MaybeUninit，c_stat 成功时才会初始化它。
    let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
    if ret != 0 {
        log_metadata_fix_failure("readonly public stat", path);
        return;
    }
    // SAFETY: 上面 c_stat 返回 0，st 已被完整初始化。
    let st = unsafe { st.assume_init() };
    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    let required = if (st.st_mode & libc::S_IFMT) == libc::S_IFDIR {
        0o0555
    } else {
        0o0444
    };
    let fixed_mode = mode | required;
    if fixed_mode == mode {
        return;
    }

    // SAFETY: c_path 指向本作用域内以 NUL 结尾的合法路径，chmod 只改权限位。
    let ret = unsafe { chmod(c_path.as_ptr(), fixed_mode) };
    if ret != 0 {
        let error_no = last_errno();
        log::warn!(
            "mount dir: readonly public chmod failed errno={} {} path={} mode={:o}",
            error_no,
            errno_text(error_no),
            path,
            fixed_mode
        );
    }
}

pub(super) fn should_apply_allowed_real_writable_metadata(
    path: &str,
    original: &str,
    user_id: i32,
) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    if relative.is_empty() || is_android_app_private_relative_path(relative) {
        return false;
    }
    path == original || relative.split('/').filter(|part| !part.is_empty()).count() >= 2
}

pub(super) fn allowed_real_metadata_path_is_excluded(
    path: &str,
    excluded_real_paths: &[String],
) -> bool {
    if excluded_real_paths.is_empty() {
        return false;
    }
    let storage_path = paths::data_media_to_storage_path(path);
    excluded_real_paths
        .iter()
        .any(|excluded| paths::matches(excluded, &storage_path, true))
}

pub(super) fn parent_preserving_backend_alias(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return trimmed.to_string();
    }
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        String::new()
    }
}

pub(super) fn storage_emulated_repair_mount(
    target: &str,
    user_id: i32,
) -> Option<(String, String)> {
    let normalized = paths::normalize(target);
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(&normalized, &storage_root)
        || paths::is_child(&normalized, &storage_root)
    {
        Some(("/data/media".to_string(), "/storage/emulated".to_string()))
    } else {
        None
    }
}

pub(super) fn remount_bind_read_only_inner(target: &str, is_recursive: bool) -> bool {
    let Ok(c_target) = CString::new(target) else {
        return false;
    };
    let mut flags = MS_BIND | MS_REMOUNT | MS_RDONLY;
    if is_recursive {
        flags |= MS_REC;
    }
    // SAFETY: remount 目标是本会话刚创建的挂载点；source/fstype/data 传 null，
    // 内核只读取 target 路径并沿用原挂载参数，不写任何调用方缓冲。
    let ret = unsafe {
        mount(
            std::ptr::null(),
            c_target.as_ptr(),
            std::ptr::null(),
            flags as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret == 0 {
        return true;
    }
    let error_no = last_errno();
    log::warn!(
        "readonly remount failed dst={} recursive={} errno={} {}",
        target,
        is_recursive,
        error_no,
        errno_text(error_no)
    );
    false
}

pub(super) fn remount_bind_read_write_inner(target: &str, is_recursive: bool) -> bool {
    let Ok(c_target) = CString::new(target) else {
        return false;
    };
    let mut flags = MS_BIND | MS_REMOUNT;
    if is_recursive {
        flags |= MS_REC;
    }
    // SAFETY: remount 目标是本会话刚创建的挂载点；source/fstype/data 传 null，
    // 内核只读取 target 路径并沿用原挂载参数，不写任何调用方缓冲。
    let ret = unsafe {
        mount(
            std::ptr::null(),
            c_target.as_ptr(),
            std::ptr::null(),
            flags as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret == 0 {
        return true;
    }
    let error_no = last_errno();
    log::warn!(
        "readwrite remount failed dst={} recursive={} errno={} {}",
        target,
        is_recursive,
        error_no,
        errno_text(error_no)
    );
    false
}

/// 判断路径在当前挂载命名空间中是否已经是挂载点。
///
/// `mount(MS_BIND | MS_REMOUNT)` 对不是挂载点的路径必然返回 EINVAL，因此在重挂载
/// 之前先用挂载表判断一次，避免用注定失败的系统调用去试探挂载状态。
///
/// 这里刻意沿用 `mountinfo_has_target` 的别名折叠比较：`/storage/self/primary` 一类
/// 别名与 `/storage/emulated/<user>` 是同一个目录对象，按字面挂载目标比较会把已经是
/// 挂载点的别名误判成普通目录，从而多挂一层 bind 覆盖原有可写挂载。
pub(super) fn is_mount_point(path: &str) -> bool {
    read_mountinfo()
        .map(|content| mountinfo_has_target(&content, path))
        .unwrap_or(false)
}

/// 判断挂载点 `target` 当前最上层记录的 `fs_type` 是否为 FUSE。
///
/// Android 的媒体 FUSE 实现 passthrough，`/storage/emulated/0`（FUSE 视图）与其下
/// `/data/media/0/...`（ext4 视图）的同一目录会返回相同的 `st_ino`。`bind_mount_inner`
/// 的同 inode 短路据此把「视图根已是这个沙箱的 FUSE 层」当成「已绑定完成」而跳过重新
/// bind，于是被 MediaProvider 拒绝访问的 FUSE 层得以保留——Android 13 场景 29 热重载后
/// 视图根列举为空、所有路径 ENOENT。这里把 FUSE 层识别出来，让调用方在短路前先跳过，
/// 改走正常的 ext4 绑定在 FUSE 层之上叠一层可访问的 ext4 视图。
pub(super) fn topmost_mount_is_fuse(target: &str) -> bool {
    let Some(content) = read_mountinfo() else {
        return false;
    };
    let normalized_target = paths::normalize(target);
    let mut is_fuse = false;
    for line in content.lines() {
        let Some(entry) = mountinfo::parse_entry(line) else {
            continue;
        };
        if paths::eq_ignore_case(
            &paths::normalize(&mountinfo::unescape_field(entry.target)),
            &normalized_target,
        ) {
            is_fuse = entry.fs_type.starts_with("fuse");
        }
    }
    is_fuse
}

pub(super) fn paths_have_same_inode(left: &str, right: &str) -> bool {
    match (stat_inode(left), stat_inode(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// `stat` 一次的 `(st_dev, st_ino)`，失败返回 `None`。
pub(super) fn stat_inode(path: &str) -> Option<(u64, u64)> {
    let c_path = CString::new(path).ok()?;
    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    // SAFETY: c_path 是以 NUL 结尾的合法路径且在调用期间保持存活；st 是正确对齐的
    // MaybeUninit，stat 成功时已由内核写满，下面仅在返回值为 0 时才 assume_init。
    let is_ok = unsafe { libc::stat(c_path.as_ptr(), st.as_mut_ptr()) } == 0;
    if !is_ok {
        return None;
    }
    // SAFETY: 上一步已确认 stat 返回 0，内核写满了整个 st。
    // SAFETY: 上面 c_stat 返回 0，st 已被完整初始化。
    let st = unsafe { st.assume_init() };
    Some((st.st_dev, st.st_ino))
}

/// 路径当前是否为目录：`stat` 成功且带 `S_IFDIR`；`stat` 失败返回 `None`。
///
/// 与存在性判断分开是必要的：`ENOTDIR`（路径存在但不是目录）与 `ENOENT`（路径不存在）
/// 是两种完全不同的失效形态，混在一个布尔里就无法区分「挂载没落地」与「挂载落地但
/// 目标被换成非目录」。
pub(super) fn stat_is_directory(path: &str) -> Option<bool> {
    let c_path = CString::new(path).ok()?;
    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    // SAFETY: c_path 是以 NUL 结尾的合法路径且在调用期间保持存活；st 是正确对齐的
    // MaybeUninit，stat 成功时已由内核写满，下面仅在返回值为 0 时才 assume_init。
    let is_ok = unsafe { libc::stat(c_path.as_ptr(), st.as_mut_ptr()) } == 0;
    if !is_ok {
        return None;
    }
    // SAFETY: 上一步已确认 stat 返回 0，内核写满了整个 st。
    // SAFETY: 上面 c_stat 返回 0，st 已被完整初始化。
    let st = unsafe { st.assume_init() };
    Some(st.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

/// 一次 FUSE 承载绑定的落地采样：`stat` 到的目标目录性，以及绑定前后的目标 inode。
pub(super) struct BindLandingSample {
    pub(super) target_is_directory: Option<bool>,
    pub(super) target_inode_after: Option<(u64, u64)>,
    pub(super) target_inode_before: Option<(u64, u64)>,
}

/// FUSE 承载绑定落地校验的结论。
pub(super) enum BindLandingVerdict {
    /// 目标确是目录，绑定按成功处理。
    Accepted,
    /// 无法判定落地效果（`stat` 失败，或目标 inode 前后未变化），按成功放行而不是误杀。
    Inconclusive,
    /// 目标解析成一个**非目录** inode：应用侧访问该路径必然得到 `ENOTDIR`，绑定必须判失败。
    NotDirectory,
}

/// 由落地采样得出绑定校验结论。
///
/// 只把「目标不再是目录」这一**确定无歧义**的失效形态判为失败：`mount(MS_BIND)` 返回 0
/// 只代表内核接受了绑定，而把文件当成目录挂在目标上会让应用访问直接得到 `ENOTDIR`，属于
/// 本模块制造的错误视图，绝不能写进状态文件与挂载身份账本。
///
/// 误报防护：绑定本身就是同一路径换 dentry，若绑定后的 `(st_dev, st_ino)` 与绑定前完全
/// 相同，说明这次 `stat` 读到的仍是绑定前那层（或读数被缓存挡住），此时**无法判定**，
/// 必须归入 `Inconclusive` 放行——不能因为读不到变化就把正常挂载误杀。
pub(super) fn fuse_backed_bind_landing_verdict(sample: BindLandingSample) -> BindLandingVerdict {
    match sample.target_is_directory {
        None | Some(true) => BindLandingVerdict::Accepted,
        Some(false) => {
            let unchanged = match (sample.target_inode_before, sample.target_inode_after) {
                (Some(before), Some(after)) => before == after,
                // 缺少绑定前读数时不做「未变化」比较，只按非目录这一事实判定。
                _ => false,
            };
            if unchanged {
                BindLandingVerdict::Inconclusive
            } else {
                BindLandingVerdict::NotDirectory
            }
        }
    }
}

/// 输出目录元数据修复失败的告警。
///
/// 多个元数据修复分支的告警格式完全相同，只有操作名不同，这里统一输出避免重复；
/// 日志文本与字段顺序保持不变，便于日志解析继续按原有格式匹配。
fn log_metadata_fix_failure(operation: &str, path: &str) {
    let error_no = last_errno();
    log::warn!(
        "mount dir: {} failed errno={} {} path={}",
        operation,
        error_no,
        errno_text(error_no),
        path
    );
}
