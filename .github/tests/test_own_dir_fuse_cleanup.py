"""执行真实夹具扫描函数，防止残留或扫描失败被报告为空目录。"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASH = shutil.which("bash")


@unittest.skipUnless(BASH, "需要 Bash 执行夹具扫描回归")
class FixtureScanTest(unittest.TestCase):
    def test_scan_results(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        function = source[source.index("assert_fixture_roots_empty() {"):source.index("\nclean_results() {")]
        with tempfile.TemporaryDirectory(dir=ROOT / "temp" if (ROOT / "temp").exists() else ROOT) as directory:
            fixture = Path(directory)
            deep = fixture / "Download/SrtDeep/a/b/c"
            deep.mkdir(parents=True)
            # Test 目录故意不存在，缺失的可选根不应让远端循环返回失败。
            for mode in ("empty", "residue", "adb_error", "find_error"):
                with self.subTest(mode=mode):
                    payload = deep / "payload"
                    if mode == "residue":
                        payload.write_text("fixture", encoding="utf-8")
                    elif payload.exists():
                        payload.unlink()
                    prefix = f"REAL_ROOT='{fixture.as_posix()}'\n"
                    if mode == "adb_error":
                        prefix += "adb_su_timeout() { return 7; }\n"
                    elif mode == "find_error":
                        prefix += 'adb_su_timeout() { find() { return 9; }; eval "$2"; }\n'
                    else:
                        prefix += 'adb_su_timeout() { eval "$2"; }\n'
                    result = subprocess.run([BASH, "-c", prefix + function + '\nassert_fixture_roots_empty regression'], capture_output=True, text=True, encoding="utf-8", timeout=30)
                    self.assertEqual(mode == "empty", result.returncode == 0, result.stderr)
                    if mode == "residue":
                        self.assertIn("fixture_residue", result.stderr)
                    elif mode.endswith("error"):
                        self.assertIn("fixture_scan_failed", result.stderr)

    def test_powershell_scan_uses_same_remote_command(self):
        sh = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        ps = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        sh = sh[sh.index("assert_fixture_roots_empty() {"):sh.index("\nclean_results() {")]
        ps = ps[ps.index("function Assert-FixtureRootsEmpty {"):ps.index("function Clear-Targets {")]
        sh_command = re.search(r'adb_su_timeout 45 "(.*)"\)', sh).group(1).replace('\\"', '"').replace('\\$', '$').replace('${REAL_ROOT}', 'ROOT')
        ps_command = re.search(r'\$command = "(.*)"', ps).group(1).replace('`"', '"').replace('`$', '$').replace('$RealRoot', 'ROOT')
        self.assertEqual(sh_command, ps_command)
        self.assertIn('$LASTEXITCODE -ne 0', ps)
        self.assertIn('fixture_scan_failed', ps)


if __name__ == "__main__":
    unittest.main()
