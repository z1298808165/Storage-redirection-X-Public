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


class GenerateChangelogTest(unittest.TestCase):
    def test_summary_input_removes_machine_review_trailers(self) -> None:
        text = CHANGELOG.summary_input(
            "修复：隔离 pending 状态",
            "变更：只处理被重定向的调用方。\n"
            "AI-Review-Summary: 机器审核内容\n"
            "验证：通过构建。",
        )
        self.assertIn("只处理被重定向的调用方", text)
        self.assertIn("通过构建", text)
        self.assertNotIn("机器审核内容", text)

    def test_commit_body_fields_are_used_as_user_facing_summary(self) -> None:
        body = """变更：仅对启用重定向的调用方登记 pending 目标。
范围：MediaStore 重定向写入；系统调用保持原始提交语义。
限制：不改变查询路径过滤。
验证：已完成构建检查。"""
        fields = CHANGELOG.parse_body_fields(body)
        commit = CHANGELOG.CommitInfo(
            sha="abc123456789",
            subject="修复(MediaProvider)：隔离 pending 状态",
            body=body,
            fields=fields,
        )
        self.assertEqual(fields["变更"], "仅对启用重定向的调用方登记 pending 目标。")
        self.assertIn("系统调用保持原始提交语义", CHANGELOG.user_summary(commit))
        self.assertIn("不改变查询路径过滤", CHANGELOG.user_summary(commit))
        self.assertNotIn("已完成构建检查", CHANGELOG.user_summary(commit))

    def test_collect_analysis_binds_entries_to_commit_files(self) -> None:
        commit = CHANGELOG.CommitInfo(
            sha="abc123456789",
            subject="修复：隔离 pending 状态",
            body="变更：仅对重定向调用方登记目标。",
            fields={"变更": "仅对重定向调用方登记目标。"},
        )
        original = CHANGELOG.commit_changed_files
        CHANGELOG.commit_changed_files = lambda _commit: ["src/daemon.rs"]
        try:
            analysis = CHANGELOG.collect_analysis("release", ["src/daemon.rs"], [commit])
        finally:
            CHANGELOG.commit_changed_files = original
        item = analysis["module"]["fixed"][0]
        self.assertEqual(item["text"], "仅对重定向调用方登记目标")
        self.assertEqual(item["files"], ["src/daemon.rs"])

    def test_process_only_commit_is_omitted_from_release(self) -> None:
        commit = CHANGELOG.CommitInfo(
            sha="abc123456789",
            subject="CI：统一 workflow 条件",
            body="变更：只调整测试工作流条件。",
            fields={"变更": "只调整测试工作流条件。"},
        )
        original = CHANGELOG.commit_changed_files
        CHANGELOG.commit_changed_files = lambda _commit: [".github/workflows/ci.yml"]
        try:
            analysis = CHANGELOG.collect_analysis("release", [".github/workflows/ci.yml"], [commit])
        finally:
            CHANGELOG.commit_changed_files = original
        self.assertFalse(any(analysis[component][section] for component in analysis for section in CHANGELOG.SECTIONS))

    def test_workflows_consume_commit_descriptions_without_model_calls(self) -> None:
        ci = (Path(__file__).parents[2] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        release = (Path(__file__).parents[2] / ".github/workflows/release.yml").read_text(encoding="utf-8")
        for workflow in (ci, release):
            self.assertIn("generate_changelog.py", workflow)
            self.assertNotIn("actions/ai-inference", workflow)
            self.assertNotIn("COPILOT_PAT", workflow)
            self.assertNotIn("prompt-output", workflow)
            self.assertNotIn("analysis-file", workflow)
        self.assertNotIn('cp "$RELEASE_NOTES" build/changelog.md', release)

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
