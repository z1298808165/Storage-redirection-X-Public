import json
import re
import unittest

import yaml
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def section(source: str, start: str, end: str) -> str:
    return source[source.index(start) : source.index(end, source.index(start))]


class ScenarioConsistencyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        manifest = json.loads(read(".github/tests/storage-redirect-scenarios.json"))
        cls.scenarios = manifest["scenarios"]
        cls.ids = [item["id"] for item in cls.scenarios]
        cls.bash = read(".github/tests/run-storage-redirect-scenarios.sh")
        cls.powershell = read(".github/tests/run-storage-redirect-scenarios.ps1")

    def test_manifest_is_contiguous_and_unique(self) -> None:
        self.assertEqual(list(range(1, 30)), self.ids)
        self.assertEqual(len(self.ids), len(set(self.ids)))

    def test_both_runners_cover_every_config_and_title(self) -> None:
        bash_config = section(self.bash, "apply_config()", "target_path()")
        ps_config = section(self.powershell, "function Apply-ScenarioConfig", "function Clear-Results")
        bash_titles = section(self.bash, "scenario_title()", "clean_targets()")
        ps_titles = section(self.powershell, "function Get-ScenarioTitle", "function Invoke-WriteCase")

        self.assertEqual(self.ids, [int(value) for value in re.findall(r"(?m)^\s{4}(\d+)\)", bash_config)])
        self.assertEqual(self.ids, [int(value) for value in re.findall(r"(?m)^\s{8}(\d+)\s*\{", ps_config)])
        for item in self.scenarios:
            self.assertIn(f'{item["id"]}) echo "{item["bash_title"]}"', bash_titles)
            self.assertIn(f'{item["id"]} {{ "{item["powershell_title"]}" }}', ps_titles)

    def test_config_modes_match_runner_switches(self) -> None:
        bash_config = section(self.bash, "apply_config()", "target_path()")
        ps_config = section(self.powershell, "function Apply-ScenarioConfig", "function Clear-Results")
        for item in self.scenarios:
            scenario_id = item["id"]
            bash_block = re.search(rf"(?ms)^\s{{4}}{scenario_id}\)\n(.*?)(?=^\s{{4}}(?:\d+|\*)\))", bash_config)
            ps_block = re.search(rf"(?ms)^\s{{8}}{scenario_id}\s*\{{(.*?)(?=^\s{{8}}(?:\d+|default)\s*\{{)", ps_config)
            self.assertIsNotNone(bash_block, scenario_id)
            self.assertIsNotNone(ps_block, scenario_id)
            bash_text = bash_block.group(1)
            ps_text = ps_block.group(1)
            mode = item["config_mode"]
            if mode == "fuse":
                self.assertIn("enable_fuse_daemon_config", bash_text)
                self.assertIn("Enable-FuseDaemonConfig", ps_text)
            elif mode == "mount_namespace":
                self.assertIn("use_mount_namespace_fallback_config", bash_text)
                self.assertIn("Use-MountNamespaceFallbackConfig", ps_text)
            elif mode.startswith("monitor_"):
                self.assertIn("test_global_config", bash_text)
                self.assertIn("FileMonitorEnabled $true", ps_text)
                expected = "true true" if mode == "monitor_fuse" else "false true"
                self.assertIn(expected, bash_text)
                self.assertIn(f"FuseDaemonEnabled ${str(mode == 'monitor_fuse').lower()}", ps_text)

    def test_workflows_run_manifest_scenarios(self) -> None:
        expected = ",".join(str(value) for value in self.ids)
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            values = re.findall(r'SRT_SCENARIOS:\s*"([0-9,]+)"', read(workflow))
            self.assertTrue(values, workflow)
            self.assertTrue(all(value == expected for value in values), workflow)

    def test_workflow_optimizations_preserve_test_flow_gate(self) -> None:
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            prepare = section(source, "  prepare:", "  init-")
            init_job = section(source, "  init-", "  module:")
            app_job = section(source, "  app:", "  test-flow-build:")
            test_flow = section(source, "  test-flow:", "  test-flow-required:")
            self.assertNotIn("needs:", prepare)
            self.assertIn("- quality", init_job)
            self.assertIn("- prepare", init_job)
            self.assertIn(':app:testDebugUnitTest :app:assembleRelease', app_job)
            # app job 只构建 APK，不需要 NDK。断言「不安装 NDK」这件事本身，
            # 而不匹配具体版本号，否则升级 NDK 还要连带改测试。
            self.assertNotIn("ndk;", app_job)
            self.assertNotIn("fetch-depth: 0", test_flow)
            self.assertIn("- quality", test_flow)
            self.assertIn("needs.quality.result == 'success'", test_flow)
            # 矩阵级必须为 false：失败时保留 4 个 Android 版本的完整证据，避免只拿到
            # 单版本证据而反复返工。版本内的快速停止由 SRT_FAIL_FAST 负责。
            self.assertIn("fail-fast: false", test_flow)
            self.assertIn("SRT_FAIL_FAST: 1", test_flow)
            for version in (13, 14, 15, 16):
                self.assertIn(f"version: {version}", test_flow)
            required = source[source.index("  test-flow-required:") :]
            self.assertIn("needs.quality.result", required)
            self.assertIn("needs.test-flow.result", required)

    def test_gradle_cache_can_be_written_by_public_builds(self) -> None:
        ci = read(".github/workflows/ci.yml")
        release = read(".github/workflows/release.yml")
        self.assertEqual(3, ci.count("cache-encryption-key: ${{ secrets.GRADLE_ENCRYPTION_KEY }}"))
        self.assertEqual(3, release.count("cache-encryption-key: ${{ secrets.GRADLE_ENCRYPTION_KEY }}"))
        self.assertEqual(3, ci.count("cache-read-only: ${{ github.event_name == 'pull_request' }}"))
        self.assertEqual(3, release.count("cache-read-only: false"))

    def test_test_flow_reports_are_not_in_runtime_artifacts(self) -> None:
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            upload_start = source.index("Upload test-flow runtime") if "Upload test-flow runtime" in source else source.index("Upload release test-flow runtime")
            reports_start = source.index("unit test reports", upload_start)
            runtime_upload = source[upload_start:reports_start]
            self.assertNotIn("build/reports/", runtime_upload)
            self.assertIn("build/test-flow/assets/*.zip", runtime_upload)
            self.assertIn("build/outputs/apk/**/*.apk", runtime_upload)

    def test_workflows_share_runtime_build_and_failure_artifacts(self) -> None:
        expected_artifacts = {
            "scenario-*-result.txt",
            "test-flow-app-mountinfo.txt",
            "test-flow-logcat.txt",
            "test-flow-module-state.txt",
            "media-health.txt",
        }
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            build = section(source, "      - name: Build x86_64 module zip and test app", "      - name: Upload")
            test_flow = section(source, "  test-flow:", "  test-flow-required:")
            artifact = test_flow[test_flow.index("          path: |") :]

            self.assertIn("bash .github/scripts/build_test_flow_runtime.sh", build)
            self.assertIn("SRT_FAIL_FAST: 1", test_flow)
            self.assertIn("SRT_SKIP_FINAL_CLEANUP: 1", test_flow)
            self.assertIn("SRT_SCENARIO_TIMEOUT_SECONDS: 300", test_flow)
            for path in expected_artifacts:
                self.assertIn(path, artifact)

        script = read(".github/scripts/build_test_flow_runtime.sh")
        self.assertIn('cargo test --target "$TARGET_TRIPLE" --no-run', script)
        self.assertIn('cargo build --target "$TARGET_TRIPLE" --release', script)
        self.assertIn(":storageRedirectTestApp:assembleDebug", script)

    def test_every_job_is_bounded_and_build_jobs_wait_for_quality(self) -> None:
        # 缺少 timeout-minutes 的 job 会用 GitHub 的 6 小时默认值，一次挂起就吃满额度
        # 并占住 draft release 阻塞清理；构建 job 不等 quality 则会在格式或快速检查
        # 失败时白跑约 11 分钟机器时间。
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            parsed = yaml.safe_load(read(workflow))
            for name, job in parsed["jobs"].items():
                self.assertIsNotNone(job.get("timeout-minutes"), f"{workflow}:{name}")
            for name in ("module", "app", "test-flow-build"):
                job = parsed["jobs"][name]
                needs = job["needs"]
                needs = [needs] if isinstance(needs, str) else needs
                self.assertIn("quality", needs, f"{workflow}:{name}")

    def test_ndk_version_has_single_source(self) -> None:
        # 本地与 CI 使用不同 NDK 会构建出不同的 hook 实现，因此版本号不得散落在多处：
        # workflow 通过 SRX_NDK_VERSION 引用，本地脚本读 gradle.properties 的同一属性。
        properties = read("gradle.properties")
        match = re.search(r"(?m)^srx\.ndkVersion=(.+)$", properties)
        self.assertIsNotNone(match)
        version = match.group(1).strip()
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            self.assertEqual(1, source.count(version), workflow)
            self.assertIn('SRX_NDK_VERSION: "%s"' % version, source)
            self.assertIn('"ndk;$SRX_NDK_VERSION"', source)
        script = read("scripts/build-local-module.ps1")
        self.assertIn('Get-GradleProperty -Name "srx.ndkVersion"', script)

    def test_cargo_cache_key_excludes_sources(self) -> None:
        # key 含 src/** 会让每次源码改动都生成新缓存条目，只能靠 restore-keys 只读
        # 回退，随后仍写入一份新缓存，导致缓存无界增长并触发仓库级驱逐。
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            for line in re.findall(r"(?m)^\s*key: .*cargo.*$", source):
                self.assertNotIn("src/**", line, f"{workflow}: {line.strip()}")

    def test_scenario_roots_are_exported_to_subshells(self) -> None:
        # 场景函数由 `bash -c` 子 shell 执行，顶层赋值必须显式 export，
        # 否则场景内读到空串，断言路径会退化并失去校验意义。
        assigned = set(re.findall(r"(?m)^([A-Z][A-Z0-9_]*)=", self.bash))
        exported = set()
        for line in re.findall(r"(?m)^export (?!-f)(.*)$", self.bash):
            exported.update(line.split())
        self.assertEqual(set(), assigned - exported)

    def test_adb_su_propagates_remote_exit_status(self) -> None:
        # adb_su 用管道剥离 CR，退出码会取自末尾的 tr 而恒为 0；
        # 子 shell 中 SHELLOPTS 不继承，必须在函数内局部启用 pipefail，
        # 否则 check_file_exists 之类的断言会无条件通过。
        adb_su = section(self.bash, "adb_su()", "adb_write_file()")
        self.assertIn("local -", adb_su)
        self.assertIn("set -o pipefail", adb_su)
        self.assertLess(adb_su.index("set -o pipefail"), adb_su.index("adb_root"))

    def test_media_monitor_waits_for_restarted_provider_hook(self) -> None:
        bash_wait = section(
            self.bash,
            "wait_media_provider_hook_ready()",
            "print_storage_state()",
        )
        ps_wait = section(
            self.powershell,
            "function Wait-MediaProviderHookReady",
            "function Clear-Targets",
        )
        bash_scenario = section(
            self.bash,
            "run_file_monitor_mediastore_scenario()",
            "app_pid()",
        )
        ps_scenario = section(
            self.powershell,
            "function Invoke-MediaStoreMonitorScenario",
            "function Get-AppPid",
        )

        for source in (bash_wait, ps_wait):
            self.assertIn("java hook open ok", source)
            self.assertIn("media_provider_hook_retry", source)
            self.assertIn("storage.redirect.x/zygisk|libsrx_core", source)
        self.assertIn('if [ -n "$sdk" ] && [ "$sdk" -le 34 ]', bash_wait)
        self.assertIn("$sdk -le 34", ps_wait)
        self.assertIn("for attempt in 1 2", bash_wait)
        self.assertIn("$attempt -le 2", ps_wait)
        self.assertIn("restart_media_provider_with_hook_ready", bash_scenario)
        self.assertIn("Restart-MediaProviderWithHookReady", ps_scenario)
        self.assertLess(
            bash_scenario.index("restart_media_provider_with_hook_ready"),
            bash_scenario.index("run_file_monitor_mediastore_success_case"),
        )
        self.assertLess(
            ps_scenario.index("Restart-MediaProviderWithHookReady"),
            ps_scenario.index("Invoke-FileMonitorMediaStoreSuccessCase"),
        )


if __name__ == "__main__":
    unittest.main()
