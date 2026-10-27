// 一次性存量属主迁移修复。
//
// 背景：ci.49 之前的版本在 CE 未解锁窗口会把应用私有树误判为公共目录并整体
// chown 成 media_rw（用户设备 5857 条乱码路径 fix 实证）；MP FUSE 以 media_rw
// fsuid 创建私有树条目的历史漂移同样遗留 owner=1023 存量。2026-10-20 事后属主
// 修复链全面拆除后，这些存量再无任何自愈通道。属主 1023 的私有目录对应用 uid
// 而言既非 owner 也非组内成员，经原生 MP FUSE 访问持续 EACCES（用户反馈：微信
// 内部异步线程因此忙重试吃满 CPU）。
//
// 设计约束（与「拆除事后修复链」的决定共存）：
// - 一次性：完成标记写盘后不再运行；仅标记缺失才执行；标记写失败视为未完成，
//   下次 daemon 启动重扫（幂等，已修复条目 owner 非 1023，重扫零写放大）；
// - 最小干预：只改 owner 恰好是 1023 的条目，chown 回「包 appId + 用户段换算」
//   的目标 uid，gid 与 mode 一律保留；其它属主（含 0/1000/其它应用）不触碰；
// - 有界可续跑：单轮扫描/修复条目数有预算，超限不写标记，下次启动重新全量
//   续扫（不做断点记忆，避免新增状态文件复杂度）；
// - CE 门控：等待 media_ce_storage_ready 后才动手，避免重蹈乱码误判覆辙。

use crate::redirect_policy as policy;
use std::ffi::CString;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

/// 完成标记。位于模块目录 tmp/ 下，随 /data 持久化；文件存在即视为迁移已完成。
// v3：运行期系统代写 hook 曾把 owner 已正确但 gid=1078 的私有目录改成
// media_rw；本轮只恢复 Android/data 私有目录的 ext_data_rw 组，不触碰文件或
// 带 other 访问位的媒体子目录。
// v4：fuse_redirect 宿主会话的 ensure_parent_for_backend 对已存在父目录重写
// 元数据，把私有树既有目录 gid 恒改写为 media_rw（真机 2026-10-27 实证：微信
// MicroMsg、QQ Tencent/qzone 等活跃写入目录 1078→1023）。修复改写源后升版
// 重扫一次，把存量 gid=1023 目录恢复 ext_data_rw；判定条件与 v3 完全一致。
const MIGRATION_MARKER: &str =
    "/data/adb/modules/storage.redirect.x/tmp/owner-migration-1023-v4.done";

/// 漂移签名：私有树内 owner 恰好是 media_rw 的条目才修复。
const MEDIA_RW_UID: u32 = 1023;
/// AOSP `AID_EXT_DATA_RW`：外部存储应用私有 data 目录组。
const EXT_DATA_RW_GID: u32 = 1078;
/// 目标 uid 必须是普通应用 uid。
const ANDROID_APP_UID_START: i32 = 10000;
/// AOSP 每用户 uid 段宽度：目标 uid = appId + user_id * 100000。
const PER_USER_RANGE: i32 = 100_000;
/// CE 就绪轮询间隔与上限（5s × 180 = 15 分钟；超限放弃，下次启动重试）。
const CE_READY_POLL: Duration = Duration::from_secs(5);
const CE_READY_MAX_POLLS: u32 = 180;
/// CE 就绪后的静置等待：避开开机挂载风暴，把迁移 I/O 与启动关键路径错开。
const BOOT_SETTLE_DELAY: Duration = Duration::from_secs(30);
/// 单轮扫描条目预算。 Android/data 全量树在极端设备上可到十万级，预算只防
/// 「无限走下去」，超限交下次启动续扫。
const MAX_SCAN_ENTRIES_PER_RUN: usize = 262_144;
/// 单轮修复条目预算，单独设上限防止极端残留打满时间预算。
const MAX_REPAIRS_PER_RUN: usize = 65_536;
/// chown 失败的告警上限（root 身份下失败罕见，多为 SELinux 特殊节点）。
const MAX_FAILURE_LOGS: usize = 8;

#[derive(Default)]
struct MigrationStats {
    scanned: usize,
    repaired: usize,
    gid_repaired: usize,
    packages: usize,
    unknown_uid_packages: usize,
    failures: usize,
    mode_fixed: usize,
    budget_exhausted: bool,
}

/// 在 daemon 启动时派生一次性迁移线程。失败仅记录，不阻塞 daemon 启动。
pub fn spawn_once() {
    let spawn_result = thread::Builder::new()
        .name("srx-owner-migrate".to_string())
        .spawn(run);
    if let Err(error) = spawn_result {
        log::warn!("owner migration thread spawn failed error={}", error);
    }
}

fn run() {
    if Path::new(MIGRATION_MARKER).exists() {
        log::info!("owner migration skip reason=already_done");
        return;
    }
    if !wait_for_ce_storage_ready() {
        return;
    }
    // 静置期内可能已有其它实例（daemon 重启）完成迁移，动手前双检一次。
    if Path::new(MIGRATION_MARKER).exists() {
        log::info!("owner migration skip reason=already_done");
        return;
    }
    let stats = migrate_all_users();
    if stats.budget_exhausted {
        // 不写标记：下次 daemon 启动重新全量续扫。
        log::warn!(
            "owner migration budget exhausted scanned={} repaired={} packages={} — 下次启动续扫",
            stats.scanned,
            stats.repaired,
            stats.packages
        );
        return;
    }
    match std::fs::write(MIGRATION_MARKER, "v4\n") {
        Ok(()) => log::info!(
            "owner migration done scanned={} repaired={} gid_repaired={} mode_fixed={} packages={} unknown_uid_packages={}",
            stats.scanned,
            stats.repaired,
            stats.gid_repaired,
            stats.mode_fixed,
            stats.packages,
            stats.unknown_uid_packages
        ),
        Err(error) => log::warn!(
            "owner migration marker write failed error={} — 下次启动将重扫",
            error
        ),
    }
}

/// 轮询等待 /data/media 用户 CE 密钥就绪。超限放弃并记录，下次启动重试。
fn wait_for_ce_storage_ready() -> bool {
    for poll in 0..CE_READY_MAX_POLLS {
        if crate::daemon::media_ce_storage_ready() {
            if poll > 0 {
                log::info!("owner migration ce ready after polls={}", poll);
            }
            thread::sleep(BOOT_SETTLE_DELAY);
            return true;
        }
        thread::sleep(CE_READY_POLL);
    }
    log::warn!(
        "owner migration abort reason=ce_not_ready polls={}",
        CE_READY_MAX_POLLS
    );
    false
}

/// 遍历 /data/media 下所有用户的 Android/{data,media,obb} 私有类别树。
fn migrate_all_users() -> MigrationStats {
    let mut stats = MigrationStats::default();
    let Ok(users) = std::fs::read_dir("/data/media") else {
        log::warn!("owner migration abort reason=data_media_unreadable");
        return stats;
    };
    let mut user_ids: Vec<i32> = users
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .collect();
    user_ids.sort_unstable();
    for user_id in user_ids {
        for kind in ["data", "media", "obb"] {
            let category_root = format!("/data/media/{user_id}/Android/{kind}");
            if !Path::new(&category_root).is_dir() {
                continue;
            }
            migrate_category_root(&category_root, user_id, &mut stats);
            if stats.budget_exhausted {
                return stats;
            }
        }
    }
    stats
}

/// 逐包迁移一个类别根。无法解析 uid 的包目录跳过（应用已卸载的残留属正常现象）。
fn migrate_category_root(category_root: &str, user_id: i32, stats: &mut MigrationStats) {
    let Ok(entries) = std::fs::read_dir(category_root) else {
        return;
    };
    let mut package_names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| crate::platform::paths::is_valid_package_name(name))
        .collect();
    package_names.sort_unstable();
    for package_name in package_names {
        if stats.budget_exhausted {
            return;
        }
        let app_id = policy::get_uid_for_package(&package_name);
        if app_id < ANDROID_APP_UID_START {
            stats.unknown_uid_packages = stats.unknown_uid_packages.saturating_add(1);
            continue;
        }
        let target_uid = (app_id + user_id * PER_USER_RANGE) as u32;
        let package_root = format!("{category_root}/{package_name}");
        stats.packages = stats.packages.saturating_add(1);
        let fixed = migrate_tree(&package_root, target_uid, stats);
        if fixed > 0 {
            log::info!(
                "owner migration fix user={} pkg={} entries={}",
                user_id,
                package_name,
                fixed
            );
        }
    }
}

/// 迁移单棵包树：根目录与全部子孙条目，只修 owner=1023 的条目。返回修复数。
fn migrate_tree(package_root: &str, target_uid: u32, stats: &mut MigrationStats) -> usize {
    let mut fixed = 0usize;
    let mut stack = vec![package_root.to_string()];
    while let Some(dir) = stack.pop() {
        if visit_entry(&dir, target_uid, stats) {
            fixed = fixed.saturating_add(1);
        }
        if stats.budget_exhausted {
            return fixed;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if stats.budget_exhausted {
                return fixed;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let child = format!("{dir}/{name}");
            if file_type.is_dir() {
                stack.push(child);
                continue;
            }
            if visit_entry(&child, target_uid, stats) {
                fixed = fixed.saturating_add(1);
            }
        }
    }
    fixed
}

/// 检查单个条目并按需修复属主。预算耗尽时置位并跳过后续修复。
fn visit_entry(path: &str, target_uid: u32, stats: &mut MigrationStats) -> bool {
    stats.scanned = stats.scanned.saturating_add(1);
    if stats.scanned > MAX_SCAN_ENTRIES_PER_RUN
        || stats.repaired.saturating_add(stats.gid_repaired) > MAX_REPAIRS_PER_RUN
    {
        stats.budget_exhausted = true;
        return false;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    // 泄漏签名收敛：auto-mkdir 曾给私有树目录打 0o2773（others 可写可穿越），
    // 与原生 02770 相悖；属主已正确（应用 uid）的这类目录同样归一。
    if metadata.is_dir() && (metadata.mode() & 0o7777) == 0o2773 {
        normalize_leaked_dir_mode(path, stats);
    }
    let owner_is_migratable = metadata.uid() == MEDIA_RW_UID;
    let gid_is_migratable = should_restore_ext_data_gid(path, target_uid, &metadata);
    if !owner_is_migratable && !gid_is_migratable {
        return false;
    }
    let desired_uid = if owner_is_migratable {
        target_uid
    } else {
        metadata.uid()
    };
    let desired_gid = if gid_is_migratable {
        EXT_DATA_RW_GID
    } else {
        metadata.gid()
    };
    repair_owner_and_group(
        path,
        desired_uid,
        desired_gid,
        owner_is_migratable,
        gid_is_migratable,
        stats,
    )
}

/// 恢复被旧 hook 覆盖的 `ext_data_rw` 目录组。
///
/// AOSP 将 1078 定义为“外部存储应用私有 data 目录组”。设备上的 1023 仍是
/// 合法语义：文件以及带 other 访问位的共享/媒体子目录需要 media_rw 访问，不能
/// 全树改写。因此只处理 owner 已是应用 UID或历史漂移值 1023、当前 gid=1023、
/// Android/data 子树且目录没有 other 权限位的条目。
fn should_restore_ext_data_gid(path: &str, target_uid: u32, metadata: &Metadata) -> bool {
    let owner_is_known = metadata.uid() == target_uid || metadata.uid() == MEDIA_RW_UID;
    if !metadata.is_dir()
        || !owner_is_known
        || metadata.gid() != MEDIA_RW_UID
        || metadata.mode() & 0o007 != 0
    {
        return false;
    }
    let Some(android_index) = path.rfind("/Android/data/") else {
        return false;
    };
    let after = &path[android_index + "/Android/data/".len()..];
    let mut parts = after.split('/').filter(|part| !part.is_empty());
    let Some(package_name) = parts.next() else {
        return false;
    };
    crate::platform::paths::is_valid_package_name(package_name)
}

/// 把私有树泄漏的 0o2773 目录收敛为原生 02770。
fn normalize_leaked_dir_mode(path: &str, stats: &mut MigrationStats) {
    let Ok(c_path) = CString::new(path) else {
        return;
    };
    // SAFETY: c_path 在调用期间保持有效并以 NUL 结尾。
    let ret = unsafe { libc::chmod(c_path.as_ptr(), 0o2770) };
    if ret == 0 {
        stats.mode_fixed = stats.mode_fixed.saturating_add(1);
        return;
    }
    if stats.failures < MAX_FAILURE_LOGS {
        log::warn!(
            "owner migration chmod failed path={} errno={}",
            path,
            crate::platform::errno::last()
        );
    }
    stats.failures = stats.failures.saturating_add(1);
}

/// lchown 按迁移判定同时修正 owner/group；符号链接不跟随。
fn repair_owner_and_group(
    path: &str,
    target_uid: u32,
    target_gid: u32,
    owner_changed: bool,
    gid_changed: bool,
    stats: &mut MigrationStats,
) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 在调用期间保持有效并以 NUL 结尾。
    // SAFETY: c_path 由 CString 保证以 NUL 结尾，target uid/gid 已由上层迁移判定限制。
    let ret = unsafe { libc::lchown(c_path.as_ptr(), target_uid, target_gid) };
    if ret == 0 {
        if owner_changed {
            stats.repaired = stats.repaired.saturating_add(1);
        }
        if gid_changed {
            stats.gid_repaired = stats.gid_repaired.saturating_add(1);
        }
        return true;
    }
    if stats.failures < MAX_FAILURE_LOGS {
        log::warn!(
            "owner migration chown failed path={} uid={} gid={} errno={}",
            path,
            target_uid,
            target_gid,
            crate::platform::errno::last()
        );
    }
    stats.failures = stats.failures.saturating_add(1);
    false
}
