"""native 夹具用 include! 复用 src 下的模块实现，模块清单必须与真实 crate 一致。

夹具把 `src/platform/paths*.rs` 直接 include 进自己的 `platform` 模块，因此
`src/platform.rs` 里新增或删除一个 `paths_*` 模块时，夹具必须同步声明，否则
夹具能编译但会静默丢掉被测实现（或反过来编译失败）。这个守卫把这条约束固化下来。
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def declared_platform_modules() -> set[str]:
    """`src/platform.rs` 中声明的模块名（含 #[path] 形式）。"""
    source = read("src/platform.rs")
    return set(re.findall(r"^pub mod ([a-z_0-9]+);", source, flags=re.MULTILINE))


def included_fixture_modules() -> set[str]:
    source = read("tests/native-config-fixtures/src/lib.rs")
    return set(re.findall(r"^    pub mod ([a-z_0-9]+) \{", source, flags=re.MULTILINE))


class NativeFixtureModuleParityTest(unittest.TestCase):
    def test_platform_module_declarations_match_fixture_includes(self) -> None:
        # 夹具只需要被测的那几个模块，因此只要求「夹具声明 ⊆ 真实声明」，
        # 反过来允许真实 crate 有夹具不关心的模块。
        declared = declared_platform_modules()
        included = included_fixture_modules()
        self.assertTrue(included, "夹具未 include 任何 platform 模块")
        self.assertTrue(
            included.issubset(declared),
            f"夹具 include 了不存在的 platform 模块：{sorted(included - declared)}",
        )

    def test_paths_split_modules_are_declared_on_both_sides(self) -> None:
        # paths 拆分出的同级模块必须两边都声明，否则调用方与夹具会看到不同的实现集合。
        for name in (
            "paths",
            "paths_alias",
            "paths_roots",
            "paths_rules",
            "paths_safety",
        ):
            self.assertIn(name, declared_platform_modules(), f"src/platform.rs 缺少 {name}")
            self.assertIn(
                name, included_fixture_modules(), f"夹具缺少 {name} 的 include 声明"
            )

    def test_paths_reexports_split_modules(self) -> None:
        # 调用方仍只面对 paths::*，重导出缺失会让现有调用点逐个失效。
        paths = read("src/platform/paths.rs")
        for name in ("paths_alias", "paths_roots", "paths_rules", "paths_safety"):
            self.assertIn(
                f"pub use crate::platform::{name}::*;" if name != "paths_alias" else name,
                paths,
            )


if __name__ == "__main__":
    unittest.main()
