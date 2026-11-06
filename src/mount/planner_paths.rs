// 挂载目标的路径归类与元数据修复。
//
// 这里只回答「这个路径属于哪一类」以及「挂载后要不要补权限/属主」：
// Android 私有目录判定、共享公共目录判定、只读与 allowed-real 的元数据修复。
// 挂载动作本身留在 planner，改归类口径不会碰到挂载顺序。

use super::apply_targets::{mountinfo_has_target, read_mountinfo};
use super::planner::{MEDIA_RW_GID, MEDIA_RW_UID};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::{mountinfo, paths};
use libc::{MS_BIND, MS_RDONLY, MS_REC, MS_REMOUNT, chmod, chown, mount, stat as c_stat};
use std::ffi::CString;

/// 共享存储里的真实目录（公共目录与映射目标）被**后端**访问所需的最小能力位：
/// 属主与属组可读写进入。others 位刻意不要求：应用看到的是 FUSE 合成属性，
/// 真实目录只服务于以 media_rw 身份读写的后端。
///
/// 不写成"目标常量"：系统既有目录的权限由 MediaProvider 决定（`DCIM` 等 `02773`、
/// `Android`/`MIUI/Mirror` 等 `02771`/`02770`），这些形态都覆盖上面这组位，一律不动。
/// 归一成常量会改写既有目录，在用户磁盘上留下模块痕迹。
/// 从系统原生口径（`platform::fs::REAL_PUBLIC_DIR_MODE`，全仓唯一定义）取交集派生，
/// 保证"够不够用"的门槛与原生同源，不会与挂载侧各自漂移。
const REQUIRED_PUBLIC_DIR_MODE: libc::mode_t = crate::platform::fs::REAL_PUBLIC_DIR_MODE & 0o770;
/// 真的缺能力位时追加的位（含 setgid，保证组继承；不追加 others 任何位）。
const PUBLIC_DIR_REPAIR_MODE: libc::mode_t = crate::platform::fs::REAL_PUBLIC_DIR_MODE & 0o2770;

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
        let ret = unsafe { chown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
        if ret != 0 {
            log_metadata_fix_failure("real public chown", path);
        }
    }

    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    repair_missing_mode_bits(
        &c_path,
        path,
        mode,
        REQUIRED_PUBLIC_DIR_MODE,
        PUBLIC_DIR_REPAIR_MODE,
        "real public chmod",
    );
}

pub(super) fn fix_allowed_real_directory_metadata(path: &str) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };

    // 先 stat：既有目录绝大多数属主与能力位都已就位，这类目录一个元数据系统调用都不发。
    // 过去这里无条件 chmod 成 02773，等于把系统按自己语义创建的既有目录（如 02771）
    // 改写成模块常量，在用户磁盘上留下可观测差异。
    let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
    // SAFETY: st 为本作用域独占的 MaybeUninit，c_stat 成功时才会初始化它。
    if unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) } != 0 {
        log_metadata_fix_failure("allowed real stat", path);
        return;
    }
    // SAFETY: 上一步返回 0，st 已被完整初始化。
    let st = unsafe { st.assume_init() };

    if st.st_uid != MEDIA_RW_UID || st.st_gid != MEDIA_RW_GID {
        let ret = unsafe { chown(c_path.as_ptr(), MEDIA_RW_UID, MEDIA_RW_GID) };
        if ret != 0 {
            log_metadata_fix_failure("allowed real chown", path);
        }
    }

    let mode = (st.st_mode as libc::mode_t) & 0o7777;
    repair_missing_mode_bits(
        &c_path,
        path,
        mode,
        REQUIRED_PUBLIC_DIR_MODE,
        PUBLIC_DIR_REPAIR_MODE,
        "allowed real chmod",
    );
}

/// 只在能力位不足时补位，绝不回收既有位。
///
/// 既有共享目录由系统 MediaProvider 按它自己的语义创建（`DCIM`/`Pictures` 等原型是
/// `02773`，`Android` 是 `02771`），两者都满足访问所需的位。把它们统一归一成模块常量
/// 会在 `/data/media/<user>` 上留下与系统不一致的权限——那是应用侧可观测的模块痕迹，
/// 也会改写用户数据目录的既有语义。因此这里只做"够不够用"的判定：
/// 缺位才追加，位齐全（无论是否等于常量）一律不动。
///
/// 返回值表示是否真的发起了 chmod，供调用方保持原有日志口径。
pub(super) fn repair_missing_mode_bits(
    c_path: &CString,
    path: &str,
    mode: libc::mode_t,
    required: libc::mode_t,
    repair: libc::mode_t,
    label: &str,
) -> bool {
    if mode & required == required {
        return false;
    }
    let fixed_mode = mode | repair;
    // SAFETY: c_path 在调用期间保持有效且以 NUL 结尾，chmod 只改权限位。
    let ret = unsafe { chmod(c_path.as_ptr(), fixed_mode) };
    if ret != 0 {
        log_metadata_fix_failure(label, path);
        return false;
    }
    log::info!(
        "mount dir: {} repaired path={} mode={:o} fixed={:o}",
        label,
        path,
        mode,
        fixed_mode
    );
    true
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
