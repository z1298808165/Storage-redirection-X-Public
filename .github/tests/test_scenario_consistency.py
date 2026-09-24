import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest

import yaml
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def section(source: str, start: str, end: str) -> str:
    return source[source.index(start) : source.index(end, source.index(start))]


def bash_path(path: Path) -> str:
    """把路径转成子进程 `bash` 认得的相对形式。

    Windows MSYS 下驱动器挂在 `/mnt/<盘符>/...`（不是 `/e/...`），传 `E:/...` 或
    `/e/...` 都会被当成"脚本不存在"而以 127 假失败；子进程继承当前工作目录，
    用相对路径在 Windows 与 Linux 上都成立。
    """
    return Path(os.path.relpath(Path(path), Path.cwd())).as_posix()


def load_workflow(path: str) -> dict:
    return yaml.safe_load(read(path))["jobs"]


def android17_matrix_entry(jobs: dict) -> dict:
    for entry in jobs["test-flow"]["strategy"]["matrix"]["android"]:
        if entry.get("version") == 17:
            return entry
    raise AssertionError("test-flow 矩阵中未包含 Android 17 条目")


def _assert_android17_unified(jobs: dict, label: str) -> dict:
    """校验 Android 17 已并入统一矩阵（无独立 job），且 17 特有参数与统一门禁都正确。

    返回 17 的矩阵条目，供调用方进一步断言。
    """
    if "test-flow-android17" in jobs:
        raise AssertionError(f"{label}: 不应保留独立的 test-flow-android17 job")
    entry = android17_matrix_entry(jobs)
    if entry["api_level"] != "37.0":
        raise AssertionError(f"{label}: api_level 应为 37.0，实际 {entry['api_level']}")
    if "v31.0" not in entry["magisk_url"]:
        raise AssertionError(f"{label}: magisk_url 应指向 v31.0")
    if entry["gpu_mode"] != "swiftshader_indirect":
        raise AssertionError(f"{label}: gpu_mode 应为 swiftshader_indirect")
    if entry["fresh_app_per_case"] != 1:
        raise AssertionError(f"{label}: 17 的 fresh_app_per_case 应为 1")
    if entry["timeout_minutes"] != 50:
        raise AssertionError(f"{label}: 17 的 timeout_minutes 应为 50")
    if entry["boot_timeout"] != 1800:
        raise AssertionError(f"{label}: 17 的 boot_timeout 应为 1800")
    if entry["disable_animations"] is not False:
        raise AssertionError(f"{label}: 17 的 disable_animations 应为 false")
    if entry["disable_linux_hw"] is not False:
        raise AssertionError(f"{label}: 17 的 disable_linux_hw 应为 false")
    others = [e for e in jobs["test-flow"]["strategy"]["matrix"]["android"] if e["version"] != 17]
    if len(others) != 4:
        raise AssertionError(f"{label}: 其余版本应为 4 个，实际 {len(others)}")
    for e in others:
        if e["fresh_app_per_case"] != 0:
            raise AssertionError(f"{label}: 其余版本 fresh_app_per_case 应为 0")
        if e["timeout_minutes"] != 45:
            raise AssertionError(f"{label}: 其余版本 timeout_minutes 应为 45")
        if e["boot_timeout"] != 1500:
            raise AssertionError(f"{label}: 其余版本 boot_timeout 应为 1500")
        if e["disable_animations"] is not True:
            raise AssertionError(f"{label}: 其余版本 disable_animations 应为 true")
        if e["disable_linux_hw"] != "auto":
            raise AssertionError(f"{label}: 其余版本 disable_linux_hw 应保持上游默认 auto")
        if e.get("magisk_url", "") != "":
            raise AssertionError(f"{label}: 其余版本不应设置 magisk_url")
    if jobs["test-flow"]["strategy"]["fail-fast"] is not True:
        raise AssertionError(f"{label}: 矩阵必须跨版本 fail-fast")
    required_needs = jobs["test-flow-required"]["needs"]
    if "test-flow" not in required_needs:
        raise AssertionError(f"{label}: 门禁必须校验统一的 test-flow 矩阵")
    if "test-flow-android17" in required_needs:
        raise AssertionError(f"{label}: 门禁不应再单列 test-flow-android17")
    return entry


def _const_product(source: str, name: str) -> int:
    """读取 `const NAME: T = <算式>;` 的数值结果，支持整数字面量与 `a * b` 形式。"""
    expression = re.search(
        re.escape(name) + r"\s*:\s*\w+\s*=\s*([0-9_\s*]+);", source
    ).group(1)
    product = 1
    for factor in expression.replace("_", "").split("*"):
        factor = factor.strip()
        if factor:
            product *= int(factor)
    return product


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

    def test_optional_diagnostics_accept_empty_output(self) -> None:
        body = section(self.powershell, "function Invoke-CaptureScenario2MediastoreHookDiag", "function Invoke-StandardScenario")
        # 日志轮转或过滤无匹配属于正常情况，空诊断不能中断后面的行为断言。
        for name in ("logcatOut", "dirOut", "runningOut", "installStateOut", "markerOut"):
            self.assertIn(f"$lines.AddRange([string[]]@(${name} | Where-Object {{ $null -ne $_ }}))", body)
        self.assertIn("../../temp/scenario-2-mediastore-hook-diag.txt", body)

    def test_private_fixture_is_prepared_after_nested_mapping_cleanup(self) -> None:
        # 场景 37 清理的父目录包含场景 34 的 data 子目录，顺序反转会产生 ENOENT。
        ps = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertLess(ps.index("'$NestedMappingRequestRoot'"),
                        ps.index("mkdir -p '$BackendOwnPrivateDataRoot'"))
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertLess(bash.index("  prepare_any_path_targets"),
                        bash.index("mkdir -p '${BACKEND_OWN_PRIVATE_DATA_ROOT}'"))
        self.assertLess(bash.index("mkdir -p '${BACKEND_OWN_PRIVATE_DATA_ROOT}'"),
                        bash.index("  fix_private_backend_permissions"))

    def test_own_private_fixture_is_materialized_through_visible_path(self) -> None:
        # 自有私有目录在应用视图里是模块 FUSE 锚点的 bind；只写真实后端时锚点可能继续
        # 以否命中文档响应应用首个 lookup（场景 34 own-data 在 Android 13 上返回 ENOENT）。
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertIn("mkdir -p '${OWN_PRIVATE_DATA_ROOT}'", bash)
        self.assertLess(bash.index("mkdir -p '${BACKEND_OWN_PRIVATE_DATA_ROOT}'"),
                        bash.index("mkdir -p '${OWN_PRIVATE_DATA_ROOT}'"))
        ps = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertIn("mkdir -p '$OwnPrivateDataRoot'", ps)

    def test_own_private_write_keeps_strict_assertions_on_retry(self) -> None:
        # 重试只重建夹具并重启应用，断言集合必须与首次一致，不得放宽或跳过落点校验。
        bash = section(self.bash, "run_own_private_directories_scenario() {", "run_any_path_mapping_scenario() {")
        self.assertIn("run_own_private_write_case", bash)
        retry = section(self.bash, "run_own_private_write_case() {", "run_any_path_mapping_scenario() {")
        for token in ("run_write_case", "check_own_private_real_landing", "check_file_missing", "own_private_write_retry", "clean_targets"):
            self.assertIn(token, retry)
        self.assertIn("export -f run_own_private_directories_scenario run_own_private_write_case", self.bash)
        ps = section(self.powershell, "function Invoke-OwnPrivateWriteCase", "function Invoke-OwnPrivateDirectoriesScenario")
        for token in ("Invoke-WriteCase", "Require-OwnPrivateRealLanding", "Require-Missing", "own_private_write_retry", "Clear-Targets"):
            self.assertIn(token, ps)
        ps_scenario = section(self.powershell, "function Invoke-OwnPrivateDirectoriesScenario", "function Invoke-AnyPathMappingScenario")
        self.assertIn("Invoke-OwnPrivateWriteCase", ps_scenario)

    def test_own_private_real_landing_accepts_version_specific_volume(self) -> None:
        # app-specific 存储在部分版本是独立卷（Android 13 的 Android/data|obb），此时可见路径
        # 与 /data/media/0 下的同名路径是两份目录；断言只接受二者之一，且沙盒必须为空。
        landing = section(self.bash, "check_own_private_real_landing() {", "run_rule_sandbox_scenario() {")
        self.assertIn("test -f '$visible_path'", landing)
        self.assertIn("test -f '$backend_path'", landing)
        self.assertLess(landing.index("test -f '$visible_path'"), landing.index("test -f '$backend_path'"))
        self.assertIn("return 1", landing)
        self.assertIn("check_own_private_real_landing", self.big_bash_export_line())
        ps_landing = section(self.powershell, "function Require-OwnPrivateRealLanding", "function Test-PublicDirectoryOwner")
        self.assertIn("test -f '$VisiblePath'", ps_landing)
        self.assertIn("test -f '$BackendPath'", ps_landing)
        self.assertLess(ps_landing.index("test -f '$VisiblePath'"), ps_landing.index("test -f '$BackendPath'"))
        ps_case = section(self.powershell, "function Invoke-OwnPrivateWriteCase", "function Invoke-OwnPrivateDirectoriesScenario")
        self.assertIn("Require-OwnPrivateRealLanding", ps_case)

    def test_own_private_visible_fixture_creation_reports_failure(self) -> None:
        # 可见路径是自有私有目录的权威落点；预置失败必须显式上报，否则症状会退化成
        # 应用侧 file_write 的裸 ENOENT（场景 34 own-data 曾如此）。
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertIn("if ! adb_su \"mkdir -p '${OWN_PRIVATE_DATA_ROOT}'", bash)
        self.assertNotIn("mkdir -p '${OWN_PRIVATE_DATA_ROOT}' '${OWN_PRIVATE_MEDIA_ROOT}' '${OWN_PRIVATE_OBB_ROOT}' 2>/dev/null", bash)
        self.assertIn("自有目录可见路径预置失败", bash)
        ps = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertIn("Test-Su \"mkdir -p '$OwnPrivateDataRoot'", ps)
        self.assertNotIn("mkdir -p '$OwnPrivateDataRoot' '$OwnPrivateMediaRoot' '$OwnPrivateObbRoot' 2>/dev/null", ps)
        self.assertIn("自有目录可见路径预置失败", ps)

    def test_own_private_fixture_ownership_normalized(self) -> None:
        # Android 13 上应用的自有私有目录视图经系统 FUSE 锚点恢复，MediaProvider 按属主
        # 过滤目录项；root 预置的夹具必须按应用属主修正，否则对应用不可见（写入 ENOENT）。
        # 两个运行器都必须把后端拷贝与可见路径两份夹具统一 chown 成应用 uid。
        bash_fix = self.bash.split("fix_own_private_fixture_permissions() {", 1)[1].split("\n}", 1)[0]
        for token in ("test_app_uid", "chown -R", "BACKEND_OWN_PRIVATE_DATA_ROOT", "OWN_PRIVATE_DATA_ROOT", "2771"):
            self.assertIn(token, bash_fix)
        bash_targets = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertIn("fix_own_private_fixture_permissions", bash_targets)
        self.assertIn("fix_own_private_fixture_permissions", self.big_bash_export_line())
        ps_fix = section(self.powershell, "function Fix-OwnPrivateFixturePermissions", "function Clear-Targets")
        for token in ("Test-AppUid", "chown -R", "$BackendOwnPrivateDataRoot", "$OwnPrivateDataRoot", "2771"):
            self.assertIn(token, ps_fix)
        ps_targets = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertIn("Fix-OwnPrivateFixturePermissions", ps_targets)

    def test_fixture_operations_bypass_app_visible_view(self) -> None:
        # /storage/emulated/0 是应用视图，刚应用的只读/映射配置会覆盖夹具父目录，root shell
        # 从该视图 mkdir 会被 EPERM 拒绝（Android 17 场景 17）。clean_targets 内的夹具操作
        # 必须整体改走原始后端路径；但共享探针的 FUSE 失效清理必须仍在可见路径上先行，
        # 否则底层删除不会通知系统 FUSE 失效其 inode 缓存（与 PowerShell 侧语义一致）。
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertIn('local REAL_ROOT="${BACKEND_ROOT}"', bash)
        self.assertLess(bash.index("rm -f '${REAL_ROOT}/Download/SrtProbe/$TEST_FILE'"),
                        bash.index('local REAL_ROOT="${BACKEND_ROOT}"'))
        self.assertLess(bash.index('local REAL_ROOT="${BACKEND_ROOT}"'),
                        bash.index("mkdir -p '${REAL_ROOT}/Download/SrtProbe'"))

    def test_own_private_diagnostics_is_exported_and_wired(self) -> None:
        self.assertIn("capture_own_private_diagnostics", self.big_bash_export_line())
        self.assertIn("capture_own_private_diagnostics || true", self.bash)

    def big_bash_export_line(self) -> str:
        # 文件里有多条 export -f；场景函数集中导出的是以 detect_adb_root_mode 开头的大列表。
        for line in self.bash.splitlines():
            if line.startswith("export -f detect_adb_root_mode"):
                return line
        self.fail("big export -f list not found")
        return ""

    def test_run_scenario_calls_only_exported_functions(self) -> None:
        """run_scenario 在独立子 shell 中执行，它调用的脚本函数都必须先 export -f。

        子 shell 只继承显式导出的函数；漏掉一个就会在设备上以 `command not found`
        让每个场景失败。实测漏导出 wait_scenario_app_view 时，五个平台 × 37 个场景
        全红，而 quality 侧完全看不出来，只有跑完整矩阵才会暴露，代价很高。
        """
        body = self.bash[self.bash.index("run_scenario() {") : self.bash.index("# 与 PowerShell 共用设备端锁")]
        defined = set(re.findall(r"(?m)^([a-z_][a-z0-9_]*)\(\) \{", self.bash))
        exported: set[str] = set()
        for line in self.bash.splitlines():
            if line.startswith("export -f "):
                exported.update(line[len("export -f ") :].split())
        missing = sorted(
            name
            for name in defined
            if name not in exported and re.search(rf"(?<![\w-]){re.escape(name)}(?![\w-])", body)
        )
        self.assertEqual([], missing, f"run_scenario 会调用但未 export -f 的函数：{missing}")

    def test_shared_probe_invalidation_precedes_backend_cleanup(self) -> None:
        # 前序场景已查询过的文件应经系统 FUSE 删除，不能仅修改底层文件系统。
        ps = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertLess(ps.index("rm -f '$RealRoot/Download/SrtProbe/$TestFile'"),
                        ps.index("rm -rf '$BackendRoot/Download/SrtProbe'"))
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        self.assertIn("rm -f '${REAL_ROOT}/Download/SrtProbe/$TEST_FILE'", bash)
        self.assertLess(bash.index("rm -f '${REAL_ROOT}/Download/SrtProbe/$TEST_FILE'"),
                        bash.index("rm -rf '${REAL_ROOT}/Download/SrtProbe'"))

    def test_fixture_directory_removal_stays_on_visible_view(self) -> None:
        # 夹具目录的删除必须留在**可见路径**上，且先于 REAL_ROOT 切到后端。
        #
        # 只删探针文件不够：模块对重定向目录的 unlink 返回 EDOM（`rm: ...: Math result
        # not representable`），该次删除不生效；目录级 `rm -rf` 才会让模块丢弃该目录项、
        # 使后续 lookup 不再命中残留。若把这条目录删除一并改走后端，前序场景写入可见
        # SrtProbe 的探针会一路存活到后续场景的控制组，被 `file_unexpected` 判失败
        # （Android 17 场景 20 `mount-ns-control-real`，残留 mtime 早于该场景十余分钟）。
        #
        # 反向证据：run `35128270269`（公开 `3565c0f4`）此处两条都在可见路径，整轮 EDOM
        # 仅 2 次、场景 20 通过；run `35238174287`（公开 `36abe4b0`）只剩文件级一条，
        # 整轮 EDOM 14 次、场景 20 失败。
        bash = self.bash.split("clean_targets() {", 1)[1].split("\n}", 1)[0]
        switch = bash.index('local REAL_ROOT="${BACKEND_ROOT}"')
        visible_dirs = bash.index("rm -rf '${REAL_ROOT}/Download/SrtProbe'")
        # 目录级删除必须存在，且必须排在 local REAL_ROOT 覆盖之前（此时 REAL_ROOT 是可见路径）。
        self.assertLess(visible_dirs, switch)
        # 覆盖范围不能靠硬编码清单：可见语句删掉的每个目录，都必须同样出现在切换后的
        # 删除语句集合里。两处都写作 `${REAL_ROOT}/...`（切换后 REAL_ROOT 即后端根），
        # 因此只要后端补了新夹具目录而被漏在可见分支，本守卫就会报出缺失项 —— 正是
        # 「只删 SrtProbe 而漏掉别名目录」这一静默缺口的拦截点。
        visible_targets = self.removal_targets(self.line_with(bash[:switch], "rm -rf '${REAL_ROOT}/Download/SrtProbe'"))
        backend_targets = self.removal_targets(bash[switch:])
        self.assertGreaterEqual(len(visible_targets), 8)
        self.assertEqual(
            sorted(visible_targets - backend_targets),
            [],
            "可见路径删除的目录必须同时出现在后端删除语句中",
        )
        # 后端路径的目录删除仍须保留（确定性落点由它保证），只是排在切换之后。
        self.assertLess(switch, bash.index("rm -rf '${REAL_ROOT}/Download/SrtProbe", switch))

        ps = section(self.powershell, "function Clear-Targets", "function Remove-TestTargetArtifacts")
        self.assertLess(ps.index("rm -rf '$RealRoot/Download/SrtProbe'"),
                        ps.index("rm -rf '$BackendRoot/Download/SrtProbe'"))
        visible_ps = self.removal_targets(self.line_with(ps, "rm -rf '$RealRoot/Download/SrtProbe'"), "$RealRoot")
        backend_ps = self.removal_targets(ps, "$BackendRoot")
        self.assertGreaterEqual(len(visible_ps), 8)
        self.assertEqual(sorted(visible_ps - backend_ps), [],
                         "可见路径删除的目录必须同时出现在后端删除语句中")

    @staticmethod
    def line_with(text: str, needle: str) -> str:
        # 取含指定片段的**单行**：两处对应语句各自成行，避免把同函数内其它删除语句混入比对。
        for line in text.splitlines():
            if needle in line:
                return line
        raise AssertionError("line containing %r not found" % needle)

    @staticmethod
    def removal_targets(text: str, prefix: str = "${REAL_ROOT}") -> set:
        # 收集 `rm -rf '<prefix>/X' ...` 语句里的目录目标。可见分支只传单行、后端分支传
        # 整段，两种用法都能正确取集合；`find ... -delete` 的目标不以 rm -rf 开头，不会误收。
        targets: set = set()
        for group in re.findall(r"rm -rf((?:\s+'[^']+')+)", text):
            targets.update(re.findall(r"'" + re.escape(prefix) + r"/([A-Za-z0-9_./-]+)'", group))
        return targets

    def test_alias_fixture_cleanup_covers_all_aliases(self) -> None:
        ps = section(self.powershell, "function Clear-AliasMediaStoreFixture", "function Invoke-NestedMappingChainScenario")
        for token in ("$QqAliasRequestRoot", "$RealRoot/Download/QQ", "$QqAliasMappedRoot", "Remove-MediaStoreRowsByPattern"):
            self.assertIn(token, ps)
        self.assertLess(ps.index("    Clear-AliasMediaStoreFixture"), ps.index("Invoke-ServiceCase"))
        bash = section(self.bash, "clear_alias_mediastore_fixture() {", "run_nested_mapping_chain_scenario() {")
        for token in ("Tencent/QQfile_recv", "Download/QQ", "Download/SrtQqAliasMapped", "srt_qq_alias_existing"):
            self.assertIn(token, bash)
        self.assertIn("export -f clear_alias_mediastore_fixture remove_mediastore_rows_by_pattern run_qq_alias_mapped_existing_file_scenario", self.bash)

    def test_bash_case_patterns_use_posix_single_backslash(self) -> None:
        # 单引号不会折叠反斜杠，`\\[` 会原样传给 grep；ubuntu-latest 的 GNU grep 3.11 按
        # POSIX BRE 解析后不再匹配字面 `[`，断言会静默失配（场景 36 曾因此在 PASS 之后判失败）。
        patterns = re.findall(
            r"""run_service_case\s+"[^"]*"\s+"[^"]*"\s+"[^"]*"\s+'([^']*)'""",
            self.bash,
        )
        self.assertTrue(patterns)
        for pattern in patterns:
            self.assertNotIn("\\\\", pattern)

    def test_config_apply_wait_is_bounded_by_line_watermark(self) -> None:
        # daemon 输出 reload 标记后会立刻跟进大批挂载日志，固定尾部行数窗口会被冲出（场景 22
        # 应用超时），等待必须以行水位为界，并由记录水位、重写配置的辅助函数统一调用。
        body = section(self.bash, "wait_config_applied() {", "\nservice_case_timeout_seconds() {")
        self.assertIn("tail -n +$((watermark + 1))", body)
        self.assertNotIn("tail -240", body)
        self.assertIn("wc -l < '$LOG_PATH'", body)
        self.assertIn('write_config "$APP_CONFIG_CONTENT"', body)
        self.assertEqual(self.bash.count('wait_config_applied "'), 1)

    def test_mount_probe_refreshes_pid_inside_bounded_poll(self) -> None:
        ps = section(self.powershell, "function Test-FuseMountActive", "function Test-ScopedFuseDaemonStarted")
        self.assertLess(ps.index("for ($i = 0; $i -lt 20; $i++)"), ps.index("$appPid = Get-AppPid"))
        self.assertIn("/proc/$appPid/mountinfo", ps)
        self.assertNotIn("Restart-App", ps)
        bash = section(self.bash, "check_fuse_mount_active() {", "check_scoped_fuse_daemon_started() {")
        self.assertLess(bash.index("for _ in $(seq 1 20)"), bash.index('pid="$(app_pid)"'))
        self.assertIn("/proc/${pid}/mountinfo", bash)
        self.assertNotIn("start_app", bash)

    def test_device_lock_precedes_backup_and_outlives_cleanup(self) -> None:
        # 两种运行器使用同一个设备锁，抢锁失败的实例不得恢复别人的配置。
        lock_path = "/data/local/tmp/srx-test-flow.lock"
        self.assertIn(lock_path, self.powershell)
        self.assertIn(lock_path, self.bash)
        ps_start = self.powershell[self.powershell.index("$script:ExitCode = 0") :]
        self.assertLess(ps_start.index("Enter-DeviceRunLock"),
                        ps_start.index("Backup-GlobalConfig"))
        self.assertIn("try { Invoke-TestArtifactCleanup } finally { Exit-DeviceRunLock }", ps_start)
        bash_start = self.bash[self.bash.index("trap finish_test_run EXIT") :]
        self.assertLess(bash_start.index("acquire_device_run_lock"),
                        bash_start.index("backup_global_config"))
        finish = section(self.bash, "finish_test_run() {", "cleanup_done=0")
        self.assertIn('if [ "$device_run_lock_held" -eq 1 ]', finish)
        self.assertLess(finish.index("cleanup_test_artifacts"), finish.index("release_device_run_lock"))
        self.assertIn('return "$status"', finish)

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
        # Release 始终全量：硬编码 SRT_SCENARIOS: "all"，不得被单场景标记收窄。
        release_values = re.findall(
            r'SRT_SCENARIOS:\s*"([^"]+)"', read(".github/workflows/release.yml")
        )
        self.assertTrue(release_values, ".github/workflows/release.yml")
        self.assertTrue(
            all(value == "all" for value in release_values),
            ".github/workflows/release.yml",
        )

        # CI 的范围由 prepare outputs 统一决定，不得在 job 内硬编码全量，
        # 也不得再用 dispatch/commit 标记就地二次解析（避免与 prepare 双源头）。
        ci = read(".github/workflows/ci.yml")
        self.assertIn("SRT_SCENARIOS: ${{ needs.prepare.outputs.scenarios }}", ci)
        self.assertNotIn("SRT_SCENARIOS_OVERRIDE:", ci)
        self.assertNotIn("SRT_COMMIT_MESSAGE:", ci)

        any_path_workflow = ROOT / ".github/workflows/ci-any-path.yml"
        if any_path_workflow.exists():
            any_path_values = re.findall(
                r'SRT_SCENARIOS:\s*"([^"]+)"', read(".github/workflows/ci-any-path.yml")
            )
            self.assertTrue(any_path_values)
            self.assertTrue(all(value == "all" for value in any_path_values))

    def test_ci_limited_scope_publishes_nothing_and_uses_prepare_outputs(self) -> None:
        # 「单场景」标记或 dispatch 收窄范围时，CI 只跑指定场景所需构建+测试+门禁，
        # 不发布 CI 资产、不更新 update.json、不额外触发全量 CI。范围与发布开关来自
        # prepare 的 outputs，下游统一引用，避免与 job 内就地解析双源头。
        ci = read(".github/workflows/ci.yml")
        prepare = section(ci, "  prepare:", "  init-ci-release:")
        self.assertIn("parse_scenario_scope.py", prepare)
        self.assertIn("scenarios: ${{ steps.scope.outputs.scenarios }}", prepare)
        self.assertIn("publish_ci: ${{ steps.scope.outputs.publish_ci }}", prepare)

        # 发布相关 job 在范围受限时整体跳过（publish_ci=false）。
        self.assertIn(
            "if: github.event_name == 'push' && !contains(github.event.head_commit.message, '仅验证CI') && needs.prepare.outputs.publish_ci == 'true'",
            ci,
        )
        self.assertIn(
            "if: github.event_name == 'push' && needs.prepare.outputs.publish_ci == 'true'",
            ci,
        )

        # module/app 构建 job 在受限范围时跳过（仅保留测试流所需的 test-flow-build）。
        module = section(ci, "  module:", "  app:")
        app = section(ci, "  app:", "  test-flow-build:")
        self.assertIn("needs.prepare.outputs.publish_ci == 'true'", module)
        self.assertIn("needs.prepare.outputs.publish_ci == 'true'", app)

        # 失败清理只在本就发布时才尝试删除草稿 Release。
        self.assertIn("PUBLISH_CI: ${{ needs.prepare.outputs.publish_ci }}", ci)


    def test_shared_fuse_host_b2a_contract(self) -> None:
        # B2-a 先建立独立宿主会话骨架：它必须在 daemon 的私有 namespace 中创建，
        # 使用 srx_fuse_host 前缀并设为 shared propagation；应用接入仍由后续 B2-b 完成。
        host = read("src/fuse_host.rs")
        daemon = read("src/daemon.rs")
        config = read("src/fuse_redirect/config.rs")
        self.assertIn("pub fn spawn_fuse_host() -> Option<FuseHost>", host)
        self.assertIn("libc::unshare(libc::CLONE_NEWNS)", host)
        self.assertIn("libc::MS_REC | libc::MS_PRIVATE", host)
        self.assertIn("mount_host_fuse(", host)
        self.assertIn("libc::MS_SHARED", config)
        self.assertIn('"srx_fuse_host"', config)
        self.assertIn("if !crate::fuse_host::ensure_global()", daemon)
        self.assertIn("crate::fuse_host::ensure_global()", daemon)
        self.assertIn("scoped path remains active", daemon)
        # B2-a 不能改变现有应用 scoped 挂载路径；host 只是新增基础设施。
        self.assertIn('"srx_fuse_redirect"', config)

    def test_shared_fuse_host_registers_app_policy_before_attach(self) -> None:
        # 共享宿主会话只持有一份「直通」策略。按 uid 策略必须由 daemon 经控制通道登记进会话，
        # 否则把宿主树 bind 到应用存储根会让应用静默失去全部重定向。这里钉死三件事：
        # 1) 登记必须先于接入闸门判断，避免接入启用后跑在旧策略上；
        # 2) 策略虚拟根取整个存储根（`mount_root=None`），与宿主会话服务的完整视图一致；
        # 3) 闸门本身仍然存在——打开接入必须是一次有意的改动。
        for path in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            source = read(path)
            body = section(source, "fn start_fuse_service_for_root", "let mut ready_sockets")
            compact = "".join(body.split())
            register_at = body.index("crate::fuse_host::register_app_policy(")
            gate_at = body.index("crate::fuse_host::can_attach_app(")
            self.assertLess(register_at, gate_at, f"{path} 必须先登记策略再判断接入闸门")
            self.assertIn("fuse_config_from_request(request,None,", compact)

        host = read("src/fuse_host.rs")
        self.assertIn("pub fn register_app_policy(", host)
        self.assertIn("pub fn can_attach_app(", host)
        self.assertIn("pub(crate) fn spawn_host_control_loop(", host)
        self.assertIn("set_host_control_fd(control_sockets[0])", host)

        policy = read("src/fuse_redirect/policy.rs")
        self.assertIn("pub(crate) struct SharedPolicyTable", policy)
        self.assertIn("pub(crate) fn register(&self, config: FuseRedirectConfig)", policy)

        # 宿主会话的未登记 uid 必须按设计拒绝，而不是回退到直通策略。回退到直通会让调用方
        # 直接读写真实存储（沙盒失效），而且这类越权落点无法靠事后清理恢复，只能在决策前挡住。
        self.assertIn(
            "pub(super) fn new(policy: RedirectPolicy, deny_unregistered: bool)", policy
        )
        self.assertIn("fn copied_as_deny_all(&self)", policy)
        decision = section(
            policy,
            "pub(super) fn backend_for_relative(",
            "let storage_path = self.storage_path_for_rel(",
        )
        self.assertIn("if self.deny_all {", decision)
        self.assertIn("deny_all: false,", policy)
        fs = read("src/fuse_redirect/mod.rs")
        self.assertIn("let deny_unregistered = config.is_passthrough_host;", fs)

        # 控制通道必须与会话同生命周期：挂载函数接收控制端，并在就绪之后启动读取循环。
        config = read("src/fuse_redirect/config.rs")
        self.assertIn("control_sock: Option<libc::c_int>", config)
        self.assertIn(
            "crate::fuse_host::spawn_host_control_loop(control_sock, policy_table)",
            config,
        )

    def test_fuse_policy_resolution_is_per_request_uid(self) -> None:
        # 共享宿主 FUSE 会话要让同一个挂载点服务多个应用，策略解析就必须从「会话级常量」
        # 改成「每请求按调用方 uid 查表」。这里钉死阶段 1 的收敛结果：回调与挂载点日志
        # 都不得再直读会话绑定策略的字段，一律经注册表取值。
        source = read("src/fuse_redirect/mod.rs")
        # `shared_table()` 是取出按 uid 策略表句柄（交给宿主会话的控制通道登记），不是读策略
        # 字段，因此允许；除它之外的 `self.policy.` 仍然只能走 `for_uid` / `session`。
        allowed = (
            "self.policy.for_uid(req.uid())",
            "self.policy.session()",
            "self.policy.shared_table()",
        )
        for number, line in enumerate(source.splitlines(), 1):
            if "self.policy." not in line:
                continue
            self.assertTrue(
                any(token in line for token in allowed),
                f"src/fuse_redirect/mod.rs:{number} 直读策略字段：{line.strip()}",
            )

        # 挂载点日志描述的是会话绑定策略，同样只能经注册表会话项取值。
        mount_source = read("src/fuse_redirect/config.rs")
        for number, line in enumerate(mount_source.splitlines(), 1):
            if "fs.policy." not in line:
                continue
            self.assertIn(
                "fs.policy.session()",
                line,
                f"src/fuse_redirect/config.rs:{number} 直读策略字段：{line.strip()}",
            )

        # 注册表必须提供按 uid 解析与会话级策略两个出口，并保证 uid 未命中时走回退策略：
        # 少了回退，宿主会话一旦接入就会把未登记调用方暴露在直通策略下。
        registry = read("src/fuse_redirect/policy.rs")
        self.assertIn("pub(super) struct PolicyRegistry", registry)
        self.assertIn(
            "pub(super) fn for_uid(&self, uid: u32) -> Arc<RedirectPolicy>", registry
        )
        self.assertIn("pub(super) fn session(&self) -> &RedirectPolicy", registry)
        # 未命中 uid 必须落到回退策略（scoped 回退到会话策略，宿主回退到拒绝策略）。
        self.assertIn("if let Ok(table) = self.by_uid.0.read()", registry)
        self.assertIn("Arc::clone(&self.fallback)", registry)

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
            # 17 并入矩阵后，SRT_FRESH_APP_PER_CASE 由矩阵字段驱动：17=1、其余=0。
            self.assertIn("SRT_FRESH_APP_PER_CASE: ${{ matrix.android.fresh_app_per_case }}", test_flow)
            self.assertIn("fresh_app_per_case: 0", test_flow)
            self.assertIn("fresh_app_per_case: 1", test_flow)
            self.assertIn('api_level: "37.0"', test_flow)
            for version in (13, 14, 15, 16):
                self.assertIn(f"version: {version}", test_flow)
            self.assertIn("version: 17", test_flow)
            # 统一门禁：校验 quality + 统一矩阵（已含 17），不再单列 android17 job。
            required = source[source.index("  test-flow-required:") :]
            self.assertIn("needs.quality.result", required)
            self.assertIn("needs.test-flow.result", required)
            self.assertNotIn("test-flow-android17", required)
            self.assertNotIn("needs.test-flow-android17.result", required)

    def test_android17_flow_is_integrated_without_diagnostic_artifact_upload(self) -> None:
        source = read(".github/workflows/ci.yml")
        # Android 17 已并入统一矩阵，不再有独立 job；门禁改用统一矩阵覆盖 17。
        self.assertNotIn("  test-flow-android17:", source)
        jobs = load_workflow(".github/workflows/ci.yml")
        entry = android17_matrix_entry(jobs)
        # 17 特有执行环境必须保留在矩阵条目中。
        self.assertEqual("37.0", entry["api_level"])
        self.assertIn("v31.0", entry["magisk_url"])
        self.assertEqual("swiftshader_indirect", entry["gpu_mode"])
        self.assertEqual(1, entry["fresh_app_per_case"])
        self.assertEqual(50, entry["timeout_minutes"])
        self.assertEqual(1800, entry["boot_timeout"])
        self.assertFalse(entry["disable_animations"])
        self.assertFalse(entry["disable_linux_hw"])
        # 其余版本保持原值：不设置 Magisk、复用进程、较短超时、关闭动画。
        others = [e for e in jobs["test-flow"]["strategy"]["matrix"]["android"] if e["version"] != 17]
        self.assertEqual(4, len(others))
        for e in others:
            self.assertEqual(0, e["fresh_app_per_case"])
            self.assertEqual(45, e["timeout_minutes"])
            self.assertEqual(1500, e["boot_timeout"])
            self.assertTrue(e["disable_animations"])
            self.assertEqual("auto", e["disable_linux_hw"])
            self.assertEqual("", e.get("magisk_url", ""))
        self.assertTrue(jobs["test-flow"]["strategy"]["fail-fast"])
        # 17 并入矩阵后，统一 job 通过 17 特有参数承载差异；env 通过矩阵字段引用。
        test_flow = section(source, "  test-flow:", "  test-flow-required:")
        self.assertIn("MAGISK_URL: ${{ matrix.android.magisk_url }}", test_flow)
        self.assertIn("EMULATOR_GPU_MODE: ${{ matrix.android.gpu_mode }}", test_flow)
        self.assertIn('ANDROID_API_LEVEL: ${{ matrix.android.api_level }}', test_flow)
        self.assertIn("SRT_FRESH_APP_PER_CASE: ${{ matrix.android.fresh_app_per_case }}", test_flow)
        self.assertIn("Download test-flow runtime", test_flow)
        self.assertIn("emulator-options: -no-window -gpu swiftshader_indirect", test_flow)
        # 17 不上传诊断 artifact：上传步骤必须以 matrix.android.version != 17 为条件。
        upload_start = test_flow.index("Upload test-flow artifacts")
        upload_block = test_flow[upload_start:]
        self.assertIn("matrix.android.version != 17", upload_block)
        self.assertIn("actions/upload-artifact@v7.0.1", upload_block)
        # 门禁：统一矩阵被校验，不再单列 android17。
        required = source[source.index("  test-flow-required:") :]
        self.assertIn("needs.test-flow.result", required)
        self.assertNotIn("test-flow-android17", required)
        self.assertNotIn("needs.test-flow-android17.result", required)

    def test_release_android17_flow_mirrors_matrix_gate(self) -> None:
        # Release 的 Android17 必须并入统一矩阵（与主矩阵同一 job），被
        # Test-flow required gate 一并校验；漏掉任一处都会让 Android17 失败静默放行发布。
        source = read(".github/workflows/release.yml")
        self.assertNotIn("  test-flow-android17:", source)
        jobs = load_workflow(".github/workflows/release.yml")
        entry = android17_matrix_entry(jobs)
        # 17 特有执行环境必须保留在矩阵条目中。
        self.assertEqual("37.0", entry["api_level"])
        self.assertIn("v31.0", entry["magisk_url"])
        self.assertEqual("swiftshader_indirect", entry["gpu_mode"])
        self.assertEqual(1, entry["fresh_app_per_case"])
        self.assertEqual(50, entry["timeout_minutes"])
        self.assertEqual(1800, entry["boot_timeout"])
        self.assertFalse(entry["disable_animations"])
        self.assertFalse(entry["disable_linux_hw"])
        # 其余版本保持原值：不设置 Magisk、复用进程、较短超时、关闭动画。
        others = [e for e in jobs["test-flow"]["strategy"]["matrix"]["android"] if e["version"] != 17]
        self.assertEqual(4, len(others))
        for e in others:
            self.assertEqual(0, e["fresh_app_per_case"])
            self.assertEqual(45, e["timeout_minutes"])
            self.assertEqual(1500, e["boot_timeout"])
            self.assertTrue(e["disable_animations"])
            self.assertEqual("auto", e["disable_linux_hw"])
            self.assertEqual("", e.get("magisk_url", ""))
        self.assertTrue(jobs["test-flow"]["strategy"]["fail-fast"])
        # 17 并入矩阵后，统一 job 通过 17 特有参数承载差异；env 通过矩阵字段引用。
        test_flow = section(source, "  test-flow:", "  test-flow-required:")
        self.assertIn("MAGISK_URL: ${{ matrix.android.magisk_url }}", test_flow)
        self.assertIn("EMULATOR_GPU_MODE: ${{ matrix.android.gpu_mode }}", test_flow)
        self.assertIn('ANDROID_API_LEVEL: ${{ matrix.android.api_level }}', test_flow)
        self.assertIn("SRT_FRESH_APP_PER_CASE: ${{ matrix.android.fresh_app_per_case }}", test_flow)
        self.assertIn("Download release test-flow runtime", test_flow)
        self.assertIn("emulator-options: -no-window -gpu swiftshader_indirect", test_flow)
        # 17 不上传诊断 artifact：上传步骤必须以 matrix.android.version != 17 为条件。
        upload_start = test_flow.index("Upload test-flow artifacts")
        upload_block = test_flow[upload_start:]
        self.assertIn("matrix.android.version != 17", upload_block)
        self.assertIn("actions/upload-artifact@v7.0.1", upload_block)
        # 门禁：统一矩阵被校验，不再单列 android17。
        required = section(source, "  test-flow-required:", "  publish-release:")
        self.assertIn("- test-flow", required)
        self.assertIn("needs.test-flow.result", required)
        self.assertNotIn("test-flow-android17", required)
        self.assertNotIn("needs.test-flow-android17.result", required)

    def test_test_flow_matrix_includes_android17_with_unified_gate(self) -> None:
        # YAML 解析级校验：Android 17 在统一矩阵内，且统一门禁覆盖（含 17）。
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            jobs = load_workflow(workflow)
            entry = _assert_android17_unified(jobs, workflow)
            self.assertEqual("swiftshader_indirect", entry["gpu_mode"])
            self.assertIn("Magisk-v31.0.apk", entry["magisk_url"])

    def test_matrix_gate_rejects_dropped_android17(self) -> None:
        # 反向验证：若 17 被错误地移出统一矩阵（回到旧的跨 job 缺口），
        # YAML 校验必须失败。用内存副本修改，绝不触碰工作区文件或 git stash。
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            jobs = load_workflow(workflow)
            matrix = jobs["test-flow"]["strategy"]["matrix"]["android"]
            jobs["test-flow"]["strategy"]["matrix"]["android"] = [
                e for e in matrix if e.get("version") != 17
            ]
            with self.assertRaises(AssertionError):
                _assert_android17_unified(jobs, workflow)

    def test_matrix_gate_rejects_stray_android17_job(self) -> None:
        # 反向验证：若仍保留独立 android17 job 却未并入矩阵，校验必须失败。
        for workflow in (".github/workflows/ci.yml", ".github/workflows/release.yml"):
            jobs = load_workflow(workflow)
            jobs["test-flow-android17"] = {"needs": ["quality"]}
            with self.assertRaises(AssertionError):
                _assert_android17_unified(jobs, workflow)

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
        startup = self.bash[self.bash.index("wait_boot_completed\nacquire_device_run_lock\nbackup_global_config") :]
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

        bash_start = self.bash[self.bash.index("wait_boot_completed\nacquire_device_run_lock\nbackup_global_config") :]
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
        self.assertIn('wait_media_provider_hook_ready "module-restart" 60', recovery)
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
        # 进程级自愈必须先于整机重启：先重启 MediaProvider 进程重新走 specialize，
        # 只有自愈仍不生效才付出整机重启代价。
        self.assertIn("restart_media_provider_process", recovery)
        self.assertLess(
            recovery.index('wait_media_provider_hook_ready "module-boot"'),
            recovery.index("restart_media_provider_process"),
        )
        self.assertLess(
            recovery.index("restart_media_provider_process"),
            recovery.index('wait_media_provider_hook_ready "module-restart"'),
        )
        self.assertLess(
            recovery.index('wait_media_provider_hook_ready "module-restart"'),
            recovery.index("adb reboot"),
        )
        self.assertLess(
            install.index("restart_media_provider_process() {"),
            install.index("verify_media_provider_hook_with_reboot_retry()"),
        )
        # 失败现场必须能区分"Zygisk 整体没注入"与"只漏了 MediaProvider"。
        self.assertIn("module_zygisk_files", wait)
        self.assertIn("module_mapped_processes", wait)
        self.assertIn("module_running_log_tail", wait)
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

    def test_mediastore_insert_patches_public_data_not_sandbox_relative(self) -> None:
        """insert 必须补公共形态的 _data，且不得把沙箱前缀写进 relative_path。

        MediaProvider 的 assertPrivatePathNotInValues 会把 _data 与 relative_path 两个字段都送进
        FileUtils.isDataOrObbRelativePath()：只要其一出现 Android/data|obb 形态且调用方无权访问，
        该次 insert 立即抛 IllegalArgumentException("Inserting private file: ... is not allowed")，
        场景 2 的 mediastore_create_file 随之返回 null。ensureFileColumns 又以 _data 是否为空分流，
        只有 _data 非空时才按该路径建父目录并跳过 relative_path 落点校验。因此守卫锁定：
        1. insert 且原本没有 _data 时必须补 _data；
        2. 补进去的 _data 与 relative_path 都必须是公共显示路径，不能带 Android/data 沙箱前缀；
        3. 不得再引入向 relative_path 写沙箱相对段的 helper；
        4. 补 _data 必须以「物理落点在沙箱内」为前置条件。
        """
        java = read("java_src/org/srx/hook/Hooker.java")
        patch = section(java, "private static ContentValuesPatch patchContentValues(", "\n  /**")
        self.assertIn('if (insertLike && dataKey == null) {', patch)
        self.assertIn('patched.put("_data", publicPath);', patch)
        self.assertIn("buildMediaStoreProbePath(relativePath, insertDisplayName, callerUid)", patch)
        # 沙箱前缀一旦进入 relative_path 就会被 MediaProvider 拒绝，helper 不得回归。
        self.assertNotIn("resolveMediaStoreSandboxRelativePath", java)
        # _data 用的必须是显示路径构造器，而不是物理路径构造器，否则同样命中私有路径校验。
        data_section = section(patch, 'if (insertLike && dataKey == null) {', "    if (!insertLike")
        self.assertNotIn("resolveMediaStoreDirectPathForValues", data_section)
        # 补 _data 会让 patchedAny 变真，下游据此把该 URI 登记成重定向目标。被 allowed_real_paths
        # 放行的路径本不该重定向，登记后随后的 open 会以 mapped_resolve_miss 失败，表现就是
        # Android 13 场景 16 的 createMedia returned null。因此必须先判定写入确实进沙箱。
        self.assertIn("&& mediaStoreValueLandsInSandbox(publicPath, callerUid)) {", data_section)
        helper = section(
            java,
            "private static boolean mediaStoreValueLandsInSandbox(",
            "\n  /**",
        )
        self.assertIn("resolveMediaStoreDirectPathForValues(path, callerUid)", helper)
        self.assertIn("isSrxSandboxFallbackPath(directPath, callerUid)", helper)

    def test_mediastore_pending_update_replays_public_target(self) -> None:
        """发布 pending 文件的 update 必须回填公共目标，不能在 ensureFileColumns 里露出沙箱路径。

        insert 成功后 MediaProvider 会在发布 pending 的 update 中再次执行 ensureFileColumns；
        若此时 values 露出 Android/data 私有目录，会以“禁止插入私有文件”结束。守卫锁定
        providerMediaFileColumnCallback 的 pending 回填分支写入的是公共 _data 与公共 relative_path。
        """
        java = read("java_src/org/srx/hook/Hooker.java")
        callback = section(
            java,
            "public Object providerMediaFileColumnCallback(",
            "private static boolean isInsertLikeMutation(",
        )
        self.assertIn('values.put(MediaStore.MediaColumns.DATA, pendingContext.publicPath);', callback)
        self.assertIn("mediaStoreRelativePath(pendingContext.publicPath)", callback)

    def test_mount_status_markers_are_not_written(self) -> None:
        """不得再往应用数据目录写 `.srx_mount_status_<pid>` 标记文件。

        旧实现按 PID 命名标记，应用每次重启都是新 PID 而清理只删「当前 PID」那一个，于是
        历史文件在 /data/user/0/<包名>/ 下无限累积。守卫锁定：生产源码里只有遗留清理模块可以
        提到该前缀，且它只做删除；一旦有人重新引入写路径，这里必须失败。
        """
        sources = sorted((ROOT / "src").rglob("*.rs"))
        offenders = []
        for path in sources:
            rel = path.relative_to(ROOT).as_posix()
            text = path.read_text(encoding="utf-8")
            if ".srx_mount_status_" not in text:
                continue
            if rel != "src/legacy_mount_marker.rs":
                offenders.append(rel)
        self.assertEqual([], offenders, "只有遗留清理模块可以引用该前缀")

        legacy = read("src/legacy_mount_marker.rs")
        self.assertIn('const LEGACY_MARKER_PREFIX: &str = ".srx_mount_status_";', legacy)
        # 清理模块只能删，不能建：出现 open/create/write 即说明写路径复活了。
        for forbidden in ("O_CREAT", "fs::write", "File::create", "OpenOptions"):
            self.assertNotIn(forbidden, legacy)

    def test_mount_readiness_is_decided_from_mountinfo(self) -> None:
        """挂载是否生效必须由应用自己读 mountinfo 判定，而不是等 daemon 写状态文件。

        守卫锁定四件事，缺一条都会让应用在启动阶段被拖死或让判定失真：

        1. 应用侧轮询本模块挂载并等到集合稳定；
        2. 轮询判据覆盖两条后端——FUSE 后端源带 `srx_fuse_*` 前缀，命名空间后端源是模块私有的
           临时锚点（`tmp/real_storage/`）；只认前缀会让命名空间后端的应用永远等不到确认；
        3. 轮询预算必须远低于 AMS 的进程启动超时（约 10 秒）——这个等待跑在应用主线程上，
           预算接近它就会让应用被判 `start timeout` 杀掉，真机上微信等应用都会 `failed to attach`；
        4. daemon 侧不再写标记，测试流的挂载确认只认两条日志。
        """
        post = read("src/lifecycle/specialize_post.rs")
        wait = section(post, "fn wait_for_module_mount(", "\nfn log_post_perf(")
        self.assertIn("app_redirect_mounts_in(0, package_name)", wait)
        self.assertIn("MOUNT_SETTLE_POLLS", wait)
        self.assertNotIn(".srx_mount_status_", wait)
        # 测试流按这一行确认挂载，措辞不能静默改掉。
        self.assertIn('"app mount confirmed pid={}', post)

        source = read("src/module_mount_source.rs")
        self.assertIn("pub fn is_module_anchor_mount_source(", source)
        self.assertIn("REAL_STORAGE_TMP_PREFIX", source)
        self.assertIn("is_module_anchor_mount_source(&source)", source)
        # 命名空间后端的 bind 记录里，沙箱路径在 `root` 而不是 `source`（source 是块设备），
        # 且必须用文件系统类型把系统自己的媒体 FUSE 排除掉，否则会把「系统挂载存在」误当成
        # 「我们的重定向已生效」。
        self.assertIn("pub fn is_namespace_redirect_mount(", source)
        self.assertIn("is_namespace_redirect_mount(&entry, package_name)", source)
        namespace_redirect = section(
            source,
            "pub fn is_namespace_redirect_mount(",
            "\n/// 列出目标命名空间内全部由本模块创建",
        )
        self.assertIn("entry.fs_type.starts_with(\"fuse\")", namespace_redirect)
        self.assertIn("unescape_field(entry.root)", namespace_redirect)
        # 按 source 匹配沙箱路径是错的（bind 的 source 永远是块设备），不得回归。
        self.assertNotIn("is_app_sandbox_mount_source", source)
        # 这个函数只负责「挂载源带固定前缀」这一条证据，不得把沙箱路径塞进来：
        # 挂载源与沙箱路径是两个独立维度，混在一起会让调用方无法分辨用的是哪一条。
        # 组合判据在 is_module_redirect_mount 里，由 test_mount_ownership_uses_sandbox_root_not_source_prefix 锁定。
        ownership = section(
            source,
            "pub fn is_module_mount_source(",
            "\n/// 判断挂载源是否指向模块自己的临时锚点目录。",
        )
        self.assertNotIn("Android/data/", ownership)
        self.assertNotIn("REAL_STORAGE_TMP_PREFIX", ownership)

        companion = read("src/lifecycle/companion_mount.rs")
        self.assertNotIn("write_mount_status_marker", companion)
        daemon_mount = read("src/daemon_mount.rs")
        self.assertNotIn("write_mount_status_marker", daemon_mount)

        confirm = section(self.bash, "wait_app_mount_confirmed() {", "\nscenario_from_label() {")
        self.assertIn("app mount confirmed pid=", confirm)
        # 变量在 shell 双引号里写作 \$pid，这里只锚定不随转义变化的部分。
        self.assertIn("daemon mount pkg=$APP_ID", confirm)
        self.assertIn("op=Reload ok=true", confirm)
        self.assertNotIn(".srx_mount_status_", confirm)

    def test_namespace_redirect_mount_rule_matches_real_mountinfo(self) -> None:
        """用真机抓到的 mountinfo 样本校验命名空间后端的识别规则。

        这条规则错过两次：先是只按 `source` 前缀匹配（bind 挂载的 source 是块设备，永远匹配
        不上），后是按 `source` 里含 `Android/data/<包名>` 匹配（同样永远匹配不上）。真实形态是
        沙箱路径在 `root` 字段，而系统自己的媒体 FUSE 挂载 `root` 里也有 `Android/data/<包名>`，
        必须靠文件系统类型排除。

        样本取自真机（Android 16 / KernelSU）上 `me.fakerqu.test.storageredirect` 的 mountinfo，
        规则在这里重写一遍是为了对真实数据做行为校验；Rust 侧的关键判据由
        `test_mount_readiness_is_decided_from_mountinfo` 做静态锁定。
        """
        package_name = "me.fakerqu.test.storageredirect"

        def is_namespace_redirect(line: str) -> bool:
            fields = line.split(" - ", 1)
            before = fields[0].split()
            root = before[3]
            fs_type = fields[1].split()[0]
            if fs_type.startswith("fuse"):
                return False
            return any(
                f"{prefix}{package_name}" in root
                for prefix in ("Android/data/", "Android/media/", "Android/obb/")
            )

        # 模块建立的命名空间绑定：root 是沙箱 sdcard 子树，类型保留底层 f2fs。
        ours = [
            "5419 5404 254:60 /media/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
            " /storage/emulated/0 rw,noatime - f2fs /dev/block/dm-60 rw,lazytime",
            "5420 5084 254:60 /media/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
            " /mnt/user/0/emulated/0 rw,nosuid,nodev,noatime - f2fs /dev/block/dm-60 rw",
        ]
        for line in ours:
            self.assertTrue(is_namespace_redirect(line), f"未识别为模块重定向: {line}")

        # 系统自己的挂载必须排除：媒体 FUSE 的 root 里同样有 Android/data/<包名>。
        system_mounts = [
            "5424 5419 0:131 /0/Android/data/me.fakerqu.test.storageredirect"
            " /storage/emulated/0/Android/data/me.fakerqu.test.storageredirect rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            "5408 5407 254:60 /user_de/0/me.fakerqu.test.storageredirect"
            " /data/user_de/0/me.fakerqu.test.storageredirect rw,nosuid,nodev,noatime"
            " - f2fs /dev/block/dm-60 rw,lazytime",
            "5409 5405 254:60 /data/me.fakerqu.test.storageredirect"
            " /data/data/me.fakerqu.test.storageredirect rw,nosuid,nodev,noatime"
            " - f2fs /dev/block/dm-60 rw,lazytime",
        ]
        for line in system_mounts:
            self.assertFalse(
                is_namespace_redirect(line), f"系统挂载被误判为模块重定向: {line}"
            )

    def test_mount_ownership_uses_sandbox_root_not_source_prefix(self) -> None:
        """摘除归属判据必须能认出「bind 继承掉挂载源」的层，否则重挂只会不断叠加。

        真机上本模块每一层的 `source` 都是继承来的 MediaProvider FUSE 的 `/dev/fuse`
        （进程内 `srx_fuse_` 前缀匹配数为 0），而旧判据只认固定前缀，于是：

        - `clear_mount_target_stack_verified` 把最顶层判成外部挂载、直接放弃摘除；
        - `capture_mount_identity` 恒返回 None，账本 `mounts=0`，监督永远产不出 `Owned`。

        后果是同一个挂载点上累积 4 层、应用读到被压在最上面的沙箱层（场景 29 的
        `srt_hot_after.txt` 落进沙箱而非真实 `Download/Test`）。

        判据必须落在 `root` 的沙箱标记 `<包名>/sdcard` 上：系统自己的媒体 FUSE 挂载
        `root` 只到 `Android/data/<包名>`，不含 `sdcard`，因此不会被误伤。
        """
        source = read("src/module_mount_source.rs")
        self.assertIn("pub fn is_module_sandbox_root(", source)
        self.assertIn("pub fn is_module_redirect_mount(", source)
        sandbox_root = section(
            source, "pub fn is_module_sandbox_root(", "\n/// 去掉 `/media` 前缀后的 `root`"
        )
        # 沙箱标记必须是 `<包名>/sdcard` 而不是裸 `<包名>`——后者与系统挂载同形，会误摘。
        self.assertIn("/sdcard", sandbox_root)
        self.assertIn("{prefix}{package_name}/sdcard", sandbox_root)
        # 判据不得限制文件系统类型：同一轮挂载会同时产出 f2fs（从 /data/media 绑）与
        # fuse（从 /storage/emulated 绑）两种形态，只认一种就会漏判一半的层。
        self.assertNotIn("fs_type", sandbox_root)

        # 路径映射的层从「映射目标」bind，`root` 里没有包名，必须靠
        # 「落在 emulated 存储视图内、且不是系统自己挂的应用专属目录」兜住；
        # 否则仅映射模式的应用永远等不到挂载确认（场景 6/7）。
        self.assertIn("fn root_is_app_private_directory(", source)
        self.assertIn("fn root_is_emulated_storage_view(", source)
        private_dir = section(
            source, "fn root_is_app_private_directory(", "\n/// 判断 `root` 是否落在 emulated 存储视图内"
        )
        # 系统形态必须是「恰好止于包名」：多一段（如 `/sdcard`）就是模块的层。
        self.assertIn("!name.contains('/')", private_dir)
        # 锚点绑定的 source 被 bind 继承成 /dev/fuse、root 是 `/0`，两者都不带模块特征，
        # 只能按**挂载点**识别（锚点目录是模块私有路径，系统不会往那里挂）。
        self.assertIn("pub fn is_module_anchor_mount_target(", source)
        predicate = section(
            source, "pub fn is_module_redirect_mount(", "\n/// 列出目标命名空间内全部由本模块创建"
        )
        self.assertIn("is_module_anchor_mount_target(target)", predicate)
        self.assertIn("target: &str", predicate)
        # 系统形态必须先排除，否则同形的模块层会被一起否掉。
        self.assertIn("root_is_app_private_directory(root)", predicate)
        self.assertIn("root_is_emulated_storage_view(root)", predicate)
        # 顺序：排除项必须排在两条包含项之前，否则同形的模块层会被一起否掉。
        self.assertLess(
            predicate.index("root_is_app_private_directory(root)"),
            predicate.index("is_module_sandbox_root(root, package_name)"),
            "系统形态的排除必须排在包含判据之前",
        )
        # 应用侧「重定向是否生效」也必须走同一份判据。
        app_mounts = section(
            source, "pub fn app_redirect_mounts_in(", "\n}\n")
        self.assertIn("is_module_redirect_mount(", app_mounts)
        # 三个调用点都要把挂载点传进去，漏一个就会在那条路径上失配。
        for path, anchor, target_arg in (
            ("src/mount_ledger.rs", "pub fn capture_mount_identity(", "mount_point,"),
            ("src/daemon_mount.rs", "fn clear_mount_target_stack_verified(", "target,"),
        ):
            body = section(read(path), anchor, "\n}\n")
            call = body[body.index("is_module_redirect_mount("):]
            call_args = call[: call.index(")") + 1]
            self.assertIn(
                target_arg,
                call_args,
                f"{path} 里的 is_module_redirect_mount 调用必须把挂载点作为参数传入",
            )

        # 摘除与账本登记都必须走同一份判据，不得退回只看挂载源。
        daemon_mount = read("src/daemon_mount.rs")
        clear = section(
            daemon_mount,
            "fn clear_mount_target_stack_verified(",
            "\nfn is_mount_stack_cleared(",
        )
        self.assertIn("is_module_redirect_mount(", clear)
        self.assertNotIn("is_module_mount_source(", clear)

        ledger = read("src/mount_ledger.rs")
        capture = section(ledger, "pub fn capture_mount_identity(", "\n/// 按本次挂载的目标集合登记账本")
        self.assertIn("is_module_redirect_mount(", capture)
        self.assertIn("package_name: &str", capture)
        # `root` 是判据的唯一可靠依据，`LiveMount` 必须把它读出来。
        self.assertIn("pub root: String", ledger)

        # 判据里的 `/sdcard` 标记与 `default_redirect_target` 是同一条约定的两处表达，
        # 沙箱目录名一旦改动必须同步，否则判据会静默失配、重挂重新开始叠加。
        paths_source = read("src/platform/paths.rs")
        default_target = section(
            paths_source,
            "pub fn default_redirect_target(",
            "\npub fn is_default_redirect_backend_path(",
        )
        self.assertIn('"{}/Android/data/{}/sdcard"', default_target)

    def test_sandbox_root_rule_separates_module_layers_from_system_mounts(self) -> None:
        """用真机抓到的 mountinfo 样本校验新的归属判据。

        样本取自真机（Android 16 / KernelSU）上 `me.fakerqu.test.storageredirect` 的 mountinfo。
        模块层与系统层的区别有两处：沙箱层 `root` 带 `<包名>/sdcard`；映射层 `root` 指向
        映射目标（不带包名，形如 `/0/Download/SrtMapOnlyMapped`）。系统层 `root` 恰好止于
        `Android/{data,media,obb}/<包名>`。
        """
        package_name = "me.fakerqu.test.storageredirect"

        def storage_tail(root: str):
            rest = root[len("/media"):] if root.startswith("/media") else root
            if not rest.startswith("/"):
                return None
            rest = rest[1:]
            user, _, tail = rest.partition("/")
            if not user or not user.isdigit():
                return None
            return tail

        def is_app_private_directory(root: str) -> bool:
            tail = storage_tail(root)
            if tail is None:
                return False
            for prefix in ("Android/data/", "Android/media/", "Android/obb/"):
                if tail.startswith(prefix):
                    name = tail[len(prefix):]
                    return bool(name) and "/" not in name
            return False

        def is_module_layer(line: str) -> bool:
            before, after = line.split(" - ", 1)
            root = before.split()[3]
            source = after.split()[1]
            if source.startswith("srx_fuse_redirect") or source.startswith("srx_fuse_host"):
                return True
            if is_app_private_directory(root):
                return False
            if f"/sdcard" in root and package_name in root:
                return True
            tail = storage_tail(root)
            return tail is not None and tail != ""

        ours = [
            # 存储根：从沙箱子树 bind，源被继承成 MediaProvider FUSE 的 /dev/fuse。
            "13661 13623 0:324 /0/Android/data/me.fakerqu.test.storageredirect/sdcard"
            " /storage/emulated/0 rw,nosuid,nodev,noexec,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 同一轮里从 /data/media 绑的那一份，类型是 f2fs。
            "13638 13637 254:60 /media/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
            " /storage/emulated/0 rw,noatime - f2fs /dev/block/dm-60 rw,lazytime",
            # 路径映射：源是沙箱里的 Download/Test。
            "13724 13683 0:324 /0/Android/data/me.fakerqu.test.storageredirect/sdcard/Download/Test"
            " /storage/emulated/0/Download/SrtProbe rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 恢复自有私有目录时沙箱前缀被叠了两层，仍必须认出来才能摘净。
            "13706 13666 0:324 /0/Android/data/me.fakerqu.test.storageredirect/sdcard/Android/data"
            "/me.fakerqu.test.storageredirect"
            " /storage/emulated/0/Android/data/me.fakerqu.test.storageredirect rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 仅映射模式：从「映射目标」bind，root 里没有包名。
            "12638 12635 0:324 /0/Download/SrtMapOnlyMapped"
            " /storage/emulated/0/Download/SrtProbe rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 路径映射指向真实公共目录时的形态。
            "13681 13661 0:324 /0/Download/Test"
            " /storage/emulated/0/Download/SrtProbe rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
        ]
        for line in ours:
            self.assertTrue(is_module_layer(line), f"未识别为本模块的层: {line}")

        system_mounts = [
            # 系统 MediaProvider 在应用专属目录上的媒体 FUSE：root 恰好止于包名。
            "13643 13638 0:324 /0/Android/data/me.fakerqu.test.storageredirect"
            " /storage/emulated/0/Android/data/me.fakerqu.test.storageredirect rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            "13649 13638 0:324 /0/Android/media/me.fakerqu.test.storageredirect"
            " /storage/emulated/0/Android/media/me.fakerqu.test.storageredirect rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            "13655 13638 0:324 /0/Android/obb/me.fakerqu.test.storageredirect"
            " /storage/emulated/0/Android/obb/me.fakerqu.test.storageredirect rw,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 存储根自身的系统 FUSE 视图。
            "13623 13479 0:324 / /storage/emulated rw,nosuid,nodev,noexec,noatime"
            " - fuse /dev/fuse rw,lazytime,user_id=0,group_id=0,allow_other",
            # 非 emulated 存储树的 bind（/data、/user_de）不能算作模块的层。
            "13627 13626 254:60 /user_de/0/me.fakerqu.test.storageredirect"
            " /data/user_de/0/me.fakerqu.test.storageredirect rw,nosuid,nodev,noatime"
            " - f2fs /dev/block/dm-60 rw,lazytime",
            "13628 13626 254:60 /data/me.fakerqu.test.storageredirect"
            " /data/data/me.fakerqu.test.storageredirect rw,nosuid,nodev,noatime"
            " - f2fs /dev/block/dm-60 rw,lazytime",
        ]
        for line in system_mounts:
            self.assertFalse(is_module_layer(line), f"系统挂载被误判为本模块的层: {line}")

    def test_mount_wait_budget_stays_under_process_start_timeout(self) -> None:
        """应用主线程上的挂载等待预算必须远低于 AMS 的进程启动超时。

        这个等待发生在 zygisk specialize 之后、应用主线程上；AMS 对进程启动的超时约 10 秒，
        预算一旦接近它，任何「判据没能立刻给出肯定答案」的情况都会让应用被判 `start timeout`
        并杀掉——真机上微信与测试应用都出现过 `failed to attach` + `start timeout`，所有依赖
        文件系统的用例随之超时。这里把上限锁在 2 秒：既留出足够余量覆盖落定确认，又保证即使
        判据完全不成立也不会把应用拖死。
        """
        timing = read("src/lifecycle/mount_timing.rs")
        # 常量可能是 `30` 这样的字面量，也可能是 `20 * 1000` 这样的算式，两种都要能读。
        count = _const_product(timing, "POST_MOUNT_STATUS_POLL_COUNT")
        delay_us = _const_product(timing, "POST_MOUNT_STATUS_POLL_DELAY_US")
        budget_ms = count * delay_us // 1000
        self.assertLessEqual(
            budget_ms,
            2000,
            f"挂载等待预算 {budget_ms}ms 过高，会把应用拖过 AMS 启动超时（约 10s）",
        )
        self.assertGreaterEqual(budget_ms, 200, "预算过短会让正常的落定确认来不及完成")
        # 等待必须由短轮询实现，不得改成单次长睡眠（那样无法在挂载就绪后立刻返回）。
        wait = section(read("src/lifecycle/specialize_post.rs"), "fn wait_for_module_mount(", "\nfn log_post_perf(")
        self.assertIn("POST_MOUNT_STATUS_POLL_DELAY_US", wait)

    def test_fixture_residue_is_rejected_before_each_scenario(self) -> None:
        """清理后夹具根必须为空，否则跨场景残留会在离现场很远的断言上爆掉。

        场景 20 的 file_unexpected 就是这么来的：前序场景的探针没被清掉，却在场景 20 的
        「不应存在」断言上失败。守卫锁定三件事：清理后有空目录前置断言；容忍式清理失败必须留下
        统一可检索标记而不是自由文本；自行预置夹具的场景必须显式豁免，否则断言会误报。
        """
        helper = section(self.bash, "assert_fixture_roots_empty() {", "\nclean_results() {")
        self.assertIn("-type f || exit 1", helper)
        self.assertNotIn("-maxdepth 2", helper)
        self.assertIn("fixture_scan_failed", helper)
        self.assertIn("fixture_residue label=", helper)
        runner = section(self.bash, "run_scenario() {", "\n# 与 PowerShell 共用设备端锁")
        self.assertIn('assert_fixture_roots_empty "scenario-${scenario}" || return 1', runner)
        self.assertRegex(runner, r"9\|20\|21\|22\|28\|31\) ;;")
        # 容忍式清理必须留下统一前缀，便于在日志里统计与定位，不能只写一句自由文本。
        self.assertIn("cleanup_tolerated reason=probe_fuse_edom", self.bash)
        self.assertIn("cleanup_tolerated reason=own_dir_fuse", self.bash)

        ps = section(self.powershell, "function Assert-FixtureRootsEmpty", "function Clear-Targets")
        self.assertIn("fixture_residue label=", ps)
        self.assertIn("-type f || exit 1", ps)
        self.assertNotIn("-maxdepth 2", ps)
        self.assertIn("fixture_scan_failed", ps)
        ps_runner = section(self.powershell, "function Invoke-Scenario {", "\n    $scenarioOk = switch")
        self.assertIn('Assert-FixtureRootsEmpty "scenario-$Scenario"', ps_runner)
        self.assertIn("-notin @(9, 10, 20, 21, 22, 28, 31)", ps_runner)

    def test_fixture_cleanup_sweeps_directories_by_prefix(self) -> None:
        """夹具清理必须按 `Srt*` 通配动态枚举目录，不能只依赖手写清单。

        手写清单一定会漏：场景 29 的前置断言就抓到过
        `fixture_residue .../SrtQqAliasMapped/srt_qq_alias_existing.bin`——该目录只被定义为变量、
        从未进入清理清单，于是场景 36 写入的文件残留到下一轮，把场景 29 卡在入口。
        枚举口径要与场景前置断言一致（同一个 `Srt*` 通配），否则清理与断言会各说各话。
        """
        # bash：可见路径与后端路径各要有一条通配清扫。两条都写作 `${REAL_ROOT}/...`（该变量在
        # 函数中途被切到后端根），所以只能按「相对切换点的位置」区分：可见那条必须在切换之前，
        # 后端那条必须在切换之后。
        clean = section(self.bash, "clean_targets() {", "\n}\n")
        switch = clean.index('local REAL_ROOT="${BACKEND_ROOT}"')
        sweeps = [index for index in range(len(clean)) if clean.startswith("'/Srt*", index)]
        self.assertGreaterEqual(len(sweeps), 2, "可见路径与后端路径都要有 Srt* 通配清扫")
        self.assertTrue(
            any(index < switch for index in sweeps), "可见路径缺少 Srt* 通配清扫"
        )
        self.assertTrue(
            any(index > switch for index in sweeps), "后端路径缺少 Srt* 通配清扫"
        )
        # 通配清扫必须在后端 mkdir 之前：清扫会把目录删掉，mkdir 负责重建。
        self.assertLess(
            sweeps[0], clean.index("mkdir -p '${REAL_ROOT}/Download/SrtProbe'")
        )

        # PowerShell：同样的两条。
        clear = section(self.powershell, "function Clear-Targets {", "\nfunction Remove-TestTargetArtifacts")
        self.assertIn("'$RealRoot/Download'/Srt*", clear)
        self.assertIn("'$BackendRoot/Download'/Srt*", clear)
        self.assertLess(
            clear.index("/Srt*"), clear.index("mkdir -p '$BackendRoot/Download/SrtProbe'")
        )
        remove = section(self.powershell, "function Remove-TestTargetArtifacts", "\nfunction ")
        self.assertIn("'$BackendRoot/Download'/Srt*", remove)

    def test_scenario_scripts_do_not_match_volatile_diagnostic_log_fields(self) -> None:
        """测试脚本只允许依赖稳定的对外日志串，不得匹配诊断字段。

        CI（`ci.yml` → `run-android-test-flow.sh` → `run-storage-redirect-scenarios.sh`）与
        本地真机跑的是**同一个** `run-storage-redirect-scenarios.sh`，`.ps1` 是同一脚本的
        PowerShell 并行实现（`scripts/verify-test-flow.ps1` 与 `run-fuse-mount-stress.ps1` 用它）。
        因此模块内部日志一旦被脚本匹配，改动它就要同时改两处脚本——而模块的归属/摘除诊断字段
        恰恰是最常调整的部分（例如 `daemon unmount skipped foreign` 加 `root=`、
        `skipped superseded` 把布尔换成 `recorded=<mount_id>`）。

        这条守卫把「脚本只认稳定串」固定下来：`app mount confirmed pid=`、
        `daemon mount ... op=Reload ok=true`、`backend_effective pkg=` 是挂载确认与后端结论的
        对外接口，改动必须同步脚本；而 `unmount skipped|cleanup incomplete|anchor polluted|
        mount identity saved|real storage anchored` 属诊断细节，**不得**被脚本匹配。
        """
        stable = [
            "app mount confirmed",
            "op=Reload ok=true",
            "backend_effective pkg=",
        ]
        volatile = [
            "unmount skipped",
            "cleanup incomplete",
            "anchor polluted",
            "anchor unavailable",
            "mount identity saved",
            "real storage anchored",
            "detach_attempts",
        ]
        for name, script in (("bash", self.bash), ("powershell", self.powershell)):
            for token in stable:
                self.assertIn(token, script, f"{name} 脚本缺少稳定的对外日志串 `{token}`")
            for token in volatile:
                self.assertNotIn(
                    token,
                    script,
                    f"{name} 脚本不得匹配诊断字段 `{token}`：改它就要动脚本，"
                    "诊断字段属实现细节，应改为匹配稳定串或断言物理状态",
                )

        # 挂载确认必须两脚本一致地认「应用侧」与「daemon 重挂」两条日志。
        # 只认应用侧那一条时，仅由 daemon 重挂的场景（热更新后的 reconcile）会在这里空等超时——
        # `.ps1` 曾经就是这样落后于 `.sh` 的。
        #
        # 断言前先剔除注释行：函数注释里也会提到这两个日志名，只断言裸串会被注释满足，
        # 把真正的代码删掉也测不出来（反向验证踩过）。
        confirm_bash = section(
            self.bash, "wait_app_mount_confirmed() {", "\nscenario_from_label() {"
        )
        confirm_ps = section(
            self.powershell, "function Wait-AppMountConfirmed {", "\nfunction "
        )
        for name, body in (("bash", confirm_bash), ("powershell", confirm_ps)):
            code = "\n".join(
                line for line in body.splitlines() if not line.strip().startswith("#")
            )
            self.assertIn(
                "app mount confirmed pid=", code, f"{name} 缺少应用侧确认串"
            )
            self.assertIn(
                "op=Reload ok=true",
                code,
                f"{name} 的挂载确认缺少 daemon 重挂串"
                "（`daemon mount pkg=... op=Reload ok=true`）",
            )
            # 两条串必须被同一个匹配表达式用上，不能只是定义后不用。
            # `.ps1` 在 PowerShell here-string 里写作 `` `$confirmed|`$daemon ``（`$` 被反引号转义），
            # 因此按「同一行 grep -Eq 同时含两个变量名」判定，不比较字面写法。
            combined = [
                line
                for line in code.splitlines()
                if "grep -Eq" in line and "$confirmed" in line and "$daemon" in line
            ]
            self.assertTrue(combined, f"{name} 未把两条串一起用于匹配")

    def test_powershell_runner_matches_bash_mountinfo_recheck(self) -> None:
        """`.ps1` 必须与 `.sh` 一样，在挂载确认后独立复核挂载点。

        `.sh` 的 `wait_app_mount_confirmed` 确认到 PID 之后会调用
        `app_mountinfo_has_expected_paths` 再查一遍 `/proc/<pid>/mountinfo`——日志只是触发器，
        真正要证明的是「挂载点确实存在」。`.ps1` 长期缺这一层，日志命中即视为通过，
        于是「日志出现过但挂载已被摘除」在 `.ps1` 路径下测不出来。
        """
        ps = self.powershell
        self.assertIn("function Get-ExpectedMountPathsForLabel", ps)
        self.assertIn("function Assert-AppMountinfoHasExpectedPaths", ps)
        self.assertIn("function Ensure-CurrentAppMountConfirmed", ps)

        # 期望路径表必须与 .sh 同口径（同样只覆盖场景 3 与 4），否则两脚本复核范围不一致。
        bash_expected = section(
            self.bash, "expected_mount_paths_for_label() {", "\napp_mountinfo_has_expected_paths() {"
        )
        ps_expected = section(
            ps, "function Get-ExpectedMountPathsForLabel {", "\nfunction Assert-AppMountinfoHasExpectedPaths"
        )
        for name, body in (("bash", bash_expected), ("powershell", ps_expected)):
            code = "\n".join(
                line for line in body.splitlines() if not line.strip().startswith("#")
            )
            self.assertIn("SrtProbe", code, f"{name} 的期望路径表缺少场景 3 的 SrtProbe")
            self.assertIn("$RealRoot/Download" if name == "powershell" else "${REAL_ROOT}/Download", code,
                          f"{name} 的期望路径表缺少场景 4 的 Download")

        # 复核必须按**显式输出标记**判定，不能只看退出码：`$LASTEXITCODE` 是全局变量，
        # 任何中间的原生命令都会覆盖它，依赖它在后续改动里容易静默失效。
        #
        # 断言前先剔除注释行——注释里会提到 `$LASTEXITCODE` 说明为什么不用它，
        # 只断言裸串会被注释满足（这个坑在本文件里踩过第二次了）。
        recheck = section(
            ps, "function Assert-AppMountinfoHasExpectedPaths {", "\nfunction Ensure-CurrentAppMountConfirmed"
        )
        recheck_code = "\n".join(
            line for line in recheck.splitlines() if not line.strip().startswith("#")
        )
        self.assertIn("all_present", recheck_code, "复核缺少成功标记，无法按输出判定")
        self.assertIn("missing=", recheck_code, "复核缺少失败标记")
        self.assertNotIn(
            "$LASTEXITCODE",
            recheck_code,
            "复核不得依赖 $LASTEXITCODE（全局变量，会被中间的原生命令覆盖）",
        )

        # PowerShell 变量名大小写不敏感，`$Pid` 与只读内置 `$PID` 冲突（实测报
        # 「无法覆盖变量 Pid，因为它是只读变量或常量」，且在赋值处才炸）。
        ps_code = "\n".join(
            line for line in ps.splitlines() if not line.strip().startswith("#")
        )
        self.assertNotRegex(
            ps_code,
            r"\$Pid\b",
            "`.ps1` 不得使用 `$Pid` 作变量/参数名（与只读内置变量 `$PID` 冲突）",
        )

    def test_scenario_scope_override_and_commit_message_parsing(self) -> None:
        """场景取景必须真能收窄范围，且不带取景时不改变全量（仍为 all）。

        解析已上移到 ``prepare`` 的 outputs，由 ``parse_scenario_scope.py`` 统一执行；
        下游 ``test-flow`` 直接引用 ``needs.prepare.outputs.scenarios``，不再在 runner
        内就地二次解析。因此这里执行真实解析器，而不是只断言字符串存在。
        """
        workflow = read(".github/workflows/ci.yml")
        # 单一来源：test-flow 引用 prepare 的 outputs，runner 内不再把提交信息/
        # 手动输入透传给取景解析片段（避免与 prepare 双源头）。
        self.assertIn(
            "SRT_SCENARIOS: ${{ needs.prepare.outputs.scenarios }}",
            workflow,
        )
        self.assertNotIn(
            "SRT_SCENARIOS_OVERRIDE:",
            workflow,
            "runner 内不应再就地解析取景，范围由 prepare 统一决定",
        )
        self.assertNotIn(
            "SRT_COMMIT_MESSAGE:",
            workflow,
            "runner 内不应再就地解析提交标记，范围由 prepare 统一决定",
        )
        self.assertIn("workflow_dispatch:", workflow, "必须提供可复用的手动触发入口")
        self.assertIn("parse_scenario_scope.py", workflow, "prepare 必须调用真实解析器")
        self.assertIn(
            "scenarios: ${{ steps.scope.outputs.scenarios }}",
            workflow,
        )
        # runner 仍需保留本地取景回退（不依赖 prepare），识别「单场景」标记。
        runner = read(".github/tests/run-android-test-flow.sh")
        self.assertIn("单场景", runner, "runner 仍需保留本地取景回退以识别「单场景」标记")
        self.assertIn(
            "srx-scenario-scope:begin", runner, "runner 取景回退片段不得被误删"
        )

        parser = ROOT / ".github" / "scripts" / "parse_scenario_scope.py"
        python_bin = "python" if shutil.which("python") else "python3"
        # (手动输入, 提交信息, 期望 scenarios；空串表示无取景=全量 all)
        cases = (
            ("", "", "all"),
            ("", "修复：热重载保留重定向根", "all"),
            ("", "修复：热重载保留重定向根 单场景 29", "29"),
            ("", "修复 单场景29", "29"),
            ("", "修复 单场景: 29", "29"),
            ("", "修复 单场景：29,34", "29,34"),
            ("", "修复 单场景 29，34", "29,34"),
            ("", "修复 单场景 29 与 34", "29"),
            # 手动输入优先于提交信息，且同样接受中文逗号。
            ("29", "修复 单场景 34", "29"),
            ("29，34", "修复：说明", "29,34"),
        )
        for override, message, expected in cases:
            proc = subprocess.run(
                [
                    python_bin,
                    str(parser),
                    "--override", override,
                    "--message", message,
                    "--format", "github",
                ],
                capture_output=True,
            )
            self.assertEqual(
                proc.returncode, 0,
                f"解析失败 override={override!r} message={message!r}: "
                + proc.stderr.decode("utf-8", "replace"),
            )
            scenarios = ""
            for line in proc.stdout.decode("utf-8", "replace").splitlines():
                if line.startswith("scenarios="):
                    scenarios = line[len("scenarios=") :]
            self.assertEqual(
                scenarios, expected,
                f"override={override!r} message={message!r} 应选择 {expected!r}，"
                f"实际 {scenarios!r}",
            )

        # 非法范围必须显式失败，不得静默回退全量（派发输入路径）。
        for bad in ("99", "0", "abc", "29,29"):
            bad_proc = subprocess.run(
                [python_bin, str(parser), "--override", bad, "--format", "github"],
                capture_output=True,
            )
            self.assertNotEqual(
                bad_proc.returncode, 0,
                f"非法范围 {bad!r} 必须报错，不得静默跑全量",
            )

        # 提交信息标记存在但写法非法：不得偷截、不得静默回退全量。
        for bad_message in (
            "单场景 abc",
            "单场景 无编号",
            "单场景 29,abc",
            "单场景 29,,34",
            "单场景 29,",
        ):
            bad_proc = subprocess.run(
                [
                    python_bin,
                    str(parser),
                    "--message", bad_message,
                    "--format", "github",
                ],
                capture_output=True,
            )
            self.assertNotEqual(
                bad_proc.returncode, 0,
                f"非法标记 {bad_message!r} 必须报错，不得静默回退全量",
            )


if __name__ == "__main__":
    unittest.main()
