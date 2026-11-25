"""执行真实夹具扫描函数，防止残留或扫描失败被报告为空目录。"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")


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
                    prefix = f"""VISIBLE_DOWNLOAD_FIXTURE_PATHS="'{fixture.as_posix()}/Download/SrtDeep' '{fixture.as_posix()}/Download/Test'"
BACKEND_DOWNLOAD_FIXTURE_PATHS="$VISIBLE_DOWNLOAD_FIXTURE_PATHS"
"""
                    check = (ROOT / ".github/tests/check-public-fixtures.sh").read_text(encoding="utf-8")

                    if mode == "adb_error":
                        prefix += "adb_su_timeout() { return 7; }\n"
                    elif mode == "find_error":
                        prefix += 'adb_su_timeout() { find() { return 9; }; eval "$2"; }\n'
                    else:
                        prefix += 'adb_su_timeout() { eval "$2"; }\n'
                    result = subprocess.run([BASH, "-c", prefix + function + '\nassert_fixture_roots_empty regression'], env=dict(os.environ, PUBLIC_FIXTURE_CHECK=check), capture_output=True, text=True, encoding="utf-8", timeout=30)
                    self.assertEqual(mode == "empty", result.returncode == 0, result.stderr)
                    if mode == "residue":
                        self.assertIn("fixture_residue", result.stderr)
                    elif mode.endswith("error"):
                        self.assertIn("fixture_scan_failed", result.stderr)

    def test_collision_ignores_a_stale_stat_directory_entry(self):
        checker = (ROOT / ".github/tests/check-public-fixtures.sh").read_text(encoding="utf-8")
        mocks = '''stat() { echo directory; }
ls() {
  if [ "$2" = /visible ]; then printf '%s\\n' "$PARENT_ENTRY"; return "$PARENT_STATUS"; fi
  echo "ls: $2: $LOOKUP_ERROR" >&2; return 1
}
'''
        for mode in ("fixture_collision", "fixture_residue"):
            for entry, status, error, absent in (
                ("SrtDlOwnerMapped", 0, "No such file or directory", True),
                ("SrtDlOwner", 0, "No such file or directory", False),
                ("", 1, "No such file or directory", False),
                ("", 0, "Permission denied", False),
            ):
                with self.subTest(mode=mode, entry=entry, status=status, error=error):
                    result = subprocess.run(
                        [BASH, "-c", mocks + checker, "_", mode, "/visible/SrtDlOwner"],
                        env=dict(os.environ, PARENT_ENTRY=entry, PARENT_STATUS=str(status), LOOKUP_ERROR=error),
                        text=True, capture_output=True, timeout=10,
                    )
                    self.assertEqual(absent, result.returncode == 0, result.stderr)

    @unittest.skipUnless(os.name == "posix" and shutil.which("pwsh"), "需要 POSIX 文件系统和 PowerShell")
    def test_preexisting_directories_survive_cleanup_and_permission_changes(self):
        scripts = ROOT / ".github/tests"
        sh = (scripts / "run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        ps = (scripts / "run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8-sig")
        sh_helpers = sh[sh.index("initialize_public_fixture_paths() {"):sh.index("adb_root() {")]
        ps_helpers = ps[ps.index("function Initialize-PublicFixturePaths {"):ps.index("function Assert-FixtureRootsEmpty {")]
        sh_final = sh[sh.index("remove_test_target_artifacts() {"):sh.index("\nread_only_media_rows() {")]
        ps_final = ps[ps.index("function Remove-TestTargetArtifacts {"):ps.index("\nfunction Invoke-ReadOnlyMediaRows(")]
        sh_sweeps = [line for line in sh_final.splitlines() if "FIXTURE_PATHS" in line]
        ps_sweeps = [line for line in ps_final.splitlines() if "PublicFixturePaths" in line]
        sh_driver = """set -eu
REAL_ROOT=$1 BACKEND_ROOT=$2 SRT_MEDIA_PROVIDER_PID_SCRIPT=$3/media-provider-pid.sh
adb_su() { bash -c "$1"; }
""" + sh_helpers + """
initialize_public_fixture_paths
echo fixture_preflight_passed
for base in "$REAL_ROOT" "$BACKEND_ROOT"; do mkdir -p "$base/Download/SrtQqAliasMapped/deep"; touch "$base/Download/SrtQqAliasMapped/deep/payload"; done
fix_shared_fixture_permissions
""" + "\n".join(sh_sweeps)
        ps_driver = """param($RealRoot,$BackendRoot)
$ErrorActionPreference='Stop'
function Invoke-Su($command) { & bash -c $command }
function Test-Su($command) { & bash -c $command | Out-Null; $LASTEXITCODE -eq 0 }
""" + ps_helpers + """
Initialize-PublicFixturePaths
Write-Output 'fixture_preflight_passed'
foreach ($base in $RealRoot,$BackendRoot) {
    [IO.Directory]::CreateDirectory("$base/Download/SrtQqAliasMapped/deep") | Out-Null
    [IO.File]::WriteAllText("$base/Download/SrtQqAliasMapped/deep/payload", 'owned')
}
Fix-SharedFixturePermissions
""" + "\n".join(ps_sweeps) + "\nexit $LASTEXITCODE"
        def identity(path):
            stat = path.stat()
            return stat.st_dev, stat.st_ino, stat.st_uid, stat.st_gid, stat.st_mode
        for exe, ext, body, flags in ((BASH, "sh", sh_driver, []), (shutil.which("pwsh"), "ps1", ps_driver, ["-NoProfile", "-File"])):
            with tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                base.chmod(0o755)
                shutil.copyfile(scripts / "check-public-fixtures.sh", base / "check-public-fixtures.sh")
                shutil.copyfile(scripts / "public-fixture-directories.txt", base / "public-fixture-directories.txt")
                shutil.copyfile(scripts / "public-fixture-files.txt", base / "public-fixture-files.txt")
                driver = base / ("case." + ext)
                driver.write_text(body, encoding="utf-8")
                roots = [base / "visible", base / "backend"]
                protected = []
                for root in roots:
                    empty = root / "Download/SrtUnrelatedEmpty"
                    data = root / "Download/SrtUnrelatedContent/data"
                    empty.mkdir(mode=0o700, parents=True)
                    data.parent.mkdir(mode=0o700)
                    data.write_bytes(b"original")
                    data.chmod(0o600)
                    protected.extend((empty, data.parent, data))
                before = [identity(path) for path in protected]
                cmd = [exe, *flags, str(driver), *map(str, roots), *([str(base)] if ext == "sh" else [])]
                manifest = base / "public-fixture-directories.txt"
                original = manifest.read_bytes()
                for invalid in ("", "Download/../Pictures", "Download/SrtProbe\n\nDownload/Test"):
                    manifest.write_text(invalid, encoding="utf-8")
                    run = subprocess.run(cmd, text=True, capture_output=True, timeout=20)
                    self.assertNotEqual(0, run.returncode)
                    self.assertEqual(before, [identity(path) for path in protected])
                    self.assertTrue(all(not (p / "Download/SrtQqAliasMapped").exists() for p in roots))
                manifest.write_bytes(original)
                for root in roots:
                    collision = root / "Download/Test"
                    collision.mkdir(mode=0o710)
                    initial = identity(collision)
                    run = subprocess.run(cmd, text=True, capture_output=True, timeout=20)
                    self.assertNotEqual(0, run.returncode)
                    self.assertIn("fixture_collision", run.stderr)
                    self.assertEqual(initial, identity(collision))
                    self.assertTrue(all(not (p / "Download/SrtQqAliasMapped").exists() for p in roots))
                    parent_mode = collision.parent.stat().st_mode
                    collision.parent.chmod(0o600)
                    try:
                        # WSL 的 root 可绕过目录搜索权限；用普通 UID 触发真实的 EACCES。
                        credentials = dict(user=65534, group=65534, extra_groups=[]) if os.geteuid() == 0 else {}
                        run = subprocess.run(cmd, text=True, capture_output=True, timeout=30, **credentials)
                    finally:
                        collision.parent.chmod(parent_mode)
                    self.assertNotEqual(0, run.returncode)
                    self.assertIn("fixture_lookup_failed", run.stderr)
                    self.assertIn("Permission denied", run.stderr)
                    self.assertNotIn("fixture_preflight_passed", run.stdout)
                    self.assertEqual(initial, identity(collision))
                    collision.rmdir()
                    collision.symlink_to("missing-target")
                    run = subprocess.run(cmd, text=True, capture_output=True, timeout=20)
                    self.assertNotEqual(0, run.returncode)
                    self.assertIn(f"fixture_collision path={collision}", run.stderr)
                    self.assertEqual(Path("missing-target"), collision.readlink())
                    collision.unlink()
                    collision = root / "Download/srt_qmark_a.txt"
                    collision.write_bytes(b"original")
                    initial = identity(collision)
                    run = subprocess.run(cmd, text=True, capture_output=True, timeout=20)
                    self.assertNotEqual(0, run.returncode)
                    self.assertIn("fixture_collision", run.stderr)
                    self.assertEqual(initial, identity(collision))
                    self.assertEqual(b"original", collision.read_bytes())
                    collision.unlink()
                run = subprocess.run(cmd, text=True, capture_output=True, timeout=20)
                self.assertEqual(0, run.returncode, run.stdout + run.stderr)
                self.assertEqual(before, [identity(path) for path in protected])
                for root in roots:
                    self.assertEqual(b"original", (root / "Download/SrtUnrelatedContent/data").read_bytes())
                    self.assertFalse((root / "Download/SrtQqAliasMapped").exists())

    def test_powershell_scan_uses_same_remote_command(self):
        sh = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        ps = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        sh = sh[sh.index("assert_fixture_roots_empty() {"):sh.index("\nclean_results() {")]
        ps = ps[ps.index("function Assert-FixtureRootsEmpty {"):ps.index("function Clear-Targets {")]
        sh_command = re.search(r'adb_su_timeout 45 "(.*)"\)', sh).group(1).replace('\\"', '"').replace('\\$', '$').replace('$VISIBLE_DOWNLOAD_FIXTURE_PATHS', 'ROOT')
        ps_command = re.search(r'\$command = "(.*)"', ps).group(1).replace('`"', '"').replace('`$', '$').replace('$($script:DownloadFixturePaths[$RealRoot])', 'ROOT')
        self.assertEqual(sh_command.replace("$PUBLIC_FIXTURE_CHECK", "CHECK"), ps_command.replace("$script:PublicFixtureCheck", "CHECK"))
        self.assertIn('$LASTEXITCODE -ne 0', ps)
        self.assertIn('fixture_scan_failed', ps)


if __name__ == "__main__":
    unittest.main()
