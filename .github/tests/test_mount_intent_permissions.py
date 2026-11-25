"""执行生产意图写入代码，验证宽松 umask 与遗留临时文件不会破坏可信读取。"""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_mount_identity_syntax import ROOT, extract_fn


@unittest.skipUnless(os.name == "posix", "需要 Unix 文件权限")
class MountIntentPermissionsTest(unittest.TestCase):
    def test_every_state_remains_private_independent_of_umask(self):
        source = (ROOT / "src/mount_intent.rs").read_text(encoding="utf8")
        code = r'''
use std::{fs, path::PathBuf};
use std::os::unix::fs::PermissionsExt;
extern crate self as log;
#[macro_export] macro_rules! debug { ($($t:tt)*) => {}; }
#[macro_export] macro_rules! warn { ($($t:tt)*) => {}; }
enum StorageBackendMode { Auto }
impl StorageBackendMode { fn as_str(&self) -> &str { "auto" } }
mod module_paths {
    pub const MOUNT_INTENT_DIR: &str = "intents";
    pub fn sanitize_name(name: &str) -> String { name.to_owned() }
}
mod platform { pub fn process_start_time_ticks(_: i32) -> Option<u64> { Some(456) } }
// __WRITER__
fn main() {
    fs::create_dir_all("intents").unwrap();
    let temp = "intents/com.example.app_123.intent.tmp";
    let path = "intents/com.example.app_123.intent";
    for state in ["native_registered", "applying", "applied", "failed"] {
        if state != "native_registered" {
            fs::write(temp, "stale").unwrap();
            fs::set_permissions(temp, fs::Permissions::from_mode(0o666)).unwrap();
        }
        write_state("com.example.app", 123, 10123, StorageBackendMode::Auto, 1, state);
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains(&format!("state={state}\n")));
        assert!(content.contains("app_start_time=456\n"));
        assert!(!std::path::Path::new(temp).exists());
    }
}
'''.replace("// __WRITER__", extract_fn(source, "write_state"))
        with tempfile.TemporaryDirectory(prefix="srx-intent-permissions-") as directory:
            directory = Path(directory)
            rust = directory / "main.rs"
            binary = directory / "intent"
            rust.write_text(code, encoding="utf8")
            compiled = subprocess.run([shutil.which("rustc"), "--edition=2024", str(rust), "-o", str(binary)], capture_output=True, text=True, timeout=30)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            for mask in (0, 0o022, 0o077):
                with self.subTest(umask=oct(mask)):
                    result = subprocess.run([str(binary)], cwd=directory, preexec_fn=lambda: os.umask(mask), capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
