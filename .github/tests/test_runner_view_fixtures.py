"""用真实运行器函数校验应用目录就绪与映射夹具失败传播。"""

import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
FLOW = (HERE / "run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")


def function(name):
    return re.search(rf"^{name}\(\) \{{.*?^\}}", FLOW, re.M | re.S).group()


@unittest.skipUnless(BASH, "需要 Bash 运行设备端命令桩")
class RunnerViewFixtures(unittest.TestCase):
    def run_shell(self, text, **variables):
        with tempfile.TemporaryDirectory() as work:
            return subprocess.run([BASH, "-s"], input=text, cwd=work,
                                  env=dict(os.environ, **variables), encoding="utf-8",
                                  capture_output=True, timeout=12)

    def test_app_view_results_and_process_changes(self):
        stubs = r'''
SRT_ARTIFACT_DIR=. SRT_APP_VIEW_TIMEOUT_SECONDS=3 SRT_RESULT_POLL_MS=1
APP_ID=test.app ACTION=test.action LAST_MOUNT_CONFIRMED_PID=31
RULE_SANDBOX_ROOT=/visible/dir READ_ONLY_ROOT=/visible/seed
fresh_app_enabled_for_label() { [[ $CASE == fresh ]]; }
sleep_ms() { :; }
timeout() { echo launch >> calls; [[ $CASE != transport ]]; }
app_pid() { if [[ $CASE == replacement && -f sent ]]; then echo 32; else echo 31; fi; }
wait_app_mount_confirmed() { echo confirm >> calls; LAST_MOUNT_CONFIRMED_PID=32; }
adb_su() {
  case $1 in
    mktemp*) echo /data/local/tmp/srx-ready.abcd ;;
    rm*) echo cleanup >> calls ;;
    nsenter*) nsenter() { return 7; }; ls() { return 17; }; eval "$1" ;;
  esac
}
wait_service_result() {
  [[ $2 == /data/local/tmp/srx-ready.abcd && $3 == file_stat ]] || return 1
  local mode=16877 path=/visible/dir
  case $CASE in
    file) mode=33188 ;;
    wrong-path) path=/visible/other ;;
    permission) echo 'FAIL [file_stat] EACCES'; return ;;
    partial) echo 'PASS [file_stat] stat path=/visible/dir, size=0, mode=16877, uid=1'; return 1 ;;
    delayed) if [[ ! -f sent ]]; then touch sent; echo 'FAIL [file_stat] ENOENT'; return; fi ;;
    replacement) if [[ ! -f sent ]]; then touch sent; mode=33188; fi ;;
    scenario9) path=/visible/seed ;;
  esac
  echo "PASS [file_stat] stat succeeded (1ms) path=$path, size=0, mode=$mode, uid=1"
}
'''
        cases = [ # 名称、场景、退出码、发起次数与诊断标记。
            ("ready", "8", 0, 1, "app_view_ready"), ("scenario9", "9", 0, 1, "/visible/seed"),
            ("file", "8", 1, 1, "not-directory"), ("wrong-path", "8", 1, 1, "app_view_not_ready"),
            ("permission", "8", 1, 1, "EACCES"), ("partial", "8", 1, 1, "result-unavailable"),
            ("transport", "8", 1, 1, "service-start"), ("delayed", "8", 0, 2, "app_view_ready"),
            ("replacement", "8", 0, 2, "pid=32"), ("fresh", "8", 0, 0, "fresh-app-per-case"),
            ("unused", "41", 0, 0, ""),
        ]
        for name, scenario, expected, launches, marker in cases:
            with self.subTest(name=name):
                script = stubs + function("logical_dir") + "\n" + function("wait_scenario_app_view")
                script += '\ntouch calls; wait_scenario_app_view "$SCENARIO"; rc=$?; cat calls; exit $rc\n'
                p = self.run_shell(script, CASE=name, SCENARIO=scenario)
                self.assertEqual(expected, p.returncode, p.stdout + p.stderr)
                self.assertIn(marker, p.stdout + p.stderr)
                self.assertEqual(launches, p.stdout.splitlines().count("launch"))
                self.assertEqual(launches, p.stdout.splitlines().count("cleanup"))
                if name == "replacement":
                    self.assertIn("confirm", p.stdout.splitlines())
                if expected:
                    self.assertIn("app_view_diag_rootprobe_rc=17", p.stderr)

    def test_fixture_commands_propagate_mkdir_and_type_failures(self):
        commands = (HERE / "prepare-any-path-directories.sh").read_text(encoding="utf-8")
        stubs = '''mkdir() { if [ "$2" = second ]; then return "$INJECT"; fi; command mkdir "$@"; }
chmod() { return 19; }
set -- 'first dir' second third
'''
        for code in (0, 23):
            with self.subTest(code=code):
                script = stubs + "(\n" + commands + '\n); rc=$?; test -d third && echo third; exit "$rc"'
                # 成功的 mkdir 也必须留下目录，覆盖错误对象类型和假成功。
                p = self.run_shell(script, INJECT=str(code))
                self.assertEqual(code or 1, p.returncode, p.stdout + p.stderr)
                self.assertNotIn("third", p.stdout)
        p = self.run_shell('chmod() { return 19; }; set -- "first dir" second\n' + commands + '\ntest -d "first dir" && test -d second')
        self.assertEqual(0, p.returncode, p.stderr)

    def test_hosts_only_prepare_backing_targets_and_check_source_directories(self):
        ps = (HERE / "run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        for script, request_names, target in [
            (FLOW, ("ANY_PUBLIC_TO_PRIVATE_REQUEST", "ANY_MEDIA_REQUEST"), "${BACKEND_ROOT}"),
            (ps, ("AnyPublicToPrivateRequest", "AnyMediaRequest"), "$BackendRoot"),
        ]:
            command = next(line for line in script.splitlines() if "set --" in line and "SrtAnyMediaTarget" in line)
            self.assertIn(target + "/Download/SrtAnyMediaTarget", command)
            for name in request_names:
                self.assertNotIn(name, command)
            self.assertIn("prepare-any-path-directories.sh", script)
        self.assertIn('prepare_any_path_targets || return 1', FLOW)
        self.assertIn('source-directory-before', ps)
        self.assertIn('source-directory-after', ps)
        for half in ('before', 'after'):
            for source in ('public-private', 'media-request'):
                self.assertIn(f'{source}-dir-{half}', FLOW)
