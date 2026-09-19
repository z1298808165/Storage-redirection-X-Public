import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / ".github" / "tests" / "harness" / "mount_identity_syntax.rs"

SRC = ROOT / "src"


def read(path: str) -> str:
    return (SRC / path).read_text(encoding="utf-8")


def extract_fn(source: str, fn_name: str) -> str:
    """用大括号计数从源码中抽出整个 fn（含签名与函数体），不依赖固定结尾标记。

    与 test_caller_attribution_boundaries.py 同款抽取逻辑：即使目标函数之后增删代码，
    也能稳定取到完整实现，避免按函数名下标截断导致抄错。
    """
    marker = f"fn {fn_name}"
    start = source.index(marker)
    # 抽取时一并带上可见性修饰符（如 `pub `），否则注入到模板里会变成私有项无法跨模块调用。
    if start >= 4 and source[start - 4 : start] == "pub ":
        start -= 4
    # 函数名后可能是泛型 `<...>` 或直接的 `(`，统一找到参数左括号再定位函数体。
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


class MountIdentitySyntaxTest(unittest.TestCase):
    def test_extracted_functions_execute(self) -> None:
        # 从真实源码抽取函数注入模板，rustc 独立编译运行边界真值表。
        # 项目禁止在 src/ 内新增内联 Rust 测试，因此 harness 放在 .github/tests/harness/。
        # 模板只含真实模块桩与类型，绝不抄写匹配实现——真正执行的是抽取到的源码。
        if not HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("rustc 不可用：未编译执行 stub harness，仅静态边界守卫生效")

        template = HARNESS.read_text(encoding="utf-8")

        # 真实 mountinfo 解析（parse_entry / unescape_field）与真实语法归一化（normalize_syntax）。
        mountinfo_src = read("platform/mountinfo.rs")
        paths_src = read("platform/paths.rs")
        mount_ledger_src = read("mount_ledger.rs")
        mount_identity_src = read("mount_identity.rs")
        daemon_mount_src = read("daemon_mount.rs")
        # 版本常量必须来自真实源码，不能由桩固定为新版本掩盖生产代码回退。
        schema = re.search(r"const IDENTITY_SCHEMA_VERSION: u32 = \d+;", mount_ledger_src)
        self.assertIsNotNone(schema)
        template = re.sub(r"const IDENTITY_SCHEMA_VERSION: u32 = \d+;", schema.group(0), template)

        mountinfo_fns = extract_fn(mountinfo_src, "parse_entry") + "\n" + extract_fn(
            mountinfo_src, "unescape_field"
        )
        paths_fns = extract_fn(paths_src, "normalize_syntax") + "\n" + extract_fn(
            paths_src, "collapse_redundant_slashes"
        )
        all_fns = "\n".join(
            [
                extract_fn(mount_ledger_src, "live_mounts_at"),
                extract_fn(mount_ledger_src, "topmost_live_mount"),
                extract_fn(mount_ledger_src, "capture_mount_identity"),
                extract_fn(mount_ledger_src, "encode"),
                extract_fn(mount_ledger_src, "decode"),
                extract_fn(mount_ledger_src, "field"),
                extract_fn(mount_identity_src, "classify_mount"),
                extract_fn(daemon_mount_src, "clear_previous_mounts"),
            ]
        )

        harness_src = build_harness(template, mountinfo_fns, paths_fns, all_fns)

        # 把抽取函数里的 `log::warn!` / `log::info!` 改成 crate 根的裸宏（模板用 #[macro_use] 提供）。
        harness_src = harness_src.replace("log::warn!", "warn!").replace("log::info!", "info!")

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "mount_identity_syntax")
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
            self.assertIn("ALL MOUNT IDENTITY SYNTAX CASES PASSED", run_proc.stdout)

            # --- 反向验证：内存里把别名保留的 normalize_syntax 换成别名折叠桩 __alias_fold ---
            # 若原函数真的保留存储别名（修复点），换成别名折叠后"仅后端挂载时主入口不可命中"、
            # "混合别名各取各自 ID" 等用例会从通过变失败，从而让 harness 退出码非 0，
            # 证明 harness 在验证真实函数而非空壳。
            if "paths::normalize_syntax(" not in all_fns:
                raise AssertionError("抽取的函数不含 normalize_syntax，无法做反向验证")
            mutated_all = all_fns.replace("paths::normalize_syntax(", "__alias_fold(")
            self.assertNotEqual(mutated_all, all_fns)
            mutated_src = build_harness(template, mountinfo_fns, paths_fns, mutated_all)
            mutated_src = mutated_src.replace("log::warn!", "warn!").replace(
                "log::info!", "info!"
            )
            mutated_path = os.path.join(tmp, "harness_mutated.rs")
            Path(mutated_path).write_text(mutated_src, encoding="utf-8")
            mutated_bin = os.path.join(tmp, "mount_identity_syntax_mutated")
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
                "反向验证失败：回退成别名折叠后边界用例仍全部通过，"
                "说明 harness 没有真正验证原函数",
            )


if __name__ == "__main__":
    unittest.main()
