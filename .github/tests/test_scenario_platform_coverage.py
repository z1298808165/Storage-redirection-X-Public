import os
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")


@unittest.skipUnless(BASH, "需要 Bash 验证受支持平台的场景覆盖")
class ScenarioPlatformCoverageTest(unittest.TestCase):

    def test_supported_versions_enter_hot_reload_assertions(self) -> None:
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        function = source[
            source.index("run_config_hot_reload_scenario() {") :
            source.index("run_backend_endpoint_recovery_scenario() {")
        ]
        script = """
SRT_FRESH_APP_PER_CASE=1
REAL_ROOT=/storage/emulated/0
PRIVATE_ROOT=/private
HOT_BEFORE_FILE=before.txt
HOT_AFTER_FILE=after.txt
app_pid() { echo 123; }
resume_hot_reload_app() { echo hot_reload_assertions_entered; return 42; }
""" + function + "\nrun_config_hot_reload_scenario 29\n"
        for arch, api in (("x86_64", 33), ("x86_64", 35), ("x86_64", 36), ("x86_64", 37), ("aarch64", 33)):
            with self.subTest(arch=arch, api=api):
                result = subprocess.run(
                    [BASH, "-eu", "-c", script],
                    text=True, capture_output=True, timeout=10,
                    env={**os.environ, "ANDROID_ARCH": arch, "ANDROID_API_LEVEL": str(api)},
                )
                skipped = arch == "x86_64" and api == 33
                self.assertEqual(0 if skipped else 1, result.returncode, result.stderr)
                self.assertEqual(not skipped, "hot_reload_assertions_entered" in result.stdout)
