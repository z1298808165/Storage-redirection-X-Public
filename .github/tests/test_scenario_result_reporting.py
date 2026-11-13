import os
import itertools
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import pwsh_case_driver


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")
PWSH = shutil.which("pwsh")


def powershell_case_script(script, tail):
    """把抽取函数与尾部断言包成 runspace 用例脚本。

    `pwsh -File` 每用例起一个进程约 0.75s，其中大头是重复解析编译被抽取的函数体
    （实测 216 个用例合计重复解析约 1.2MB PowerShell 源码）。改成单进程 + 每用例一个
    runspace 后约 0.03s/case，且 runspace 各自持有独立 `$script:`/`$global:` 作用域，
    等价于原来的进程级隔离。语义差异与约束见 `pwsh_case_driver` 模块文档。

    尾部断言**不得使用 `exit`**：runspace 里 `exit` 会被静默吞掉（不抛异常、不改退出码），
    会让用例假通过。改用 `$global:SRX_CASE_RC = <退出码>`。
    """
    return script + "\n" + tail + "\n"


SERVICE_CASE_PWSH_STUBS = r'''
param([string]$ArtifactDir)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$ErrorActionPreference = 'Stop'
$script:ArtifactDir = $ArtifactDir
$script:Failures = [System.Collections.Generic.List[string]]::new()
$script:ServiceCaseSettleMilliseconds = 0
$script:FailFast = $env:SRX_REPORT_FAULT -eq 'fail-fast'
function Prepare-ServiceCase { if ($env:SRX_REPORT_FAULT -eq 'prepare') { $script:Failures.Add('prepare failed') } }
function Clear-Results {}
function Invoke-Su {
    param([string]$Command)
    $global:LASTEXITCODE = if ($env:SRX_REPORT_FAULT -eq 'marker' -and $Command.StartsWith('touch ')) { 1 } else { 0 }
}
function Invoke-Adb {}
function Invoke-AdbWithTimeout {
    param($Arguments)
    if ($Arguments[-3] -ne '--es' -or $Arguments[-2] -ne 'result_token' -or $Arguments[-1] -notlike '/data/local/tmp/srx-result-*.marker') {
        throw 'missing invocation token'
    }
    [pscustomobject]@{ ExitCode = [int]$env:SRX_REPORT_START_EXIT; Output = 'cmp=stub' }
}
function Ensure-CurrentAppMountConfirmed { $env:SRX_REPORT_FAULT -ne 'mount' }
function Get-ExpectedMountPathsForLabel { @('/expected/mount') }
function Get-ServiceCaseTimeoutSeconds { 0 }
function Stop-AppAndWaitFuseCleanup {}
function Start-Sleep {}
function Test-LabelExpectsMount { $env:SRX_REPORT_FAULT -eq 'mount' }
function Wait-Storage {}
function Wait-ServiceResult {
    $script:Attempt++
    if ($env:SRX_REPORT_FAULT -eq 'retry-failure' -or ($env:SRX_REPORT_FAULT -eq 'retry-success' -and $script:Attempt -eq 1)) {
        return [pscustomobject]@{ Found = $false; Path = ''; Text = '' }
    }
    [pscustomobject]@{ Found = $env:SRX_REPORT_FOUND -eq '1'; Path = '/device/result'; Text = $env:SRX_REPORT_RAW }
}
'''


def run_service_cases(combinations, bases, function, extension):
    """把 service 用例整批交给单进程 runspace 驱动跑完。"""
    entries = []
    for index, (pattern, raw, found, start_exit, _passed, fault) in enumerate(combinations):
        base = bases[index]
        case = "all" if "\n" in raw or raw.startswith("FAIL [all]") else "file_symlink"
        command = f"Invoke-ServiceCase scenario-42 probe {case} @{{}} '{pattern}'"
        tail = "$result = " + command + "\nif (-not $result.Ok) { $global:SRX_CASE_RC = 1 }"
        (base / f"report.{extension}").write_text(
            powershell_case_script(SERVICE_CASE_PWSH_STUBS + function, tail),
            encoding="utf-8", newline="\n")
        entries.append({
            "name": str(index),
            "script": str(base / "report.ps1"),
            "args": [str(base)],
            "env": {"SRX_REPORT_RAW": raw, "SRX_REPORT_FOUND": "1" if found else "0",
                    "SRX_REPORT_FAULT": fault, "SRX_REPORT_START_EXIT": str(start_exit)},
        })
    batch = pwsh_case_driver.run_pwsh_cases(entries, PWSH)
    return [batch[str(index)] for index in range(len(combinations))]


class ScenarioResultReportingTest(unittest.TestCase):
    def test_lock_release_failure_invalidates_run(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (("release_device_run_lock() {", "SHARED_FUSE_BEFORE_CAPTURED=0") if runner == "bash" else
                          ("function Exit-DeviceRunLock {", "function Write-DeviceConfig {"))
            function = source[source.index(start):source.index(end)]
            for succeeds in (False, True):
                with self.subTest(runner=runner, succeeds=succeeds), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    if runner == "bash":
                        script = '''
device_run_lock_held=1
device_run_lock_path=/lock
device_run_lock_token=token
adb_su_timeout() { [[ $SRX_UNLOCK_OK == 1 ]]; }
''' + function + '\nrelease_device_run_lock\n'
                        command = [BASH, str(base / "unlock.sh")]
                    else:
                        script = '''
$script:DeviceRunLockHeld=$true
$script:DeviceRunLockPath='/lock'
$script:DeviceRunLockToken='token'
$script:ExitCode=0
$script:Failures=[Collections.Generic.List[string]]::new()
function Test-Su { $env:SRX_UNLOCK_OK -eq '1' }
''' + function + '\nExit-DeviceRunLock\nexit $script:ExitCode\n'
                        command = [PWSH, "-NoProfile", "-File", str(base / "unlock.ps1")]
                    (base / f"unlock.{extension}").write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15,
                                            env={**os.environ, "SRX_UNLOCK_OK": "1" if succeeds else "0"})
                    self.assertEqual(succeeds, result.returncode == 0, result.stdout + result.stderr)
                    self.assertEqual(not succeeds, "FAIL [cleanup] step=device_lock" in result.stdout + result.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证后端路径边界")
    def test_backend_path_conversion_respects_path_segments(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Convert-ToBackendPath {"):source.index("function Require-File {")]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "paths.ps1"
            path.write_text('''
$RealRoot='/storage/emulated/0'
$BackendRoot='/data/media/0'
''' + function + '''
$cases=@{
    '/storage/emulated/0'='/data/media/0'
    '/storage/emulated/0/Download/file'='/data/media/0/Download/file'
    '/storage/emulated/01/file'='/storage/emulated/01/file'
    '/storage/emulated/0-other/file'='/storage/emulated/0-other/file'
    '/Storage/emulated/0/file'='/Storage/emulated/0/file'
    '/data/media/0/file'='/data/media/0/file'
}
foreach ($case in $cases.GetEnumerator()) {
    if ((Convert-ToBackendPath $case.Key) -cne $case.Value) { exit 1 }
}
''', encoding="utf-8")
            result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], text=True, capture_output=True, timeout=15)
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)

    def test_device_restore_preserves_early_command_failures(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (("restore_device_execution_state() {", "write_config() {") if runner == "bash" else
                          ("function Restore-DeviceExecutionState {", "function Test-FuseBackendScenarioSupport {"))
            function = source[source.index(start):source.index(end)]
            for failure, previous in itertools.product((0, 1, 2, 3), ("null", "3")):
                with self.subTest(runner=runner, failure=failure, previous=previous), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    if runner == "bash":
                        script = '''
device_execution_state_backup_ready=1
original_deviceidle_whitelist=0
original_stay_on_while_plugged_in=$SRX_PREVIOUS
APP_ID=test.app
attempt=0
adb() { attempt=$((attempt + 1)); [[ $attempt != "$SRX_RESTORE_FAULT" ]]; }
''' + function + '\nrestore_device_execution_state\n'
                        command = [BASH, str(base / "restore.sh")]
                    else:
                        script = '''
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$script:DeviceExecutionStateBackupReady=$true
$script:OriginalDeviceIdleWhitelist=$false
$script:OriginalAppInactive=$false
$script:OriginalStayOnWhilePluggedIn=$env:SRX_PREVIOUS
$AppId='test.app'
$script:Attempt=0
function Invoke-Adb { $script:Attempt++; $global:LASTEXITCODE=if ($script:Attempt -eq $env:SRX_RESTORE_FAULT) { 7 } else { 0 } }
''' + function + '\nRestore-DeviceExecutionState\n'
                        command = [PWSH, "-NoProfile", "-File", str(base / "restore.ps1")]
                    (base / f"restore.{extension}").write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15,
                                            env={**os.environ, "SRX_RESTORE_FAULT": str(failure), "SRX_PREVIOUS": previous})
                    self.assertEqual(failure == 0, result.returncode == 0, result.stdout + result.stderr)

    @unittest.skipUnless(BASH, "需要 Bash 验证清理错误传播")
    def test_bash_cleanup_reports_each_failure_and_continues(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        cleanup = source[source.index("cleanup_test_artifacts() {"):source.index("latest_result() {")]
        finish = source[source.index("finish_test_run() {"):source.index("cleanup_done=0")]
        steps = ("adb", "restore_cross_app_config", "restore_provider_config", "restore_app_config", "restore_global_config",
                 "clean_results", "remove_test_target_artifacts", "clear_alias_mediastore_fixture", "remove_random_mediastore_rows",
                 "remove_random_physical_media_files", "restart_media_provider", "restore_device_execution_state")
        for failure, entry in itertools.product(("none", *steps), ("normal", "exit-trap")):
            with self.subTest(failure=failure, entry=entry), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                stubs = '\n'.join(f'{step}() {{ echo {step} >> "$SRX_CLEANUP_TRACE"; [[ $SRX_CLEANUP_FAULT != {step} ]]; }}' for step in steps)
                script = 'set -euo pipefail\nAPP_ID=test.app\ncleanup_done=0\n' + stubs + '\n' + cleanup
                script += '''
capture_shared_fuse_snapshot_once() { :; }
set_download_test_network() { :; }
release_device_run_lock() { echo unlock >> "$SRX_CLEANUP_TRACE"; }
device_run_lock_held=1
global_config_backup_ready=1
'''
                if entry == "normal":
                    script += 'status=0\ncleanup_test_artifacts || status=1\nexit "$status"\n'
                else:
                    script += finish + '\ntrap finish_test_run EXIT\nexit 0\n'
                path = base / "cleanup.sh"
                path.write_text(script, encoding="utf-8", newline="\n")
                result = subprocess.run([BASH, str(path)], text=True, capture_output=True, timeout=15,
                                        env={**os.environ, "SRX_CLEANUP_FAULT": failure, "SRX_CLEANUP_TRACE": (base / "trace").as_posix()})
                self.assertEqual(failure == "none", result.returncode == 0, result.stdout + result.stderr)
                self.assertEqual([*steps, *(["unlock"] if entry == "exit-trap" else [])], (base / "trace").read_text().splitlines())
                self.assertEqual(0 if failure == "none" else 1, (result.stdout + result.stderr).count("FAIL [cleanup]"))

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证清理错误传播")
    def test_powershell_cleanup_reports_exit_codes_and_exceptions(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        cleanup = source[source.index("function Invoke-TestArtifactCleanup {"):source.index("function Restart-App {")]
        steps = ("Invoke-Adb", "Restore-CrossAppConfig", "Restore-AppConfig", "Restore-GlobalConfig", "Clear-Results", "Invoke-Su",
                 "Remove-TestTargetArtifacts", "Clear-AliasMediaStoreFixture", "Remove-RandomMediaStoreRows",
                 "Remove-RandomPhysicalMediaFiles", "Restart-MediaProvider", "Restore-DeviceExecutionState")
        combinations = list(itertools.product(("none", *steps), ("exit", "throw")))
        stubs = '\n'.join(f'''function {step} {{
    Add-Content -LiteralPath $env:SRX_CLEANUP_TRACE -Value '{step}'
    if ($env:SRX_CLEANUP_FAULT -eq '{step}') {{
        if ($env:SRX_CLEANUP_MODE -eq 'throw') {{ throw 'injected cleanup error' }}
        $global:LASTEXITCODE=7
    }}
}}''' for step in steps)
        stub = '''
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$script:CleanupDone=$false
$script:GlobalConfigBackupReady=$true
$script:ExitCode=0
$script:Failures=[Collections.Generic.List[string]]::new()
''' + stubs + '\n' + cleanup
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            entries = []
            for index, (failure, mode) in enumerate(combinations):
                path = base / f"cleanup-{index}.ps1"
                # 原脚本用 `exit $script:ExitCode` 传退出码；runspace 下 exit 会被静默吞掉，
                # 改写成驱动可读的 SRX_CASE_RC。
                body = stub + '\nInvoke-TestArtifactCleanup\n$global:SRX_CASE_RC = $script:ExitCode\n'
                path.write_text(body, encoding="utf-8", newline="\n")
                entries.append({
                    "name": str(index),
                    "script": str(path),
                    "env": {"SRX_CLEANUP_FAULT": failure, "SRX_CLEANUP_MODE": mode,
                            "SRX_CLEANUP_TRACE": str(base / f"trace-{index}")},
                })
            results = pwsh_case_driver.run_pwsh_cases(entries, PWSH)

            for index, (failure, mode) in enumerate(combinations):
                with self.subTest(failure=failure, mode=mode):
                    result = results[str(index)]
                    self.assertEqual(failure == "none", result.returncode == 0, result.stdout + result.stderr)
                    self.assertEqual(list(steps), (base / f"trace-{index}").read_text().splitlines())
                    self.assertEqual(0 if failure == "none" else 1, (result.stdout + result.stderr).count("FAIL [cleanup]"))
        self.assertGreater(source.index('Write-Host "ALL_SCENARIOS_PASSED"'), source.index('try { Invoke-TestArtifactCleanup }'))

    @unittest.skipUnless(BASH and os.name != "nt", "需要 POSIX 链接验证残留检查")
    def test_missing_path_rejects_dangling_links(self):
        for runner in ("bash", "pwsh"):
            if runner == "pwsh" and not PWSH:
                continue
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{'sh' if runner == 'bash' else 'ps1'}").read_text(encoding="utf-8")
            start, end = (("check_file_missing() {", "check_own_private_real_landing() {") if runner == "bash" else
                          ("function Test-PathMissing {", "function Convert-ToBackendPath {"))
            function = source[source.index(start):source.index(end)]
            for kind in ("missing", "file", "directory", "symlink", "dangling", "loop"):
                with self.subTest(runner=runner, kind=kind), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    target = base / "entry"
                    if kind == "file":
                        target.write_text("leak")
                    elif kind == "directory":
                        target.mkdir()
                    elif kind in ("symlink", "dangling", "loop"):
                        if kind == "symlink":
                            (base / "existing").write_text("leak")
                        target.symlink_to("entry" if kind == "loop" else "existing")
                    if runner == "bash":
                        script = 'REAL_ROOT=/logical\nBACKEND_ROOT=$1\nadb_su() { bash -c "$1"; }\n' + function + '\ncheck_file_missing probe /logical/entry\n'
                        path = base / "missing.sh"
                        command = [BASH, str(path), str(base)]
                    else:
                        script = '''
param([string]$BasePath)
function Convert-ToBackendPath { param($Path) "$BasePath/entry" }
function Test-Su { param($Command) & bash -c $Command; $LASTEXITCODE -eq 0 }
''' + function + '\nif (-not (Test-PathMissing /logical/entry)) { exit 1 }\n'
                        path = base / "missing.ps1"
                        command = [PWSH, "-NoProfile", "-File", str(path), str(base)]
                    path.write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15)
                    self.assertEqual(kind == "missing", result.returncode == 0, result.stdout + result.stderr)

    def test_missing_daemon_fails_the_failopen_check(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (
                ("run_monitor_capture_failopen_scenario() {", "run_file_monitor_disabled_redirect_scenario() {") if runner == "bash" else
                ("function Invoke-MonitorCaptureFailOpenScenario {", "function Invoke-DisabledRedirectMonitorScenario {")
            )
            function = source[source.index(start):source.index(end)]
            with self.subTest(runner=runner), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                if runner == "bash":
                    script = "adb_su() { :; }\n" + function + "\nrun_monitor_capture_failopen_scenario 23\n"
                    command = [BASH, "-eu", str(base / "daemon.sh")]
                else:
                    script = "function Invoke-Su {}\n" + function + "\nif (-not (Invoke-MonitorCaptureFailOpenScenario 23)) { exit 1 }\n"
                    command = [PWSH, "-NoProfile", "-File", str(base / "daemon.ps1")]
                (base / f"daemon.{extension}").write_text(script, encoding="utf-8", newline="\n")
                result = subprocess.run(command, text=True, capture_output=True, timeout=15)
                self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
                self.assertIn("FAIL [monitor_capture_failopen]", result.stdout + result.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证场景汇总")
    def test_powershell_summary_requires_one_true_result(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        diagnostics = source.index('    Write-Host "  - scenario-$Scenario/backend-effective"')
        start = source.rfind("    $ok =", source.index("function Invoke-Scenario {"), diagnostics)
        tail = source[start:source.index("function Invoke-BasicAll {")]
        stub = r'''
$Scenario = 42
$ok = $env:SRX_SUMMARY_SETUP -eq 'True'
$before = 0
$script:Failures = [System.Collections.Generic.List[string]]::new()
$script:Summary = [System.Collections.Generic.List[object]]::new()
function Invoke-Su {}
function Stop-AppAndWaitFuseCleanup { $env:SRX_SUMMARY_CLEANUP -eq 'True' }
function Get-ScenarioTitle { 'test' }
if ($env:SRX_SUMMARY_FAILURE -eq 'True') { $script:Failures.Add('physical evidence mismatch') }
'''
        combinations = list(itertools.product(
            ("$true", "$false", "@($true, $false)", "@($false, $true)", "@($true, $true)", "$null", "'true'"),
            (True, False), (True, False), (True, False),
        ))
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            entries = []
            for index, (expression, setup, cleanup, failure) in enumerate(combinations):
                path = base / f"summary-{index}.ps1"
                path.write_text(
                    stub + f"$scenarioOk = {expression}\nfunction Test-Summary {{\n" + tail +
                    "\nTest-Summary\nif (-not $script:Summary[0].Passed) { $global:SRX_CASE_RC = 1 }\n",
                    encoding="utf-8", newline="\n",
                )
                entries.append({
                    "name": str(index),
                    "script": str(path),
                    "env": {"SRX_SUMMARY_SETUP": str(setup), "SRX_SUMMARY_CLEANUP": str(cleanup),
                            "SRX_SUMMARY_FAILURE": str(failure)},
                })
            results = pwsh_case_driver.run_pwsh_cases(entries, PWSH)

            for index, (expression, setup, cleanup, failure) in enumerate(combinations):
                with self.subTest(expression=expression, setup=setup, cleanup=cleanup, failure=failure):
                    result = results[str(index)]
                    passed = expression == "$true" and setup and cleanup and not failure
                    self.assertEqual(passed, result.returncode == 0, result.stdout + result.stderr)
                    self.assertIn(("PASS" if passed else "FAIL") + " [scenario] id=42", result.stdout)

    def test_directory_absence_requires_a_successful_listing(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (
                ("expect_no_app_entry() {", "find_written_file() {") if runner == "bash" else
                ("function Expect-NoAppEntry {", "# 采集场景 2")
            )
            function = source[source.index(start):source.index(end)]
            for observation in ("empty", "present", "substring", "regex-lookalike", "case-sensitive", "missing-entries", "truncated", "failed", "timeout"):
                with self.subTest(runner=runner, observation=observation), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    if runner == "bash":
                        stub = r'''
SRT_ARTIFACT_DIR=$1
TEST_FILE=forbidden.txt
SRT_RESULT_POLL_MS=0
sleep_ms() { :; }
run_service_case() {
  local entries="" count=0
  if [[ $SRX_REPORT_CASE == present ]]; then entries=$TEST_FILE; fi
  if [[ $SRX_REPORT_CASE == substring ]]; then entries="prefix-$TEST_FILE|$TEST_FILE.extra"; count=2; fi
  if [[ $SRX_REPORT_CASE == regex-lookalike ]]; then entries=forbiddenXtxt; count=1; fi
  if [[ $SRX_REPORT_CASE == case-sensitive ]]; then entries=FORBIDDEN.TXT; count=1; fi
  if [[ $SRX_REPORT_CASE == present || $SRX_REPORT_CASE == truncated ]]; then count=1; fi
  local result="PASS [file_list_dir] listedCount=$count, entries=$entries"
  if [[ $SRX_REPORT_CASE == missing-entries ]]; then result='PASS [file_list_dir] incomplete metadata'; fi
  printf '%s\n' "$result" > "$SRT_ARTIFACT_DIR/scenario-3-check-result.txt"
  [[ $SRX_REPORT_CASE != failed && $SRX_REPORT_CASE != timeout ]]
}
'''
                        script = stub + function + "\nexpect_no_app_entry 3 check /directory\n"
                        command = [BASH, "-eu", str(base / "absence.sh"), str(base)]
                    else:
                        stub = r'''
$script:Failures = [System.Collections.Generic.List[string]]::new()
function Invoke-ServiceCase {
    $ok = $env:SRX_REPORT_CASE -notin @('failed', 'timeout')
    $entries = if ($env:SRX_REPORT_CASE -eq 'present') { 'forbidden.txt' } else { '' }
    $count = if ($entries) { 1 } else { 0 }
    if ($env:SRX_REPORT_CASE -eq 'substring') { $entries = 'prefix-forbidden.txt|forbidden.txt.extra'; $count=2 }
    if ($env:SRX_REPORT_CASE -eq 'regex-lookalike') { $entries = 'forbiddenXtxt'; $count=1 }
    if ($env:SRX_REPORT_CASE -eq 'case-sensitive') { $entries = 'FORBIDDEN.TXT'; $count=1 }
    if ($env:SRX_REPORT_CASE -eq 'truncated') { $count=1 }
    $text="listedCount=$count, entries=$entries"
    if ($env:SRX_REPORT_CASE -eq 'missing-entries') { $text='incomplete metadata' }
    [pscustomobject]@{ Ok = $ok; Text = $text }
}
'''
                        script = stub + function + "\nif (-not (Expect-NoAppEntry 3 check /directory forbidden.txt)) { exit 1 }\n"
                        command = [PWSH, "-NoProfile", "-File", str(base / "absence.ps1")]
                    (base / f"absence.{extension}").write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15,
                                            env={**os.environ, "SRX_REPORT_CASE": observation})
                    self.assertEqual(observation in ("empty", "substring", "regex-lookalike", "case-sensitive"), result.returncode == 0, result.stdout + result.stderr)

    def test_directory_presence_requires_an_exact_filename(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (("expect_app_entry() {", "expect_no_app_entry() {") if runner == "bash" else
                          ("function Expect-AppEntry {", "function Expect-NoAppEntry {"))
            function = source[source.index(start):source.index(end)]
            for name in ("expected.txt", "expected.txt.extra", "prefix-expected.txt", "expectedXtxt", "EXPECTED.TXT", "other"):
                with self.subTest(runner=runner, name=name), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    if runner == "bash":
                        script = r'''
SRT_ARTIFACT_DIR=$1
TEST_FILE=expected.txt
SRT_RESULT_POLL_MS=0
sleep_ms() { :; }
app_view_namespace_refresh_allowed() { return 1; }
run_service_case() { printf 'PASS [file_list_dir] listedCount=1, entries=%s\n' "$SRX_ENTRY" > "$SRT_ARTIFACT_DIR/scenario-3-check-result.txt"; }
''' + function + '\nexpect_app_entry 3 check /directory\n'
                        command = [BASH, "-eu", str(base / "presence.sh"), base.as_posix()]
                    else:
                        script = r'''
$script:Failures=[Collections.Generic.List[string]]::new()
function Invoke-ServiceCase { [pscustomobject]@{ Ok=$true; Text="PASS [file_list_dir] listedCount=1, entries=$env:SRX_ENTRY`n__SRT_RESULT_END__=1" } }
''' + function + '\nif (-not (Expect-AppEntry 3 check /directory expected.txt)) { exit 1 }\n'
                        command = [PWSH, "-NoProfile", "-File", str(base / "presence.ps1")]
                    (base / f"presence.{extension}").write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15, env={**os.environ, "SRX_ENTRY": name})
                    self.assertEqual(name == "expected.txt", result.returncode == 0, result.stdout + result.stderr)

    def test_unredirected_link_control_rejects_errors_and_residue(self):
        for runner in ("bash", "pwsh"):
            if not (BASH if runner == "bash" else PWSH):
                continue
            extension = "sh" if runner == "bash" else "ps1"
            source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
            start, end = (
                ("run_file_link_control() {", "run_file_mutation_metadata_scenario() {") if runner == "bash" else
                ("function Invoke-FileLinkControl {", "function Invoke-FileMutationMetadataScenario {")
            )
            function = source[source.index(start):source.index(end)]
            for case in ("created", "denied", "eperm", "unexpected", "residue", "timeout", "target-failed", "prepare-failed"):
                with self.subTest(runner=runner, case=case), tempfile.TemporaryDirectory() as directory:
                    base = Path(directory)
                    if runner == "bash":
                        stub = r'''
SRT_ARTIFACT_DIR=$1
REAL_ROOT=/storage/emulated/0
BACKEND_ROOT=/data/media/0
PAYLOAD=payload
apply_config() { test "$1" = 1; echo redirect_disabled; }
clean_targets() { :; }
fix_shared_fixture_permissions() { test "$prepared" = 1; }
start_app_and_confirm_mount() { test "$2" = 0; }
wait_storage_ready() { :; }
app_pid() { echo 123; }
prepared=0
adb_su() {
  if [[ $1 == 'mkdir -p '* ]]; then prepared=1; [[ $SRX_REPORT_CASE != prepare-failed ]]; return; fi
  test "$SRX_REPORT_CASE" != residue
}
run_service_case() {
  local scenario=$1 label=$2 test_case=$3
  [[ $SRX_REPORT_CASE != timeout ]] || return 1
  if [[ $test_case == file_create ]]; then [[ $prepared == 1 && $SRX_REPORT_CASE != target-failed ]]; return; fi
  local result="FAIL [$test_case] link failed error=EACCES"
  if [[ $SRX_REPORT_CASE == created ]]; then result="PASS [$test_case] link succeeded"; fi
  if [[ $SRX_REPORT_CASE == unexpected ]]; then result="FAIL [$test_case] link failed error=EIO Permission denied"; fi
  if [[ $SRX_REPORT_CASE == eperm && $test_case == file_link ]]; then result="FAIL [$test_case] link failed error=EPERM"; fi
  printf '%s\n' "$result" > "$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
}
'''
                        script = stub + function + "\nrun_file_link_control\n"
                        command = [BASH, "-eu", str(base / "control.sh"), str(base)]
                    else:
                        stub = r'''
$ErrorActionPreference = 'Stop'
$RealRoot = '/storage/emulated/0'
$BackendRoot = '/data/media/0'
$Payload = 'payload'
function Apply-ScenarioConfig { param($Scenario) if ($Scenario -ne 1) { throw 'control must disable redirect' }; Write-Host redirect_disabled }
function Clear-Targets { $script:Prepared=$false }
function Fix-SharedFixturePermissions { if (-not $script:Prepared) { throw 'control parent not prepared' } }
function Restart-App { param($Label, $ExpectMount) if ($ExpectMount) { throw 'control must not redirect' }; $true }
function Get-AppPid { '123' }
function Test-Su {
    param($Command)
    if ($Command.StartsWith('mkdir -p ')) { $script:Prepared=$true; return $env:SRX_REPORT_CASE -ne 'prepare-failed' }
    $env:SRX_REPORT_CASE -ne 'residue'
}
function Invoke-ServiceCase {
    param($Scenario, $Label, $TestCase)
    if ($env:SRX_REPORT_CASE -eq 'timeout') { return [pscustomobject]@{ Ok = $false; Text = 'timeout' } }
    if ($TestCase -eq 'file_create') { return [pscustomobject]@{ Ok = $env:SRX_REPORT_CASE -ne 'target-failed'; Text = 'created' } }
    $text = "FAIL [$TestCase] link failed error=EACCES"
    if ($env:SRX_REPORT_CASE -eq 'created') { $text = "PASS [$TestCase] link succeeded" }
    if ($env:SRX_REPORT_CASE -eq 'unexpected') { $text = "FAIL [$TestCase] link failed error=EIO Permission denied" }
    if ($env:SRX_REPORT_CASE -eq 'eperm' -and $TestCase -eq 'file_link') { $text = "FAIL [$TestCase] link failed error=EPERM" }
    [pscustomobject]@{ Ok = $true; Text = "$text`n__SRT_RESULT_END__=1" }
}
'''
                        script = stub + function + "\nif (-not (Invoke-FileLinkControl)) { exit 1 }\n"
                        command = [PWSH, "-NoProfile", "-File", str(base / "control.ps1")]
                    (base / f"control.{extension}").write_text(script, encoding="utf-8", newline="\n")
                    result = subprocess.run(command, text=True, capture_output=True, timeout=15,
                                            env={**os.environ, "SRX_REPORT_CASE": case})
                    self.assertEqual(case in ("created", "denied", "eperm"), result.returncode == 0, result.stdout + result.stderr)
                    self.assertIn("redirect_disabled", result.stdout)

    @unittest.skipUnless(BASH, "需要 Bash 执行结果报告回归")
    def test_bash_service_verdict_and_raw_evidence(self):
        self.check_service_reporting("bash")

    @unittest.skipUnless(PWSH, "需要 PowerShell 执行结果报告回归")
    def test_powershell_service_verdict_and_raw_evidence(self):
        self.check_service_reporting("pwsh")

    def check_service_reporting(self, runner):
        extension = "sh" if runner == "bash" else "ps1"
        source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
        start, end = (
            ("run_service_case() {", "run_write_case() {") if runner == "bash" else
            ("function Invoke-ServiceCase {", "function Test-FreshAppEnabled {")
        )
        function = source[source.index(start):source.index(end)]
        success = "PASS [file_symlink] link succeeded"
        denied = "FAIL [file_symlink] link failed error=EACCES (Permission denied)"
        cases = (
            ("", denied, True, 0, True), ("", success, True, 0, True),
            (r"^PASS \[file_symlink\]", denied, True, 0, False),
            (r"^PASS \[file_symlink\]", success, True, 0, True),
            ("", denied, False, 0, False), ("", denied, True, 7, True),
            (r"^PASS \[file_symlink\]", success, True, 7, True),
            (r"^FAIL \[file_symlink\].*EACCES", denied, True, 0, True),
            (r"^FAIL \[file_symlink\].*EACCES", denied.replace("EACCES", "EIO"), True, 0, False),
            (r"^PASS ", success + "\nFAIL [file_read] read failed", True, 0, False),
            (r"^PASS ", success + "\nPASS [file_read] read succeeded", True, 0, True),
            (r"^PASS ", "FAIL [file_read] first failed\n" + success, True, 0, False),
            (r"^PASS ", "FAIL [all] startForeground failed", True, 0, False),
            (r"^PASS \[file_symlink\]", "FAIL [file_symlink] bad metadata=PASS", True, 0, False),
            (r"^FAIL \[file_symlink\].*error=EACCES", denied.replace("EACCES", "EPERM"), True, 0, False),
            (r"^PASS \[file_symlink\]", success, False, 124, False),
        )
        cases = [(*case, "none") for case in cases]
        for start_exit in (1, 7, 124, 137):
            for found in (True, False):
                cases.append((r"^PASS \[file_symlink\]", success, found, start_exit, found, "none"))
        for fault in ("prepare", "marker", "mount", "artifact", "retry-success", "retry-failure"):
            cases.append((r"^PASS \[file_symlink\]", success, True, 0, fault == "retry-success", fault))
        for raw, found in ((success, True), (denied, True), (success, False)):
            cases.append((r"^PASS \[file_symlink\]", raw, found, 0, raw == success and found, "fail-fast"))
        combinations = list(cases)
        # 每个用例独立子目录：断言依赖写到该目录下的产物文件（结果文件、adb-args）。
        workspace = tempfile.TemporaryDirectory()
        self.addCleanup(workspace.cleanup)
        root = Path(workspace.name)
        bases = []
        for index, (_pattern, _raw, _found, _start_exit, _passed, fault) in enumerate(combinations):
            base = root / f"case-{index}"
            base.mkdir()
            if fault == "artifact":
                (base / "scenario-42-probe-result.txt").mkdir()
            bases.append(base)

        bash_stubs = r"""
set -euo pipefail
SRT_ARTIFACT_DIR=$1
SRT_SERVICE_CASE_SETTLE_MS=0
APP_ID=test.app
ACTION=test.action
prepare_service_case() { [[ $SRX_REPORT_FAULT != prepare ]]; }
timeout() { shift; "$@"; }
adb() { printf '%s\n' "$*" >> "$SRT_ARTIFACT_DIR/adb-args.txt"; echo cmp=stub; return "$SRX_REPORT_START_EXIT"; }
adb_su() { [[ $SRX_REPORT_FAULT != marker || $1 != touch* ]]; }
sleep_ms() { :; }
start_app_and_confirm_mount() { :; }
wait_storage_ready() { :; }
clean_results() { :; }
label_expects_mount() { [[ $SRX_REPORT_FAULT == mount ]]; }
ensure_current_app_mount_confirmed() { return 1; }
service_case_timeout_seconds() { echo 0; }
wait_service_result() {
  local attempt=0
  if [[ -f $SRT_ARTIFACT_DIR/attempt ]]; then read -r attempt < "$SRT_ARTIFACT_DIR/attempt"; fi
  echo "$((attempt + 1))" > "$SRT_ARTIFACT_DIR/attempt"
  if [[ $SRX_REPORT_FAULT == retry-failure || ( $SRX_REPORT_FAULT == retry-success && $attempt == 0 ) ]]; then return 1; fi
  printf '%s\n' "$SRX_REPORT_RAW"; test "$SRX_REPORT_FOUND" = 1
}
"""

        results = []
        if runner == "bash":
            for index, (pattern, raw, found, start_exit, _passed, fault) in enumerate(combinations):
                base = bases[index]
                case = "all" if "\n" in raw or raw.startswith("FAIL [all]") else "file_symlink"
                command = f"run_service_case 42 probe {case} '{pattern}'"
                (base / f"report.{extension}").write_text(
                    bash_stubs + function + "\n" + command + "\n", encoding="utf-8", newline="\n")
                results.append(subprocess.run(
                    [BASH, "-eu", str(base / "report.sh"), str(base)],
                    text=True, encoding="utf-8", errors="backslashreplace", capture_output=True, timeout=20,
                    env={**os.environ, "SRX_REPORT_RAW": raw, "SRX_REPORT_FOUND": "1" if found else "0",
                         "SRX_REPORT_FAULT": fault, "SRX_REPORT_START_EXIT": str(start_exit)}))
        else:
            results = run_service_cases(combinations, bases, function, extension)

        for index, (pattern, raw, found, start_exit, passed, fault) in enumerate(combinations):
            with self.subTest(pattern=pattern, raw=raw, found=found, start_exit=start_exit, fault=fault):
                base = bases[index]
                result = results[index]
                if fault in ("prepare", "marker", "mount", "artifact", "retry-failure"):
                    self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
                    self.assertNotRegex(result.stdout, r"(?m)^PASS \[")
                    continue
                self.assertEqual(passed, result.returncode == 0, result.stdout + result.stderr)
                if not pattern and found:
                    self.assertIn("PENDING [file_symlink]", result.stdout)
                    self.assertNotRegex(result.stdout, r"(?m)^(PASS|FAIL) ")
                    artifact = base / "scenario-42-probe-result.txt"
                    self.assertEqual(raw, artifact.read_text(encoding="utf-8-sig").strip())
                    if runner == "bash":
                        self.assertEqual(raw, (base / "scenario-42-result.txt").read_text().strip())
                elif not found:
                    self.assertNotIn("PENDING [", result.stdout)
                    self.assertNotIn("PASS ", result.stdout)
                    self.assertRegex(result.stdout, "TIMEOUT|result_timeout")
                else:
                    self.assertRegex(result.stdout + result.stderr, r"(?m)^" + ("PASS" if passed else "FAIL") + r" \[")
                    verdicts = [line for line in (result.stdout + result.stderr).splitlines() if line.startswith(("PASS [", "FAIL ["))]
                    self.assertEqual(1, len(verdicts), result.stdout + result.stderr)
                if found:
                    self.assertIn("OBSERVED [", result.stdout)
                    self.assertIn("app_status=", result.stdout)
                    self.assertEqual(raw, (base / "scenario-42-probe-result.txt").read_text(encoding="utf-8-sig").strip())
                    if runner == "bash":
                        self.assertIn("--es result_token /data/local/tmp/srx-result-", (base / "adb-args.txt").read_text())


    @unittest.skipUnless(PWSH, "需要 PowerShell 验证链接边界最终结果")
    def test_powershell_boundary_verdict_requires_all_evidence(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Invoke-FileMutationMetadataScenario {"):source.index("function Invoke-StandardScenario {")]
        cases = [(case, "") for case in ("denied", "created", "control-mismatch", "native-mismatch", "unexpected", "misleading-message", "errno-in-path", "leak", "timeout", "service-status", "mount-failure")]
        cases += list(itertools.product(
            ("chmod", "fchmod", "stat-error", "wrong-symlink", "case-symlink", "readlink-error", "missing-entry", "case-entry", "extra-entry", "listing-error", "content", "inode", "shadow"),
            ("Sandbox", "Allowed", "Request", "RequestSibling"),
        ))
        stub = r'''
$ErrorActionPreference = 'Stop'
$RealRoot = '/storage/emulated/0'
$BackendRoot = '/data/media/0'
$BackendPrivateRoot = '/data/media/0/private'
$Payload = 'payload'
$script:Failures = [System.Collections.Generic.List[string]]::new()
$script:NativeSymlinkResult = if ($env:SRX_REPORT_CASE -eq 'native-mismatch') { 'created' } else { 'EACCES' }
$script:NativeHardlinkResult = 'EPERM'
function Invoke-ServiceCase {
    param($Scenario, $Label, $TestCase, $Extras, $PassRegex)
    if ($env:SRX_REPORT_CASE -eq 'mount-failure') { $script:Failures.Add('mount not confirmed') }
    if ($TestCase -eq 'file_chmod') { $script:Mode = '440' }
    if ($TestCase -eq 'file_fchmod') { $script:Mode = '220' }
    if ($TestCase -eq 'file_symlink') {
        if ($env:SRX_REPORT_CASE -eq 'timeout') { return [pscustomobject]@{ Ok = $false; Text = 'timeout' } }
        $created = $env:SRX_REPORT_ROUTE -or $env:SRX_REPORT_CASE -eq 'created' -or ($env:SRX_REPORT_CASE -eq 'control-mismatch' -and $kind -eq 'Allowed')
        $text = if ($created) { 'PASS [file_symlink] link succeeded' }
            elseif ($env:SRX_REPORT_CASE -in @('unexpected', 'misleading-message')) { 'FAIL [file_symlink] link failed error=EIO Permission denied EACCES' }
            elseif ($env:SRX_REPORT_CASE -eq 'errno-in-path') { 'FAIL [file_symlink] link failed from=error=EACCES (directory), error=EIO' }
            else { 'FAIL [file_symlink] link failed error=EACCES' }
    } elseif ($TestCase -eq 'file_link') { $text = if ($env:SRX_REPORT_ROUTE) { 'PASS [file_link] link succeeded' } else { 'FAIL [file_link] link failed error=EPERM' } }
    else { $text = "PASS [$TestCase] operation succeeded" }
    if (-not $PassRegex) { Write-Host "PENDING [$TestCase]" }
    [pscustomobject]@{ Ok = -not ($env:SRX_REPORT_CASE -eq 'service-status' -and $TestCase -eq 'file_symlink'); Text = "$text`n__SRT_RESULT_END__=1" }
}
function Invoke-Su {
    param([string]$Command)
    $global:LASTEXITCODE=0
    $fault = $kind -eq $env:SRX_REPORT_ROUTE
    if ($Command.StartsWith('stat ')) {
        if ($fault -and $env:SRX_REPORT_CASE -eq 'stat-error') { $global:LASTEXITCODE=7 }
        if ($fault -and (($env:SRX_REPORT_CASE -eq 'chmod' -and $script:Mode -eq '440') -or ($env:SRX_REPORT_CASE -eq 'fchmod' -and $script:Mode -eq '220'))) { return '777' }
        return $script:Mode
    }
    if ($Command.StartsWith('readlink ')) {
        if ($fault -and $env:SRX_REPORT_CASE -eq 'readlink-error') { $global:LASTEXITCODE=7 }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'wrong-symlink') { return 'wrong-target.txt' }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'case-symlink') { return 'LINK-TARGET.TXT' }
        return 'link-target.txt'
    }
    if ($Command.StartsWith('entries=')) {
        $entries = 'complete.txt|link-target.txt|metadata.txt|'
        if ($env:SRX_REPORT_CASE -eq 'created' -or ($env:SRX_REPORT_CASE -eq 'control-mismatch' -and $kind -eq 'Allowed')) { $entries += 'symbolic-link|' }
        if ($env:SRX_REPORT_ROUTE) { $entries='complete.txt|hard-link|link-target.txt|metadata.txt|symbolic-link|' }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'missing-entry') { $entries=$entries.Replace('metadata.txt|', '') }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'case-entry') { $entries=$entries.Replace('complete.txt|', 'COMPLETE.TXT|') }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'extra-entry') { $entries+='unexpected|' }
        if ($fault -and $env:SRX_REPORT_CASE -eq 'listing-error') { $global:LASTEXITCODE=7 }
        return $entries
    }
    throw "unexpected physical query: $Command"
}
function Test-Su {
    param([string]$Command)
    if ($kind -eq $env:SRX_REPORT_ROUTE) {
        if ($env:SRX_REPORT_CASE -eq 'inode' -and $Command.Contains(' -ef ')) { return $false }
        if ($env:SRX_REPORT_CASE -in @('content', 'shadow') -and $Command.Contains('/metadata.txt')) { return $false }
    }
    return -not ($env:SRX_REPORT_CASE -eq 'leak' -and $kind -eq 'RequestSibling')
}
'''
        tail = "if (-not (Invoke-FileMutationMetadataScenario 42)) { $global:SRX_CASE_RC = 1 }"
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            entries = []
            for index, (case, route) in enumerate(cases):
                path = base / f"boundary-{index}.ps1"
                path.write_text(powershell_case_script(stub + function, tail), encoding="utf-8", newline="\n")
                entries.append({
                    "name": str(index),
                    "script": str(path),
                    "env": {"SRX_REPORT_CASE": case, "SRX_REPORT_ROUTE": route},
                })
            results = pwsh_case_driver.run_pwsh_cases(entries, PWSH)

            for index, (case, route) in enumerate(cases):
                with self.subTest(case=case, route=route):
                    result = results[str(index)]
                    passed = case in ("denied", "created")
                    self.assertEqual(passed, result.returncode == 0, result.stdout + result.stderr)
                    verdicts = [line for line in result.stdout.splitlines() if line.startswith("PASS [file_") and "_boundary]" in line]
                    self.assertEqual(8 if passed else 0, len(verdicts), result.stdout)
                    if passed:
                        self.assertNotIn("FAIL [", result.stdout)
                        self.assertGreater(result.stdout.index(verdicts[0]), result.stdout.rindex("PENDING ["))
                        outcome = "created" if case == "created" else "denied errno=EACCES control=unredirected,Allowed"
                        self.assertIn(f"PASS [file_symlink_boundary] scenario=42 route=RequestSibling outcome={outcome}", result.stdout)
                    else:
                        self.assertIn("FAIL [", result.stdout)
                        if case == "control-mismatch":
                            self.assertIn("reason=eacces_without_matching_controls", result.stdout)
                        if case == "unexpected":
                            self.assertIn("error=EIO", result.stdout)
