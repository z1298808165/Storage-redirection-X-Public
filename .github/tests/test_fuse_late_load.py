"""用虚拟时钟和排队器执行生产晚加载重试，验证退避、合并及排队失败。"""

import ctypes
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from test_mount_identity_syntax import extract_fn

ROOT = Path(__file__).resolve().parents[2]


class FuseLateLoadTest(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux", "需要 Linux 动态库和管道")
    def test_loaded_checked_read_preserves_bounds_and_complete_request_identity(self) -> None:
        read = (ROOT / "src/hook/ops/read.rs").read_text(encoding="utf8")
        start = read.index('unsafe extern "C" {\n    fn __read_chk(')
        wrappers = read[start:read.index("// 借用 FUSE 协议头", start)]
        wrappers = wrappers.replace('unsafe extern "C" fn hooked_read_chk_fuse', '#[unsafe(no_mangle)]\npub unsafe extern "C" fn hooked_read_chk_fuse')
        fixture = '''use std::ffi::{c_int, c_void};
mod context {
    use std::cell::Cell;
    thread_local! { pub static CALLER: Cell<(i32, i32)> = const { Cell::new((0, 0)) }; }
    pub fn set_fuse_caller_uid(uid: i32) { CALLER.update(|(_, pid)| (uid, pid)); }
    pub fn set_fuse_caller_pid(pid: i32) { CALLER.update(|(uid, _)| (uid, pid)); }
}
#[unsafe(no_mangle)] pub extern "C" fn caller() -> u64 {
    let (uid, pid) = context::CALLER.get(); ((uid as u64) << 32) | pid as u64
}
'''
        with tempfile.TemporaryDirectory(prefix="srx-checked-read-") as directory:
            source, library = Path(directory, "engine.rs"), Path(directory, "engine.so")
            source.write_text(fixture + wrappers, encoding="utf8")
            subprocess.run(["rustc", "--edition=2024", "--crate-type=cdylib", str(source), "-o", str(library)], check=True, capture_output=True)
            engine = ctypes.CDLL(str(library))
            checked_read = engine.hooked_read_chk_fuse
            checked_read.argtypes = (ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t)
            checked_read.restype = ctypes.c_ssize_t
            engine.caller.restype = ctypes.c_uint64
            header = struct.pack("=IIQQIIII", 40, 1, 13, 2, 10192, 10192, 782, 0)
            read_fd, write_fd = os.pipe()
            try:
                buffer = ctypes.create_string_buffer(len(header))
                for payload in (header[:8], header):
                    os.write(write_fd, payload)
                    self.assertEqual(checked_read(read_fd, buffer, len(header), len(buffer)), len(payload))
                    self.assertEqual(buffer.raw[:len(payload)], payload)
                    self.assertEqual(engine.caller(), (10192 << 32 | 782) if payload == header else 0)
            finally:
                os.close(read_fd)
                os.close(write_fd)

            overflow = subprocess.run(
                [
                    sys.executable,
                    "-c",
                    """import ctypes, resource, sys
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
engine = ctypes.CDLL(sys.argv[1])
checked_read = engine.hooked_read_chk_fuse
checked_read.argtypes = (ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t)
buffer = ctypes.create_string_buffer(1)
checked_read(-1, buffer, 2, 1)
""",
                    str(library),
                ],
                capture_output=True,
                text=True,
                cwd=directory,
                timeout=10,
            )
            self.assertEqual(overflow.returncode, -signal.SIGABRT, overflow.stderr)

    def test_profiles_and_retry_events(self) -> None:
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc)
        read = (ROOT / "src/hook/ops/read.rs").read_text(encoding="utf8")
        native = (ROOT / "src/hook/fuse_fix.rs").read_text(encoding="utf8")
        entries = (ROOT / "src/hook/entries.rs").read_text(encoding="utf8")
        definitions = entries[: entries.index("pub struct HookEntry")]
        state = read[read.index("static PATCH_NEXT_ATTEMPT_MS") : read.index("/// 有库刚被")]
        events = "\n".join(
            extract_fn(read, name)
            for name in ("retry_after_late_load", "apply_late_load_patches", "reset_patch_backoff", "patch_backoff")
        ).replace("std::thread::Builder", "scheduler::Builder").replace("std::thread::sleep", "scheduler::sleep")
        native_events = "pub(crate) " + extract_fn(native, "retry_after_late_load") + "\n" + extract_fn(native, "retry_if_target_enabled")
        template = (ROOT / ".github/tests/harness/fuse_late_load.rs").read_text(encoding="utf8")
        code = template.replace("// __DEFINITIONS__", definitions).replace("// __STATE__", state).replace("// __EVENTS__", events).replace("// __NATIVE_EVENTS__", native_events)
        with tempfile.TemporaryDirectory(prefix="srx-late-load-") as tmp:
            source, binary = Path(tmp) / "main.rs", Path(tmp) / "late-load"
            source.write_text(code, encoding="utf8")
            for cmd in ([rustc, "--edition=2024", str(source), "-o", str(binary)], [str(binary)]):
                result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_successful_dynamic_load_hooks_trigger_fuse_retry(self) -> None:
        source = (ROOT / "src/hook/ops/dlopen.rs").read_text(encoding="utf8")
        for name, event in (
            ("hooked_dlopen", "dlopen"),
            ("hooked_android_dlopen_ext", "android_dlopen_ext"),
        ):
            with self.subTest(hook=name):
                body = extract_fn(source, name)
                _, found, after_success = body.partition("if !result.is_null()")
                self.assertTrue(found, f"{name} must gate late-load work on success")
                success_block, closed, _ = after_success.partition("}")
                self.assertTrue(closed, f"{name} success block is incomplete")
                refresh = f'hub.refresh_hooks_after_late_load("{event}")'
                retry = "retry_fuse_engine_patch_after_late_load(hub)"
                self.assertIn(refresh, success_block)
                self.assertIn(retry, success_block)
                self.assertLess(success_block.index(refresh), success_block.index(retry))

        retry = extract_fn(source, "retry_fuse_engine_patch_after_late_load")
        self.assertIn("is_media_provider_package", retry)
        self.assertIn("super::read::retry_after_late_load()", retry)


if __name__ == "__main__":
    unittest.main()
