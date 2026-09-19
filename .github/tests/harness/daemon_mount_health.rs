// 守护进程挂载健康判据与无账本拒绝 harness 模板（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 与 mount_identity_syntax.rs 同思路：抽出真实 fn 注入模板，编译并执行源码里的真实实现，
// 而不是一份抄写副本（抄写副本无法证明原函数有没有被改坏）。
//
// 桩只暴露被抽取函数用到的签名/全局状态，绝不复制"匹配实现"：
// - `__INJECT_MOUNTINFO__` / `__INJECT_PATHS__` 是**真实**抽取的函数；
// - `platform::paths::normalize` 是历史桩（模拟"别名折叠到主入口"语义），健康判据路径已不再调用它；
//   仅反向验证（把 /mnt 视图重新折叠进主入口）会引用它，不复制任何挂载点匹配逻辑；
// - `log` 宏桩丢弃告警。
//
// 被测修复点（来自主代理对 src/daemon_mount.rs 的修改，场景二十九）：
//   1. mount_targets_present_with / canonical_health_target：健康判据只把明确符号链接入口
//      （/sdcard、/storage/self/primary、/data/data）折叠到主入口视图；/mnt/... 是独立挂载视图
//      （并非符号链接）、后端 /data/media 是独立真实挂载，二者均保持独立分组且大小写敏感，
//      避免后端或 /mnt 视图替主入口"顶包"掩盖主入口缺失；
//   2. reload_refuse_when_unverified：Reload 且清理未验证且无可信账本（load 为 None）时，
//      保守拒绝本次注入，避免无限叠加死挂载层（保留既有有账本预算行为）。
//
// 反向验证（Python）：
//   - 把 canonical_health_target 里 /mnt 视图的"独立分组"分支改回 `paths::normalize` 折叠进主入口
//     ——此时"独立视图缺失却被主入口顶替"等用例应从通过变失败，证明 harness 真在验证该修复；
//   - 把 reload_refuse_when_unverified 的 `!has_ledger` 改成 `false`——此时"无账本应拒绝"用例应
//     从通过变失败，证明 harness 真在验证该修复。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

use std::collections::HashSet;
use std::sync::Mutex;

// ---- 真实 mountinfo 解析（parse_entry / unescape_field 被抽取注入） ----
mod platform {
    pub mod mountinfo {
        // 数据类型的副本（非匹配实现）：字段与 src/platform/mountinfo.rs 一致。
        pub struct MountInfoEntry<'a> {
            pub mount_id: u64,
            pub root: &'a str,
            pub target: &'a str,
            pub fs_type: &'a str,
            pub source: &'a str,
        }

        // __INJECT_MOUNTINFO__
    }

    pub mod paths {
        // __INJECT_PATHS__

        // 最小桩：模拟"别名折叠到主入口"的语义，仅供反向验证引用（把 /mnt 视图重新折叠进主入口）。
        // 修复版 canonical_health_target 不再调用本桩：/sdcard、/storage/self/primary、/data/data
        // 的折叠在真实函数内显式完成；/mnt/... 与 /data/media 保持独立分组。
        pub fn normalize(path: &str) -> String {
            let normalized = normalize_syntax(path);
            if normalized.starts_with("/data/media/") {
                return format!("/storage/emulated{}", &normalized["/data/media".len()..]);
            }
            if normalized.starts_with("/sdcard") {
                return format!("/storage/emulated/0{}", &normalized["/sdcard".len()..]);
            }
            if normalized.starts_with("/storage/self/primary") {
                return format!(
                    "/storage/emulated/0{}",
                    &normalized["/storage/self/primary".len()..]
                );
            }
            if normalized.starts_with("/mnt/user/0/emulated/0") {
                return format!(
                    "/storage/emulated/0{}",
                    &normalized["/mnt/user/0/emulated/0".len()..]
                );
            }
            normalized
        }

        pub fn storage_user_root_for_user(user_id: i32) -> String {
            format!("/storage/emulated/{}", user_id)
        }

        pub fn data_media_user_root_for_user(user_id: i32) -> String {
            format!("/data/media/{}", user_id)
        }
    }

    // 仅用于 mount_targets_present_with 的 user_id -> user 映射；测试统一用 uid 0 -> 0。
    pub fn user_id_from_uid(_uid: i32) -> i32 {
        0
    }
}

// ---- log 宏桩：canonical/health 判定用 warn! 记录，这里丢弃。 ----
// 抽取函数里原本是 `log::warn!`，由 Python 在注入前替换成 `warn!`。
#[macro_use]
mod log {
    macro_rules! warn {
        ($($arg:tt)*) => {{
            let _ = format_args!($($arg)*);
        }};
    }
}

// ---- 被测函数用到的类型（数据定义副本，非匹配实现） ----
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountOperation {
    Reload,
    Disable,
}

#[derive(Clone, Debug)]
pub struct MountRequest {
    pub operation: MountOperation,
    pub pid: i32,
    pub uid: i32,
    pub package_name: String,
}

// ---- 让抽取函数能解析到桩模块 ----
use crate::platform::mountinfo;
use crate::platform::paths;

// ---- 抽取的真实函数（daemon_mount 两处修复） ----
// __INJECT_ALL__

// ===================== 测试驱动 =====================

fn main() {
    let mut failures: Vec<&'static str> = Vec::new();

    let req = MountRequest {
        operation: MountOperation::Reload,
        pid: 1,
        uid: 0,
        package_name: "pkg".to_string(),
    };

    // --- 用例 1：主入口缺失但后端仍在 -> 修复后健康判据必须判定缺失（不再被后端顶包）。 ---
    let mi_main_missing = "200 99 0:1 /Download/X /data/media/0/Download/X - fuse.srx srx_fuse\n";
    let present = mount_targets_present_with(
        mi_main_missing,
        &["/storage/emulated/0/Download/X".to_string()],
        &req,
    );
    if present {
        println!("FAIL health_main_missing_masked: 主入口缺失却被后端顶包判为健康");
        failures.push("health_main_missing_masked");
    } else {
        println!("PASS health_main_missing_masked: 主入口缺失被检出");
    }

    // --- 用例 2：主入口与后端都在 -> 健康。 ---
    let mi_both = "200 99 0:1 /Download/X /storage/emulated/0/Download/X - fuse.srx srx_fuse\n\
                   201 99 0:1 /Download/X /data/media/0/Download/X - fuse.srx srx_fuse\n";
    let present_both = mount_targets_present_with(
        mi_both,
        &["/storage/emulated/0/Download/X".to_string()],
        &req,
    );
    if !present_both {
        println!("FAIL health_both_present: 主后端都在却判为不健康");
        failures.push("health_both_present");
    } else {
        println!("PASS health_both_present");
    }

    // --- 用例 3：符号链接入口 /sdcard（无字面挂载）仅主入口在 -> 健康（不要求字面记录存在）。 ---
    let mi_symlink = "200 99 0:1 /Download/X /storage/emulated/0/Download/X - fuse.srx srx_fuse\n";
    let present_symlink = mount_targets_present_with(
        mi_symlink,
        &["/sdcard/Download/X".to_string()],
        &req,
    );
    if !present_symlink {
        println!("FAIL health_symlink_entry: 符号链接入口不应要求字面挂载");
        failures.push("health_symlink_entry");
    } else {
        println!("PASS health_symlink_entry: 符号链接入口折叠到主入口命中");
    }

    // --- 用例 4：后端记录缺失、仅主入口在 -> 不健康（后端作为独立分组需单独核实）。 ---
    let mi_backend_missing =
        "200 99 0:1 /Download/X /storage/emulated/0/Download/X - fuse.srx srx_fuse\n";
    let present_backend_missing = mount_targets_present_with(
        mi_backend_missing,
        &["/data/media/0/Download/X".to_string()],
        &req,
    );
    if present_backend_missing {
        println!("FAIL health_backend_missing_masked: 后端缺失却被主入口顶包");
        failures.push("health_backend_missing_masked");
    } else {
        println!("PASS health_backend_missing_masked: 后端缺失被检出");
    }

    // --- 用例 5：/mnt 是独立挂载视图，不能由主入口顶替（独立不可顶替反例）。 ---
    let mi_mnt = "200 99 0:1 /Download/X /storage/emulated/0/Download/X - fuse.srx srx_fuse\n";
    let present_mnt = mount_targets_present_with(
        mi_mnt,
        &["/mnt/user/0/emulated/0/Download/X".to_string()],
        &req,
    );
    if present_mnt {
        println!("FAIL health_mnt_independent_masked: /mnt 独立视图缺失却被主入口顶替判为健康");
        failures.push("health_mnt_independent_masked");
    } else {
        println!("PASS health_mnt_independent: /mnt 独立视图缺失被检出");
    }

    // --- 用例 5b：/mnt 独立分组确实可单独满足（挂载记录落在 /mnt 视图上即健康）。 ---
    let mi_mnt_present =
        "200 99 0:1 /Download/X /mnt/user/0/emulated/0/Download/X - fuse.srx srx_fuse\n";
    let present_mnt_ok = mount_targets_present_with(
        mi_mnt_present,
        &["/mnt/user/0/emulated/0/Download/X".to_string()],
        &req,
    );
    if !present_mnt_ok {
        println!("FAIL health_mnt_present: /mnt 视图挂载在场却判为不健康");
        failures.push("health_mnt_present");
    } else {
        println!("PASS health_mnt_present: /mnt 独立视图在场被判定健康");
    }

    // --- 用例 6：reload_refuse_when_unverified 纯判定。 ---
    if !reload_refuse_when_unverified(MountOperation::Reload, false, false) {
        println!("FAIL refuse_no_ledger: 无账本且清理未验证应拒绝");
        failures.push("refuse_no_ledger");
    } else {
        println!("PASS refuse_no_ledger");
    }
    if reload_refuse_when_unverified(MountOperation::Reload, false, true) {
        println!("FAIL refuse_has_ledger: 有账本应放行给预算处理");
        failures.push("refuse_has_ledger");
    } else {
        println!("PASS refuse_has_ledger");
    }
    if reload_refuse_when_unverified(MountOperation::Disable, false, false) {
        println!("FAIL refuse_disable: Disable 不应被此判据拒绝");
        failures.push("refuse_disable");
    } else {
        println!("PASS refuse_disable");
    }
    if reload_refuse_when_unverified(MountOperation::Reload, true, false) {
        println!("FAIL refuse_cleared: 清理已清空应放行");
        failures.push("refuse_cleared");
    } else {
        println!("PASS refuse_cleared");
    }

    if failures.is_empty() {
        println!("ALL DAEMON MOUNT HEALTH CASES PASSED");
    } else {
        println!("DAEMON MOUNT HEALTH CASE(S) FAILED: {:?}", failures);
        std::process::exit(1);
    }
}
