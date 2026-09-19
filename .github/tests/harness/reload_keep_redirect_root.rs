// 热重载保留重定向根 harness 模板（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 与 daemon_mount_health.rs / mount_identity_syntax.rs 同思路：抽出真实 fn 注入模板，编译并
// 执行源码里的真实实现，而不是一份抄写副本（抄写副本无法证明原函数有没有被改坏）。
//
// 接缝说明：`is_module_redirect_mount` 在这里是**可注入的输入**。它本身由
// mount_identity_syntax harness 与 test_scenario_consistency 的归属断言锁定，本 harness
// 只验证"保留判定"这一层逻辑——包括它在归属为假时必须拒绝保留。
//
// 被测不变量（来自 src/daemon_mount.rs）：
//   `should_keep_reload_redirect_root` 只在本轮仍是重定向、后端没有换成需要 scoped FUSE 根、
//   该入口最上层确是本模块层、且那一层的沙箱根与本次重定向目标一致时才保留既有绑定。
//   放宽任何一条都会让热重载在错误时机保留旧挂载（改沙箱目标后仍指向旧沙箱）。
//   另外：`is_mapping_mode_only` 为 true 时必须返回 false——从默认重定向切到仅映射模式
//   时旧沙箱根绑定必须摘除重建，否则仍保留旧重定向根会让应用读到错误视图。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

mod paths {
    // __INJECT_PATHS__
}

mod mount_identity {
    use std::cell::Cell;

    thread_local! {
        static OWNERSHIP: Cell<bool> = const { Cell::new(false) };
    }

    pub fn set_ownership(value: bool) {
        OWNERSHIP.with(|slot| slot.set(value));
    }

    // 接缝：把"这一层是不是本模块的"when 结果交给调用方，隔离被测的保留判定。
    pub fn is_module_redirect_mount(_source: &str, _root: &str, _target: &str, _pkg: &str) -> bool {
        OWNERSHIP.with(|slot| slot.get())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MountOperation {
    Reload,
    Apply,
    Disable,
}

pub struct MountRequest {
    pub operation: MountOperation,
    pub package_name: String,
    pub redirect_target: String,
    pub is_mapping_mode_only: bool,
}

// 抽取函数里的 `paths::` / `mount_identity::` 是 crate 根模块路径，模板在同名模块内定义桩。

// ---- 抽取的真实函数 ----
// __INJECT_ALL__

// ===================== 测试驱动 =====================

const SANDBOX_TARGET: &str = "/storage/emulated/0/Android/data/com.demo/sdcard";

fn request(operation: MountOperation, redirect_target: &str, is_mapping_mode_only: bool) -> MountRequest {
    MountRequest {
        operation,
        package_name: "com.demo".to_string(),
        redirect_target: redirect_target.to_string(),
        is_mapping_mode_only,
    }
}

fn check(failures: &mut Vec<&'static str>, label: &'static str, actual: bool, expected: bool) {
    if actual == expected {
        println!("PASS {label}");
    } else {
        println!("FAIL {label} actual={actual} expected={expected}");
        failures.push(label);
    }
}

fn main() {
    let mut failures: Vec<&'static str> = Vec::new();
    let no_scoped_roots: Vec<String> = Vec::new();
    let scoped_roots = vec!["/storage/emulated/0".to_string()];

    // 主入口：本模块沙箱根 + 与本次重定向目标一致 -> 保留。
    mount_identity::set_ownership(true);
    check(
        &mut failures,
        "reload_matching_sandbox_root_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        true,
    );

    // 系统 FUSE 视图写法（root 以 /<user> 开头）是同一棵树的等价写法，同样保留。
    check(
        &mut failures,
        "reload_fuse_view_root_kept",
        should_keep_reload_redirect_root(
            "/mnt/user/0/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/fuse", "/0/Android/data/com.demo/sdcard")),
        ),
        true,
    );

    // 只读/媒体视图根同样属于存储视图根。
    check(
        &mut failures,
        "reload_media_view_root_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(
                MountOperation::Reload,
                "/storage/emulated/0/Android/media/com.demo/sdcard",
                false,
            ),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/media/com.demo/sdcard")),
        ),
        true,
    );

    // 沙箱目标改了（另一个应用/另一个包名）：必须摘掉重建，不能保留旧沙箱。
    check(
        &mut failures,
        "reload_other_package_sandbox_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.other/sdcard")),
        ),
        false,
    );

    // 自定义重定向目标（尾部不是 Android/... 形态）：退回摘了重建的安全路径。
    check(
        &mut failures,
        "reload_custom_redirect_target_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, "/storage/emulated/0/SrtCustomSandbox", false),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/SrtCustomSandbox")),
        ),
        false,
    );

    // 平台自己的应用专属目录挂载（root 止于包名，没有 /sdcard）：不是本模块沙箱根。
    check(
        &mut failures,
        "reload_app_private_directory_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0/Android/data/com.demo",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/fuse", "/0/Android/data/com.demo")),
        ),
        false,
    );

    // 归属为假（不是本模块的层）：一律不保留，交给原有摘除判定处理。
    mount_identity::set_ownership(false);
    check(
        &mut failures,
        "reload_foreign_layer_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/fuse", "/0/Android/data/com.demo/sdcard")),
        ),
        false,
    );
    mount_identity::set_ownership(true);

    // 该入口没有活动挂载：没有可保留的东西。
    check(
        &mut failures,
        "reload_absent_layer_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &no_scoped_roots,
            None,
        ),
        false,
    );

    // 本轮不是热重载（首次挂载）：不走保留分支。
    check(
        &mut failures,
        "apply_operation_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Apply, SANDBOX_TARGET, false),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        false,
    );

    // 未启用重定向（无重定向目标）：旧层必须摘干净。
    check(
        &mut failures,
        "empty_redirect_target_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, "", false),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        false,
    );

    // 后端换成了需要 scoped FUSE 根：该入口要改由 FUSE 接管，必须摘掉旧 bind。
    check(
        &mut failures,
        "scoped_fuse_root_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        false,
    );

    // scoped 根只覆盖子树时，父入口本身仍应保留。
    check(
        &mut failures,
        "scoped_fuse_child_root_keeps_parent",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, false),
            &["/storage/emulated/0/Download".to_string()],
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        true,
    );

    // 仅映射模式：即便本模块沙箱根与本次重定向目标一致，也不得保留旧重定向根，
    // 否则从默认重定向切到仅映射模式后旧沙箱根仍在，应用读到错误视图。
    // （ownership 仍为真，验证 is_mapping_mode_only 守卫优先于归属/沙箱根一致性。）
    check(
        &mut failures,
        "reload_mapping_mode_only_not_kept",
        should_keep_reload_redirect_root(
            "/storage/emulated/0",
            &request(MountOperation::Reload, SANDBOX_TARGET, true),
            &no_scoped_roots,
            Some(("/dev/block/dm-60", "/media/0/Android/data/com.demo/sdcard")),
        ),
        false,
    );

    if failures.is_empty() {
        println!("ALL RELOAD KEEP REDIRECT ROOT CASES PASSED");
        return;
    }
    eprintln!("failures={}", failures.len());
    std::process::exit(1);
}
