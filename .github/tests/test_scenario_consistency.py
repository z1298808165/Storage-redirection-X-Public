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


def powershell_case_values(label: str) -> list[str]:
    condition = re.search(r"@\(([^)]+)\)", label)
    if condition:
        return [value.strip() for value in condition.group(1).split(",")]
    return [value.strip() for value in re.split(r"[,|]", label) if value.strip()]


class ScenarioConsistencyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        manifest = json.loads(read(".github/tests/storage-redirect-scenarios.json"))
        cls.scenarios = manifest["scenarios"]
        cls.ids = [item["id"] for item in cls.scenarios]
        cls.bash = read(".github/tests/run-storage-redirect-scenarios.sh")
        cls.powershell = read(".github/tests/run-storage-redirect-scenarios.ps1")

    def test_manifest_is_contiguous_and_unique(self) -> None:
        self.assertEqual(list(range(1, max(self.ids) + 1)), self.ids)
        self.assertEqual(len(self.ids), len(set(self.ids)))

    def test_both_runners_cover_every_config_and_title(self) -> None:
        bash_config = section(self.bash, "apply_config()", "target_path()")
        ps_config = section(self.powershell, "function Apply-ScenarioConfig", "function Clear-Results")
        bash_titles = section(self.bash, "scenario_title()", "clean_targets()")
        ps_titles = section(self.powershell, "function Get-ScenarioTitle", "function Invoke-WriteCase")

        bash_config_ids = [
            int(value)
            for group in re.findall(r"(?m)^\s{4}([0-9|]+)\)", bash_config)
            for value in group.split("|")
        ]
        ps_config_ids = [
            int(value)
            for label in re.findall(
                r"(?m)^\s{8}((?:[0-9|, ]+)|(?:\{\s*\$_\s+-in\s+@\([^)]+\)\s*\}))\s*\{",
                ps_config,
            )
            for value in powershell_case_values(label)
        ]
        self.assertEqual(self.ids, sorted(bash_config_ids))
        self.assertEqual(self.ids, sorted(ps_config_ids))
        self.assertEqual(len(self.ids), len(set(bash_config_ids)))
        self.assertEqual(len(self.ids), len(set(ps_config_ids)))
        for item in self.scenarios:
            self.assertIn(f'{item["id"]}) echo "{item["bash_title"]}"', bash_titles)
            self.assertIn(f'{item["id"]} {{ "{item["powershell_title"]}" }}', ps_titles)

    def test_auto_backend_config_is_used_by_all_runner_switches(self) -> None:
        bash_config = section(self.bash, "apply_config()", "target_path()")
        ps_config = section(self.powershell, "function Apply-ScenarioConfig", "function Clear-Results")
        for item in self.scenarios:
            scenario_id = item["id"]
            bash_block = next(
                (
                    match
                    for match in re.finditer(
                        r"(?ms)^\s{4}([0-9|]+)\)\n(.*?)(?=^\s{4}(?:[0-9|]+|\*)\))",
                        bash_config,
                    )
                    if str(scenario_id) in match.group(1).split("|")
                ),
                None,
            )
            ps_block = next(
                (
                    match
                    for match in re.finditer(
                        r"(?ms)^\s{8}((?:[0-9|, ]+)|(?:\{\s*\$_\s+-in\s+@\([^)]+\)\s*\}))\s*\{(.*?)(?=^\s{8}(?:[0-9|, ]+|\{\s*\$_\s+-in\s+@\([^)]+\)\s*\}|default)\s*\{)",
                        ps_config,
                    )
                    if str(scenario_id) in powershell_case_values(match.group(1))
                ),
                None,
            )
            self.assertIsNotNone(bash_block, scenario_id)
            self.assertIsNotNone(ps_block, scenario_id)
            bash_text = bash_block.group(2)
            ps_text = ps_block.group(2)
            mode = item["config_mode"]
            self.assertNotIn("enable_fuse_daemon_config", bash_text)
            self.assertNotIn("Enable-FuseDaemonConfig", ps_text)
            self.assertNotIn("use_mount_namespace_fallback_config", bash_text)
            self.assertNotIn("Use-MountNamespaceFallbackConfig", ps_text)
            if mode.startswith("monitor_"):
                self.assertIn("FileMonitorEnabled $true", ps_text)
                self.assertIn('storage_backend_mode":"auto', self.bash)

    def test_workflows_run_manifest_scenarios(self) -> None:
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            values = re.findall(r'SRT_SCENARIOS:\s*"([^"]+)"', read(workflow))
            self.assertTrue(values, workflow)
            self.assertTrue(all(value == "all" for value in values), workflow)

        any_path_workflow = ROOT / ".github/workflows/ci-any-path.yml"
        if any_path_workflow.exists():
            any_path_values = re.findall(
                r'SRT_SCENARIOS:\s*"([^"]+)"', read(".github/workflows/ci-any-path.yml")
            )
            self.assertTrue(any_path_values)
            self.assertTrue(all(value == "all" for value in any_path_values))

    def test_all_selector_expands_to_every_manifest_scenario(self) -> None:
        expected_max = max(self.ids)
        self.assertIn(f"scenarios=($(seq 1 {expected_max}))", self.bash)
        self.assertIn(f"return @(1..{expected_max})", self.powershell)

    def test_any_path_workflow_runs_all_android_test_flow_shards(self) -> None:
        if not (ROOT / ".github/workflows/ci-any-path.yml").exists():
            self.skipTest("主分支不包含实验分支专用 workflow")
        workflow = read(".github/workflows/ci-any-path.yml")
        for job in ("prepare:", "module:", "app:", "test-flow-build:", "test-flow:"):
            self.assertIn(f"  {job}", workflow)
        self.assertIn("test-flow-android17:", workflow)
        self.assertIn("api: 33", workflow)
        self.assertIn("api: 34", workflow)
        self.assertIn("api: 35", workflow)
        self.assertIn("api: 36", workflow)
        self.assertIn('ANDROID_API_LEVEL: "37.0"', workflow)
        self.assertIn("disable-linux-hw-accel: false", workflow)
        self.assertIn("test-flow-required:", workflow)
        self.assertIn("upload-branch-assets:", workflow)
        self.assertIn("build/test-flow/assets/*.zip", workflow)
        self.assertIn("*.apk", workflow)

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
            # 矩阵级为 true：任一 Android 版本失败即取消其余，尽快释放模拟器资源。
            # 需要跨版本对比证据时用不带 --failed 的完整重跑单独获取。
            # 版本内的快速停止仍由 SRT_FAIL_FAST 负责。
            self.assertIn("fail-fast: true", test_flow)
            self.assertIn("SRT_FAIL_FAST: 1", test_flow)
            for version in (13, 14, 15, 16):
                self.assertIn(f"version: {version}", test_flow)
            required = source[source.index("  test-flow-required:") :]
            self.assertIn("needs.quality.result", required)
            self.assertIn("needs.test-flow.result", required)

    def test_android17_flow_is_integrated_without_diagnostic_artifact_upload(self) -> None:
        source = read(".github/workflows/ci.yml")
        experimental = section(source, "  test-flow-android17:", "  test-flow-required:")
        self.assertIn("github.ref_name == 'SRX-R' || github.base_ref == 'SRX-R'", experimental)
        self.assertIn("ANDROID_TARGET: google_apis", experimental)
        self.assertIn("emulator-options: -no-window -gpu swiftshader_indirect", experimental)
        self.assertIn("EMULATOR_GPU_MODE: swiftshader_indirect", experimental)
        self.assertIn('ANDROID_API_LEVEL: "37.0"', experimental)
        self.assertIn("SRT_FRESH_APP_PER_CASE: 0", experimental)
        self.assertIn("MAGISK_URL: https://github.com/topjohnwu/Magisk/releases/download/v30.7/Magisk-v30.7.apk", experimental)
        self.assertIn("Download test-flow runtime", experimental)
        self.assertNotIn("Upload Android 17 diagnostic artifacts", experimental)
        self.assertNotIn("actions/upload-artifact@v7.0.1", experimental)
        self.assertNotIn("gh release", experimental)
        self.assertNotIn("update.json", experimental)
        required = section(source, "  test-flow-required:", "  create-ci-release:")
        self.assertIn("- test-flow-android17", required)
        self.assertIn("needs.test-flow-android17.result", required)

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
            "test-flow-backend-diagnostic.txt",
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

    def test_initial_storage_recovery_reboots_only_after_timeout(self) -> None:
        recovery = section(
            self.bash,
            "ensure_initial_storage_ready()",
            "media_provider_query_ready()",
        )
        self.assertIn('if wait_storage_ready "initial" 60', recovery)
        self.assertIn("adb reboot", recovery)
        self.assertIn('wait_storage_ready "initial-reboot" 120', recovery)
        self.assertLess(recovery.index("wait_storage_ready"), recovery.index("adb reboot"))
        startup = self.bash[self.bash.index("wait_boot_completed\nbackup_global_config") :]
        self.assertIn("ensure_initial_storage_ready", startup)
        self.assertLess(startup.index("backup_global_config"), startup.index("ensure_initial_storage_ready"))

    def test_windows_test_flow_keeps_python_command_as_object(self) -> None:
        script = read("scripts/verify-test-flow.ps1")
        resolver = section(script, "function Get-PythonCommand", "function Get-ResolvedVersionData")
        version = section(script, "function Get-ResolvedVersionData", "function New-ModulePackage")
        self.assertIn("[pscustomobject]", resolver)
        self.assertIn("$candidate.FilePath", resolver)
        self.assertIn("@($candidate.Arguments)", resolver)
        self.assertNotIn("$candidate[0]", resolver)
        self.assertIn("$python.FilePath", version)
        self.assertIn("@($python.Arguments)", version)

    def test_windows_test_flow_polls_boot_property_without_remote_shell_expression(self) -> None:
        script = read("scripts/verify-test-flow.ps1")
        wait = section(script, "function Wait-DeviceBootCompleted", "function Assert-ModuleRuntimeState")
        self.assertIn('Invoke-Checked -FilePath "adb" -Arguments @("wait-for-device")', wait)
        self.assertIn("adb shell getprop sys.boot_completed", wait)
        self.assertIn('$bootCompleted -eq "1"', wait)
        self.assertNotIn("while [", script)
        self.assertEqual(2, script.count("Wait-DeviceBootCompleted\n"))

    def test_basic_all_leaves_media_cleanup_to_the_device_runner(self) -> None:
        runner = read(
            "tests/storage-redirect-test/app/src/main/java/"
            "me/fakerqu/test/storageredirect/test/StorageRedirectTestRunner.kt"
        )
        all_case = section(runner, "private fun runAllExceptDelete", "private fun runLogged")
        self.assertNotIn("contentResolver.delete", all_case)
        self.assertNotIn("createdMedia", all_case)
        self.assertIn("cleanupBootstrapDirs(bootstrapDirs)", all_case)
        self.assertIn("Remove-RandomMediaStoreRows", self.powershell)
        self.assertIn("remove_random_mediastore_rows", self.bash)

    def test_device_flow_prevents_and_restores_background_freezing(self) -> None:
        for source in (self.powershell, self.bash):
            self.assertIn("stay_on_while_plugged_in", source)
            self.assertIn("WAKEUP", source)
            self.assertIn("dismiss-keyguard", source)
            self.assertIn("get-inactive", source)
            self.assertIn("set-inactive", source)
            self.assertIn("deviceidle", source)
        self.assertIn("OriginalAppInactive", self.powershell)
        self.assertIn('",${APP_ID},"', self.bash)
        self.assertIn("original_app_inactive", self.bash)

        ps_start = self.powershell[self.powershell.index("try {\n    Backup-GlobalConfig") :]
        self.assertLess(
            ps_start.index("Backup-DeviceExecutionState"),
            ps_start.index("Prepare-DeviceExecutionState"),
        )
        ps_cleanup = section(
            self.powershell,
            "function Invoke-TestArtifactCleanup",
            "function Restart-App",
        )
        self.assertLess(
            ps_cleanup.index("Restart-MediaProvider"),
            ps_cleanup.index("Restore-DeviceExecutionState"),
        )

        bash_start = self.bash[self.bash.index("wait_boot_completed\nbackup_global_config") :]
        self.assertLess(
            bash_start.index("backup_device_execution_state"),
            bash_start.index("prepare_device_execution_state"),
        )
        bash_cleanup = section(self.bash, "cleanup_test_artifacts()", "latest_result()")
        self.assertLess(
            bash_cleanup.index("restart_media_provider"),
            bash_cleanup.index("restore_device_execution_state"),
        )

    def test_no_config_scenario_disables_automatic_app_enablement(self) -> None:
        for source in (self.powershell, self.bash):
            self.assertIn('"auto_enable_redirect_for_new_apps":false', source)
            self.assertIn('"app_config_auto_save":false', source)

    def test_scoped_fuse_start_check_ignores_clean_session_end(self) -> None:
        bash_check = section(
            self.bash,
            "check_scoped_fuse_daemon_started()",
            "run_fuse_daemon_allow_wildcard_scenario()",
        )
        ps_check = section(
            self.powershell,
            "function Test-ScopedFuseDaemonStarted",
            "function Invoke-RuleSandboxScenario",
        )

        for source in (bash_check, ps_check):
            self.assertIn("fuse redirect mount start", source)
            self.assertIn("fuse redirect mount failed", source)
            self.assertIn("daemon hybrid fuse scoped service failed", source)
            self.assertNotIn("fuse redirect session ended", source)
            self.assertLess(
                source.index("fuse redirect mount start"),
                source.index("fuse redirect mount failed"),
            )

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
            self.assertIn("stage=init_ok", source)
            self.assertIn("boot_id", source)
            self.assertIn("media_provider_hook_retry", source)
            self.assertIn("storage.redirect.x/zygisk|libsrx_core", source)
        self.assertNotIn("skip_media_provider_restart", bash_wait)
        self.assertIn("stage=init_ok pid=${pid} boot_id=${boot_id}", bash_wait)
        self.assertNotIn("skip_media_provider_restart", ps_wait)
        self.assertIn("stage=init_ok pid=$mediaPid boot_id=$bootId", ps_wait)
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

    def test_initial_and_scenario_two_recover_missing_media_provider_hook(self) -> None:
        bash_standard = section(self.bash, "run_standard_scenario()", "run_scenario()")
        ps_standard = section(
            self.powershell,
            "function Invoke-StandardScenario",
            "function Set-ReadOnlySeed",
        )
        self.assertIn('ensure_media_provider_hook_ready "initial"', self.bash)
        self.assertIn('ensure_media_provider_hook_ready "scenario-${scenario}-before-mediastore"', bash_standard)
        self.assertIn('Confirm-MediaProviderHookReady "initial"', self.powershell)
        self.assertIn('Confirm-MediaProviderHookReady "scenario-$Scenario-before-mediastore"', ps_standard)
        self.assertLess(
            bash_standard.index("ensure_media_provider_hook_ready"),
            bash_standard.index('run_service_case "$scenario" "mediastore-sandbox-only"'),
        )
        self.assertLess(
            ps_standard.index("Confirm-MediaProviderHookReady"),
            ps_standard.index('Invoke-ServiceCase "scenario-$Scenario" "mediastore-sandbox-only"'),
        )

    def test_backend_endpoint_recovery_keeps_app_pid(self) -> None:
        bash_scenario = section(
            self.bash,
            "run_backend_endpoint_recovery_scenario()",
            "run_standard_scenario()",
        )
        ps_scenario = section(
            self.powershell,
            "function Invoke-BackendEndpointRecoveryScenario",
            "function Invoke-TestArtifactCleanup",
        )
        self.assertIn('am force-stop "$APP_ID"', bash_scenario)
        self.assertIn("app-restart", bash_scenario)
        self.assertIn("backend_recovery", bash_scenario)
        self.assertIn("pid", bash_scenario)
        self.assertIn("Restart-App", ps_scenario)
        self.assertIn("app-restart", ps_scenario)
        self.assertIn("MediaProvider", ps_scenario)
        self.assertIn("backend recovery", ps_scenario)
        self.assertIn("pid", ps_scenario)

    def test_quick_media_provider_hot_reload_preserves_processes(self) -> None:
        bash_scenario = section(
            self.bash,
            "run_quick_media_provider_restart_recovery_scenario()",
            "check_health()",
        )
        ps_scenario = section(
            self.powershell,
            "function Invoke-QuickMediaProviderRestartRecoveryScenario",
            "function Invoke-TestArtifactCleanup",
        )
        for source in (bash_scenario, ps_scenario):
            self.assertIn("srxctl remount-running", source)
            self.assertIn("running app remount completed request=", source)
            self.assertIn("quick-before", source)
            self.assertIn("quick-after", source)
        self.assertIn("media_provider", bash_scenario)
        self.assertIn("MediaProvider", ps_scenario)
        self.assertIn("quick_restart_media_provider_pid_changed", bash_scenario)
        self.assertIn("quick restart MediaProvider pid changed", ps_scenario)
        self.assertIn("quick_restart_app_preserved", bash_scenario)
        self.assertIn("quick restart unexpectedly changed running app pid", ps_scenario)
        self.assertIn("quick_restart_app_unexpectedly_changed", bash_scenario)
        self.assertIn("quick restart app pid changed", ps_scenario)
        self.assertIn("wait_media_provider_hook_ready", bash_scenario)
        self.assertIn("Wait-MediaProviderHookReady", ps_scenario)
        self.assertIn("start_app_and_confirm_mount", bash_scenario)
        self.assertIn("Restart-App", ps_scenario)

    def test_quick_media_provider_hot_reload_keeps_processes_running(self) -> None:
        source = read("assets/zygisk_module/bin/srxctl")
        restart = section(source, "restart_media_provider() {", "start_collectors_if_needed() {")
        self.assertIn("write_media_provider_hot_reload_request", restart)
        self.assertIn("signal_media_provider_hot_reload", restart)
        self.assertIn("kill -USR2", source)
        self.assertIn("wait_for_media_provider_hot_reload", restart)
        self.assertNotIn("am force-stop", restart)
        self.assertNotIn("kill -9", restart)
        self.assertNotIn("kill_package_processes", restart)
        self.assertNotIn("srx_restart_running_app", source)
        self.assertIn('request_running_app_remount "$require_running_remount"', restart)
        self.assertIn('[ "$required" = "1" ] && return 1', source)

    def test_quick_media_provider_restart_records_app_after_before_case(self) -> None:
        bash_scenario = section(
            self.bash,
            "run_quick_media_provider_restart_recovery_scenario()",
            "check_health()",
        )
        ps_scenario = section(
            self.powershell,
            "function Invoke-QuickMediaProviderRestartRecoveryScenario",
            "function Invoke-TestArtifactCleanup",
        )
        for source in (bash_scenario, ps_scenario):
            before_case = source.index("quick-before")
            app_pid = source.index("initial_pid", before_case) if "initial_pid" in source else source.index("initialPid", before_case)
            self.assertGreater(app_pid, before_case)

    def test_mount_confirmation_ignores_stale_same_package_pid(self) -> None:
        bash_wait = section(self.bash, "wait_app_mount_confirmed()", "scenario_from_label()")
        ps_wait = section(self.powershell, "function Wait-AppMountConfirmed", "function Wait-MediaProviderReady")
        self.assertIn("for (i=1; i<=NF; i++)", bash_wait)
        self.assertIn("END {if (max !=", bash_wait)
        self.assertIn("for (i=1; i<=NF; i++)", ps_wait)
        self.assertIn("END {if (max !=", ps_wait)

    def test_auto_fuse_parent_mount_covers_nested_mapping(self) -> None:
        mount_paths = section(
            self.bash,
            "expected_mount_paths_for_label()",
            "app_mountinfo_has_expected_paths()",
        )
        scenario_four = re.search(r"(?ms)^    4\)\n(.*?)(?=^\s*esac)", mount_paths)
        self.assertIsNotNone(scenario_four)
        self.assertIn('"${REAL_ROOT}/Download"', scenario_four.group(1))
        self.assertNotIn('"${REAL_ROOT}/Download/SrtProbe"', scenario_four.group(1))

    def test_app_restart_waits_for_previous_process_to_exit(self) -> None:
        bash_start = section(self.bash, "start_app_and_confirm_mount()", "wait_storage_ready()")
        bash_wait = section(self.bash, "wait_app_process_stopped()", "resume_hot_reload_app()")
        ps_stop = section(self.powershell, "function Stop-AppAndWaitFuseCleanup", "function Invoke-ConfigHotReloadScenario")
        ps_wait = section(self.powershell, "function Wait-AppProcessStopped", "function Test-AppHasNoStaleFuseMount")
        self.assertIn("wait_app_process_stopped 10", bash_start)
        self.assertIn('previous_pid="$(app_pid)"', bash_start)
        self.assertIn('quick-initial-app', bash_start)
        self.assertIn("app_pid", bash_wait)
        self.assertIn("Wait-AppProcessStopped", ps_stop)
        self.assertIn('quick-initial-app', ps_stop)

        quick_bash = section(self.bash, "run_quick_media_provider_restart_recovery_scenario()", "check_health()")
        quick_ps = section(self.powershell, "function Invoke-QuickMediaProviderRestartRecoveryScenario", "function Get-TargetPath")
        self.assertIn("ensure_current_app_mount_confirmed", quick_bash)
        self.assertIn("Get-AppPid", quick_ps)
        self.assertIn("Wait-AppMountConfirmed", quick_ps)
        self.assertIn("Get-AppPid", ps_wait)

    def test_media_provider_hot_reload_protocol_is_present(self) -> None:
        source = read("src/java_hook/hot_reload.rs")
        java_hook = read("src/java_hook.rs")
        specialize_post = read("src/lifecycle/specialize_post.rs")
        srxctl = read("assets/zygisk_module/bin/srxctl")
        webui = read("assets/zygisk_module/webroot/js/api.js")
        self.assertIn("MEDIA_PROVIDER_HOT_RELOAD_REQUEST_FILE", source)
        self.assertIn("MEDIA_PROVIDER_HOT_RELOAD_ACK_FILE", source)
        self.assertIn("stage=hot_reload_ok", source)
        self.assertIn("SIGUSR2", source)
        self.assertIn("SIGNAL_PENDING", source)
        self.assertIn("hot_reload_completed_count", srxctl)
        self.assertIn('boot_id=$boot_id"', srxctl)
        self.assertIn("stage=hot_reload_ok", srxctl)
        self.assertIn('withSrxCtlFallback("remount-running", "exit 1")', webui)
        self.assertNotIn("restartMediaProviderHotReloadFallbackCommand", webui)
        self.assertNotIn('kill -9 "$pid"', webui)
        self.assertIn('grep -F "media provider hot reload completed"', srxctl)
        self.assertIn('grep -F "pid=$provider_pid"', srxctl)
        self.assertIn("start_hot_reload_after_specialize", java_hook)
        init_section = section(java_hook, "fn init(", "pub fn start_hot_reload_after_specialize")
        self.assertNotIn("hot_reload::start();", init_section)
        self.assertIn("java_hook::start_hot_reload_after_specialize();", specialize_post)

    def test_hot_reload_has_no_manual_app_restart_notice(self) -> None:
        srxctl = read("assets/zygisk_module/bin/srxctl")
        web_api = read("assets/zygisk_module/webroot/js/api.js")
        web_app = read("assets/zygisk_module/webroot/js/app.js")
        controller = read("app/src/main/java/org/srx/manager/data/RootModuleController.kt")
        dashboard = read("app/src/main/java/org/srx/manager/ui/screen/DashboardScreen.kt")
        for source in (srxctl, web_api, web_app, controller, dashboard):
            self.assertNotIn("srx_restart_running_app", source)
            self.assertNotIn("MediaProviderRestartNotice", source)
        self.assertNotIn("showMediaProviderRestartNotice", web_app)
        self.assertIn("重新挂载运行中应用", dashboard)

    def test_remount_ui_reports_completion_and_serializes_requests(self) -> None:
        view_model = read("app/src/main/java/org/srx/manager/ui/SrxViewModel.kt")
        web_app = read("assets/zygisk_module/webroot/js/app.js")
        self.assertIn("private val remountMutex = Mutex()", view_model)
        self.assertIn("正在重新挂载运行中应用（等待完成）", view_model)
        self.assertIn("finally {", section(view_model, "fun restartMediaProvider()", "fun refreshLogs()"))
        self.assertIn("mediaProviderReloadRunning: false", web_app)
        self.assertIn("if (State.mediaProviderReloadRunning) return;", web_app)
        self.assertIn("State.mediaProviderReloadRunning = false", section(web_app, "async function restartMediaProviderWithLoading()", "// ═══ Logs"))

    def test_running_app_remount_is_requested_after_provider_hot_reload(self) -> None:
        srxctl = read("assets/zygisk_module/bin/srxctl")
        daemon = read("src/daemon.rs")
        log_daemon = read("src/log_daemon.rs")
        self.assertIn("request_running_app_remount", srxctl)
        self.assertIn('control "reconcile-running:$request_id"', srxctl)
        self.assertIn("running_app_remount_completed_count", srxctl)
        self.assertIn("running app remount timed out request=", srxctl)
        self.assertIn('if ! request_running_app_remount "$require_running_remount"; then', srxctl)
        self.assertIn("take_reconcile_request", daemon)
        self.assertIn("control_reconcile", daemon)
        self.assertIn("running app remount completed request=", daemon)
        self.assertIn('const CONTROL_RECONCILE_RUNNING: &str = "reconcile-running"', log_daemon)
        self.assertIn("RECONCILE_REQUEST", log_daemon)

    def test_module_boot_recovers_missing_media_provider_hook_once(self) -> None:
        install = read(".github/tests/install-storage-redirect-module.sh")
        wait = section(
            install,
            "wait_media_provider_hook_ready()",
            "verify_media_provider_hook_with_reboot_retry()",
        )
        recovery = section(
            install,
            "verify_media_provider_hook_with_reboot_retry()",
            "install_test_app_before_module_boot",
        )

        self.assertIn("stage=init_ok pid=${pid} boot_id=${boot_id}", wait)
        self.assertNotIn("media_provider_hook_check_skipped", wait)
        self.assertIn("stage=init_ok pid=${pid} boot_id=${boot_id}", wait)
        self.assertIn('wait_media_provider_hook_ready "module-boot" 60', recovery)
        self.assertIn("adb reboot", recovery)
        self.assertIn('wait_media_provider_hook_ready "module-clean-boot" 120', recovery)
        self.assertEqual(1, recovery.count("adb reboot"))
        self.assertLess(
            recovery.index('wait_media_provider_hook_ready "module-boot"'),
            recovery.index("adb reboot"),
        )
        self.assertLess(
            recovery.index("adb reboot"),
            recovery.index('wait_media_provider_hook_ready "module-clean-boot"'),
        )
        self.assertIn("defer_media_provider_hook_check_for_lazy_provider", install)
        self.assertIn("MediaProvider 采用惰性启动", install)

    def test_android17_skips_provider_restart_that_detaches_fuse_storage(self) -> None:
        source = read(".github/tests/run-storage-redirect-scenarios.sh")
        restart = section(source, "restart_media_provider() {", "ensure_monitor_collector()")
        self.assertIn('[ "$sdk" -ge 37 ]', restart)
        self.assertIn("can detach emulated storage", restart)

        boot = read("assets/zygisk_module/service.d/boot.sh")
        deferred = section(boot, "restart_media_provider_for_deferred_hooks() {", "  media_pkgs=")
        self.assertIn('getprop ro.build.version.sdk', deferred)
        self.assertIn('skip MediaProvider restart for lazy Android', deferred)

    def test_test_app_install_waits_for_package_service(self) -> None:
        install = read(".github/tests/install-storage-redirect-module.sh")
        section_text = section(install, "install_test_app_before_module_boot()", "seed_storage_redirect_test_environment()")
        self.assertIn("cmd package list packages", section_text)
        self.assertIn("APP_INSTALL_ATTEMPTS", section_text)
        self.assertIn("adb reconnect", section_text)

    def test_test_flow_waits_for_services_after_module_reboot(self) -> None:
        flow = read(".github/tests/run-android-test-flow.sh")
        post_install = section(flow, "bash .github/tests/install-storage-redirect-module.sh", "adb shell appops set")
        self.assertIn("wait_for_adb_ready", post_install)
        self.assertIn("package_service_deadline", post_install)
        self.assertIn("cmd package list packages", post_install)

    def test_android17_disables_graphics_readback_before_and_after_module_reboot(self) -> None:
        flow = read(".github/tests/run-android-test-flow.sh")
        workaround = section(
            flow,
            "android17_disable_graphics_readback()",
            "prepare_device_health\n",
        )
        post_install = section(flow, "bash .github/tests/install-storage-redirect-module.sh", "adb shell appops set")

        self.assertIn("service call window 137 i32 0", workaround)
        self.assertIn("service call window 135", workaround)
        self.assertIn(r"Parcel\([[:space:]]*00000000", workaround)
        self.assertIn("test-flow-graphics-state.txt", workaround)
        self.assertIn('[ "$sdk" -ne 37 ]', workaround)
        self.assertIn("cmd package list packages -d --user 0", workaround)
        self.assertIn("pidof com.android.systemui", workaround)
        self.assertIn("pidof system_server", workaround)
        self.assertIn("stable_system_server_pid", workaround)
        self.assertIn("stable_isTaskSnapshotSupported", workaround)
        self.assertIn("SystemUI 是 persistent 系统进程", workaround)
        self.assertNotIn("killall com.android.systemui", workaround)
        self.assertIn("sleep 8", workaround)
        self.assertIn('android17_disable_graphics_readback "初次启动后"', flow)
        self.assertIn('android17_disable_graphics_readback "模块重启后"', post_install)
        self.assertEqual(3, flow.count("android17_disable_graphics_readback"))
        self.assertLess(
            post_install.index('android17_disable_graphics_readback "模块重启后"'),
            post_install.index("=== shell_packages ==="),
        )

    def test_scenario_runner_recovers_transient_adb_boot_errors(self) -> None:
        flow = read(".github/tests/run-storage-redirect-scenarios.sh")
        boot = section(flow, "wait_boot_completed()", "backup_device_execution_state()")
        self.assertIn("adb get-state", boot)
        self.assertIn("adb reconnect offline", boot)
        self.assertIn("timeout 10s adb shell getprop sys.boot_completed", boot)

    def test_bash_root_timeout_wraps_the_adb_executable_not_shell_function(self) -> None:
        flow = read(".github/tests/run-storage-redirect-scenarios.sh")
        root = section(flow, "adb_root()", "adb_write_file()")
        start = section(flow, "start_app_and_confirm_mount()", "wait_storage_ready()")

        self.assertIn('timeout_command=(timeout --foreground "${ADB_ROOT_TIMEOUT_SECONDS}s")', root)
        self.assertIn("adb_su_timeout()", root)
        self.assertIn('adb_su_timeout 30 ": > \'$LOG_PATH\' 2>/dev/null || true"', start)
        self.assertNotRegex(flow, r"timeout(?: --foreground)? [0-9]+s? adb_su(?:\s|$)")
        self.assertIn("cat \\\"\\$q/cmdline\\\" 2>/dev/null", flow)
        self.assertIn('adb_su_timeout "$host_timeout_seconds"', section(flow, "wait_service_result()", "scenario_from_label()"))
        self.assertIn('adb_su_timeout "$host_timeout_seconds"', section(flow, "wait_config_applied()", "service_case_timeout_seconds()"))

    def test_android17_diagnostics_do_not_report_expected_missing_files(self) -> None:
        flow = read(".github/tests/run-storage-redirect-scenarios.sh")
        storage = section(flow, "print_storage_state()", "run_service_case()")
        rootavd = read(".github/vendor/rootAVD/rootAVD.sh")

        self.assertIn("optional_storage_alias_absent path=", storage)
        self.assertIn('if ! $BB wget -q --no-check-certificate $SRCURL$JSON || [ ! -s "$JSON" ]', rootavd)
        self.assertLess(rootavd.index('[ ! -s "$JSON" ]'), rootavd.index('VER=$(json_value "version" < $JSON)'))

    def test_media_provider_readiness_rejects_query_errors_and_uses_ps_fallback(self) -> None:
        flow = read(".github/tests/run-storage-redirect-scenarios.sh")
        query = section(flow, "media_provider_query_ready()", "wait_media_provider_ready()")
        self.assertIn('timeout 15s adb shell content query', query)
        self.assertIn("Error while accessing provider:media", query)
        readiness = section(flow, "wait_media_provider_ready()", "media_provider_pid()")
        self.assertIn("local uris=(\"content://media/external_primary/file\")", readiness)
        hook = section(flow, "ensure_media_provider_hook_ready()", "restart_media_provider_with_hook_ready()")
        self.assertIn("if media_provider_is_lazy; then", hook)
        self.assertIn("timeout --foreground 120s bash -c 'check_health'", flow)
        self.assertIn("timeout --foreground 180s bash -c 'capture_test_flow_artifacts'", flow)
        pid = section(flow, "media_provider_pid()", "wait_media_provider_hook_ready()")
        self.assertIn("ps -A -o PID,NAME,ARGS", pid)

    def test_bash_prepares_mapping_source_on_real_backend(self) -> None:
        prepare = section(
            self.bash,
            "prepare_backend_core_targets()",
            "clean_targets()",
        )
        clean = section(self.bash, "clean_targets()", "clean_results()")

        self.assertIn("'${BACKEND_ROOT}/Download/Test'", prepare)
        self.assertIn("'${BACKEND_ROOT}/Download/SrtPriority'", prepare)
        self.assertIn("'${BACKEND_ROOT}/Download/SrtPriorityMapped'", prepare)
        self.assertIn("test -d '${BACKEND_ROOT}/Download/Test'", prepare)
        self.assertIn("test -d '${BACKEND_ROOT}/Download/SrtPriority'", prepare)
        self.assertNotIn("'${REAL_ROOT}/Download/Test'", prepare)
        self.assertIn("prepare_backend_core_targets", clean)
        self.assertIn("export -f", self.bash)
        self.assertRegex(self.bash, r"export -f [^\n]*prepare_backend_core_targets")
        self.assertLess(
            clean.index("prepare_backend_core_targets"),
            clean.index("fix_private_backend_permissions"),
        )


if __name__ == "__main__":
    unittest.main()
