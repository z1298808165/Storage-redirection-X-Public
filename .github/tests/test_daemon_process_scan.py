"""执行进程枚举的失败边界，避免包查询故障被当成空进程表。"""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_mount_identity_syntax import extract_fn

ROOT = Path(__file__).resolve().parents[2]


class DaemonProcessScanTest(unittest.TestCase):
    def test_package_failure_is_not_a_successful_empty_scan(self):
        source = (ROOT / "src/daemon.rs").read_text(encoding="utf8")
        code = r'''
#![allow(dead_code, unused_variables)]
use std::{fs as std_fs, io};
extern crate self as log;
#[macro_export] macro_rules! warn { ($($args:tt)*) => {{}}; }
static QUERY_FAILS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
const ANDROID_APP_UID_START: i32 = 10000;
struct AppProcess { pid: i32, uid: i32, package_name: String, is_uninterruptible: bool }
mod platform {
    pub fn process_start_time_ticks(_: i32) -> Option<u64> { Some(1) }
    pub mod mount_namespace { pub fn private_mount_namespace(_: i32) -> Option<()> { Some(()) } }
    pub mod packages {
        pub fn read_uids() -> std::io::Result<std::collections::HashMap<i32, Vec<String>>> {
            if crate::QUERY_FAILS.load(std::sync::atomic::Ordering::Relaxed) {
                Err(std::io::Error::from_raw_os_error(11))
            } else { Ok(Default::default()) }
        }
    }
}
mod mount_intent {
    pub struct Record { pub pid: i32, pub package: String }
    impl Record { pub fn matches(&self, _: i32, _: Option<u64>, _: i32) -> bool { false } }
    pub fn registered_processes() -> Vec<Record> { vec![] }
}
mod policy { pub fn is_media_provider_package(_: &str) -> bool { false } }
fn read_process_status(_: i32) -> Option<(i32, bool)> { None }
fn read_process_package(_: i32) -> Option<String> { None }
''' + extract_fn(source, "list_app_processes") + r'''
fn main() {
    assert_eq!(list_app_processes().err().unwrap().raw_os_error(), Some(11));
    QUERY_FAILS.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(list_app_processes().unwrap().is_empty());
}
'''
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc)
        with tempfile.TemporaryDirectory(prefix="srx-process-scan-") as directory:
            rust = Path(directory, "scan.rs")
            binary = Path(directory, "scan")
            rust.write_text(code, encoding="utf8")
            for command in ([rustc, "--edition=2024", str(rust), "-o", str(binary)], [str(binary)]):
                result = subprocess.run(command, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        reconcile = extract_fn(source, "reconcile_running_apps")
        failure = reconcile.split("let Ok(processes) = list_app_processes() else {", 1)[1].split("};", 1)[0]
        self.assertIn("return (false, true);", failure)


if __name__ == "__main__":
    unittest.main()
