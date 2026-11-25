"""执行 Provider 安装记录的多进程读写及 shell 身份查询。"""

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_mount_identity_syntax import extract_fn

ROOT = Path(__file__).resolve().parents[2]


class MediaHookStateTest(unittest.TestCase):
    def test_process_records_survive_another_users_provider(self) -> None:
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc)
        writer = (ROOT / "src/lifecycle/specialize_pre/writer_state.rs").read_text(encoding="utf8")
        reader = (ROOT / "src/daemon/media_hook_heal.rs").read_text(encoding="utf8")
        with tempfile.TemporaryDirectory(prefix="srx-media-state-") as directory:
            path = Path(directory)
            state = path / "install-state"
            code = r'''
use std::{ffi::CString, fs};
mod libc { pub unsafe fn chmod(_: *const std::ffi::c_char, _: u32) -> i32 { 0 } }
static PROCESS_START_TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(42);
mod platform {
    pub fn is_boot_completed() -> bool { true }
    pub fn read_boot_id() -> String { "current-boot".into() }
    pub fn process_start_time_ticks(_: i32) -> Option<u64> {
        Some(crate::PROCESS_START_TICKS.load(std::sync::atomic::Ordering::Relaxed))
    }
}
struct MediaHookState { stage: String, pid: i32, start_ticks: Option<u64>, boot_id: String }
const READY_STAGE: &str = "init_ok";
''' + 'mod module_paths { pub const MEDIA_HOOK_INSTALL_STATE_FILE: &str = ' + json.dumps(str(state)) + '; }\n'
            code += "\n".join(
                extract_fn(source, name)
                for source, name in (
                    (writer, "record_media_hook_install_state"),
                    (reader, "parse_install_state"),
                    (reader, "read_install_state"),
                    (reader, "is_hook_ready_for"),
                )
            )
            code += r'''
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "write" {
        record_media_hook_install_state(&args[2]);
        println!("{}", std::process::id());
    } else {
        let pid: i32 = args[1].parse().unwrap();
        let current_start_ticks: u64 = args[3].parse().unwrap();
        PROCESS_START_TICKS.store(current_start_ticks, std::sync::atomic::Ordering::Relaxed);
        let state = read_install_state(pid);
        assert_eq!(state.as_ref().unwrap().stage, args[2]);
        assert_eq!(
            is_hook_ready_for(state.as_ref(), pid, "current-boot"),
            args[4] == "ready"
        );
        assert!(!is_hook_ready_for(state.as_ref(), pid, "other-boot"));
        assert!(!is_hook_ready_for(state.as_ref(), pid + 1, "current-boot"));
    }
}
'''
            source, binary = path / "state.rs", path / "state"
            source.write_text(code, encoding="utf8")

            def run(*args: str) -> str:
                result = subprocess.run(args, capture_output=True, text=True, timeout=30)
                self.assertEqual(0, result.returncode, result.stdout + result.stderr)
                return result.stdout.strip()

            run(rustc, "--edition=2024", str(source), "-o", str(binary))
            first = run(str(binary), "write", "init_ok")
            second = run(str(binary), "write", "init_failed")
            self.assertNotEqual(first, second)
            run(str(binary), first, "init_ok", "42", "ready")
            run(str(binary), second, "init_failed", "42", "not-ready")
            second_path = Path(str(state) + "." + second)
            second_path.unlink()
            run(str(binary), second, "init_failed", "42", "not-ready")
            second_path.write_text(state.read_text(), encoding="utf8")
            state.write_text(state.read_text().replace("init_failed", "init_ok"), encoding="utf8")
            run(str(binary), second, "init_failed", "42", "not-ready")
            run(str(binary), first, "init_ok", "43", "not-ready")
            second_path.write_text(
                f"stage=init_ok pid={second} boot_id=current-boot boot_completed=true\n",
                encoding="utf8",
            )
            run(str(binary), second, "init_ok", "42", "not-ready")


if __name__ == "__main__":
    unittest.main()
