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


def read_paths_module() -> str:
    """读取 `src/platform/paths*.rs` 的全部内容。

    paths 已按职责拆出 paths_alias / paths_roots / paths_rules / paths_safety 同级
    模块，守卫测试需要看到合并后的内容，否则抽取函数时会因文件边界而失败。
    """
    platform_dir = ROOT / "src" / "platform"
    files = sorted(platform_dir.glob("paths*.rs"))
    return "\n".join(path.read_text(encoding="utf-8") for path in files)


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


def extract_const(source: str, name: str) -> str:
    match = re.search(rf"const {name}: &str = \"[^\"]*\";", source)
    if match is None:
        raise AssertionError(f"未找到常量 {name}")
    return match.group(0)


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
        paths_src = read_paths_module()
        daemon_mount_src = read("daemon_mount.rs")

        mountinfo_fns = extract_fn(mountinfo_src, "parse_entry") + "\n" + extract_fn(
            mountinfo_src, "unescape_field"
        )
        # 别名清单必须来自真实源码：桩一份手写清单会掩盖生产别名表被改坏的情况。
        paths_fns = "\n".join(
            [
                extract_const(paths_src, "STORAGE_EMULATED_PREFIX"),
                extract_const(paths_src, "DATA_MEDIA_PREFIX"),
                extract_fn(paths_src, "normalize_syntax"),
                extract_fn(paths_src, "collapse_redundant_slashes"),
                extract_fn(paths_src, "storage_user_root_for_user"),
                extract_fn(paths_src, "data_media_user_root_for_user"),
                extract_fn(paths_src, "storage_alias_roots_for_user"),
            ]
        )
        all_fns = "\n".join(
            [
                extract_fn(daemon_mount_src, "mount_targets_present_with"),
                extract_fn(daemon_mount_src, "canonical_health_target"),
                extract_fn(daemon_mount_src, "mount_target_count_from_mountinfo"),
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
                self.fail("harness 编译失败:\n" + compile_proc.stdout + compile_proc.stderr)
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

            # --- 反向验证：把别名折叠循环置空，模拟"每个别名各自成组"的错误口径。
            # 此时"仅后端别名在场""仅 /mnt 视图记录"等正例应从通过变失败，让 harness 退出码
            # 非 0，证明它真在验证别名分组语义。 ---
            mutated_all = all_fns.replace(
                "for alias_root in alias_roots {",
                "for alias_root in alias_roots.iter().take(0) {",
                1,
            )
            self.assertNotEqual(mutated_all, all_fns, "反向变异未生效")
            mutated_src = build_harness(template, mountinfo_fns, paths_fns, mutated_all)
            mutated_src = mutated_src.replace("log::warn!", "warn!")
            mutated_path = os.path.join(tmp, "harness_mutated.rs")
            Path(mutated_path).write_text(mutated_src, encoding="utf-8")
            mutated_bin = os.path.join(tmp, "daemon_mount_health_mutated")
            mut_compile = subprocess.run(
                [rustc, mutated_path, "-O", "-o", mutated_bin],
                capture_output=True,
                text=True,
            )
            if mut_compile.returncode != 0:
                self.fail(
                    "反向验证 harness 编译失败:\n" + mut_compile.stdout + mut_compile.stderr
                )
            mut_run = subprocess.run([mutated_bin], capture_output=True, text=True)
            print(mut_run.stdout)
            self.assertNotEqual(
                mut_run.returncode,
                0,
                "反向验证失败：禁用别名折叠后边界用例仍全部通过，"
                "说明 harness 没有真正验证别名分组健康判据",
            )


if __name__ == "__main__":
    unittest.main()
