"""执行生产侧身份与恢复判定，覆盖系统视图延迟覆盖和未知挂载接管。"""

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_mount_identity_syntax import extract_fn, read_paths_module

ROOT = Path(__file__).resolve().parents[2]


class StorageViewRecoveryTest(unittest.TestCase):
    def test_production_recovery_matrix(self) -> None:
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc, "需要 rustc 执行恢复真值表")
        ledger = (ROOT / "src/mount_ledger.rs").read_text(encoding="utf8")
        identity = (ROOT / "src/mount_identity.rs").read_text(encoding="utf8")
        supervisor = (ROOT / "src/fuse_supervisor.rs").read_text(encoding="utf8")
        daemon = (ROOT / "src/daemon_mount.rs").read_text(encoding="utf8")
        host = (ROOT / "src/fuse_host.rs").read_text(encoding="utf8")
        paths = read_paths_module()
        mountinfo = (ROOT / "src/platform/mountinfo.rs").read_text(encoding="utf8")
        template = (ROOT / ".github/tests/harness/storage_view_recovery.rs").read_text(
            encoding="utf8"
        )
        functions = "\n".join(
            extract_fn(source, name)
            for source, name in (
                (ledger, "is_system_storage_view"),
                (ledger, "known_devs_for_boot"),
                (identity, "classify_mount"),
                (identity, "recorded_mounts_current"),
                (supervisor, "plan_recovery"),
                (supervisor, "severity"),
                (supervisor, "aggregate_actions"),
                (daemon, "clear_mount_target_stack_verified"),
                (daemon, "supervise_mount_request"),
                (host, "storage_alias_relative_path"),
            )
        )
        # 生产枚举和动作方法一起执行，避免桩固定优先级或允许注入的结论。
        actions = supervisor[
            supervisor.index("pub enum RecoveryAction") : supervisor.index(
                "/// 按身份判定与端点健康规划恢复动作"
            )
        ]
        normalize = supervisor[
            supervisor.index("impl RecoveryAction {", supervisor.index("pub fn plan_recovery"))
            : supervisor.index("/// 汇总多个挂载点的恢复动作")
        ]
        paths_code = "\n".join(
            extract_fn(paths, name)
            for name in (
                "normalize_syntax",
                "collapse_redundant_slashes",
                "storage_user_root_for_user",
                "data_media_user_root_for_user",
                "storage_alias_roots_for_user",
            )
        )
        source = (
            template.replace("// __PATHS__", paths_code)
            .replace("// __ACTIONS__", "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n" + actions + normalize)
            .replace("// __FUNCTIONS__", functions)
            .replace("// __HISTORY__", extract_fn(ledger, "known_module_fuse_devs"))
            .replace("// __MOUNTINFO__", mountinfo[mountinfo.index("pub struct MountInfoEntry"):mountinfo.index("/// 解析一行")] + extract_fn(mountinfo, "parse_entry"))
        )
        with tempfile.TemporaryDirectory(prefix="srx-storage-recovery-") as tmp:
            script = Path(tmp) / "main.rs"
            binary = Path(tmp) / "recovery"
            script.write_text(source, encoding="utf8")
            self.run_checked([rustc, "--edition=2024", str(script), "-o", str(binary)])
            self.run_checked([str(binary)])
            # 有意恢复旧版「任何 Superseded 都阻止恢复」，测试必须检出回归。
            blocked_recovery = source.replace(
                "MountVerdict::Superseded(_) if known_storage_view => RecoveryAction::DetachThenReinject,",
                "MountVerdict::Superseded(_) if known_storage_view => RecoveryAction::SkipSuperseded,",
            )
            foreign_probe = source.replace(
                "MountVerdict::Superseded(_) | MountVerdict::StaleNamespace",
                "MountVerdict::Detached",
            )
            untranslated_parent_root = source.replace(
                'format!("{}/{suffix}", backing_root.trim_end_matches(\'/\'))',
                "backing_root",
            )
            wrong_user_parent_root = source.replace(
                'format!("/user/{user_id}")',
                "init.root",
            )
            unrelated_host_root = source.replace('live.root == format!("/{suffix}")', "true")
            for name, mutant in (
                ("blocked_recovery", blocked_recovery),
                ("foreign_probe", foreign_probe),
                ("untranslated_parent_root", untranslated_parent_root),
                ("wrong_user_parent_root", wrong_user_parent_root),
                ("unrelated_host_root", unrelated_host_root),
            ):
                with self.subTest(mutant=name):
                    self.assertNotEqual(mutant, source)
                    script.write_text(mutant, encoding="utf8")
                    self.run_checked([rustc, "--edition=2024", str(script), "-o", str(binary)])
                    failed = subprocess.run([str(binary)], capture_output=True, text=True)
                    self.assertNotEqual(failed.returncode, 0, "故障变体不得通过恢复测试")

    def run_checked(self, command: list[str]) -> None:
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
