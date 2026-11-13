import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASH = shutil.which("bash")


@unittest.skipUnless(BASH and os.name == "posix", "需要 POSIX Bash 验证 native 测试编排")
class NativeTestRunnerTest(unittest.TestCase):
    def test_build_uses_reported_artifacts_and_rejects_incomplete_sets(self):
        for missing in ("", "srx_daemon"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for name in ("srx_core", "srx_daemon", "zn-runtime-fixture"):
                    (root / name).write_bytes(name.encode())
                output = root / "output"
                output.mkdir()
                (output / "srx_daemon-tests").write_bytes(b"stale")
                cargo = root / "cargo"
                cargo.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
for name in (("srx_core", "srx_daemon") if sys.argv[1] == "test" else ("zn-runtime-fixture",)):
    if name == os.environ["SRX_NATIVE_MISSING"]:
        continue
    print(json.dumps({"reason": "compiler-artifact", "executable": str(Path(os.environ["SRX_NATIVE_BIN"]) / name),
                      "target": {"name": name}, "profile": {"test": sys.argv[1] == "test"}}))
''')
                cargo.chmod(0o755)
                result = subprocess.run(
                    [BASH, ".github/scripts/build_native_tests.sh", "x86_64-linux-android", str(output)],
                    cwd=ROOT, capture_output=True, text=True, timeout=15,
                    env={**os.environ, "PATH": f"{root}:{os.environ['PATH']}",
                         "SRX_NATIVE_BIN": str(root), "SRX_NATIVE_MISSING": missing},
                )
                self.assertEqual(not missing, result.returncode == 0, result.stdout + result.stderr)
                if missing:
                    self.assertEqual(b"stale", (output / "srx_daemon-tests").read_bytes())
                else:
                    for name in ("srx_core", "srx_daemon", "zn-runtime-fixture"):
                        target = name if name == "zn-runtime-fixture" else name + "-tests"
                        self.assertEqual(name.encode(), (output / target).read_bytes())

    def test_failures_preserve_exit_status_and_always_attempt_cleanup(self):
        for stage, expected in (("none", 0), ("push", 7), ("test", 8), ("cleanup", 1)):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                adb = root / "adb"
                adb.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$SRX_NATIVE_TRACE"
case "$1 $2" in
  'shell mktemp') echo /data/local/tmp/srx-native-tests.fixture ;;
  'push '*) [ "$SRX_NATIVE_FAILURE" != push ] || exit 7 ;;
  'shell su '*) [ "$SRX_NATIVE_FAILURE" != test ] || exit 8 ;;
  'shell rm') [ "$SRX_NATIVE_FAILURE" != cleanup ] || exit 9 ;;
esac
''')
                adb.chmod(0o755)
                trace = root / "trace"
                result = subprocess.run(
                    [BASH, ".github/tests/run-native-tests.sh", str(root), "x86_64"],
                    cwd=ROOT, capture_output=True, text=True, timeout=15,
                    env={**os.environ, "PATH": f"{root}:{os.environ['PATH']}",
                         "SRX_NATIVE_TRACE": str(trace), "SRX_NATIVE_FAILURE": stage},
                )
                self.assertEqual(expected, result.returncode, result.stdout + result.stderr)
                self.assertEqual("shell rm -rf /data/local/tmp/srx-native-tests.fixture", trace.read_text().splitlines()[-1])
                self.assertEqual(stage == "cleanup", "FAIL [native_cleanup]" in result.stderr)
