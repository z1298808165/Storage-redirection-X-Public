"""验证缓存顺序、资产版本与摘要，以及 KernelSU 安装失败边界。"""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import zipfile

import yaml


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")


def read_yaml(path: str) -> dict:
    return yaml.safe_load((ROOT / path).read_text(encoding="utf-8"))


def action_steps(name: str) -> list[dict]:
    return read_yaml(f".github/actions/{name}/action.yml")["runs"]["steps"]


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / f".github/scripts/{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CiBuildCacheTests(unittest.TestCase):
    def test_workflows_share_caches_and_kernelsu_for_a15_to_a17(self):
        for workflow in ("ci.yml", "release.yml"):
            jobs = read_yaml(f".github/workflows/{workflow}")["jobs"]
            for job, target in (("module", "aarch64-linux-android"), ("test-flow-build", "x86_64-linux-android")):
                setup = next(s for s in jobs[job]["steps"] if s.get("uses") == "./.github/actions/setup-native-cache")
                self.assertEqual(target, setup["with"]["target"])
            flow = jobs["test-flow"]
            self.assertEqual([15, 16, 17], [e["version"] for e in flow["strategy"]["matrix"]["android"]])
            self.assertEqual("su0", flow["env"]["ADB_ROOT_MODE"])
            self.assertNotIn("MODULE_INSTALL_SCRIPT", flow["env"])
            steps = flow["steps"]
            emulator = next(i for i, s in enumerate(steps) if s.get("uses") == "./.github/actions/run-android-emulator")
            self.assertEqual("AvdLiteWithoutKsu", flow["env"]["ANDROID_TARGET"])
            self.assertEqual(flow["env"]["ANDROID_TARGET"], flow["env"]["AVD_NAME"])
            assets = {s["with"]["name"]: (i, s) for i, s in enumerate(steps) if s.get("uses") == "./.github/actions/cache-release-asset"}
            self.assertEqual({"zygisk-next", "kernelsu"}, set(assets))
            self.assertIn("lib/x86_64/libksud.so", assets["kernelsu"][1]["with"]["required-entries"])
            for name, (index, step) in assets.items():
                resolve = next(i for i, s in enumerate(steps) if f"resolve_test_release.py {name}" in s.get("run", ""))
                self.assertLess(resolve, index)
                self.assertLess(index, emulator)
                self.assertNotIn("if", step)
                for field in ("url", "version", "asset", "sha256"):
                    self.assertEqual("${{ steps." + name + "-release.outputs." + field + " }}", step["with"][field])
            sdk = next(i for i, s in enumerate(steps) if s.get("uses") == "./.github/actions/setup-android-emulator")
            self.assertLess(sdk, emulator)

    def test_sdk_caches_are_saved_after_installation(self):
        for action in ("setup-native-cache", "setup-android-emulator"):
            steps = action_steps(action)
            restores = {s["id"]: (i, s["with"]) for i, s in enumerate(steps) if s.get("uses", "").startswith("actions/cache/restore@")}
            for index, step in enumerate(steps):
                if not step.get("uses", "").startswith("actions/cache/save@"):
                    continue
                cache_id = step["if"].split(".")[1]
                restore_index, inputs = restores[cache_id]
                self.assertEqual(inputs, step["with"])
                self.assertTrue(any("sdkmanager" in s.get("run", "") for s in steps[restore_index:index]))
                self.assertNotIn("always()", step["if"])

    def test_emulator_cache_uses_verified_latest_stable_revisions(self):
        steps = action_steps("setup-android-emulator")
        resolve = next(i for i, s in enumerate(steps) if s.get("id") == "revisions")
        verify = next(i for i, s in enumerate(steps) if "android_sdk_revision.py verify" in s.get("run", ""))
        for index, step in enumerate(steps):
            if step.get("uses", "").startswith("actions/cache/"):
                self.assertLess(resolve, index)
                self.assertIn("steps.revisions.outputs.", step["with"]["key"])
                self.assertNotIn("restore-keys", step["with"])
                if "/save@" in step["uses"]:
                    self.assertLess(verify, index)
                else:
                    self.assertLess(index, verify)
        resolve_image = next(i for i, s in enumerate(steps) if s.get("id") == "image")
        cache_image = next(i for i, s in enumerate(steps) if s.get("uses") == "./.github/actions/cache-release-asset")
        imported = next(i for i, s in enumerate(steps) if "import_avdlite.py" in s.get("run", ""))
        self.assertLess(resolve_image, cache_image)
        self.assertLess(cache_image, imported)
        self.assertIn('resolve_test_release.py "avdlite-$ANDROID_API_LEVEL"', steps[resolve_image]["run"])
        self.assertEqual("${{ steps.image.outputs.sha256 }}", steps[cache_image]["with"]["sha256"])
        self.assertNotIn("system-images;", "\n".join(s.get("run", "") for s in steps))

    def test_sdk_revision_selection_and_installed_verification(self):
        resolver = load_script("android_sdk_revision")
        entries = []
        for package, major, minor, micro, channel, extra in (
            ("emulator", 37, 2, 13, 0, ""),
            ("emulator", 37, 3, 8, 2, ""),
            ("emulator", 37, 2, 9, 0, ""),
            ("emulator", 38, 0, 0, 0, "<preview>1</preview>"),
            ("system-images;android-35;google_apis;x86_64", 9, 0, 0, 0, ""),
        ):
            entries.append(f'<remotePackage path="{package}"><revision><major>{major}</major>'
                           f'<minor>{minor}</minor><micro>{micro}</micro>{extra}</revision>'
                           f'<channelRef ref="channel-{channel}"/><archives><archive/></archives></remotePackage>')
        entries.append('<remotePackage path="emulator" obsolete="true"><revision><major>99</major>'
                       '</revision><channelRef ref="channel-0"/><archives><archive/></archives></remotePackage>')
        entries.append('<remotePackage path="emulator"><revision><major>99</major></revision>'
                       '<channelRef ref="channel-0"/><archives><archive><host-os>windows</host-os>'
                       '</archive></archives></remotePackage>')
        xml = ("<repository>" + "".join(entries) + "</repository>").encode()
        self.assertEqual("37.2.13", resolver.latest_revision(xml, "emulator"))
        self.assertEqual("9.0.0", resolver.latest_revision(xml, "system-images;android-35;google_apis;x86_64"))
        with self.assertRaises(ValueError):
            resolver.latest_revision(xml, "system-images;android-36;google_apis;x86_64")
        with tempfile.TemporaryDirectory() as directory:
            properties = Path(directory) / "source.properties"
            for value, expected, valid in (("9", "9.0.0", True), ("37.2.13", "37.2.13", True),
                                           ("8", "9.0.0", False), ("9 rc1", "9.0.0", False)):
                properties.write_text(f"Pkg.Revision={value}\n")
                if valid:
                    resolver.verify_revision(properties, expected)
                else:
                    with self.assertRaises(ValueError):
                        resolver.verify_revision(properties, expected)

    @unittest.skipUnless(BASH, "Bash is required")
    def test_native_compiler_environment_is_target_specific(self):
        configure = next(s["run"] for s in action_steps("setup-native-cache") if s.get("name") == "配置编译缓存")
        for target in ("aarch64-linux-android", "x86_64-linux-android", "invalid"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as directory:
                env_file = Path(directory) / "environment"
                result = subprocess.run(
                    [BASH, "-eu", "-c", configure], capture_output=True, text=True, timeout=10,
                    env=dict(os.environ, ANDROID_TARGET=target, RUNNER_TEMP=directory, GITHUB_WORKSPACE=directory, GITHUB_ENV=str(env_file)),
                )
                if target == "invalid":
                    self.assertNotEqual(0, result.returncode)
                    self.assertFalse(env_file.exists())
                    continue
                self.assertEqual(0, result.returncode, result.stderr)
                values = dict(line.split("=", 1) for line in env_file.read_text().splitlines())
                for key, compiler in (("CC", "clang"), ("CXX", "clang++")):
                    self.assertEqual(f"ccache {target}29-{compiler}", values[f"{key}_{target.replace('-', '_')}"])
                self.assertEqual("ccache", values["CMAKE_C_COMPILER_LAUNCHER"])
                self.assertEqual("ccache", values["CMAKE_CXX_COMPILER_LAUNCHER"])

    def test_release_resolver_rejects_ambiguous_or_unverified_assets(self):
        resolver = load_script("resolve_test_release")
        for kind, repository, name, tag in (
            ("zygisk-next", "LSPosed/ZygiskNext", "Zygisk-Next-1.2.3-123-abcd-release.zip", "v1.2.3"),
            ("kernelsu", "tiann/KernelSU", "KernelSU_v1.2.3_123-release.apk", "v1.2.3"),
            ("avdlite-35", "PuerNya/AvdLite", "AvdLiteWithoutKsu-Android15-api35-4k.zip", "images"),
            ("avdlite-36", "PuerNya/AvdLite", "AvdLiteWithoutKsu-Android16-api36-4k.zip", "images"),
            ("avdlite-37.0", "PuerNya/AvdLite", "AvdLiteWithoutKsu-Android17-api37.0-4k.zip", "images"),
        ):
            url = f"https://github.com/{repository}/releases/download/{tag}/{name}"
            asset = {"name": name, "digest": "sha256:" + "a" * 64, "browser_download_url": url}
            release = {"tag_name": tag, "draft": False, "prerelease": False, "assets": [asset]}
            self.assertEqual({"version": tag, "asset": name, "sha256": "a" * 64, "url": url}, resolver.select_asset(release, kind))
            for invalid in (
                None, {**release, "draft": True}, {**release, "prerelease": True},
                {**release, "tag_name": "v1\ninjected=value"},
                {**release, "assets": None}, {**release, "assets": []},
                {**release, "assets": [asset, asset]},
                {**release, "assets": [{**asset, "digest": None}]},
                {**release, "assets": [{**asset, "name": name.replace("4k", "16k")}] } if kind.startswith("avdlite-") else None,
                {**release, "assets": [{**asset, "browser_download_url": "https://example.com/file.zip"}]},
            ):
                with self.subTest(kind=kind, invalid=invalid), self.assertRaises(ValueError):
                    resolver.select_asset(invalid, kind)

    def test_avdlite_import_rejects_wrong_image_or_unsafe_entries(self):
        importer = load_script("import_avdlite")
        manifest = {"image": {"api": "35", "page_size": "4k", "abi": "x86_64"}, "ksu": None}
        for case in ("valid", "api", "page_size", "abi", "ksu", "verification", "path"):
            with self.subTest(case=case):
                data = json.loads(json.dumps(manifest))
                if case in ("api", "page_size", "abi"):
                    data["image"][case] = {"api": "36", "page_size": "16k", "abi": "arm64-v8a"}[case]
                if case == "ksu":
                    data["ksu"] = {"tag": "old"}
                with zipfile.ZipFile(io.BytesIO(), "w") as archive:
                    archive.writestr("manifest.json", json.dumps(data))
                    archive.writestr("verification.json", json.dumps({"status": "failed" if case == "verification" else "passed"}))
                    if case == "path":
                        archive.writestr("../escape", b"invalid")
                    if case == "valid":
                        importer.validate_bundle(archive, "35")
                    else:
                        with self.assertRaises(ValueError):
                            importer.validate_bundle(archive, "35")

    @unittest.skipUnless(BASH, "Bash is required")
    def test_avdlite_launcher_checks_page_size_propagates_failure_and_cleans_up(self):
        source = (ROOT / ".github/scripts/run_avdlite.sh").read_text(encoding="utf-8")
        stubs = """
adb() {
  case "$*" in
    'shell getprop sys.boot_completed') echo 1;;
    'shell getconf PAGE_SIZE') echo "$MOCK_PAGE_SIZE";;
    'emu kill') echo cleaned > "$RUNNER_TEMP/cleanup";;
  esac
}
timeout() { shift; "$@"; }
sleep() { :; }
bash() { echo scenarios; return "$MOCK_SCENARIO_EXIT"; }
"""
        for page_size, scenario_exit, boot_timeout, expected in (
            ("4096", "0", "30", 0), ("4096", "7", "30", 7), ("16384", "0", "30", 1), ("4096", "0", "0", 1),
        ):
            with self.subTest(page_size=page_size, scenario_exit=scenario_exit, boot_timeout=boot_timeout), tempfile.TemporaryDirectory() as directory:
                emulator = Path(directory) / "emulator/emulator"
                emulator.parent.mkdir()
                emulator.write_text("#!/bin/sh\nexec sleep 600\n")
                emulator.chmod(0o755)
                result = subprocess.run(
                    [BASH, "-eu", "-c", stubs + source.replace("#!/usr/bin/env bash\n", "")],
                    env=dict(os.environ, ANDROID_HOME=directory, RUNNER_TEMP=directory, AVD_NAME="test",
                             BOOT_TIMEOUT=boot_timeout, EMULATOR_PORT="5554", EMULATOR_OPTIONS="-no-window",
                             DISABLE_HW_ACCEL="false", DISABLE_ANIMATIONS="true", MOCK_PAGE_SIZE=page_size,
                             MOCK_SCENARIO_EXIT=scenario_exit), capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(expected, result.returncode, result.stderr)
                self.assertEqual(page_size == "4096" and boot_timeout != "0", "scenarios" in result.stdout)
                self.assertTrue((Path(directory) / "cleanup").exists())

    @unittest.skipUnless(BASH and shutil.which("python3"), "Bash and python3 are required")
    def test_release_archive_validation_and_corrupt_cache_recovery(self):
        validate = next(s["run"] for s in action_steps("cache-release-asset") if "run" in s)
        for case in ("valid", "missing-entry", "corrupt-cache", "outdated-cache", "bad-digest"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                download = root / "download.zip"
                with zipfile.ZipFile(download, "w") as archive:
                    archive.writestr("module.prop", b"id=zygisksu")
                    if case != "missing-entry":
                        archive.writestr("lib/x86_64/libzygisk.so", b"payload")
                asset = root / "release-assets/zygisk-next/release.zip"
                asset.parent.mkdir(parents=True)
                asset.write_bytes(b"corrupt" if case == "corrupt-cache" else download.read_bytes())
                if case == "outdated-cache":
                    with zipfile.ZipFile(asset, "w") as archive:
                        archive.writestr("module.prop", b"old release")
                digest = "0" * 64 if case == "bad-digest" else hashlib.sha256(download.read_bytes()).hexdigest()
                stubs = 'curl() { cp "$MOCK_DOWNLOAD" "${@: -1}"; }\n'
                result = subprocess.run(
                    [BASH, "-eu", "-c", stubs + validate], capture_output=True, text=True, timeout=10,
                    env=dict(os.environ, RUNNER_TEMP=directory, GITHUB_ENV=str(root / "environment"), MOCK_DOWNLOAD=str(download),
                             ASSET_CACHE_NAME="zygisk-next", ASSET_URL="https://github.com/example/repo/releases/download/v1/release.zip",
                             ASSET_NAME=asset.name, ASSET_SHA256=digest, ASSET_ENV_NAME="ZYGISK_NEXT_CACHE_ZIP",
                             ASSET_REQUIRED_ENTRIES="module.prop,lib/x86_64/libzygisk.so"),
                )
                valid = case in ("valid", "corrupt-cache", "outdated-cache")
                self.assertEqual(valid, result.returncode == 0, result.stderr)
                self.assertEqual(valid, (root / "environment").exists())
                if valid:
                    self.assertEqual(download.read_bytes(), asset.read_bytes())

    @unittest.skipUnless(BASH and shutil.which("unzip"), "Bash and unzip are required")
    def test_kernelsu_daemon_extraction_requires_nonempty_x86_64_payload(self):
        source = (ROOT / ".github/tests/install-storage-redirect-module.sh").read_text(encoding="utf-8")
        function = source[source.index("ensure_ksud() {"):source.index("wait_for_root_shell() {")]
        for entry, payload, valid in (("lib/x86_64/libksud.so", b"ksud", True),
                                      ("lib/arm64-v8a/libksud.so", b"ksud", False),
                                      ("lib/x86_64/libksud.so", b"", False)):
            with self.subTest(entry=entry, payload=payload), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with zipfile.ZipFile(root / "KernelSU.apk", "w") as archive:
                    archive.writestr(entry, payload)
                script = 'adb_root() { echo installed; }\n'
                script += 'adb() { echo pushed; }\n' + function
                result = subprocess.run(
                    [BASH, "-eu", "-c", script + "\nensure_ksud"],
                    env=dict(os.environ, KSUAVD_DIR=directory), capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(valid, result.returncode == 0, result.stderr)
                self.assertEqual(valid, "pushed" in result.stdout)
                self.assertEqual(valid, "installed" in result.stdout)

    @unittest.skipUnless(BASH, "Bash is required")
    def test_kernelsu_install_preserves_order_and_bounds_retries(self):
        source = (ROOT / ".github/tests/install-storage-redirect-module.sh").read_text(encoding="utf-8")
        function = source[source.index("install_modules_with_ksud() {"):source.index("seed_storage_redirect_test_environment() {")]
        stubs = """
adb() { :; }
adb_root() { echo root "$1"; case "$1" in module_dir=*) eval "$1";; esac; }
assert_installed_module_files() { echo payload_checked; }
dump_root_diagnostics() { echo install_failed; }
sleep() { :; }
attempts=0
adb_ksud() {
  attempts=$((attempts + 1))
  echo "$1"
  [ "$FAIL_MODE" = never ] || { [ "$FAIL_MODE" = once ] && [ "$attempts" -gt 1 ]; }
}
KSUAVD_DIR=/unused
MODULE_ZIP=/unused.zip
"""
        for mode, payload, success in (("never", True, True), ("once", True, True),
                                       ("always", True, False), ("never", False, False)):
            with self.subTest(mode=mode, payload=payload), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "module.prop").write_text("id=zygisksu\n")
                if payload:
                    (root / "lib64").mkdir()
                    (root / "lib64/libzygisk.so").write_bytes(b"library")
                script = function.replace("/data/adb/modules_update/zygisksu", '"$MOCK_INSTALL_DIR"')
                result = subprocess.run(
                    [BASH, "-eu", "-c", stubs + script + "\ninstall_modules_with_ksud"],
                    env=dict(os.environ, FAIL_MODE=mode, MOCK_INSTALL_DIR=directory),
                    capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(success, result.returncode == 0, result.stderr)
                calls = [line for line in result.stdout.splitlines() if line.startswith("module install")]
                self.assertEqual(2 if mode != "once" else 3, len(calls))
                self.assertTrue(calls[0].endswith("zygisk-next.zip"))
                self.assertEqual(mode != "always", "payload_checked" in result.stdout)
                self.assertEqual(mode != "always", calls[-1].endswith("storage-redirect-x.zip"))
                self.assertLess(result.stdout.index("root sync"), result.stdout.index("module install"))

    @unittest.skipUnless(BASH, "Bash is required")
    def test_zygisk_injection_requires_active_status(self):
        source = (ROOT / ".github/tests/install-storage-redirect-module.sh").read_text(encoding="utf-8")
        function = source[source.index("verify_zygisk_injection_chain() {"):source.index("media_provider_pid() {")]
        for status in ("inject_state: 1", "inject_state:0", "inject_state:10", ""):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                script = 'adb_root() { printf "%s\\n" "$MOCK_STATUS"; }\n'
                script += function + '\nverify_zygisk_injection_chain'
                result = subprocess.run(
                    [BASH, "-eu", "-c", script], cwd=directory,
                    env=dict(os.environ, MOCK_STATUS=status), capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(status == "inject_state: 1", result.returncode == 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
