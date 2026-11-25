"""执行场景 33 的两个入口，区分本次重挂完成、旧记录和应用结果。"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
BASH = shutil.which("bash")
PWSH = shutil.which("pwsh")


def section(path, first, following):
    text = (ROOT / path).read_text(encoding="utf-8-sig")
    return text[text.index(first):text.index(following, text.index(first))]


@unittest.skipUnless(BASH and PWSH and os.name == "posix", "需要 POSIX Bash 和 PowerShell")
class RemountReceiptTest(unittest.TestCase):
    def test_hot_reload_wait_matches_request_and_provider(self):
        ctl = "assets/zygisk_module/bin/srxctl"
        funcs = section(ctl, "hot_reload_completed_count()", "request_running_app_remount()")
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            ack = work / "ack"
            log = work / "running.log"
            prefix = "12:34:56 I HotReload: media provider hot reload completed "
            current = "request=777-1 pid=42"
            boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
            command = funcs + '''
HOT_RELOAD_ACK_FILE=ack RUNNING_LOG_FILE=running.log
usleep() { :; }
wait_for_media_provider_hot_reload 777-1 42 "$1" 1 1
'''
            for label, message, record, before, ok in (
                ("new matching log", current, "", 0, True),
                ("unrelated request", "request=777-10 pid=42", "", 0, False),
                ("other provider", "request=777-1 pid=420", "", 0, False),
                ("existing completion", current, "", 1, False),
                ("matching ack", "", f"stage=hot_reload_ok {current} boot_id={boot}", 0, True),
                ("other boot", "", f"stage=hot_reload_ok {current} boot_id={boot}0", 0, False),
            ):
                with self.subTest(label=label):
                    log.write_text(prefix + message + "\n" if message else "")
                    ack.write_text(record + "\n")
                    run = subprocess.run([BASH, "-c", command, "test", str(before)], cwd=work,
                                         capture_output=True, timeout=5)
                    self.assertEqual(run.returncode, int(not ok), run.stderr)

    def test_request_completion_and_app_result(self):
        ctl = "assets/zygisk_module/bin/srxctl"
        control = (
            section(ctl, "running_app_remount_completed_count()", "wait_for_media_provider_hot_reload()")
            + section(ctl, "request_running_app_remount()", "\n}\n") + "\n}\n"
        )
        bash_flow = section(".github/tests/run-storage-redirect-scenarios.sh",
                            "run_quick_media_provider_restart_recovery_scenario()", "check_health()")
        ps_flow = section(".github/tests/run-storage-redirect-scenarios.ps1",
                          "function Invoke-QuickMediaProviderRestartRecoveryScenario", "function Invoke-TestArtifactCleanup")
        shell = r'''
app_pid() { echo 1001; }
media_provider_pid() { echo 2002; }
ensure_media_provider_hook_ready() { :; }
ensure_current_app_mount_confirmed() { :; }
wait_media_provider_ready() { :; }
wait_media_provider_hook_ready() { :; }
check_file_exists() { :; }
run_mediastore_image_create_case() { [ "$2" != quick-after ] || [ "$MODE" != app-failed ]; }
adb_su() {
  case "$1" in
    *srxctl\ remount-running) bash control ;;
    grep\ *) sh -c "$1" ;;
    *) return 99 ;;
  esac
}
LOG_PATH=running.log
''' + bash_flow + "\nrun_quick_media_provider_restart_recovery_scenario 33\n"
        powershell = r'''
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
$Failures=[Collections.Generic.List[string]]::new()
$LogPath='running.log'
function Confirm-MediaProviderHookReady { $true }
function Wait-MediaProviderReady { $true }
function Wait-MediaProviderHookReady { $true }
function Get-AppPid { '1001' }
function Get-MediaProviderPid { '2002' }
function Require-File { $true }
function Invoke-MediaStoreImageCreateCase { [pscustomobject]@{Ok=($args[1] -ne 'quick-after' -or $env:MODE -ne 'app-failed')} }
function Invoke-Su($command) {
    if ($command -like 'grep *') { & sh -c $command; return }
    if ($command -notlike '*srxctl remount-running') { throw $command }
    & bash control
}
function Test-Su($command) { & sh -c $command | Out-Null; return $LASTEXITCODE -eq 0 }
''' + ps_flow + "\nif (-not (Invoke-QuickMediaProviderRestartRecoveryScenario 33)) { exit 1 }\n"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "bin").mkdir()
            daemon = root / "bin/srx_daemon"
            daemon.write_text('''#!/bin/sh
case "$MODE" in
  failed) exit 23 ;;
  stale) exit 0 ;;
esac
printf 'running app remount completed request=%s applied=1\\n' "${2#reconcile-running:}" >>running.log
''', encoding="utf8")
            daemon.chmod(0o755)
            (root / "control").write_text(control + '''
MODDIR=. RUNNING_LOG_FILE=running.log
log_ctl() { :; }
remount_fail_reason() { :; }
is_daemon_alive() { return 0; }
usleep() { :; }
if [ "$MODE" = wrong-id ]; then echo 'running app remount completed request=42-17'; exit 0; fi
request_running_app_remount 1
''', encoding="utf8")
            (root / "scenario.sh").write_text(shell, encoding="utf8")
            (root / "scenario.ps1").write_text(powershell, encoding="utf8")
            for cmd in ([BASH, "scenario.sh"], [PWSH, "-NoProfile", "-File", "scenario.ps1"]):
                for mode in ("failed", "stale", "wrong-id", "ok", "app-failed"):
                    with self.subTest(host=cmd[0], mode=mode):
                        (root / "running.log").write_text("running app remount completed request=1-2 applied=1\n")
                        run = subprocess.run(cmd, cwd=root, env=dict(os.environ, MODE=mode),
                                             capture_output=True, text=True, timeout=20)
                        self.assertEqual(run.returncode, int(mode != "ok"), run.stdout + run.stderr)
                        if mode == "ok":
                            actual = (root / "running.log").read_text().splitlines()
                            self.assertEqual(len(actual), 2)
                            self.assertIn("remount-running exit=0 " + actual[1].removesuffix(" applied=1"), run.stdout)
                            self.assertIn(actual[1], run.stdout.splitlines())


if __name__ == "__main__":
    unittest.main()
