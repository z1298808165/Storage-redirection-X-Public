// 守护进程挂载健康判据 harness 模板（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 与 mount_identity_syntax.rs 同思路：抽出真实 fn 注入模板，编译并执行源码里的真实实现，
// 而不是一份抄写副本（抄写副本无法证明原函数有没有被改坏）。
//
// 桩只暴露被抽取函数用到的签名/全局状态，绝不复制"匹配实现"：
// - `__INJECT_MOUNTINFO__` / `__INJECT_PATHS__` 是**真实**抽取的函数；
// - `platform::user_id_from_uid` 是最小桩（测试统一 uid 0 -> user 0）；
// - `log` 宏桩丢弃告警。
//
// 被测不变量（来自 src/daemon_mount.rs）：
//   `mount_targets_present_with`、`canonical_health_target`、`mount_target_count_from_mountinfo`
//   必须按"存储别名组"判定健康。理由：同一份共享存储在应用命名空间里有多个内核视图，
//   主入口与后端、符号链接入口、历史别名、以及各个 mnt 挂载视图都是它的别名，状态文件
//   记录哪个别名取决于挂载当时的遍历路径，因此组内任一别名在场即视为该逻辑目标仍挂载。
//
// 反向验证（Python）：把 canonical_health_target 的别名折叠循环置空后，"仅后端别名在场"
// 等正例应从通过变失败，证明 harness 真在验证别名分组语义而非空壳。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

use std::collections::HashSet;

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
    }

    // 仅用于 mount_targets_present_with 的 uid -> user 映射；测试统一用 uid 0 -> 0。
    pub fn user_id_from_uid(_uid: i32) -> i32 {
        0
    }
}

// ---- log 宏桩：健康判定用 warn! 记录，这里丢弃。 ----
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
#[derive(Clone, Debug)]
pub struct MountRequest {
    pub pid: i32,
    pub uid: i32,
    pub package_name: String,
}

// ---- 让抽取函数能解析到桩模块 ----
use crate::platform::mountinfo;
use crate::platform::paths;

// ---- 抽取的真实函数 ----
// __INJECT_ALL__

// ===================== 测试驱动 =====================

fn main() {
    let mut failures: Vec<&'static str> = Vec::new();

    let req = MountRequest {
        pid: 1,
        uid: 0,
        package_name: "pkg".to_string(),
    };

    // --- 用例 1：主入口自身在场 -> 健康。 ---
    let mi_main = "200 99 0:1 /Download/X /storage/emulated/0/Download/X - fuse.srx srx_fuse\n";
    if !mount_targets_present_with(
        mi_main,
        &["/storage/emulated/0/Download/X".to_string()],
        &req,
    ) {
        println!("FAIL health_main_present: 主入口在场却判为不健康");
        failures.push("health_main_present");
    } else {
        println!("PASS health_main_present");
    }

    // --- 用例 2：记录主入口、只有后端 /data/media 别名在场 -> 仍健康（同一别名组）。 ---
    let mi_backend =
        "200 99 0:1 /Download/X /data/media/0/Download/X - fuse.srx srx_fuse\n";
    if !mount_targets_present_with(
        mi_backend,
        &["/storage/emulated/0/Download/X".to_string()],
        &req,
    ) {
        println!("FAIL health_backend_alias_group: 后端别名属同一存储组，不该判为缺失");
        failures.push("health_backend_alias_group");
    } else {
        println!("PASS health_backend_alias_group: 后端别名折叠到主入口组");
    }

    // --- 用例 3：记录 /mnt 视图、只有主入口在场 -> 仍健康（同一别名组）。 ---
    if !mount_targets_present_with(
        mi_main,
        &["/mnt/user/0/emulated/0/Download/X".to_string()],
        &req,
    ) {
        println!("FAIL health_mnt_alias_group: /mnt 视图与主入口是同一存储组");
        failures.push("health_mnt_alias_group");
    } else {
        println!("PASS health_mnt_alias_group: /mnt 视图折叠到主入口组");
    }

    // --- 用例 4：记录 /sdcard 符号链接入口、只有主入口在场 -> 仍健康。 ---
    if !mount_targets_present_with(mi_main, &["/sdcard/Download/X".to_string()], &req) {
        println!("FAIL health_sdcard_alias_group: /sdcard 是主入口的符号链接");
        failures.push("health_sdcard_alias_group");
    } else {
        println!("PASS health_sdcard_alias_group");
    }

    // --- 用例 5：/data/data 历史别名按 /data/user/0 折叠。 ---
    let mi_user0 = "200 99 0:1 /pkg/cache /data/user/0/pkg/cache - fuse.srx srx_fuse\n";
    if !mount_targets_present_with(mi_user0, &["/data/data/pkg/cache".to_string()], &req) {
        println!("FAIL health_data_data_alias: /data/data 折叠到 /data/user/0 后应命中");
        failures.push("health_data_data_alias");
    } else {
        println!("PASS health_data_data_alias");
    }

    // --- 用例 6：整组缺失（现场只剩无关路径）-> 必须判为不健康。 ---
    let mi_unrelated =
        "200 99 0:1 /Other/Y /storage/emulated/0/Other/Y - fuse.srx srx_fuse\n";
    if mount_targets_present_with(
        mi_unrelated,
        &["/storage/emulated/0/Download/X".to_string()],
        &req,
    ) {
        println!("FAIL health_group_missing: 整组缺失却判为健康");
        failures.push("health_group_missing");
    } else {
        println!("PASS health_group_missing");
    }

    // --- 用例 7：空 mountinfo -> 必须判为不健康。 ---
    if mount_targets_present_with("", &["/storage/emulated/0/Download/X".to_string()], &req) {
        println!("FAIL health_empty_mountinfo: 无任何挂载却判为健康");
        failures.push("health_empty_mountinfo");
    } else {
        println!("PASS health_empty_mountinfo");
    }

    // --- 用例 8：多目标分别由不同别名满足 -> 健康。 ---
    let mi_mixed = "200 99 0:1 /Download/X /data/media/0/Download/X - fuse.srx srx_fuse\n\
                    201 99 0:1 /Pictures/Y /storage/emulated/0/Pictures/Y - fuse.srx srx_fuse\n";
    if !mount_targets_present_with(
        mi_mixed,
        &[
            "/storage/emulated/0/Download/X".to_string(),
            "/storage/emulated/0/Pictures/Y".to_string(),
        ],
        &req,
    ) {
        println!("FAIL health_mixed_aliases: 两个目标各自由别名满足时应判为健康");
        failures.push("health_mixed_aliases");
    } else {
        println!("PASS health_mixed_aliases");
    }

    if failures.is_empty() {
        println!("ALL DAEMON MOUNT HEALTH CASES PASSED");
    } else {
        println!("DAEMON MOUNT HEALTH CASE(S) FAILED: {:?}", failures);
        std::process::exit(1);
    }
}
