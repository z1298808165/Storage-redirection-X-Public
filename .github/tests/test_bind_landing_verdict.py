import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HARNESS = ROOT / ".github" / "tests" / "harness" / "bind_landing_verdict.rs"

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


def extract_struct_with_derives(source: str, struct_name: str) -> str:
    """抽出结构体定义（含其上紧邻的 derive 属性行）。"""
    marker = f"pub(super) struct {struct_name}"
    start = source.index(marker)
    # 往上吞掉紧邻的 #[derive(...)] 行，否则默认的 Eq/PartialEq 比较会缺实现。
    lines = source[:start].rstrip("\n").split("\n")
    keep = start
    idx = len(lines) - 1
    while idx >= 0 and lines[idx].strip().startswith("#["):
        keep = sum(len(line) + 1 for line in lines[:idx])
        idx -= 1
    end = source.index("}", start)
    # 结构体定义以右花括号加换行结束；这里取到第一个 `}`，字段里没有内联块。
    return source[keep : end + 1]


def extract_enum(source: str, enum_name: str) -> str:
    """抽出 `pub(super) enum X { ... }` 定义（含紧邻的 derive 行）。"""
    marker = f"pub(super) enum {enum_name}"
    start = source.index(marker)
    lines = source[:start].rstrip("\n").split("\n")
    keep = start
    idx = len(lines) - 1
    while idx >= 0 and lines[idx].strip().startswith("#["):
        keep = sum(len(line) + 1 for line in lines[:idx])
        idx -= 1
    brace_start = source.index("{", start)
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
    return source[keep:idx]


def strip_visibility(source: str) -> str:
    """去掉 `pub(super)`/`pub(crate)` 可见性：harness 把抽出的项放在 crate 根，
    保留可见性会得到 "too many leading super keywords"。"""
    return source.replace("pub(super) ", "").replace("pub(crate) ", "")


class BindLandingVerdictTest(unittest.TestCase):
    """锁定 FUSE 承载绑定的落地校验判据。

    Android 13 场景 29 的形态是 `alias diag overlay bind ... mounted=true` 与应用访问
    ENOENT/ENOTDIR 同秒并存：`mount(MS_BIND)` 返回 0，但应用行走落不进那层。旧实现直接
    `record_mounted_target` + `return true`，把「syscall 成功」当成「落地有效」写进状态文件与
    挂载身份账本。本用例执行真实函数，验证判据只在「目标确定不是目录」时判失败，且
    inode 前后未变化时必须放行，不误杀正常挂载。
    """

    def test_extracted_functions_execute(self) -> None:
        if not HARNESS.exists():
            self.skipTest(f"harness 模板缺失: {HARNESS}")
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("rustc 不可用：未编译执行 stub harness，仅静态边界守卫生效")

        template = HARNESS.read_text(encoding="utf-8")
        core_src = read("mount/planner.rs")

        injected = strip_visibility(
            "\n".join(
                [
                    extract_enum(core_src, "BindLandingVerdict"),
                    extract_struct_with_derives(core_src, "BindLandingSample"),
                    extract_fn(core_src, "fuse_backed_bind_landing_verdict"),
                ]
            )
        )
        harness_src = template.replace("// __INJECT_VERDICT__", injected)
        self.assertNotIn("// __INJECT_VERDICT__", harness_src, "harness 占位符未替换")

        with tempfile.TemporaryDirectory() as tmp:
            harness_path = os.path.join(tmp, "harness.rs")
            Path(harness_path).write_text(harness_src, encoding="utf-8")
            bin_path = os.path.join(tmp, "bind_landing_verdict")
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
            self.assertIn("ALL BIND LANDING VERDICT CASES PASSED", run_proc.stdout)

            # --- 反向验证一：把「非目录」一律判为通过。
            # `not_directory_*` 三类反例应从失败转为通过，证明 harness 真在验证非目录判定。 ---
            mutated = injected.replace(
                "BindLandingVerdict::NotDirectory\n            } else {\n                BindLandingVerdict::Inconclusive\n            }",
                "BindLandingVerdict::Accepted\n            } else {\n                BindLandingVerdict::Accepted\n            }",
            )
            if mutated == injected:
                # 换一种更宽松的锚点：整段 Some(false) 分支替换为 Accepted。
                start = injected.index("Some(false) => {")
                end = injected.index("BindLandingVerdict::NotDirectory") + len(
                    "BindLandingVerdict::NotDirectory"
                )
                mutated = injected[:start] + "Some(false) => BindLandingVerdict::Accepted" + injected[end:]
            self.assertNotEqual(mutated, injected, "反向变异未生效（非目录判定）")
            self._assert_mutation_fails(tmp, template, mutated, "非目录判定")

            # --- 反向验证二：把「inode 未变化」的误报防护去掉。
            # `not_directory_unchanged` 应从 inconclusive 转为 not_directory 而被判失败，
            # 证明 harness 确实锁定了「未变化时不得误杀」这条防护。 ---
            mutated2 = injected.replace(
                "if unchanged {\n                BindLandingVerdict::Inconclusive\n            } else {\n                BindLandingVerdict::NotDirectory\n            }",
                "BindLandingVerdict::NotDirectory",
            )
            self.assertNotEqual(mutated2, injected, "反向变异未生效（误报防护）")
            self._assert_mutation_fails(tmp, template, mutated2, "误报防护")

    def _assert_mutation_fails(
        self, tmp: str, template: str, injected: str, label: str
    ) -> None:
        harness_src = template.replace("// __INJECT_VERDICT__", injected)
        harness_path = os.path.join(tmp, "harness_mutated.rs")
        Path(harness_path).write_text(harness_src, encoding="utf-8")
        bin_path = os.path.join(tmp, "mutated_bin")
        compile_proc = subprocess.run(
            ["rustc", harness_path, "-O", "-o", bin_path],
            capture_output=True,
            text=True,
        )
        if compile_proc.returncode != 0:
            # 变异后无法编译同样说明 harness 绑定着真实实现。
            return
        run_proc = subprocess.run([bin_path], capture_output=True, text=True)
        self.assertNotEqual(
            run_proc.returncode,
            0,
            f"反向验证失败：{label} 变异后 harness 仍然全通过，说明该判据未被真实锁定\n"
            + run_proc.stdout,
        )


if __name__ == "__main__":
    unittest.main()
