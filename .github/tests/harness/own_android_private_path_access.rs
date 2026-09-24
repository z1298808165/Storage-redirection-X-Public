// 应用访问自身 Android 私有目录判定 harness（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 回归背景（2026-09 真机报障，修复见提交 23388f01）：MediaProvider 非 BPF 模式下
// is_app_accessible_path 对 owned 路径先于系统 Java 层判定拒绝，应用缺少
// AppDataIsolation 绑定挂载时连所有者访问自身包目录也会被挡住，微信/QQ 打开自身
// 目录内的图片与缓存报“权限有问题”。修复在 FuseFix accessible 判定链尾追加
// should_allow_own_android_private_path_access：owner uid 与调用 uid 完全相等、
// 同一 Android 用户、且目录所属包排除系统写入包与媒体中间包时放行。
//
// 与 daemon_mount_health.rs 同思路：抽出真实 fn 注入模板编译执行，绝不抄写实现；
// 桩只暴露被抽取函数用到的签名。`policy::get_uid_for_package` 依赖运行期包数据库，
// 这里用固定表桩；包名合法性、用户段解析、路径归一化前缀、uid→user 折算等
// 判定输入全部使用真实抽取的实现。
//
// 反向验证（Python）：把 owner 相等判定改坏后，owner 放行用例必须从通过变失败，
// 证明 harness 真在验证原函数而非空壳——这正是修复合入前该函数缺失 owner 放行时
// 会暴露的形态。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

// ---- 真实 redirect::writer 模块的桩：只提供 uid 区间常量（值与 src/redirect/writer.rs 一致）。 ----
mod writer {
    pub const ANDROID_APP_UID_START: i32 = 10000;
}

// ---- 真实 platform 模块：uid -> user 折算使用真实实现。 ----
mod platform {
    pub const ANDROID_USER_ID_OFFSET: i32 = 100000;

    pub fn user_id_from_uid(uid: i32) -> i32 {
        if uid >= 0 {
            uid / ANDROID_USER_ID_OFFSET
        } else {
            0
        }
    }
}

// ---- 真实 platform::paths 模块：抽取 is_valid_package_name 注入；
//      normalize/starts_with 是最小桩（用例输入均为规整路径，桩语义与真实实现
//      在这些输入上等价，别名改写不在被测范围内）。 ----
pub mod paths {
    // __INJECT_PATHS__

    pub fn normalize(path: &str) -> String {
        let mut collapsed = String::with_capacity(path.len());
        let mut prev_slash = false;
        for ch in path.chars() {
            if ch == '/' {
                if !prev_slash {
                    collapsed.push(ch);
                }
                prev_slash = true;
            } else {
                collapsed.push(ch);
                prev_slash = false;
            }
        }
        let trimmed = collapsed.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn starts_with(path: &str, prefix: &str) -> bool {
        path.starts_with(prefix)
    }
}

// ---- 真实 redirect::policy 模块的桩：包名集合用最小表；uid 查询用固定表。
//      集合内容本身不是被测对象，被测函数是否正确**查阅**这些判定才是。 ----
mod policy {
    pub fn get_uid_for_package(package_name: &str) -> i32 {
        match package_name {
            "com.example.owner" => 10384,
            "com.example.other" => 10111,
            "com.example.systemwriter" => 10242,
            "com.android.providers.media.module" => 10260,
            _ => -1,
        }
    }

    pub fn is_media_intermediate_package(package_name: &str) -> bool {
        matches!(package_name, "com.android.providers.media.module" | "com.android.providers.media")
    }

    pub fn is_system_writer_package(package_name: &str) -> bool {
        matches!(package_name, "com.example.systemwriter")
    }
}

// ---- log 宏桩：抽取函数里原本是 `log::debug!`，由 Python 在注入前替换成 `debug!`。 ----
#[macro_use]
mod log {
    macro_rules! debug {
        ($($arg:tt)*) => {{
            let _ = format_args!($($arg)*);
        }};
    }
}

// ---- 抽取的真实函数（src/hook/media_fuse.rs） ----
// __INJECT_MEDIA_FUSE__

// ===================== 测试驱动 =====================

fn expect_allow(name: &'static str, path: &str, caller_uid: i32) -> bool {
    if should_allow_own_android_private_path_access(path, caller_uid) {
        println!("PASS {}: path={} caller_uid={}", name, path, caller_uid);
        true
    } else {
        println!("FAIL {}: 应放行 owner 对自身私有目录的访问 path={} caller_uid={}", name, path, caller_uid);
        false
    }
}

fn expect_deny(name: &'static str, path: &str, caller_uid: i32) -> bool {
    if should_allow_own_android_private_path_access(path, caller_uid) {
        println!("FAIL {}: 不该放行 path={} caller_uid={}", name, path, caller_uid);
        false
    } else {
        println!("PASS {}: path={} caller_uid={}", name, path, caller_uid);
        true
    }
}

fn main() {
    let mut failures: Vec<&'static str> = Vec::new();

    // --- owner 放行：三类自有私有目录（media/data/obb）都必须对 owner 开放。 ---
    if !expect_allow(
        "own_media_owner_allowed",
        "/storage/emulated/0/Android/media/com.example.owner/fun_stable/cache.json",
        10384,
    ) {
        failures.push("own_media_owner_allowed");
    }
    if !expect_allow(
        "own_data_owner_allowed",
        "/storage/emulated/0/Android/data/com.example.owner/files/pic.jpg",
        10384,
    ) {
        failures.push("own_data_owner_allowed");
    }
    if !expect_allow(
        "own_obb_owner_allowed",
        "/storage/emulated/0/Android/obb/com.example.owner/patch.dat",
        10384,
    ) {
        failures.push("own_obb_owner_allowed");
    }

    // --- owner 拒绝边界：非 owner 应用不得借该判定访问他人私有目录。 ---
    if !expect_deny(
        "foreign_owner_denied",
        "/storage/emulated/0/Android/media/com.example.owner/fun_stable/cache.json",
        10111,
    ) {
        failures.push("foreign_owner_denied");
    }
    if !expect_deny(
        "foreign_data_denied",
        "/storage/emulated/0/Android/data/com.example.owner/files/pic.jpg",
        10111,
    ) {
        failures.push("foreign_data_denied");
    }

    // --- 非 app uid（root / system / media）不放宽。 ---
    if !expect_deny(
        "root_uid_denied",
        "/storage/emulated/0/Android/media/com.example.owner/fun_stable/cache.json",
        0,
    ) {
        failures.push("root_uid_denied");
    }
    if !expect_deny(
        "media_uid_denied",
        "/storage/emulated/0/Android/data/com.example.owner/files/pic.jpg",
        1023,
    ) {
        failures.push("media_uid_denied");
    }

    // --- 跨用户不放宽：路径 user 10 与调用 uid 折算的 user 0 不一致，直接拒绝；
    //     user 段一致但包 owner 属另一用户时，owner uid 相等性也必须挡住。 ---
    if !expect_deny(
        "cross_user_path_denied",
        "/storage/emulated/10/Android/media/com.example.owner/fun_stable/cache.json",
        10384,
    ) {
        failures.push("cross_user_path_denied");
    }
    if !expect_deny(
        "cross_user_owner_uid_denied",
        "/storage/emulated/10/Android/media/com.example.owner/fun_stable/cache.json",
        1010384,
    ) {
        failures.push("cross_user_owner_uid_denied");
    }

    // --- 系统写入包与媒体中间包目录不放宽：即使 uid 表上完全相等也必须拒绝。 ---
    if !expect_deny(
        "system_writer_package_denied",
        "/storage/emulated/0/Android/data/com.example.systemwriter/cache.bin",
        10242,
    ) {
        failures.push("system_writer_package_denied");
    }
    if !expect_deny(
        "media_intermediate_package_denied",
        "/storage/emulated/0/Android/media/com.android.providers.media.module/pending.bin",
        10260,
    ) {
        failures.push("media_intermediate_package_denied");
    }

    // --- 非存储路径与公共路径：归一化前缀不匹配或解析不出 owner 一律拒绝。 ---
    if !expect_deny(
        "non_storage_path_denied",
        "/data/media/0/Android/media/com.example.owner/fun_stable/cache.json",
        10384,
    ) {
        failures.push("non_storage_path_denied");
    }
    if !expect_deny(
        "public_path_denied",
        "/storage/emulated/0/Pictures/WeiXin/wx_camera.jpg",
        10384,
    ) {
        failures.push("public_path_denied");
    }
    if !expect_deny(
        "empty_path_denied",
        "",
        10384,
    ) {
        failures.push("empty_path_denied");
    }
    if !expect_deny(
        "android_root_without_category_denied",
        "/storage/emulated/0/Android/停止.log",
        10384,
    ) {
        failures.push("android_root_without_category_denied");
    }

    if failures.is_empty() {
        println!("ALL OWN ANDROID PRIVATE PATH CASES PASSED");
    } else {
        println!("OWN ANDROID PRIVATE PATH CASE(S) FAILED: {:?}", failures);
        std::process::exit(1);
    }
}
