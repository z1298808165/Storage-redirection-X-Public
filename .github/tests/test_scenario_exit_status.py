"""实际执行场景入口，验证失败传播与首个现场保留，不连接设备。"""

import json
import itertools
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
FLOW = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")
PWSH = shutil.which("pwsh")


@unittest.skipUnless(BASH, "需要 Bash 执行场景入口回归")
class ScenarioExitTests(unittest.TestCase):
    def run_case(self, scenario=1, overrides=""):
        function = FLOW[FLOW.index("run_scenario() {") : FLOW.index("# 与 PowerShell 共用设备端锁")]
        launcher = re.search(r'"\$\{SRT_SCENARIO_TIMEOUT_SECONDS\}s" (bash .+?) _ "\$scenario"', FLOW).group(1)
        # 只截取实际 Bash 选项，避免本机 PATH 将 Git Bash 转发到 WSL。
        options = launcher.split(" -c ", 1)[0].split()[1:]
        stubs = """
apply_config() { :; }
apply_config_and_wait() { :; }
wait_config_applied() { :; }
clean_targets() { :; }
assert_fixture_roots_empty() { :; }
adb() { :; }
adb_su() { echo diagnostic; }
label_expects_mount() { :; }
start_app_and_confirm_mount() { :; }
wait_storage_ready() { :; }
wait_scenario_app_view() { :; }
clean_results() { :; }
prepare_read_only_media_image() { :; }
run_standard_scenario() { echo assertion; }
check_file_missing() { :; }
check_file_exists() { :; }
run_service_case() { :; }
run_file_link_control() { :; }
cleanup_frozen_daemon() { :; }
restore_shell_config() { :; }
export -f cleanup_frozen_daemon restore_shell_config
LOG_PATH=/unused
APP_ID=test.app
REAL_ROOT=/unused
BACKEND_ROOT=/unused
BACKEND_PRIVATE_ROOT=/unused
READ_ONLY_IMAGE_FILE=image.jpg
READ_ONLY_MEDIA_ROOT=/unused
PRIVATE_READ_ONLY_MEDIA_ROOT=/unused
"""
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as directory:
            # run_scenario 的结果文件写 ${SRT_ARTIFACT_DIR}/scenario-N-result.txt；
            # 该变量由脚本头部的仓库默认值或编排器（run-android-test-flow.sh）经
            # 环境传入，函数体抽取后两者都不在。按编排器方式用环境变量传递，
            # 不注入脚本文本（-c 内联脚本在 Windows/MSYS 下有命令行长度上限）。
            env = dict(os.environ, SRT_ARTIFACT_DIR=directory)
            # 脚本落临时文件再执行，不走 `-c` 内联：Windows/MSYS 的命令行上限约 8191
            # 字符，run_scenario 函数体加桩后已逼近该上限，再加一个场景分支就会被截断成
            # 「unexpected EOF」（2026-11-06 场景 40 第三段即撞上）。
            script = Path(directory) / "run_scenario_case.sh"
            script.write_text(
                stubs + overrides + function + f"\nrun_scenario {scenario}\n",
                encoding="utf-8",
                newline="\n",
            )
            file_options = [option for option in options if option != "-c"]
            return subprocess.run(
                [BASH, *file_options, str(script)],
                cwd=directory, env=env, text=True, encoding="utf-8", capture_output=True, timeout=10,
            )

    def test_success_keeps_backend_diagnostics(self):
        result = self.run_case()
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertIn("assertion", result.stdout)
        self.assertIn("step 6/7", result.stdout)

    def test_assertion_failure_is_not_overwritten(self):
        result = self.run_case(overrides="run_standard_scenario() { return 37; }\n")
        self.assertEqual(37, result.returncode, result.stderr)
        self.assertNotIn("step 6/7", result.stdout)

    def test_prepare_failure_stops_before_assertion(self):
        result = self.run_case(overrides="apply_config() { return 23; }\n")
        self.assertEqual(23, result.returncode, result.stderr)
        self.assertNotIn("assertion", result.stdout)

    def test_pipeline_failure_stops_before_diagnostics(self):
        result = self.run_case(overrides="run_standard_scenario() { false | cat; }\n")
        self.assertNotEqual(0, result.returncode)
        self.assertNotIn("step 6/7", result.stdout)

    def test_and_chain_failure_is_not_overwritten(self):
        result = self.run_case(31, "check_file_missing() { return 19; }\n")
        self.assertEqual(19, result.returncode, result.stderr)
        self.assertNotIn("step 6/7", result.stdout)

    def test_compound_scenarios_run_every_check_and_keep_failures(self):
        groups = {
            23: ("run_file_monitor_disabled_redirect_scenario", "run_monitor_capture_failopen_scenario"),
            40: ("run_system_uid_attribution_scenario", "run_app_caller_media_provider_attribution_scenario", "run_shell_executor_boundary_scenario"),
            42: ("run_file_mutation_lifecycle_scenario", "run_file_mutation_metadata_scenario"),
        }
        for scenario, names in groups.items():
            with self.subTest(scenario=scenario):
                stubs = "\n".join(f'{name}() {{ echo check-{index}; ' + ('false; echo unexpected;' if index == 0 else ':;') + ' }' for index, name in enumerate(names))
                stubs += "\nexport -f " + " ".join(names) + "\n"
                result = self.run_case(scenario, stubs)
                self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
                for index in range(len(names)):
                    self.assertIn(f"check-{index}", result.stdout)
                self.assertNotIn("unexpected", result.stdout)

    def test_mutation_subchecks_receive_native_link_controls(self):
        result = self.run_case(42, r'''
run_file_link_control() { native_symlink_result=EACCES; native_hardlink_result=EPERM; }
run_file_mutation_lifecycle_scenario() { :; }
run_file_mutation_metadata_scenario() {
  test "${native_symlink_result:-}" = EACCES
  test "${native_hardlink_result:-}" = EPERM
}
export -f run_file_mutation_lifecycle_scenario run_file_mutation_metadata_scenario
''')
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)

    def test_frozen_daemon_resumes_after_subcheck_failure(self):
        cleanup = FLOW[FLOW.index("cleanup_frozen_daemon() {") : FLOW.index("trap cleanup_frozen_daemon EXIT")]
        freeze = FLOW[FLOW.index("run_monitor_capture_failopen_scenario() {") : FLOW.index("run_file_monitor_disabled_redirect_scenario() {")]
        stubs = r'''
export SRT_FROZEN_DAEMON_PID_FILE="$SRT_ARTIFACT_DIR/frozen"
run_file_monitor_disabled_redirect_scenario() { :; }
adb_su() { case "$1" in *"for p in"*) echo 424242 ;; esac; }
adb_su_timeout() { echo resumed; }
sleep_ms() { return 37; }
export -f run_file_monitor_disabled_redirect_scenario adb_su adb_su_timeout sleep_ms
'''
        stubs += cleanup + freeze + "\nexport -f cleanup_frozen_daemon run_monitor_capture_failopen_scenario\n"
        result = self.run_case(23, stubs)
        self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertIn("resumed", result.stdout)

    def test_finalizer_resumes_daemon_and_preserves_failure_status(self):
        function = FLOW[FLOW.index("finish_test_run() {") : FLOW.index("cleanup_done=0", FLOW.index("finish_test_run() {"))]
        script = "cleanup_frozen_daemon() { echo resumed; }; capture_shared_fuse_snapshot_once() { :; }; device_run_lock_held=0\n"
        result = subprocess.run([BASH, "-c", script + function + "\ntrap finish_test_run EXIT\nexit 37"], text=True, encoding="utf-8", capture_output=True, timeout=10)
        self.assertEqual(37, result.returncode, result.stderr)
        self.assertIn("resumed", result.stdout)

    def test_shell_executor_keeps_native_path_instead_of_app_sandbox(self):
        function = FLOW[FLOW.index("run_shell_executor_boundary_scenario() {") : FLOW.index("backup_shell_config() {")]
        stubs = r'''
backup_shell_config() { :; }
restore_shell_config() { echo restored; }
adb_write_file() { :; }
adb() {
  case "$*" in
    *"exec mkdir"*) return MKDIR_STATUS ;;
    *"rm -rf"*) return CLEANUP_STATUS ;;
  esac
}
sleep() { :; }
adb_su() {
  case "$1" in
    "test -d '/real'") return REAL_STATUS ;;
    "test -d '/sandbox'") return SANDBOX_STATUS ;;
  esac
}
SHELL_CONFIG=/config
SHELL_SANDBOX_REL=Download/probe
SHELL_SANDBOX_REAL_DIR=/real
SHELL_SANDBOX_BACKEND_DIR=/sandbox
LOG_PATH=/log
'''
        for real, sandbox, mkdir_status, cleanup_status in itertools.product((False, True), (False, True), (0, 17), (0, 19)):
            script = stubs.replace("REAL_STATUS", "0" if real else "1").replace("SANDBOX_STATUS", "0" if sandbox else "1").replace("MKDIR_STATUS", str(mkdir_status)).replace("CLEANUP_STATUS", str(cleanup_status))
            result = subprocess.run([BASH, "-e", "-c", script + function + "\nrun_shell_executor_boundary_scenario 40"], text=True, encoding="utf-8", capture_output=True, timeout=10)
            self.assertEqual(0 if real and not sandbox and not mkdir_status and not cleanup_status else 1, result.returncode, result.stdout + result.stderr)
            self.assertIn("restored", result.stdout)
            self.assertNotIn("unexpected App sandbox routing", result.stdout + result.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证 shell 执行结果与清理失败")
    def test_powershell_shell_executor_checks_commands_and_directory_state(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8-sig")
        function = source[source.index("function Invoke-ShellExecutorBoundaryScenario {"):source.index("function Invoke-FuseDaemonAllowWildcardScenario {")]
        script = r'''
$ErrorActionPreference='Stop'
$ShellConfig='/config'
$ShellSandboxRel='Download/probe'
$ShellSandboxRealDir='/real'
$ShellSandboxBackendDir='/sandbox'
$LogPath='/log'
function Start-Sleep {}
function Invoke-Adb {
    param($Arguments)
    $global:LASTEXITCODE = if ($Arguments[-1] -like '*exec mkdir*') { $script:MkdirExit } else { $script:CleanupExit }
}
function Invoke-Su {
    param([string]$Command)
    $global:LASTEXITCODE=0
    if ($Command.StartsWith("test -d '/real'")) { if ($script:Real) { 'exit=0' } else { 'exit=1' } }
    elseif ($Command.StartsWith("test -d '/sandbox'")) { if ($script:Sandbox) { 'exit=0' } else { 'exit=1' } }
}
''' + function + r'''
foreach ($real in @($false,$true)) { foreach ($sandbox in @($false,$true)) {
    foreach ($mkdirExit in @(0,17)) { foreach ($cleanupExit in @(0,19)) {
        $script:Real=$real; $script:Sandbox=$sandbox
        $script:MkdirExit=$mkdirExit; $script:CleanupExit=$cleanupExit
        $actual=Invoke-ShellExecutorBoundaryScenario 40
        $expected=$real -and -not $sandbox -and $mkdirExit -eq 0 -and $cleanupExit -eq 0
        if ($actual -isnot [bool] -or $actual -ne $expected) { throw "unexpected boundary result $actual expected $expected" }
    }}
}}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "shell-executor.ps1"
            path.write_text(script, encoding="utf-8")
            result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], capture_output=True, text=True, encoding="utf-8", timeout=30)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertNotIn("unexpected App sandbox routing", result.stdout)

    @unittest.skipUnless(PWSH, "需要 PowerShell 执行子项失败收集回归")
    def test_powershell_subchecks_continue_after_false_and_exception(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Invoke-ScenarioChecks {") : source.index("function Invoke-Scenario {")]
        for fail_fast, visits in ((False, 3), (True, 1)):
            script = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ErrorActionPreference='Stop'; $script:Failures=[Collections.Generic.List[string]]::new(); $script:FailFast=" + ("$true" if fail_fast else "$false") + "; $script:Visits=0\n"
            script += function + "\n$result=Invoke-ScenarioChecks 40 @({$script:Visits++; $false}, {$script:Visits++; throw 'injected'}, {$script:Visits++; $true})\n"
            script += f"if ($result -ne $false -or $script:Visits -ne {visits}) {{ throw 'wrong result or skipped checks' }}"
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "subchecks.ps1"
                path.write_text(script, encoding="utf-8")
                result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], text=True, encoding="utf-8", capture_output=True, timeout=20)
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证场景终态完整性")
    def test_powershell_preparation_failures_keep_final_markers_and_summary(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Invoke-Scenario {") : source.index("function Invoke-BasicAll {")]
        report = source[source.index("function Add-ScenarioFailure {") : source.index("function Invoke-ScenarioChecks {")]
        loop = source[source.index("    foreach ($scenario in $scenarios) {") : source.index('    Write-Host "== summary =="')]
        stubs = r'''
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$ErrorActionPreference='Stop'
$script:Failures=[Collections.Generic.List[string]]::new()
$script:Summary=[Collections.Generic.List[object]]::new()
$scenarios=@(1,2)
function Get-ScenarioTitle { 'stub' }
function Stop-AppAndWaitFuseCleanup {
    param($Label)
    if ($Fault -eq 'abort-cleanup' -and $Label -like '*/abort') { throw 'injected cleanup failure' }
    $true
}
function Apply-ScenarioConfig {
    param($Number)
    if ($Number -eq 1 -and $Fault -in @('prepare', 'abort-cleanup')) { throw 'injected prepare failure' }
}
function Clear-Targets {}
function Fix-SharedFixturePermissions {}
function Assert-FixtureRootsEmpty {
    param($Label)
    -not ($Fault -eq 'residue' -and $Label -eq 'scenario-1')
}
function Restart-App { $true }
function Invoke-StandardScenario { $true }
function Invoke-Su {}
'''
        for fault in ("none", "residue", "prepare", "abort-cleanup"):
            for fail_fast in (False, True):
                with self.subTest(fault=fault, fail_fast=fail_fast), tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "terminal.ps1"
                    setup = f"$Fault='{fault}'; $script:FailFast=" + ("$true" if fail_fast else "$false") + "\n"
                    path.write_text(stubs + setup + report + function + loop + '\nWrite-Host ("SRX_SUMMARY=" + (ConvertTo-Json -InputObject @($script:Summary) -Compress))\n', encoding="utf-8")
                    result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], text=True, encoding="utf-8", capture_output=True, timeout=20)
                self.assertEqual(0, result.returncode, result.stdout + result.stderr)
                rows = json.loads(re.search(r"(?m)^SRX_SUMMARY=(.+)$", result.stdout).group(1))
                self.assertEqual(1 if fail_fast and fault != "none" else 2, len(rows))
                self.assertEqual(fault == "none", rows[0]["Passed"])
                for row in rows:
                    marker = f'{"PASS" if row["Passed"] else "FAIL"} [scenario] id={row["Scenario"]}'
                    self.assertEqual(1, result.stdout.splitlines().count(marker), result.stdout)
                if len(rows) == 2:
                    self.assertTrue(rows[1]["Passed"])

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证基础套件失败后继续场景")
    def test_powershell_basic_failure_preserves_result_and_respects_fail_fast(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        report = source[source.index("function Add-ScenarioFailure {") : source.index("function Invoke-ScenarioChecks {")]
        entry = source[source.index("    if (-not $SkipBasicAll) {") : source.index("    foreach ($scenario in $scenarios) {")]
        for fails in (False, True):
            for fail_fast in (False, True):
                with self.subTest(fails=fails, fail_fast=fail_fast), tempfile.TemporaryDirectory() as directory:
                    script = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ErrorActionPreference='Stop'; $SkipBasicAll=$false; $script:Failures=[Collections.Generic.List[string]]::new(); $script:Summary=[Collections.Generic.List[object]]::new()\n"
                    script += "$script:FailFast=" + ("$true" if fail_fast else "$false") + "\n"
                    script += "function Stop-AppAndWaitFuseCleanup { throw 'injected cleanup failure' }\nfunction Get-ScenarioList { @(1..42) }\n"
                    script += "function Invoke-BasicAll { " + ("throw 'injected basic failure'" if fails else "Write-Host 'PASS [basic]' ") + " }\n"
                    script += report + entry + '\nWrite-Host "SCENARIO_COUNT=$(@($scenarios).Count)"\nWrite-Host "SUMMARY_COUNT=$($script:Summary.Count)"\n'
                    path = Path(directory) / "basic.ps1"
                    path.write_text(script, encoding="utf-8")
                    result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], text=True, encoding="utf-8", capture_output=True, timeout=20)
                self.assertEqual(0, result.returncode, result.stdout + result.stderr)
                self.assertIn(f"SCENARIO_COUNT={0 if fails and fail_fast else 42}", result.stdout)
                self.assertIn(f"SUMMARY_COUNT={1 if fails else 0}", result.stdout)
                self.assertEqual(1, result.stdout.splitlines().count("FAIL [basic]" if fails else "PASS [basic]"))

    @unittest.skipUnless(PWSH, "需要 PowerShell 验证应用启动期限")
    def test_powershell_app_launch_is_bounded_and_preserves_wait_mode(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Start-TestApp {") : source.index("function Invoke-Su {")]
        stubs = r'''
$ErrorActionPreference='Stop'
$AppId='test.app'
function Invoke-AdbWithTimeout {
    param([string[]]$Arguments, [int]$TimeoutSeconds)
    if ($TimeoutSeconds -ne 90) { throw 'unbounded launch' }
    if (($Arguments -contains '-W') -ne $WaitMode) { throw 'wrong wait mode' }
    if (($Arguments[-2..-1] -join '|') -ne '-n|test.app/.MainActivity') { throw 'wrong activity' }
    [pscustomobject]@{ExitCode=$Code; Output='injected output'}
}
'''
        for wait in (False, True):
            for code in (0, 7, 124):
                with self.subTest(wait=wait, code=code), tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "launch.ps1"
                    setup = f"$Code={code}; $WaitMode=" + ("$true" if wait else "$false") + "\n"
                    invoke = "try { Start-TestApp -Wait:$WaitMode; if ($Code -ne 0) { exit 2 } } catch { if ($Code -eq 0 -or $_ -notmatch ('exit=' + $Code + '：injected output')) { throw } }"
                    path.write_text(stubs + setup + function + invoke, encoding="utf-8")
                    result = subprocess.run([PWSH, "-NoProfile", "-File", str(path)], text=True, encoding="utf-8", capture_output=True, timeout=20)
                self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertNotIn('Invoke-Adb @("shell", "am", "start",', source)

    def test_system_uid_attribution_requires_current_hook_and_monitor_record(self):
        function = FLOW[FLOW.index("run_system_uid_attribution_scenario() {") : FLOW.index("run_shell_executor_boundary_scenario() {")]
        for ready, recorded in ((False, True), (True, False), (True, True)):
            with self.subTest(ready=ready, recorded=recorded):
                stubs = f"""
ensure_media_provider_hook_ready() {{ return {0 if ready else 1}; }}
adb_su() {{
  case "$1" in
    *MKDIR*) return {0 if recorded else 1} ;;
  esac
}}
adb() {{ :; }}
sleep() {{ :; }}
clear_file_monitor_log() {{ echo 开始探测; }}
BACKEND_ROOT=/backend
UID_ATTRIB_REL=SrtUidAttrib
UID_ATTRIB_PROBE_REL=SrtUidAttrib/probe
FILE_MONITOR_LOG_PATH=/monitor
"""
                result = subprocess.run(
                    [BASH, "-eu", "-c", stubs + function + "\nrun_system_uid_attribution_scenario 40"],
                    text=True, encoding="utf-8", capture_output=True, timeout=10,
                )
                self.assertEqual(0 if ready and recorded else 1, result.returncode, result.stderr)
                self.assertEqual(ready, "开始探测" in result.stdout)

    def test_system_uid_filters_require_executor_task_evidence(self):
        powershell = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        commands = {
            "bash": re.search(r'if adb_su "(grep -F \'\|MKDIR\|[^"\n]+)"; then', FLOW).group(1),
            "powershell": re.search(r'\$record = Invoke-Su "(grep -F \'\|MKDIR\|[^"\n]+)"', powershell).group(1),
        }
        base = "time|provider|mkdir|MKDIR|/storage/emulated/0/probe|ret=0|errno=0|identify_method=executor|identify_reliability=high|caller_uid=2000|caller_pid=42|caller_start_time=55|caller_context=u:r:shell:s0|caller_boundary=shell|native=mkdir"
        variants = [(base, True)] + [
            (base.replace(old, new), False) for old, new in [
                ("identify_method=executor", "identify_method=caller"),
                ("identify_reliability=high", "identify_reliability=low"),
                ("caller_uid=2000", "caller_uid=20000"),
                ("caller_pid=42", "caller_pid=0"),
                ("caller_start_time=55", "caller_start_time=0"),
                ("caller_context=u:r:shell:s0", "caller_context=u:r:ksu:s0"),
                ("caller_boundary=shell", "caller_boundary="),
                ("native=mkdir", "native=sh"),
                ("ret=0", "ret=-1"),
                ("/probe|", "/other|"),
            ]
        ]
        for runner, command in commands.items():
            for line, expected in variants:
                with self.subTest(runner=runner, line=line):
                    with tempfile.TemporaryDirectory() as directory:
                        log = Path(directory) / "monitor.log"
                        log.write_text(line + "\n", encoding="utf-8")
                        expanded = command.replace("${probe_rel}", "probe").replace("$probeRel", "probe")
                        expanded = expanded.replace("$FILE_MONITOR_LOG_PATH", str(log)).replace("$FileMonitorLogPath", str(log))
                        filtered = subprocess.run([BASH, "-euo", "pipefail", "-c", expanded], text=True, encoding="utf-8", capture_output=True, timeout=10)
                    matched = filtered.returncode == 0 and (runner == "bash" or bool(filtered.stdout.strip()))
                    self.assertEqual(expected, matched, filtered.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 执行系统 uid 归因回归")
    def test_powershell_system_uid_attribution_requires_current_hook_and_monitor_record(self):
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        function = source[source.index("function Invoke-SystemUidAttributionScenario {") : source.index("function Invoke-AppCallerMediaProviderAttributionScenario {")]
        for ready, recorded in ((False, True), (True, False), (True, True)):
            with self.subTest(ready=ready, recorded=recorded):
                stubs = """
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Confirm-MediaProviderHookReady { READY }
function Invoke-Su {
    param([string]$Command)
    if ($Command -like '*test -d*') { 'exit=0' }
    elseif ($Command -like '*MKDIR*' -and RECORDED) { '匹配记录' }
}
function Start-Process {
    Write-Host '开始探测'
    [pscustomobject]@{} | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {} -PassThru
}
function Start-Sleep {}
$BackendRoot='/backend'
$UidAttribRel='SrtUidAttrib'
$UidAttribProbeRel='SrtUidAttrib/probe'
$FileMonitorLogPath='/monitor'
$Serial='stub'
""".replace("READY", "$true" if ready else "$false").replace("RECORDED", "$true" if recorded else "$false")
                with tempfile.TemporaryDirectory() as directory:
                    script = Path(directory) / "system_uid.ps1"
                    script.write_text(stubs + function + "\nif (Invoke-SystemUidAttributionScenario 40) { exit 0 }; exit 1", encoding="utf-8")
                    result = subprocess.run(
                        [PWSH, "-NoProfile", "-File", str(script)],
                        text=True, encoding="utf-8", capture_output=True, timeout=20,
                    )
                self.assertEqual(0 if ready and recorded else 1, result.returncode, result.stderr)
                self.assertEqual(ready, "开始探测" in result.stdout)

    def test_private_cleanup_distinguishes_fuse_and_backend_failures(self):
        clean = FLOW[FLOW.index("clean_targets() {") : FLOW.index("clean_results() {")]
        lines = [line for line in clean.splitlines() if line.startswith('  adb_su "rm -rf') and "OWN_PRIVATE_DATA_ROOT" in line]
        self.assertEqual(2, len(lines))
        for backend_status in (0, 41):
            script = """
OWN_PRIVATE_DATA_ROOT=/storage/data
OWN_PRIVATE_MEDIA_ROOT=/storage/media
OWN_PRIVATE_OBB_ROOT=/storage/obb
BACKEND_OWN_PRIVATE_DATA_ROOT=/backend/data
BACKEND_OWN_PRIVATE_MEDIA_ROOT=/backend/media
BACKEND_OWN_PRIVATE_OBB_ROOT=/backend/obb
SANDBOX_OWN_PRIVATE_DATA_ROOT=/sandbox/data
SANDBOX_OWN_PRIVATE_MEDIA_ROOT=/sandbox/media
SANDBOX_OWN_PRIVATE_OBB_ROOT=/sandbox/obb
adb_su() {
  "$BASH" -n -c "$1" || return 2
  case "$1" in
    # quality-allow(chinese-language): shell路径匹配与返回码为执行式回归代码。
    *"/storage/"*) return 13 ;;
    # quality-allow(chinese-language): 后端返回码占位由测试参数替换，非面向用户文案。
    *"/backend/"*) return BACKEND_STATUS ;;
  esac
}
""".replace("BACKEND_STATUS", str(backend_status))
            result = subprocess.run([BASH, "-e", "-c", script + "\n".join(lines) + "\necho 完成"], text=True, encoding="utf-8", capture_output=True, timeout=10)
            self.assertEqual(backend_status, result.returncode, result.stderr)
            self.assertEqual(backend_status == 0, "完成" in result.stdout)

    def test_first_failure_preserves_artifacts_and_stops(self):
        loop = FLOW[FLOW.index("failure_artifacts_captured=0\n") : FLOW.index('if [ "${SRT_SKIP_FINAL_CLEANUP', FLOW.index("failure_artifacts_captured=0\n"))]
        stub = """
scenarios=(1 2)
SRT_SCENARIO_TIMEOUT_SECONDS=300
SRT_FAIL_FAST=1
fail=0
scenario_title() { echo test; }
cleanup_frozen_daemon() { echo daemon-resumed; }
timeout() {
  case "$*" in
    *run_scenario*) echo 场景已执行; return 7 ;;
    *capture_test_flow_artifacts*) echo 现场已采集 ;;
    *print_diagnostics*) echo 失败诊断 ;;
    *check_health*) echo 健康已检查 ;;
  esac
}
"""
        result = subprocess.run([BASH, "-c", stub + loop + '\nexit "$fail"'], text=True, encoding="utf-8", capture_output=True, timeout=10)
        self.assertEqual(1, result.returncode)
        self.assertEqual(1, result.stdout.count("场景已执行"))
        self.assertEqual(1, result.stdout.count("现场已采集"))
        self.assertLess(result.stdout.index("现场已采集"), result.stdout.index("健康已检查"))
        self.assertLess(result.stdout.index("现场已采集"), result.stdout.index("daemon-resumed"))

if __name__ == "__main__":
    unittest.main()
