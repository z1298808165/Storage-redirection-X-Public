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
/// 判据必须**同时**满足三条，缺一条就会误摘：
///
/// 1. `fs_type == "fuse"`；
/// 2. `source == "/dev/fuse"`；
/// 3. `root` 恰好是应用专属目录（[`module_mount_source::root_is_app_private_directory`]，
///    形如 `/0/Android/data/<包名>`，止于包名且没有更深的路径段）。
///
/// 第 3 条不能省。摘除目标里的 `Android/{data,media,obb}/<包名>` 与模块自己的 bind **同路径**，
/// 而 bind 会把底层的 `source` 与 `fs_type` 一并继承（真机上就是 `/dev/fuse` + `fuse`），
/// 只看前两条会把模块自己的挂载也摘掉——应用随后写入既不落沙箱也不落后端的空视图，
/// 表现为「写入 PASS 但可见路径与后端都查不到文件」。
/// [`crate::module_mount_source`] 的模块文档把这条记为不可混用的判据，这里正是它的使用点。
///
/// 模块自己的 scoped FUSE 会话的 `source` 带 `srx_fuse_redirect`/`srx_fuse_host` 前缀，
/// 但真机上经 fusermount 回退时该前缀不进 `source`，因此不能只依赖它。
/// 非系统 FUSE 的最上层（模块的 bind）视为已收敛，直接返回成功。
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
        let is_system_fuse = top.fs_type == "fuse"
            && top.source == "/dev/fuse"
            && crate::module_mount_source::root_is_app_private_directory(&top.root);
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

/// 枚举需要**观察**系统 FUSE 视图层状态的路径（供 [`log_view_stack_for_package`] 采样）。
///
/// 包含两类：
///
/// 1. **存储别名根**（`/storage/emulated/<user>` 等，见 [`paths::storage_alias_roots_for_user`]）；
/// 2. **自有包名目录**——`Android/data/<包名>`、`Android/media/<包名>`、`Android/obb/<包名>`。
///
/// 第 2 类**只用于观察，不用于摘除**。摘除目标与这里必须区分开，因为子路径上的系统 FUSE
/// 视图在正常平台上正是 MediaProvider 代写通道的依托，摘掉会让写入落空；而它又是失败现场最
/// 需要看清的地方（Android 13 的 `ENOENT` 就出在这类路径上）。见
/// [`clear_system_fuse_view_for_package`] 里的三轮对照结论。
///
/// 返回去重后的列表：别名根之间可能互相包含，自有目录也可能与别名重复。
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

/// 摘掉该用户所有存储别名上的系统 FUSE 视图层，并报告是否全部收敛。
///
/// **只摘别名根，不摘自有包名目录**——这是实测结论，不是保守选择。CI artifact
/// `test-flow-app-mountinfo.txt` 的三轮对照：
///
/// | 平台 | 只摘别名根 | 连子路径一起摘 |
/// |---|---|---|
/// | Android 13 | 子路径各有 2 层系统 FUSE → 失败（`ENOENT`） | 子路径 0 层 → 仍失败（`no_native`） |
/// | Android 14 | 子路径 data×6/media×2/obb×2 → **三个断言全过** | 子路径 0 层 → **三个断言全失败** |
///
/// 两点结论：
///
/// 1. **`Android/{data,media,obb}/<包名>` 上的系统 FUSE 视图不是障碍，而是 MediaProvider
///    代写通道的依托。** 摘掉它之后 MediaProvider 的路径解析落到
///    `rwVals no_native ... fallback=null`，写入既不落沙箱也不落后端（`file_write` 返回成功
///    但目录为空）。Android 14 因此从全过变成全败。
/// 2. **摘子路径对 Android 13 也无效**：该平台摘与不摘的失败形态不同（`ENOENT` vs
///    `no_native`）但都不通过，说明它的限制另有原因，反复摘除不是解法。
///
/// 因此这里回到「只摘别名根」：`Android 14/15/16/17` 的正常路径依赖它，而 Android 13 需要
/// 另行定位真正的阻塞点。诊断采样仍覆盖子路径（见 [`log_view_stack_for_package`]），
/// 以便继续观察那两类形态。
///
/// `package_name` 目前不参与摘除目标，保留它是为了让两条挂载路径的调用点与诊断函数保持同一
/// 形态，后续若要按包名区分平台策略无需再改签名。
pub fn clear_system_fuse_view_for_package(uid: i32, package_name: &str) -> bool {
    let _ = package_name;
    let user_id = user_id_from_uid(uid);
    let mut all_cleared = true;
    for alias in paths::storage_alias_roots_for_user(user_id) {
        if !clear_system_fuse_view_layers(&alias) {
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
/// 采样范围**宽于**摘除范围（摘除只动别名根，见 [`clear_system_fuse_view_for_package`]），
/// 这是有意为之：子路径上的层虽不摘，但必须能观察到——Android 13 的 `ENOENT` 与摘除后的
/// `no_native` 两种形态就是靠这里的采样区分出来的。诊断范围可以大于操作范围，
/// **反过来绝不可以**（摘了却没采，会把「摘干净」与「漏目标」混为一谈）。
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
