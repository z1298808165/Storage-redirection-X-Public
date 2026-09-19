import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / ".github" / "tests" / "harness" / "daemon_mount_health.rs"

SRC = ROOT / "src"


def read(path: str) -> str:
    return (SRC / path).read_text(encoding="utf-8")


def extract_fn(source: str, fn_name: str) -> str:
    """用大括号计数从源码中抽出整个 fn（含签名与函数体），不依赖固定结尾标记。

    与 test_mount_identity_syntax.py / test_caller_attribution_boundaries.py 同款抽取逻辑。
    """
    marker = f"fn {fn_name}"
    start = source.index(marker)
    if start >= 4 and source[start - 4 : start] == "pub ":
        start -= 4
    paren = source.index("(", start)
    brace_start = source.index("{", paren)
    depth = 0
    idx = brace_start
    while idx < len(source):
        ch = source[idx]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                idx += 1
                break
        idx += 1
    return source[start:idx]


def build_harness(template: str, mountinfo_fns: str, paths_fns: str, all_fns: str) -> str:
    out = template
    for placeholder, body in (
        ("// __INJECT_MOUNTINFO__", mountinfo_fns),
        ("// __INJECT_PATHS__", paths_fns),
        ("// __INJECT_ALL__", all_fns),
    ):
        if placeholder not in out:
            raise AssertionError(f"harness 模板缺少占位符: {placeholder}")
        out = out.replace(placeholder, body)
    return out


class DaemonMountHealthTest(unittest.TestCase):
    def test_extracted_functions_execute(self) -> None:
        if not HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("rustc 不可用：未编译执行 stub harness，仅静态边界守卫生效")

        template = HARNESS.read_text(encoding="utf-8")

        mountinfo_src = read("platform/mountinfo.rs")
        paths_src = read("platform/paths.rs")
        daemon_mount_src = read("daemon_mount.rs")

        mountinfo_fns = extract_fn(mountinfo_src, "parse_entry") + "\n" + extract_fn(
            mountinfo_src, "unescape_field"
        )
        paths_fns = extract_fn(paths_src, "normalize_syntax") + "\n" + extract_fn(
            paths_src, "collapse_redundant_slashes"
        )
        all_fns = "\n".join(
            [
                extract_fn(daemon_mount_src, "mount_targets_present_with"),
                extract_fn(daemon_mount_src, "canonical_health_target"),
                extract_fn(daemon_mount_src, "mount_target_count_from_mountinfo"),
                extract_fn(daemon_mount_src, "reload_refuse_when_unverified"),
            ]
        )

        harness_src = build_harness(template, mountinfo_fns, paths_fns, all_fns)
        # 把抽取函数里的 `log::warn!` 改成 crate 根的 `warn!`（模板用 #[macro_use] 提供）。
        harness_src = harness_src.replace("log::warn!", "warn!")

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "daemon_mount_health")
            compile_proc = subprocess.run(
                [rustc, harness_path, "-O", "-o", bin_path],
                capture_output=True,
                text=True,
            )
            if compile_proc.returncode != 0:
                self.fail(
                    "harness 编译失败:\n" + compile_proc.stdout + compile_proc.stderr
                )
            run_proc = subprocess.run([bin_path], capture_output=True, text=True)
            print(run_proc.stdout)
            if run_proc.stderr:
                print(run_proc.stderr)
            self.assertEqual(
                run_proc.returncode,
                0,
                "harness 边界用例未全部通过:\n" + run_proc.stdout + run_proc.stderr,
            )
            self.assertIn("ALL DAEMON MOUNT HEALTH CASES PASSED", run_proc.stdout)

            # --- 反向验证 1：把 canonical_health_target 里 /mnt 视图的"独立分组"分支改回
            # `paths::normalize` 折叠进主入口（即旧的错误行为），模拟"把 /mnt 当成符号链接别名
            # 折叠"。此时"/mnt 独立视图缺失却被主入口顶替"等用例应从通过变失败，让 harness 退出码
            # 非 0，证明它真在验证该修复。 ---
            mutated_a = re.sub(
                r'if normalized\.starts_with\("/mnt/"\) \{\n        return normalized;\n    \}',
                'if normalized.starts_with("/mnt/") {\n        return paths::normalize(&normalized);\n    }',
                all_fns,
                count=1,
            )
            self.assertNotEqual(mutated_a, all_fns, "反向变异 1 未生效")
            mutated_src_a = build_harness(template, mountinfo_fns, paths_fns, mutated_a)
            mutated_src_a = mutated_src_a.replace("log::warn!", "warn!")
            mutated_path_a = os.path.join(tmp, "harness_a.rs")
            Path(mutated_path_a).write_text(mutated_src_a, encoding="utf-8")
            mutated_bin_a = os.path.join(tmp, "daemon_mount_health_a")
            mut_compile_a = subprocess.run(
                [rustc, mutated_path_a, "-O", "-o", mutated_bin_a],
                capture_output=True,
                text=True,
            )
            if mut_compile_a.returncode != 0:
                self.fail(
                    "反向验证 harness A 编译失败:\n"
                    + mut_compile_a.stdout
                    + mut_compile_a.stderr
                )
            mut_run_a = subprocess.run([mutated_bin_a], capture_output=True, text=True)
            print(mut_run_a.stdout)
            self.assertNotEqual(
                mut_run_a.returncode,
                0,
                "反向验证失败：回退成后端折叠进主入口后边界用例仍全部通过，"
                "说明 harness 没有真正验证 canonical_health_target 修复",
            )

            # --- 反向验证 2：把 reload_refuse_when_unverified 的 `!has_ledger` 改成 `false`，
            # 模拟"无账本也不拒绝"的旧行为。此时"无账本应拒绝"用例应从通过变失败。 ---
            mutated_b = all_fns.replace("!has_ledger", "false")
            self.assertNotEqual(mutated_b, all_fns, "反向变异 2 未生效")
            mutated_src_b = build_harness(template, mountinfo_fns, paths_fns, mutated_b)
            mutated_src_b = mutated_src_b.replace("log::warn!", "warn!")
            mutated_path_b = os.path.join(tmp, "harness_b.rs")
            Path(mutated_path_b).write_text(mutated_src_b, encoding="utf-8")
            mutated_bin_b = os.path.join(tmp, "daemon_mount_health_b")
            mut_compile_b = subprocess.run(
                [rustc, mutated_path_b, "-O", "-o", mutated_bin_b],
                capture_output=True,
                text=True,
            )
            if mut_compile_b.returncode != 0:
                self.fail(
                    "反向验证 harness B 编译失败:\n"
                    + mut_compile_b.stdout
                    + mut_compile_b.stderr
                )
            mut_run_b = subprocess.run([mutated_bin_b], capture_output=True, text=True)
            print(mut_run_b.stdout)
            self.assertNotEqual(
                mut_run_b.returncode,
                0,
                "反向验证失败：回退成无账本也放行后边界用例仍全部通过，"
                "说明 harness 没有真正验证 reload_refuse_when_unverified 修复",
            )


if __name__ == "__main__":
    unittest.main()
