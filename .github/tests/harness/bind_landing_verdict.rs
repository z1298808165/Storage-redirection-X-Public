// FUSE 承载绑定落地校验 harness 模板（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 与 reload_keep_redirect_root.rs / daemon_mount_health.rs 同思路：抽出真实 fn 注入模板，
// 编译并执行源码里的真实实现，而不是一份抄写副本（抄写副本无法证明原函数有没有被改坏）。
//
// 背景：Android 13 场景 29 上出现「`alias diag overlay bind ... mounted=true` 与应用
// ENOENT 同秒并存」——`mount(MS_BIND)` 返回 0，但应用行走落不进那层。旧实现直接
// `record_mounted_target(target)` + `return true`，把「syscall 成功」当成「落地有效」写进
// 状态文件与挂载身份账本，后续所有自检都继承这个乐观结论。本 harness 锁定修复后的判据：
// 只有「目标解析成非目录」才判失败，且绑定前后 inode 未变化时必须归入 Inconclusive
// 放行，不得误杀正常挂载。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

// __INJECT_VERDICT__

/// 与源码 `BindLandingSample` 字段一一对应；由注入的 verdict 函数消费。
fn sample(
    is_directory: Option<bool>,
    inode_before: Option<(u64, u64)>,
    inode_after: Option<(u64, u64)>,
) -> BindLandingSample {
    BindLandingSample {
        target_is_directory: is_directory,
        target_inode_after: inode_after,
        target_inode_before: inode_before,
    }
}

fn verdict_name(verdict: &BindLandingVerdict) -> &'static str {
    match verdict {
        BindLandingVerdict::Accepted => "accepted",
        BindLandingVerdict::Inconclusive => "inconclusive",
        BindLandingVerdict::NotDirectory => "not_directory",
    }
}

fn main() {
    let mut failures = 0usize;
    let mut check = |name: &str, actual: &str, expected: &str| {
        if actual == expected {
            println!("PASS {} -> {}", name, actual);
        } else {
            println!("FAIL {} actual={} expected={}", name, actual, expected);
            failures += 1;
        }
    };

    // 目标确是目录：绑定按成功处理。
    check(
        "target_directory",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(true),
            Some((1, 10)),
            Some((1, 20)),
        ))),
        "accepted",
    );

    // stat 失败：无法判定目录性，按成功放行（沿用原有「FUSE 承载路径不做 inode 校验」口径）。
    check(
        "stat_failed",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            None,
            Some((1, 10)),
            None,
        ))),
        "accepted",
    );

    // 目标不是目录且 inode 已变化：确定无歧义的失效形态，必须判失败。
    check(
        "not_directory_changed",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(false),
            Some((1, 10)),
            Some((2, 30)),
        ))),
        "not_directory",
    );

    // 目标不是目录但 inode 与绑定前完全相同：读数可能仍是绑定前那层，无法判定，不得误杀。
    check(
        "not_directory_unchanged",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(false),
            Some((1, 10)),
            Some((1, 10)),
        ))),
        "inconclusive",
    );

    // 只有 st_dev 变化也算已变化，仍按失效判定。
    check(
        "not_directory_dev_only_changed",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(false),
            Some((1, 10)),
            Some((7, 10)),
        ))),
        "not_directory",
    );

    // 缺绑定前读数且目标不是目录：按事实判定失败。
    check(
        "not_directory_no_before",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(false),
            None,
            Some((1, 10)),
        ))),
        "not_directory",
    );

    // 目录形态一律放行，不受 inode 未变化影响。
    check(
        "directory_unchanged_inode",
        verdict_name(&fuse_backed_bind_landing_verdict(sample(
            Some(true),
            Some((1, 10)),
            Some((1, 10)),
        ))),
        "accepted",
    );

    if failures > 0 {
        println!("{} case(s) failed", failures);
        std::process::exit(1);
    }
    println!("ALL BIND LANDING VERDICT CASES PASSED");
}
