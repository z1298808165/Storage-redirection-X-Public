"""实际执行场景入口，验证失败传播与首个现场保留，不连接设备。"""

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
LOG_PATH=/unused
APP_ID=test.app
REAL_ROOT=/unused
READ_ONLY_IMAGE_FILE=image.jpg
READ_ONLY_MEDIA_ROOT=/unused
PRIVATE_READ_ONLY_MEDIA_ROOT=/unused
"""
        with tempfile.TemporaryDirectory() as directory:
            return subprocess.run(
                [BASH, *options, "-c", stubs + overrides + function + f"\nrun_scenario {scenario}\n"],
                cwd=directory, text=True, encoding="utf-8", capture_output=True, timeout=10,
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

if __name__ == "__main__":
    unittest.main()
