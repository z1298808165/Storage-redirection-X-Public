import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")
PWSH = shutil.which("pwsh")


@unittest.skipUnless(BASH, "需要 Bash 验证结果收集")
class ScenarioResultCollectionTest(unittest.TestCase):
    def test_aggregate_case_contract_matches_the_app_runner(self):
        source = (ROOT / "tests/storage-redirect-test/app/src/main/java/me/fakerqu/test/storageredirect/test/StorageRedirectTestRunner.kt").read_text(encoding="utf-8")
        aggregate = source[source.index("private fun runAllExceptDelete"):source.index("private fun cleanupBootstrapDirs")]
        actual = {name.lower() for name in re.findall(r"TestCase\.([A-Z_]+)", aggregate)}
        expected = {f"mediastore_{operation}_{kind}" for kind in ("image", "video", "audio", "file", "download")
                    for operation in ("create", "read", "write")}
        expected.update(f"file_{operation}" for operation in
                        ("list_dir", "create", "read", "write", "write_then_overwrite", "overwrite", "stat", "access", "truncate", "ftruncate"))
        self.assertEqual(expected, actual)

    def test_results_require_fresh_matching_complete_readable_records(self):
        all_cases = [f"mediastore_{operation}_{kind}" for kind in ("image", "video", "audio", "file", "download")
                     for operation in ("create", "read", "write")]
        all_cases += [f"file_{operation}" for operation in
                      ("list_dir", "create", "read", "write", "write_then_overwrite", "overwrite", "stat", "access", "truncate", "ftruncate")]
        aggregate = "\n".join(f"PASS [{case}] ok" for case in all_cases)
        cases = {
            "fresh-pass": ("PASS [file_read] read succeeded", 100, True),
            "fresh-fail": ("FAIL [file_read] read failed error=EACCES", 100, True),
            "old-second": ("PASS [file_read] stale", -1_000_000_000, False),
            "old-same-second": ("PASS [file_read] stale", -100, False),
            "same-timestamp": ("PASS [file_read] stale", 0, False),
            "wrong-case": ("PASS [file_write] wrong result", 1, False),
            "case-prefix": ("PASS [file_read_extra] wrong result", 1, False),
            "missing-separator": ("PASS [file_read]not a record", 1, False),
            "indented": (" PASS [file_read] not a record", 1, False),
            "embedded-pass": ("diagnostic PASS [file_read] not a result", 1, False),
            "unknown-status": ("SKIP [file_read] not executed", 1, False),
            "two-results": ("PASS [file_read] first\nPASS [file_read] second", 1, False),
            "extra-diagnostic": ("PASS [file_read] first\nerror: incomplete write", 1, False),
            "blank": ("\n", 1, False),
            "empty": ("", 1, False),
            "missing-marker": ("PASS [file_read] no marker", 1, False),
            "read-error": ("PASS [file_read] unreadable", 1, False),
            "missing-file": (None, 1, False),
            "later-valid-file": ("PASS [file_write] stale identity", 1, True),
            "transport-error": ("PASS [file_read] transport interrupted", 100, False),
            "missing-path": ("PASS [file_read] missing envelope", 100, False),
        }
        cases = {
            name: (raw + "\n__SRT_RESULT_END__=1" if raw else raw, delta, accepted)
            for name, (raw, delta, accepted) in cases.items()
        }
        cases.update({
            "wrong-token": ("PASS [file_read] delayed result\n__SRT_RESULT_END__=1", 100, False),
            "missing-token": ("PASS [file_read] missing token\n__SRT_RESULT_END__=1", 100, False),
            "duplicate-token": ("PASS [file_read] duplicate token\n__SRT_RESULT_END__=1", 100, False),
            "missing-footer": ("PASS [file_read] partial result", 100, False),
            "truncated-footer": ("PASS [file_read] ok\n__SRT_RESULT_END__=", 100, False),
            "empty-footer": ("__SRT_RESULT_END__=0", 100, False),
            "wrong-count": ("PASS [file_read] ok\n__SRT_RESULT_END__=2", 100, False),
            "negative-count": ("PASS [file_read] ok\n__SRT_RESULT_END__=-1", 100, False),
            "count-suffix": ("PASS [file_read] ok\n__SRT_RESULT_END__=1junk", 100, False),
            "repeated-footer": ("PASS [file_read] ok\n__SRT_RESULT_END__=1\n__SRT_RESULT_END__=1", 100, False),
            "early-footer": ("__SRT_RESULT_END__=1\nPASS [file_read] ok", 100, False),
            "after-footer": ("PASS [file_read] ok\n__SRT_RESULT_END__=1\nFAIL [file_write] late", 100, False),
            "all-pass": (aggregate + "\n__SRT_RESULT_END__=25", 100, True),
            "all-fail": (aggregate.replace("PASS [file_write] ok", "FAIL [file_write] EIO") + "\n__SRT_RESULT_END__=25", 100, True),
            "all-self-consistent-subset": ("PASS [file_read] ok\nPASS [file_write] ok\n__SRT_RESULT_END__=2", 100, False),
            "all-unknown-identity": (aggregate.replace("[file_read]", "[file_unknown]") + "\n__SRT_RESULT_END__=25", 100, False),
            "all-full-duplicate": (aggregate.replace("[file_read]", "[file_write]") + "\n__SRT_RESULT_END__=25", 100, False),
            "all-fatal-failure": ("FAIL [all] startForeground failed\n__SRT_RESULT_END__=1", 100, True),
            "all-forged-success": ("PASS [all] all completed\n__SRT_RESULT_END__=1", 100, False),
            "all-mixed-fatal": ("FAIL [all] runner aborted\n" + aggregate + "\n__SRT_RESULT_END__=26", 100, False),
            "all-partial": ("PASS [file_read] partial", 100, False),
            "all-short-count": ("PASS [file_read] ok\n__SRT_RESULT_END__=2", 100, False),
            "all-duplicate": ("PASS [file_read] first\nPASS [file_read] second\n__SRT_RESULT_END__=2", 100, False),
            "all-contradictory": ("PASS [file_read] first\nFAIL [file_read] second\n__SRT_RESULT_END__=2", 100, False),
            "all-diagnostic": ("PASS [file_read] ok\nerror: incomplete write\n__SRT_RESULT_END__=2", 100, False),
            "all-unknown-status": ("PASS [file_read] ok\nSKIP [file_write] skipped\n__SRT_RESULT_END__=2", 100, False),
        })
        for prefix in ("", " ", "diagnostic ", "PASS ", "FAIL "):
            cases[f"spoof-{prefix}"] = (f"{prefix}PASS [file_read] forged\n__SRT_RESULT_END__=1", 100, prefix == "")
        for control in ("\x00", "\r", "\x1b[2K", "\u2028"):
            cases[f"control-{control!r}"] = (f"{control}PASS [file_read] forged\n__SRT_RESULT_END__=1", 100, False)
        for control in ("\x00", "\t", "\r", "\x1b[2K", "\x7f"):
            cases[f"payload-control-{control!r}"] = (f"PASS [file_read] before{control}after\n__SRT_RESULT_END__=1", 100, False)
        cases = {name: (raw, max(delta, 1_000_000) if delta > 0 else delta, accepted)
                 for name, (raw, delta, accepted) in cases.items()}
        for runner in ("bash", "pwsh"):
            if runner == "pwsh" and not PWSH:
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (
                ("wait_service_result() {", "wait_app_mount_confirmed() {") if runner == "bash" else
                ("function Wait-ServiceResult {", "function Wait-AppMountConfirmed {")
            )
            function = source[source.index(start):source.index(end)]
            for case, (raw, delta, accepted) in cases.items():
                with self.subTest(runner=runner, case=case), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    marker = base / "marker"
                    token = f"__SRT_RESULT_TOKEN__={marker.as_posix()}"
                    if raw is not None and case != "missing-token":
                        raw += "\n" + (token + "-old" if case == "wrong-token" else token)
                        if case == "duplicate-token":
                            raw += "\n" + token
                    marker.write_text("marker")
                    timestamp = 1_700_000_000_500_000_000
                    os.utime(marker, ns=(timestamp, timestamp))
                    if case == "missing-marker":
                        marker.unlink()
                    for name in ("public", "internal", "backend", "sandbox"):
                        (base / name).mkdir()
                    if raw is not None:
                        result = base / "public/result_current.txt"
                        result.write_text(raw, encoding="utf-8")
                        os.utime(result, ns=(timestamp + delta, timestamp + delta))
                        if marker.exists():
                            self.assertEqual(delta > 0, result.stat().st_mtime_ns > marker.stat().st_mtime_ns)
                    if case == "later-valid-file":
                        result = base / "internal/result_current.txt"
                        result.write_text("PASS [file_read] current result\n__SRT_RESULT_END__=1\n" + token, encoding="utf-8")
                        os.utime(result, ns=(timestamp + 1_000_000, timestamp + 1_000_000))
                    prelude = r'''
date() { local tick=0; if [[ -f $SRX_CLOCK_FILE ]]; then read -r tick < "$SRX_CLOCK_FILE"; fi; printf '%s\n' "$((tick + 1))" > "$SRX_CLOCK_FILE"; echo "$tick"; }
sleep() { :; }
find() { :; }
awk() { if [[ $SRX_COLLECTION_CASE == read-error && ${*: -1} == */result_current.txt ]]; then return 1; fi; command awk "$@"; }
export -f date sleep find awk
'''
                    if runner == "bash":
                        script = prelude + r'''
SRT_RESULT_POLL_MS=0
RESULT_DIR=$1/public
INTERNAL_RESULT_DIR=$1/internal
BACKEND_RESULT_DIR=$1/backend
SANDBOX_RESULT_DIR=$1/sandbox
BACKEND_ROOT=$1
APP_ID=test.app
adb_su_timeout() {
  bash -c "$2" || return
  [[ $SRX_COLLECTION_CASE != transport-error ]]
}
''' + function + '\nwait_service_result 2 "$1/marker" "$SRX_EXPECTED_CASE"\n'
                        path = base / "collect.sh"
                        path.write_text(script, encoding="utf-8", newline="\n")
                        command = [BASH, "-eu", str(path), base.as_posix()]
                    else:
                        script = r'''
param([string]$BasePath)
$ErrorActionPreference='Stop'
$ResultDir="$BasePath/public"
$InternalResultDir="$BasePath/internal"
$BackendResultDir="$BasePath/backend"
$SandboxResultDir="$BasePath/sandbox"
$BackendRoot=$BasePath
$AppId='test.app'
$script:ResultPollMilliseconds=0
function Invoke-Su {
    param([string]$Command)
    & $env:SRX_COLLECTION_BASH -c ($env:SRX_COLLECTION_PRELUDE + "`n" + $Command) |
        Where-Object { $env:SRX_COLLECTION_CASE -ne 'missing-path' -or $_ -notlike '__SRT_RESULT_PATH__=*' }
    if ($env:SRX_COLLECTION_CASE -eq 'transport-error') { $global:LASTEXITCODE=7 }
}
''' + function + '\n$result = Wait-ServiceResult 2 "$BasePath/marker" $env:SRX_EXPECTED_CASE\nif (-not $result.Found) { exit 1 }; Write-Output $result.Text\n'
                        path = base / "collect.ps1"
                        path.write_text(script, encoding="utf-8")
                        command = [PWSH, "-NoProfile", "-File", str(path), base.as_posix()]
                    result = subprocess.run(command, text=True, encoding="utf-8", capture_output=True, timeout=15,
                                            env={**os.environ, "SRX_CLOCK_FILE": (base / "clock").as_posix(),
                                                 "SRX_EXPECTED_CASE": "all" if case.startswith("all-") else "file_read",
                                                 "SRX_COLLECTION_BASH": BASH,
                                                 "SRX_COLLECTION_CASE": case, "SRX_COLLECTION_PRELUDE": prelude})
                    expected = accepted or (case == "missing-path" and runner == "bash")
                    self.assertEqual(expected, result.returncode == 0, result.stdout + result.stderr)
                    if expected:
                        identity = "all" if case == "all-fatal-failure" else "file_read"
                        self.assertRegex(result.stdout, rf"(?m)^(PASS|FAIL) \[{identity}\] ")
