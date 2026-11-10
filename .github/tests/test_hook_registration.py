"""执行生产 hook 结果处理与调用方筛选，验证安装状态及注入器排除。"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def extract_function(source: str, name: str) -> str:
    start = source.index(f"fn {name}(")
    brace = source.index("{", start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class HookRegistrationTest(unittest.TestCase):
    def test_registration_cleanup_and_caller_filter(self) -> None:
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("环境缺少 rustc，执行验证已跳过")
        source = (ROOT / "src/hook/stats.rs").read_text(encoding="utf8")
        single = extract_function(source, "take_checked_hook_stub")
        batch = extract_function(source, "take_checked_hook_batch_stubs")
        cleanup = extract_function(source, "cleanup_after_install_failure")
        caller_filter = "\n".join(
            extract_function(source, name) for name in (
                "should_hook_caller_module", "is_runtime_generated_or_java_artifact",
                "is_java_artifact_module", "is_hook_sensitive_render_module",
                "should_hook_system_writer_caller_module", "should_use_bridge_caller_filter",
            )
        )
        caller_filter += "\n" + next(
            line for line in source.splitlines() if line.startswith("const ZYGISK_MODULE_BASENAME:")
        )
        start = source.index("const UNSTABLE_CALLER_MODULE_BASENAMES:")
        caller_filter += "\n" + source[start:source.index("];", start) + 2]
        harness = r'''
extern crate self as log;
use std::sync::atomic::{AtomicBool, Ordering};
macro_rules! log_message { ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }}; }
pub(crate) use log_message as warn;
pub(crate) use log_message as error;
mod srx_hook {
    pub type HookStub = u64;
    pub struct RefreshError { pub module_path: String, pub errno: i32 }
    pub struct HookError {
        pub status: i32, pub stub: Option<HookStub>, pub errors: Vec<RefreshError>,
    }
    pub struct HookBatchError {
        pub status: i32, pub stubs: Vec<HookStub>, pub errors: Vec<RefreshError>,
    }
    pub struct ClearReport {
        pub revision: u64, pub removed_slots: usize, pub remaining_slots: usize,
        pub remaining_protections: usize, pub errors: Vec<RefreshError>,
    }
    // 手写默认值，避免夹具出现以井号开头的派生属性行。
    impl Default for ClearReport {
        fn default() -> Self {
            Self {
                revision: 0, removed_slots: 0, remaining_slots: 0,
                remaining_protections: 0, errors: Vec::new(),
            }
        }
    }
    pub struct ClearError { pub status: i32, pub report: ClearReport }
    thread_local! {
        pub static CLEAR_RESULT: std::cell::RefCell<Option<Result<ClearReport, ClearError>>> =
            const { std::cell::RefCell::new(None) };
    }
    pub fn clear_checked() -> Result<ClearReport, ClearError> {
        CLEAR_RESULT.with(|result| result.borrow_mut().take().unwrap())
    }
}
struct InterceptHub { is_hooks_installed: AtomicBool }
impl InterceptHub {
    fn get_package_name(&self) -> &'static str { "com.example.test" }
    // __CLEANUP__
}
// __SINGLE__
// __BATCH__
// __CALLER_FILTER__
fn module_errors() -> Vec<srx_hook::RefreshError> {
    vec![srx_hook::RefreshError { module_path: "libtest.so".into(), errno: 1 }]
}
fn main() {
    use srx_hook::*;
    assert!(!should_hook_caller_module("/data/adb/modules/zygisksu/lib64/libzygisk.so"));
    assert!(!should_hook_caller_module("/data/adb/modules/zygisksu/lib/libzygisk.so"));
    assert!(should_hook_caller_module("/system/lib64/libfuse.so"));
    assert!(should_hook_caller_module("/system/lib64/libappfuse.so"));
    assert!(should_hook_caller_module("/system/lib64/libnotzygisk.so"));
    for profile in ["system-writer", "system-writer-monitor", "monitor", "media-runtime"] {
        assert!(should_use_bridge_caller_filter(profile), "{profile}");
    }
    for profile in ["app-write", "fuse-fix", "full"] {
        assert!(!should_use_bridge_caller_filter(profile), "{profile}");
    }
    for path in [
        "/apex/com.android.art/lib64/libandroidio.so",
        "/system/lib64/libandroid_runtime_lazy.so",
        "/system/lib64/libnativebridge_lazy.so",
        "/system/lib64/libnativeloader_lazy.so",
        "/system/lib64/libvndksupport.so",
    ] {
        assert!(!should_hook_system_writer_caller_module(path), "{path}");
    }
    for path in [
        "/system/lib64/libfuse.so",
        "/system/lib64/libappfuse.so",
        "/apex/com.android.mediaprovider/MediaProvider.apk!/lib/x86_64/libfuse_jni.so",
    ] {
        assert!(should_hook_system_writer_caller_module(path), "{path}");
    }
    assert_eq!(take_checked_hook_stub("open", Ok(7)), Some(7));
    assert_eq!(take_checked_hook_stub("open", Err(HookError {
        status: 1, stub: Some(7), errors: module_errors(),
    })), Some(7));
    assert_eq!(take_checked_hook_stub("open", Err(HookError {
        status: 1, stub: None, errors: module_errors(),
    })), None);
    assert_eq!(take_checked_hook_batch_stubs(Ok(vec![7, 9]), "test", "pkg"), Some(vec![7, 9]));
    assert_eq!(take_checked_hook_batch_stubs(Err(HookBatchError {
        status: 1, stubs: vec![7, 9], errors: module_errors(),
    }), "test", "pkg"), Some(vec![7, 9]));
    assert_eq!(take_checked_hook_batch_stubs(Err(HookBatchError {
        status: 1, stubs: vec![], errors: module_errors(),
    }), "test", "pkg"), None);
    for result in [
        Ok(ClearReport::default()),
        Ok(ClearReport { remaining_slots: 1, ..Default::default() }),
        Ok(ClearReport { remaining_protections: 1, ..Default::default() }),
        Err(ClearError { status: 1, report: ClearReport {
            errors: module_errors(), ..Default::default()
        }}),
    ] {
        let hub = InterceptHub { is_hooks_installed: AtomicBool::new(true) };
        CLEAR_RESULT.with(|state| *state.borrow_mut() = Some(result));
        assert!(!hub.cleanup_after_install_failure("test"));
        assert!(!hub.is_hooks_installed.load(Ordering::Relaxed));
    }
}
'''
        harness = harness.replace("// __SINGLE__", single).replace("// __BATCH__", batch)
        harness = harness.replace("// __CLEANUP__", cleanup)
        harness = harness.replace("// __CALLER_FILTER__", caller_filter)
        with tempfile.TemporaryDirectory(prefix="hook-registration-") as tmp:
            path = Path(tmp) / "harness.rs"
            binary = Path(tmp) / ("registration.exe" if os.name == "nt" else "registration")
            path.write_text(harness, encoding="utf8")
            for command in ([rustc, "--edition=2024", str(path), "-o", str(binary)], [str(binary)]):
                result = subprocess.run(command, capture_output=True, text=True, encoding="utf8")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
