import os
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")


@unittest.skipUnless(BASH, "需要 Bash 验证受支持平台的场景覆盖")
class ScenarioPlatformCoverageTest(unittest.TestCase):
    def test_supported_versions_execute_all_mapping_routes(self) -> None:
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        function = source[
            source.index("run_any_path_mapping_scenario() {") :
            source.index("run_qq_alias_mapped_existing_file_scenario() {")
        ]
        script = """
APP_ID=test.app
PAYLOAD=test-payload
BACKEND_ROOT=/backend
BACKEND_PRIVATE_ROOT=/sandbox
ANY_RELATIVE_REQUEST=/relative
ANY_RELATIVE_PUBLIC_TARGET=/relative-target
ANY_ABSOLUTE_USER_REQUEST=/absolute
ANY_ABSOLUTE_PUBLIC_TARGET=/absolute-target
ANY_USER_ID_REQUEST=/user-id
ANY_LEGACY_DATA_REQUEST=/legacy
ANY_PUBLIC_TO_PRIVATE_REQUEST=/public-private
ANY_USER_PRIVATE_TARGET=/private-target
ANY_MEDIA_REQUEST=/media-request
ANY_MEDIA_TARGET=/media-target
ANY_MEDIA_FILE=media.bin
run_write_case() { echo "write:$2"; }
run_mediastore_download_create_case() { echo "media:$2"; }
check_file_exists() { echo "exists:$1"; test "$1" != "${FAIL_LABEL:-}"; }
check_file_missing() { echo "missing:$2"; test "$1" != "${FAIL_LABEL:-}"; }
""" + function + "\nrun_any_path_mapping_scenario 35\n"
        expected_writes = [
            "write:relative-android-data-to-public",
            "write:absolute-data-user-to-public",
            "write:absolute-legacy-alias-to-public",
            "write:user-id-data-user-to-private",
            "write:user-id-legacy-alias-to-private",
            "write:legacy-data-data-to-private",
            "write:legacy-canonical-alias-to-private",
            "write:public-to-absolute-private",
        ]
        directory_checks = ["missing:/public-private", "missing:/media-request"]
        missing_files = [
            "/backend/Android/data/test.app/srt_any_relative/srt_any_relative.txt",
            "/absolute/srt_any_absolute.txt", "/data/data/test.app/files/srt_any_absolute_alias.txt",
            "/user-id/srt_any_user_id.txt", "/data/data/test.app/cache/srt_any_user_id_alias.txt",
            "/legacy/srt_any_legacy.txt", "/data/user/0/test.app/code_cache/srt_any_legacy_alias.txt",
            "/public-private/srt_any_public_private.txt",
            "/sandbox/Download/SrtAnyPublicToPrivate/srt_any_public_private.txt",
            "/media-request/media.bin",
        ]
        for arch, api in (("x86_64", 33), ("x86_64", 35), ("x86_64", 36), ("x86_64", 37), ("aarch64", 33)):
            for fail_label in (
                "", "scenario-35-relative-public", "scenario-35-absolute-alias-public",
                "scenario-35-user-id-alias-private", "scenario-35-legacy-alias-private",
                "scenario-35-public-private-shadow", "scenario-35-public-private-dir-before",
                "scenario-35-media-request-dir-before", "scenario-35-public-private-dir-after",
                "scenario-35-media-request-dir-after",
            ):
                with self.subTest(arch=arch, api=api, fail_label=fail_label):
                    result = subprocess.run(
                        [BASH, "-eu", "-c", script],
                        text=True, capture_output=True, timeout=10,
                        env={**os.environ, "ANDROID_ARCH": arch, "ANDROID_API_LEVEL": str(api), "FAIL_LABEL": fail_label},
                    )
                    skipped = arch == "x86_64" and api == 33
                    self.assertEqual(0 if skipped or not fail_label else 1, result.returncode, result.stderr)
                    lines = result.stdout.splitlines()
                    writes = [line for line in lines if line.startswith("write:")]
                    expected = [] if skipped else expected_writes
                    stop_after = {
                        "scenario-35-public-private-dir-before": 0,
                        "scenario-35-media-request-dir-before": 0,
                        "scenario-35-relative-public": 1,
                        "scenario-35-absolute-alias-public": 3,
                        "scenario-35-user-id-alias-private": 5,
                        "scenario-35-legacy-alias-private": 7,
                    }.get(fail_label, len(expected))
                    expected = expected[:stop_after]
                    self.assertEqual(expected, writes)
                    reaches_media = not skipped and (not fail_label or fail_label.endswith("-dir-after"))
                    self.assertEqual(reaches_media, "media:mapped-mediastore-target" in lines)
                    if not skipped and not fail_label:
                        self.assertEqual(9, sum(line.startswith("exists:") for line in lines))
                        self.assertCountEqual(directory_checks, lines[:2])
                        self.assertCountEqual(directory_checks, lines[-2:])
                        checked_files = [line.removeprefix("missing:") for line in lines[2:-2] if line.startswith("missing:")]
                        self.assertCountEqual(missing_files, checked_files)

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
