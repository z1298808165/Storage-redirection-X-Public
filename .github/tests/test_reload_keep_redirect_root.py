import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / ".github" / "tests" / "harness" / "reload_keep_redirect_root.rs"

SRC = ROOT / "src"


def read(path: str) -> str:
    return (SRC / path).read_text(encoding="utf-8")


def extract_fn(source: str, fn_name: str) -> str:
    """用大括号计数从源码中抽出整个 fn（含签名与函数体），不依赖固定结尾标记。"""
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


def build_harness(template: str, paths_fns: str, all_fns: str) -> str:
    out = template
    for placeholder, body in (
        ("// __INJECT_PATHS__", paths_fns),
        ("// __INJECT_ALL__", all_fns),
    ):
        if placeholder not in out:
            raise AssertionError(f"harness 模板缺少占位符: {placeholder}")
        out = out.replace(placeholder, body)
    return out


class ReloadKeepRedirectRootTest(unittest.TestCase):
    """锁定热重载的"重定向根保留"判据。

    场景 29（热重载）失败形态是应用写入沙箱内才存在的路径得到 ENOENT：存储视图根的 bind
    被摘掉再重挂时换了 dentry，应用行走落回真实公共存储。修复让重定向根在配置未换沙箱、
    后端未换 FUSE 时保留既有绑定，本用例执行真实函数验证这条判据不放宽。
    """

    def test_extracted_functions_execute(self) -> None:
        if not HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("rustc 不可用：未编译执行 stub harness，仅静态边界守卫生效")

        template = HARNESS.read_text(encoding="utf-8")
        paths_src = read("platform/paths.rs")
        daemon_mount_src = read("daemon_mount.rs")

        paths_fns = "\n".join(
            [
                extract_fn(paths_src, "starts_with_ignore_case"),
                extract_fn(paths_src, "is_same_or_child"),
            ]
        )
        all_fns = "\n".join(
            [
                extract_fn(daemon_mount_src, "should_keep_reload_redirect_root"),
                extract_fn(daemon_mount_src, "sandbox_root_matches_redirect_target"),
            ]
        )

        harness_src = build_harness(template, paths_fns, all_fns)

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "reload_keep_redirect_root")
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
            self.assertIn("ALL RELOAD KEEP REDIRECT ROOT CASES PASSED", run_proc.stdout)

            # --- 反向验证一：去掉"沙箱根必须与本次重定向目标一致"的守卫。
            # 改沙箱目标、自定义目标、平台应用专属目录三类反例应从失败转为通过，
            # 证明 harness 真在验证沙箱目标一致性而非空壳。 ---
            mutated_all = all_fns.replace(
                "sandbox_root_matches_redirect_target(root, &request.redirect_target)",
                "true",
                1,
            )
            self.assertNotEqual(mutated_all, all_fns, "反向变异未生效（沙箱目标守卫）")
            self._assert_mutation_fails(tmp, template, paths_fns, mutated_all, "沙箱目标守卫")

            # --- 反向验证二：去掉"必须是本模块层"的归属守卫。
            # 平台自己的挂载也会被保留，反例 reload_foreign_layer_not_kept 必须转为失败。 ---
            ownership_mutated = all_fns.replace(
                "if !mount_identity::is_module_redirect_mount(source, root, target, &request.package_name) {",
                "if false {",
                1,
            )
            self.assertNotEqual(ownership_mutated, all_fns, "反向变异未生效（归属守卫）")
            self._assert_mutation_fails(
                tmp, template, paths_fns, ownership_mutated, "归属守卫"
            )

            # --- 反向验证三：去掉"仅映射模式不走重定向根保留"的守卫。
            # 从默认重定向切到仅映射模式时，原反例 reload_mapping_mode_only_not_kept
            # 必须由 false 变为 true（错误保留旧重定向根），证明该守卫确为必需，
            # 而非空壳判据——归属与沙箱根一致本身不足以保留旧根。 ---
            mapping_mutated = all_fns.replace(
                "if request.is_mapping_mode_only {",
                "if false {",
                1,
            )
            self.assertNotEqual(
                mapping_mutated, all_fns, "反向变异未生效（仅映射模式守卫）"
            )
            self._assert_mutation_fails(
                tmp, template, paths_fns, mapping_mutated, "仅映射模式守卫"
            )

    def _assert_mutation_fails(
        self,
        tmp: str,
        template: str,
        paths_fns: str,
        mutated_all: str,
        label: str,
    ) -> None:
        mutated_src = build_harness(template, paths_fns, mutated_all)
        mutated_path = os.path.join(tmp, "harness_mutated.rs")
        Path(mutated_path).write_text(mutated_src, encoding="utf-8")
        mutated_bin = os.path.join(tmp, "reload_keep_redirect_root_mutated")
        mut_compile = subprocess.run(
            [shutil.which("rustc"), mutated_path, "-O", "-o", mutated_bin],
            capture_output=True,
            text=True,
        )
        if mut_compile.returncode != 0:
            self.fail(f"反向验证 harness 编译失败（{label}）:\n" + mut_compile.stderr)
        mut_run = subprocess.run([mutated_bin], capture_output=True, text=True)
        print(mut_run.stdout)
        self.assertNotEqual(
            mut_run.returncode,
            0,
            f"反向验证失败（{label}）：去掉守卫后边界用例仍全部通过，"
            "说明 harness 没有真正验证该判据",
        )


if __name__ == "__main__":
    unittest.main()
