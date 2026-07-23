import json
import re
import unittest
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
            app_job = section(source, "  app:", "  test-flow-build:")
            test_flow = section(source, "  test-flow:", "  test-flow-required:")
            self.assertIn(':app:testDebugUnitTest :app:assembleRelease', app_job)
            self.assertNotIn('"ndk;30.0.14904198"', app_job)
            self.assertNotIn("fetch-depth: 0", test_flow)
            self.assertIn("fail-fast: true", test_flow)
            for version in (13, 14, 15, 16):
                self.assertIn(f"version: {version}", test_flow)
            required = source[source.index("  test-flow-required:") :]
            self.assertIn("needs.test-flow.result", required)

    def test_public_sync_derives_review_for_filtered_staged_tree(self) -> None:
        source = read("scripts/srx-sync-public.ps1")
        register = section(source, "function Register-DerivedPublicReview", "function Set-SanitizedSnapshot")
        self.assertIn('"-Commit", $SourceCommit', register)
        self.assertIn('Get-GitOutput -Arguments @("write-tree")', register)
        self.assertIn('Get-GitOutput -Arguments @("diff", "--cached", "--name-only"', register)
        self.assertIn('"-ReportPath", $reportPath', register)
        commit_loop = source[source.index("foreach ($currentSourceCommit") :]
        self.assertLess(
            commit_loop.index("Register-DerivedPublicReview -SourceCommit $currentSourceCommit"),
            commit_loop.index('Invoke-Checked -FilePath "git" -Arguments @("commit"'),
        )

    def test_test_flow_reports_are_not_in_runtime_artifacts(self) -> None:
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            source = read(workflow)
            upload_start = source.index("Upload test-flow runtime") if "Upload test-flow runtime" in source else source.index("Upload release test-flow runtime")
            reports_start = source.index("unit test reports", upload_start)
            runtime_upload = source[upload_start:reports_start]
            self.assertNotIn("build/reports/", runtime_upload)
            self.assertIn("build/test-flow/assets/*.zip", runtime_upload)
            self.assertIn("build/outputs/apk/**/*.apk", runtime_upload)


if __name__ == "__main__":
    unittest.main()
