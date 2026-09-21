//! 系统 MediaProvider FUSE 视图层的识别与摘除（lib/bin 共用）。
//!
//! 模块把存储视图根（`/storage/emulated/<user>`）bind 到沙箱后，系统 MediaProvider 会在同一
//! 视图根上响应性地建立 app data isolation FUSE 视图，形如：
//!
//! ```text
//! 0:98 /0/Android/data/<包名>/sdcard /storage/emulated/0 fuse /dev/fuse ...
//! ```
//!
//! 该视图的 `root` 是 FUSE 内部的 app-private 路径，会被 MediaProvider 的
//! `is_app_accessible_path` 拒绝，于是应用读写自己的 `Android/data` 目录得到 ENOENT，
//! MediaStore 写入也会在早期失败。
//!
//! 正常情况下 native FuseFix 会 hook 放行该判定；但 x86_64 Android 13/14 模拟器镜像上安装
//! FuseFix 会让 MediaProvider 起 FUSE 会话时 SIGSEGV（见
//! `crate::hook::fuse_fix::should_skip_native_fuse_fix_for_platform`），只能显式摘掉这些
//! 系统 FUSE 层，让模块的 ext4 bind 回到最顶层。
//!
//! **为什么放在 lib 的独立模块**：挂载有两条路径——root daemon（bin 侧 `daemon_mount`）与
//! 应用进程内的 companion（lib 侧 `lifecycle::companion_mount`）。摘除逻辑最初只写在 daemon
//! 路径里，于是走 companion 的应用（普通应用正是这条）在同样的平台上继续失败，只能逐个场景
//! 打平台豁免。摘除与「只重建映射」必须对两条路径使用同一份实现，否则同一件事在两条路径上
//! 的结果不一致——这正是本项目反复踩的坑。
//!
//! # 调用顺序（两条路径都必须遵守）
//!
//! 1. `MountPlanner::apply_*` 完成挂载；
//! 2. [`should_clear_system_fuse_view_for_platform`] 放行时调用
//!    [`clear_system_fuse_view_for_uid`]；
//! 3. 若有 `path_mappings`，调用 `MountPlanner::reapply_path_mappings_only` 补回映射。
//!
//! 顺序不能变：`apply` 重新 bind 视图根会触发 MediaProvider 重建 FUSE，摘在 apply 之前等于
//! 白摘；而摘除的 `MNT_DETACH` 会级联 detach 掉挂在 FUSE 层之下的映射子路径 bind，所以摘完
//! 必须重建映射。重建只能走 `reapply_path_mappings_only`——它复用已建立的
//! `real_storage_anchor`，只 bind 映射子路径、不碰视图根，因此不会再触发重建；重新 bind
//! 视图根就会把上面这个循环打回去。

use crate::mount_ledger::topmost_live_mount;
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::{paths, user_id_from_uid};
use libc::{MNT_DETACH, umount2};
use std::ffi::CString;

/// 同一视图根上连续摘除的收敛预算。视图根正常只有一层系统 FUSE，出现异常堆叠时用它兜底，
/// 避免在无法收敛的挂载栈上无限循环。
const MAX_UNMOUNT_PASSES_PER_TARGET: usize = 32;

/// 给定 API 级别，判断该平台是否需要软件摘除系统 FUSE 视图。
///
/// 入参显式传入而非内部读属性，便于测试注入；生产调用取
/// [`should_clear_system_fuse_view_for_platform`]。判据与 native FuseFix 的平台跳过条件
/// 必须永远一致，因此 [`crate::hook::fuse_fix`] 也复用本函数，避免两处独立实现漂移。
pub fn needs_system_fuse_view_clear(api_level: i32) -> bool {
    cfg!(target_arch = "x86_64") && matches!(api_level, 33 | 34)
}

/// 当前平台是否需要显式摘除系统 FUSE 视图层。
///
/// 与 `crate::hook::fuse_fix`（native FuseFix 装不上的平台组合）共用
/// [`needs_system_fuse_view_clear`] 判据。真机 arm64 上 FuseFix 正常安装，本门禁为假，
/// 不改变既有语义。
pub fn should_clear_system_fuse_view_for_platform() -> bool {
    needs_system_fuse_view_clear(crate::platform::android_api_level())
}

/// 摘掉 `view_root` 上系统 MediaProvider 建立的 app data isolation FUSE 视图层。
///
/// 判据只看 `source == "/dev/fuse"` 且 `fs_type == "fuse"`：模块自己的 scoped FUSE 会话的
/// `source` 带 `srx_fuse_redirect`/`srx_fuse_host` 前缀，不会被误摘。非系统 FUSE 的最上层
/// （模块的 ext4 bind）视为已收敛，直接返回成功。
///
/// 返回 `true` 表示已无系统 FUSE 层残留（包括本来就没有）。
pub fn clear_system_fuse_view_layers(view_root: &str) -> bool {
    let mut passes = 0usize;
    loop {
        let Some(top) = topmost_live_mount(0, view_root) else {
            if passes > 0 {
                log::info!(
                    "cleared system fuse view layers target={} passes={}",
                    view_root,
                    passes
                );
            }
            return true;
        };
        let is_system_fuse = top.fs_type == "fuse" && top.source == "/dev/fuse";
        if !is_system_fuse {
            if passes > 0 {
                log::info!(
                    "cleared system fuse view layers target={} passes={}",
                    view_root,
                    passes
                );
            }
            return true;
        }
        if passes >= MAX_UNMOUNT_PASSES_PER_TARGET {
            log::warn!("clear system fuse view stack exceeded target={}", view_root);
            return false;
        }
        let Ok(c_target) = CString::new(view_root) else {
            return false;
        };
        // SAFETY: c_target 是以 NUL 结尾的合法路径且在本次调用期间保持存活；MNT_DETACH
        // 只影响当前命名空间的挂载视图，不触碰其它命名空间。
        if unsafe { umount2(c_target.as_ptr(), MNT_DETACH) } == 0 {
            passes += 1;
            log::info!(
                "cleared system fuse view layer target={} mount_id={} source={} root={} pass={}",
                view_root,
                top.mount_id,
                top.source,
                top.root,
                passes
            );
            continue;
        }
        let errno = last_errno();
        if errno == libc::EINVAL || errno == libc::ENOENT {
            return true;
        }
        log::warn!(
            "clear system fuse view failed target={} mount_id={} errno={} {}",
            view_root,
            top.mount_id,
            errno,
            errno_text(errno)
        );
        return false;
    }
}

/// 枚举需要清理系统 FUSE 视图层的全部目标路径。
///
/// **必须同时包含两类目标**（CI artifact `test-flow-app-mountinfo.txt` 实测证据）：
///
/// 1. **存储别名根**（`/storage/emulated/<user>` 等，见 [`paths::storage_alias_roots_for_user`]）；
/// 2. **自有包名目录**——`Android/data/<包名>`、`Android/media/<包名>`、`Android/obb/<包名>`。
///
/// 第 2 类是必须的：MediaProvider 的 app-data-isolation 视图**主要不建在别名根上，而是直接建在
/// 这些子路径上**，且每条路径会叠**两层** `source=/dev/fuse`。只摘别名根时视图根确实回到
/// ext4（`47196 ... ext4 /dev/block/dm-34`），但子路径上的 FUSE 层仍在
/// （`47201`/`47203 ... fuse /dev/fuse`），应用读写自有 `Android/data|media|obb` 依然被拒。
///
/// 返回去重后的目标列表：别名根之间可能互相包含，自有目录也可能与别名重复。
pub fn system_fuse_view_targets_for_package(uid: i32, package_name: &str) -> Vec<String> {
    let user_id = user_id_from_uid(uid);
    let mut targets = paths::storage_alias_roots_for_user(user_id);
    if !package_name.is_empty() {
        for alias in paths::storage_alias_roots_for_user(user_id) {
            for kind in ["data", "media", "obb"] {
                let dir = format!("{}/Android/{}/{}", alias, kind, package_name);
                if !targets.iter().any(|target| target == &dir) {
                    targets.push(dir);
                }
            }
        }
    }
    targets
}

/// 摘掉该用户所有存储别名与自有包名目录上的系统 FUSE 视图层，并报告是否全部收敛。
///
/// 目标枚举见 [`system_fuse_view_targets_for_package`]，那里解释了为什么自有包名目录必须包含。
pub fn clear_system_fuse_view_for_package(uid: i32, package_name: &str) -> bool {
    let mut all_cleared = true;
    for target in system_fuse_view_targets_for_package(uid, package_name) {
        if !clear_system_fuse_view_layers(&target) {
            all_cleared = false;
        }
    }
    all_cleared
}

/// 采样所有存储别名根与自有包名目录的最上层挂载，供诊断「应用视角到底能不能访问自有目录」。
///
/// 场景 34/35/36 的失败点都在 `Android/data/<包名>/...`，而 `fs_type` 能直接区分最上层是
/// 系统 MediaProvider 的 FUSE 视图（`fuse` + `/dev/fuse`）还是模块自己的 ext4 bind。摘除
/// 前后各采一次即可定论摘除是否真的换掉了最上层——不再需要从应用侧的 ENOENT 反推。
///
/// 采样点必须与 [`system_fuse_view_targets_for_package`] 完全一致：诊断与摘除若各用一套目标
/// 列表，就会出现「摘了 A 没采 B、采了 B 没摘 A」的假象，把「摘干净了」和「漏了目标」混为
/// 一谈。两者共用同一个枚举函数是这条不变量唯一的保证。
pub fn log_view_stack_for_package(uid: i32, package_name: &str, phase: &str) {
    for point in system_fuse_view_targets_for_package(uid, package_name) {
        match topmost_live_mount(0, &point) {
            Some(mount) => log::info!(
                "view stack phase={} target={} top_source={} top_fs={} top_root={} mount_id={}",
                phase,
                point,
                mount.source,
                mount.fs_type,
                mount.root,
                mount.mount_id
            ),
            None => log::info!("view stack phase={} target={} top=none", phase, point),
        }
    }
}
