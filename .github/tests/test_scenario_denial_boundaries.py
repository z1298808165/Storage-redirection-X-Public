import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import pwsh_case_driver


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")
PWSH = shutil.which("pwsh")


class DenialBoundaryTest(unittest.TestCase):
    def test_owner_drift_checks_layout_errno_contents_and_residue(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (
                ("run_owner_drift_own_dir_scenario() {", "# 读取沙盒根") if runner == "bash" else
                ("function Invoke-OwnerDriftOwnDirScenario {", "function Set-ReadOnlySeed {")
            )
            function = source[source.index(start):source.index(end)]
            combinations = [
                (layout, fault)
                for layout in ("bind", "fuse", "mixed")
                for fault in ("none", "EIO", "ENOENT", "errno-in-path", "false-success",
                              "changed-owner", "changed-content", "residue")
            ]
            workspace = tempfile.TemporaryDirectory()
            self.addCleanup(workspace.cleanup)
            root = Path(workspace.name)
            results = {}
            if runner == "bash":
                stub = r'''
APP_ID=test.app
BACKEND_ROOT=/data/media/0
PAYLOAD=payload
app_pid() { echo 123; }
app_view_dev_differs() { [[ $SRX_DRIFT_LAYOUT != bind ]]; }
adb_su() {
  case "$1" in
    *grep*) [[ $SRX_DRIFT_LAYOUT == mixed ]] ;;
    *"stat -c"*) [[ $SRX_DRIFT_FAULT != changed-owner && $SRX_DRIFT_FAULT != changed-content ]] ;;
    *"test ! -e"*) [[ $SRX_DRIFT_FAULT != residue ]] ;;
    *) return 9 ;;
  esac
}
run_service_case() {
  local pattern=$4 test_case=$3
  local raw="PASS [$test_case] operation succeeded"
  if [[ $SRX_DRIFT_LAYOUT != fuse && $SRX_DRIFT_FAULT != false-success ]]; then
    raw="FAIL [$test_case] operation failed error=/file: open failed: EACCES (Permission denied)"
  fi
  if [[ $SRX_DRIFT_FAULT == EIO || $SRX_DRIFT_FAULT == ENOENT ]]; then
    raw="FAIL [$test_case] operation failed error=/file/EACCES: open failed: $SRX_DRIFT_FAULT (Permission denied)"
  fi
  if [[ $SRX_DRIFT_FAULT == errno-in-path ]]; then
    raw="FAIL [$test_case] operation failed error=/file/open failed: EACCES (directory)/file: open failed: EIO"
  fi
  if [[ $test_case == file_read && $* != *expected_payload*srt-drift-payload* ]]; then return 8; fi
  [[ $raw =~ $pattern ]]
}
'''
                for index, (layout, fault) in enumerate(combinations):
                    base = root / f"case-{index}"
                    base.mkdir()
                    (base / f"drift.{extension}").write_text(
                        stub + function + "\nrun_owner_drift_own_dir_scenario 6\n",
                        encoding="utf-8", newline="\n")
                    results[f"{layout}|{fault}"] = subprocess.run(
                        [BASH, "-eu", str(base / "drift.sh")],
                        text=True, capture_output=True, timeout=15,
                        env={**os.environ, "SRX_DRIFT_LAYOUT": layout, "SRX_DRIFT_FAULT": fault})
            else:
                stub = r'''
$ErrorActionPreference = 'Stop'
$AppId='test.app'
$RealRoot='/storage/emulated/0'
$BackendRoot='/data/media/0'
$Payload='payload'
$script:Failures=[System.Collections.Generic.List[string]]::new()
function Get-AppPid { '123' }
function Test-Su {
    param([string]$Command)
    if ($Command.StartsWith('grep ')) { return $env:SRX_DRIFT_LAYOUT -eq 'mixed' }
    if ($Command.StartsWith('a=')) { return $env:SRX_DRIFT_LAYOUT -ne 'bind' }
    if ($Command.Contains('stat -c')) { return $env:SRX_DRIFT_FAULT -notin @('changed-owner', 'changed-content') }
    if ($Command.StartsWith('test ! -e')) { return $env:SRX_DRIFT_FAULT -ne 'residue' }
    throw "unexpected physical check: $Command"
}
function Invoke-ServiceCase {
    param($Scenario, $Label, $TestCase, $Extras, $PassRegex)
    $raw="PASS [$TestCase] operation succeeded"
    if ($env:SRX_DRIFT_LAYOUT -ne 'fuse' -and $env:SRX_DRIFT_FAULT -ne 'false-success') {
        $raw="FAIL [$TestCase] operation failed error=/file: open failed: EACCES (Permission denied)"
    }
    if ($env:SRX_DRIFT_FAULT -in @('EIO', 'ENOENT')) {
        $raw="FAIL [$TestCase] operation failed error=/file/EACCES: open failed: $env:SRX_DRIFT_FAULT (Permission denied)"
    }
    if ($env:SRX_DRIFT_FAULT -eq 'errno-in-path') {
        $raw="FAIL [$TestCase] operation failed error=/file/open failed: EACCES (directory)/file: open failed: EIO"
    }
    if ($TestCase -eq 'file_read' -and $Extras.expected_payload -ne 'srt-drift-payload') { throw 'missing content assertion' }
    [pscustomobject]@{ Ok = $raw -match $PassRegex; Text = $raw }
}
'''
                entries = []
                for index, (layout, fault) in enumerate(combinations):
                    base = root / f"case-{index}"
                    base.mkdir()
                    (base / f"drift.{extension}").write_text(
                        stub + function + "\nif (-not (Invoke-OwnerDriftOwnDirScenario 6)) { $global:SRX_CASE_RC = 1 }\n",
                        encoding="utf-8", newline="\n")
                    entries.append({
                        "name": f"{layout}|{fault}",
                        "script": str(base / "drift.ps1"),
                        "env": {"SRX_DRIFT_LAYOUT": layout, "SRX_DRIFT_FAULT": fault},
                    })
                batch = pwsh_case_driver.run_pwsh_cases(entries, PWSH)
                results = dict(batch)

            for layout, fault in combinations:
                with self.subTest(runner=runner, layout=layout, fault=fault):
                    result = results[f"{layout}|{fault}"]
                    passed = fault == "none" or (layout == "fuse" and fault in ("false-success", "residue"))
                    self.assertEqual(passed, result.returncode == 0, result.stdout + result.stderr)
