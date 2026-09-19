// 边界回归 harness 模板：由 Python 注入「真实函数」后，用 rustc 独立编译运行。
//
// 本文件不直接包含 should_passthrough_provider_allowed_parent_mkdir 的实现——Python 会从
// src/hook/ops/mutation/dir.rs 用大括号计数抽出整个 fn，注入到下面的占位符处，从而真正编译并执行
// 源码里的真实函数，而不是一份抄写副本（抄写副本无法验证原函数有没有被改坏）。
// 项目禁止在 src/ 内新增内联 Rust 测试，因此 harness 放在 .github/tests/harness/ 由 rustc 独立运行。
//
// 这里只保留真实 crate::hook / writer / policy 模块的「桩」与 InterceptHub 类型：桩只暴露被抽取函数
// 用到的签名，状态用 AtomicBool 保存（不用 static mut / unsafe），绝不复制真实实现。

// ---- 真实 crate::hook 模块的桩 ----
// 真实实现维护 MediaProvider passthrough / virtual 作用域的全局开关；这里用 AtomicBool 近似。
mod hook {
    use std::sync::atomic::{AtomicBool, Ordering};

    static PASSTHROUGH_ACTIVE: AtomicBool = AtomicBool::new(false);
    static VIRTUAL_SCOPE_ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn is_provider_passthrough_active() -> bool {
        PASSTHROUGH_ACTIVE.load(Ordering::SeqCst)
    }

    pub fn is_provider_virtual_scope_active() -> bool {
        VIRTUAL_SCOPE_ACTIVE.load(Ordering::SeqCst)
    }

    // 仅测试驱动用：按用例设置作用域开关。
    pub fn __test_set_passthrough(v: bool) {
        PASSTHROUGH_ACTIVE.store(v, Ordering::SeqCst);
    }
    pub fn __test_set_virtual(v: bool) {
        VIRTUAL_SCOPE_ACTIVE.store(v, Ordering::SeqCst);
    }
}

// ---- 真实 writer 模块的桩 ----
// 真实实现查询 caller 的「放行真实路径」集合；这里用单个 AtomicBool 表示 source 是否为
// 该 caller 放行真实路径的父级。
mod writer {
    use std::sync::atomic::{AtomicBool, Ordering};

    static PARENT_ALLOWED: AtomicBool = AtomicBool::new(false);

    pub fn is_path_parent_of_caller_allowed_real_path(_source: &str, _pkg: &str, _uid: u32) -> bool {
        PARENT_ALLOWED.load(Ordering::SeqCst)
    }

    // 仅测试驱动用：按用例设置「是否为放行真实路径的父级」。
    pub fn __test_set_parent_allowed(v: bool) {
        PARENT_ALLOWED.store(v, Ordering::SeqCst);
    }
}

// ---- 真实 policy 模块的桩 ----
// 真实实现查 writer 白名单；这里用「包名非空」近似 system-writer 判定。
mod policy {
    pub fn is_system_writer_package(pkg: &str) -> bool {
        !pkg.is_empty()
    }
}

// ---- InterceptHub 类型桩：仅暴露被抽取函数用到的字段与方法 ----
struct InterceptHub {
    monitor_only: bool,
    package_name: &'static str,
    caller_package: &'static str,
    caller_uid: u32,
}

impl InterceptHub {
    fn is_monitor_only(&self) -> bool {
        self.monitor_only
    }
    fn with_package_name(&self, f: fn(&str) -> bool) -> bool {
        f(self.package_name)
    }
    fn get_current_caller_package(&self) -> String {
        self.caller_package.to_string()
    }
    fn get_current_caller_uid(&self) -> u32 {
        self.caller_uid
    }
}

// ---- RedirectDecision 类型桩：被抽取函数只用 is_redirect() 与 is_mapping，无 new_path ----
#[derive(Clone, Copy)]
struct RedirectDecision {
    is_redirect: bool,
    is_mapping: bool,
}

impl RedirectDecision {
    fn is_redirect(&self) -> bool {
        self.is_redirect
    }
}

// Python 注入点：从 src/hook/ops/mutation/dir.rs 抽出的整个 fn 会替换下面这行占位符。
// __INJECT_SHOULD_PASSTHROUGH_FN__

// ---- 10 个边界用例 + 真值表驱动 ----
// 前 3 个期望 TRUE（含 virtual-only 修复点），后 7 个期望 FALSE。用 Default 只写出需要置真的字段。
struct Case {
    name: &'static str,
    is_redirect: bool,
    is_mapping: bool,
    monitor_only: bool,
    provider_passthrough: bool,
    provider_virtual: bool,
    system_writer: bool,
    caller_nonempty: bool,
    parent_allowed: bool,
    expect: bool,
}

impl Default for Case {
    fn default() -> Self {
        Case {
            name: "",
            is_redirect: false,
            is_mapping: false,
            monitor_only: false,
            provider_passthrough: false,
            provider_virtual: false,
            system_writer: false,
            caller_nonempty: false,
            parent_allowed: false,
            expect: false,
        }
    }
}

fn main() {
    let cases = [
        // virtual-only + 放行父路径 -> TRUE：本次修复点，virtual 作用域也必须被接受。
        Case { name: "virtual_only_and_allowed_parent_true", is_redirect: true, provider_virtual: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: true, ..Default::default() },
        // passthrough 激活 -> TRUE：既有行为必须保留。
        Case { name: "passthrough_true", is_redirect: true, provider_passthrough: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: true, ..Default::default() },
        // 两种作用域同时激活时仍允许真实祖先创建。
        Case { name: "passthrough_and_virtual_true", is_redirect: true, provider_passthrough: true, provider_virtual: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: true, ..Default::default() },
        // 无任何作用域 -> FALSE。
        Case { name: "no_scope_false", is_redirect: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: false, ..Default::default() },
        // 非 redirect -> FALSE。
        Case { name: "not_redirect_false", provider_passthrough: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: false, ..Default::default() },
        // mapping 重定向 -> FALSE。
        Case { name: "mapping_false", is_redirect: true, is_mapping: true, provider_passthrough: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: false, ..Default::default() },
        // 仅监视 -> FALSE。
        Case { name: "monitor_only_false", is_redirect: true, monitor_only: true, provider_passthrough: true, system_writer: true, caller_nonempty: true, parent_allowed: true, expect: false, ..Default::default() },
        // 非 system-writer 包 -> FALSE。
        Case { name: "non_writer_false", is_redirect: true, provider_passthrough: true, caller_nonempty: true, parent_allowed: true, expect: false, ..Default::default() },
        // caller 包名为空 -> FALSE。
        Case { name: "no_caller_false", is_redirect: true, provider_passthrough: true, system_writer: true, parent_allowed: true, expect: false, ..Default::default() },
        // caller 合法但路径非放行真实路径的父级 -> FALSE。
        Case { name: "not_parent_false", is_redirect: true, provider_passthrough: true, system_writer: true, caller_nonempty: true, expect: false, ..Default::default() },
    ];

    let mut failures: Vec<&'static str> = Vec::new();
    for c in &cases {
        hook::__test_set_passthrough(c.provider_passthrough);
        hook::__test_set_virtual(c.provider_virtual);
        writer::__test_set_parent_allowed(c.parent_allowed);
        let hub = InterceptHub {
            monitor_only: c.monitor_only,
            package_name: if c.system_writer { "com.android.providers.media" } else { "" },
            caller_package: if c.caller_nonempty { "com.android.providers.media" } else { "" },
            caller_uid: 1000,
        };
        let redirect_result = RedirectDecision { is_redirect: c.is_redirect, is_mapping: c.is_mapping };
        let got = should_passthrough_provider_allowed_parent_mkdir(&hub, "/storage/emulated/0/DCIM", &redirect_result);
        if got != c.expect {
            println!("FAIL {}: expected={} got={}", c.name, c.expect, got);
            failures.push(c.name);
        } else {
            println!("PASS {}: got={}", c.name, got);
        }
    }

    if failures.is_empty() {
        println!("ALL {} CASES PASSED", cases.len());
    } else {
        println!("{} CASE(S) FAILED: {:?}", failures.len(), failures);
        std::process::exit(1);
    }
}
