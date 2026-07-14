import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPTS_DIR = Path(__file__).parents[1] / "scripts"


def load_script(name: str):
    path = SCRIPTS_DIR / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


CHANGELOG = load_script("generate_changelog")
RELEASE_NOTES = load_script("validate_release_notes")


def commit(subject: str) -> object:
    summary = CHANGELOG.summarize_commit_text(subject)
    return CHANGELOG.CommitInfo(
        sha="abc1234",
        subject=subject,
        body="",
        summary=summary,
        kind=CHANGELOG.classify_commit(subject, summary),
    )


class GenerateChangelogTest(unittest.TestCase):
    def test_previous_release_uses_highest_lower_version(self) -> None:
        self.assertEqual(
            CHANGELOG.select_previous_release_tag(
                ["v1.2.57", "v1.2.55", "v1.2.56", "ci-build-1"],
                "1.2.57",
            ),
            "v1.2.56",
        )

    def test_release_fixes_are_merged_by_final_area(self) -> None:
        sections = CHANGELOG.classify_release_changes(
            ["src/hook/media.rs"],
            [
                commit("修复：调整 MediaProvider 调用方归因"),
                commit("回退：恢复 MediaProvider 原有处理"),
                commit("修复：稳定系统媒体代写路由"),
                commit("构建：消除 warning"),
                commit("维护：移除文件跟踪"),
                commit("修复(CI)：稳定 Release 产物上传"),
            ],
            "",
        )

        self.assertEqual(
            sections["fixed"],
            ["- 修复媒体文件保存、系统代写归因和存储路由中的兼容性问题。"],
        )
        self.assertFalse(any("warning" in item for item in sections["fixed"]))
        self.assertFalse(any("跟踪" in item for item in sections["fixed"]))
        self.assertFalse(any("Release" in item for item in sections["fixed"]))

    def test_release_feature_and_behavior_change_are_separate(self) -> None:
        sections = CHANGELOG.classify_release_changes(
            ["app/src/main/java/SettingsScreen.kt", "src/config.rs"],
            [
                commit("功能(App)：新增自动保存设置开关"),
                commit("功能：支持启动阶段状态同步"),
            ],
            "",
        )

        self.assertTrue(sections["features"])
        self.assertTrue(sections["changes"])

    def test_summary_removes_ci_markers(self) -> None:
        self.assertEqual(
            CHANGELOG.summarize_commit_text("修复：稳定保存链路 仅验证CI"),
            "稳定保存链路",
        )
        self.assertEqual(
            CHANGELOG.summarize_commit_text("修复：稳定保存链路 [skip ci]"),
            "稳定保存链路",
        )
        self.assertEqual(
            CHANGELOG.summarize_commit_text("界面(App)：统一主题设置"),
            "统一主题设置",
        )

    def test_release_notes_validator_accepts_final_result_sections(self) -> None:
        markdown = """# Storage Redirect X v1.2.58

## 模块更新

### 修复了什么问题

- 修复文件保存和路径映射问题。

### 功能变化

- 调整配置默认行为。
"""

        self.assertEqual(RELEASE_NOTES.validation_errors(markdown, "1.2.58"), [])

    def test_release_notes_validator_rejects_process_details(self) -> None:
        markdown = """# Storage Redirect X v1.2.58

## 其它更新

### 其它

- 消除 warnings。
- 移除文件跟踪。
- CI 第三次尝试修复失败后回退。

### 提交列表

- `abc1234` 第一次尝试修复。
"""

        errors = RELEASE_NOTES.validation_errors(markdown, "1.2.58")

        self.assertTrue(any("warning" in error for error in errors))
        self.assertTrue(any("文件跟踪" in error for error in errors))
        self.assertTrue(any("尝试" in error for error in errors))
        self.assertTrue(any("CI" in error for error in errors))
        self.assertTrue(any("提交列表" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
