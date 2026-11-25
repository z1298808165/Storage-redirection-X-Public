"""模块层枚举必须排除被父树或同路径栈顶遮住的旧记录。"""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ModuleMountVisibilityTest(unittest.TestCase):
    def test_production_enumerator_only_returns_reachable_layers(self) -> None:
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc, "需要 rustc 执行模块挂载枚举")
        source = (ROOT / "src/module_mount_source.rs").read_text(encoding="utf8")
        harness = r'''
#![allow(dead_code)]
static TABLE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static READ_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
mod platform {
    pub mod paths {
        pub fn normalize(path: &str) -> String { path.into() }
        pub fn eq_ignore_case(a: &str, b: &str) -> bool { a.eq_ignore_ascii_case(b) }
        pub fn storage_user_root_for_user(user: i32) -> String { format!("/storage/emulated/{user}") }
    }
    pub mod module_paths {
        pub const REAL_STORAGE_TMP_PREFIX: &str = "/data/adb/modules/storage.redirect.x/tmp/real_storage/";
    }
    pub mod proc_direct {
        pub fn read_proc_file(_: &str, _: usize) -> Option<String> {
            (!crate::READ_FAILED.load(std::sync::atomic::Ordering::Relaxed)).then(|| crate::TABLE.lock().unwrap().clone())
        }
    }
    #[path = "__MOUNTINFO__"] pub mod mountinfo;
}
#[path = "__MODULE__"] mod module_mount_source;
fn main() {
    let base = "1 0 254:1 / / rw - ext4 rootfs rw\n10 1 0:84 / /storage/emulated rw - fuse /dev/fuse rw\n11 10 254:1 /media/0/Android/data/test/sdcard /storage/emulated/0 rw - ext4 rootfs rw\n";
    for (extra, expected) in [
        ("", vec![11]),
        ("12 10 0:85 / /storage/emulated rw - fuse /dev/fuse rw\n", vec![]),
        ("12 11 0:85 / /storage/emulated/0 rw - tmpfs foreign rw\n", vec![]),
        ("12 10 0:85 / /storage/emulated rw - tmpfs foreign rw\n3 12 254:1 /media/0/Android/data/test/sdcard /storage/emulated/0 rw - ext4 rootfs rw\n", vec![3]),
    ] {
        *TABLE.lock().unwrap() = format!("{base}{extra}");
        let actual = module_mount_source::app_redirect_mounts_in(0, "test").unwrap();
        assert_eq!(actual.iter().map(|mount| mount.mount_id).collect::<Vec<_>>(), expected);
        assert!(actual.iter().all(|mount| mount.is_sandbox_root));
    }
    READ_FAILED.store(true,std::sync::atomic::Ordering::Relaxed);
    assert!(module_mount_source::app_redirect_mounts_in(0, "test").is_none());
}
'''
        with tempfile.TemporaryDirectory(prefix="srx-module-visible-") as tmp:
            folder = Path(tmp)
            module = folder / "module.rs"
            script = folder / "main.rs"
            binary = folder / "visible"
            harness = harness.replace("__MOUNTINFO__", (ROOT / "src/platform/mountinfo.rs").as_posix()).replace("__MODULE__", module.as_posix())
            script.write_text(harness, encoding="utf8")
            mutant = source.replace("mountinfo::visible_mount_at(&entries, &target)?.mount_id != entry.mount_id", "false")
            self.assertNotEqual(source, mutant)
            for name, code, succeeds in (("production", source, True), ("hidden-layer", mutant, False)):
                with self.subTest(variant=name):
                    module.write_text(code, encoding="utf8")
                    compile_result = subprocess.run([rustc, "--edition=2024", str(script), "-o", str(binary)], capture_output=True, text=True)
                    self.assertEqual(compile_result.returncode, 0, compile_result.stderr)
                    result = subprocess.run([str(binary)], capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
