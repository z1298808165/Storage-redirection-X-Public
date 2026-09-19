"""parse_scenario_scope 的真实解析、无标记全量与非法范围硬失败测试。"""

import importlib.util
import json
import sys
import unittest
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))
SCRIPT_PATH = SCRIPTS_DIR / "parse_scenario_scope.py"
SPEC = importlib.util.spec_from_file_location("parse_scenario_scope", SCRIPT_PATH)
SCOPE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = SCOPE
SPEC.loader.exec_module(SCOPE)


class ScenarioScopeParseTest(unittest.TestCase):
    # ---- 无标记：保持全量、完整发布流程 ----

    def test_no_marker_no_override_is_full(self) -> None:
        data = SCOPE.resolve_scope("", "")
        self.assertEqual(data["scenarios"], "all")
        self.assertFalse(data["scenario_limited"])
        self.assertEqual(data["validation_mode"], "full")
        self.assertTrue(data["publish_ci"])
        self.assertEqual(data["source"], "none")

    def test_empty_override_falls_back_to_full(self) -> None:
        # dispatch 传空串等同未指定，不应误判为受限范围。
        data = SCOPE.resolve_scope("修复：稳定保存链路", "")
        self.assertEqual(data["scenarios"], "all")
        self.assertTrue(data["publish_ci"])

    # ---- 真实解析：dispatch inputs / 提交标记 ----

    def test_override_single(self) -> None:
        data = SCOPE.resolve_scope("", "29")
        self.assertEqual(data["scenarios"], "29")
        self.assertTrue(data["scenario_limited"])
        self.assertEqual(data["validation_mode"], "scope")
        self.assertFalse(data["publish_ci"])
        self.assertEqual(data["source"], "dispatch inputs")

    def test_override_pair(self) -> None:
        data = SCOPE.resolve_scope("", "29,34")
        self.assertEqual(data["scenarios"], "29,34")

    def test_override_fullwidth_separator_normalized(self) -> None:
        data = SCOPE.resolve_scope("", "29，34")
        self.assertEqual(data["scenarios"], "29,34")

    def test_override_whitespace_trimmed(self) -> None:
        data = SCOPE.resolve_scope("", " 29 , 34 ")
        self.assertEqual(data["scenarios"], "29,34")

    def test_message_single(self) -> None:
        data = SCOPE.resolve_scope("修复：逐场景回归 单场景 29", "")
        self.assertEqual(data["scenarios"], "29")
        self.assertEqual(data["source"], "commit message")

    def test_message_fullwidth_colon(self) -> None:
        data = SCOPE.resolve_scope("单场景：29,34", "")
        self.assertEqual(data["scenarios"], "29,34")

    def test_message_no_space_attached(self) -> None:
        # 「单场景29」无空格紧跟数字，也应识别为标记。
        data = SCOPE.resolve_scope("修复 单场景29", "")
        self.assertEqual(data["scenarios"], "29")

    def test_message_in_body(self) -> None:
        data = SCOPE.resolve_scope("修复：xxx\n\n单场景 17\n\n细节说明", "")
        self.assertEqual(data["scenarios"], "17")

    def test_override_takes_precedence_over_message(self) -> None:
        data = SCOPE.resolve_scope("单场景 29", "34")
        self.assertEqual(data["scenarios"], "34")
        self.assertEqual(data["source"], "dispatch inputs")

    def test_boundary_scenario_37_is_valid(self) -> None:
        data = SCOPE.resolve_scope("", "37")
        self.assertEqual(data["scenarios"], "37")

    # ---- dispatch / commit 标记 "all" 按全量处理 ----

    def test_override_all_is_full(self) -> None:
        # 派发输入写 "all" 表示全量，沿用既有输入约定，不得报错。
        data = SCOPE.resolve_scope("", "all")
        self.assertEqual(data["scenarios"], "all")
        self.assertTrue(data["publish_ci"])
        self.assertEqual(data["source"], "dispatch inputs")

    def test_message_all_is_full(self) -> None:
        data = SCOPE.resolve_scope("修复 单场景 all", "")
        self.assertEqual(data["scenarios"], "all")
        self.assertTrue(data["publish_ci"])
        self.assertEqual(data["source"], "commit message")

    # ---- 非法范围：必须显式失败，绝不静默跑全量 ----

    def test_non_numeric_override_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("", "abc")

    def test_zero_out_of_range_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("", "0")

    def test_above_max_out_of_range_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("", "38")

    def test_duplicate_number_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("", "29,29")

    def test_custom_max_scenario_enforced(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("", "5", max_scenario=3)
        self.assertEqual(SCOPE.resolve_scope("", "3", max_scenario=3)["scenarios"], "3")

    # ---- 提交标记存在但范围写法非法：必须报错，不静默回退全量 ----

    def test_marker_with_only_text_raises(self) -> None:
        # 「单场景 abc」有标记但无合法编号，必须报错而非回退 all。
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 abc", "")

    def test_marker_with_chinese_text_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 无编号", "")

    def test_marker_with_bad_continuation_raises(self) -> None:
        # 「单场景 29,abc」不得偷截为 29，必须报错。
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 29,abc", "")

    def test_marker_with_empty_element_raises(self) -> None:
        # 「单场景 29,,34」含空元素，不得静默丢弃后只跑 29,34。
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 29,,34", "")

    def test_marker_with_trailing_separator_raises(self) -> None:
        # 「单场景 29,」尾部逗号产生空元素，必须报错。
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 29,", "")

    def test_marker_with_illegal_number_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 99", "")

    def test_marker_with_duplicate_raises(self) -> None:
        with self.assertRaises(SCOPE.ScenarioScopeError):
            SCOPE.resolve_scope("单场景 29,29", "")

    # ---- manifest 实际结构：不得用默认 37 掩盖解析失败 ----

    def test_max_scenario_reads_real_manifest(self) -> None:
        manifest = (
            SCRIPTS_DIR.parent / "tests" / "storage-redirect-scenarios.json"
        )
        expected = max(
            item["id"]
            for item in json.loads(manifest.read_text(encoding="utf-8"))["scenarios"]
        )
        # 必须真正读取清单得到上限，而不是无脑返回 DEFAULT_MAX_SCENARIO。
        self.assertEqual(SCOPE.max_scenario_from_json(), expected)
        self.assertEqual(expected, 37)

    def test_max_scenario_missing_manifest_uses_default(self) -> None:
        self.assertEqual(
            SCOPE.max_scenario_from_json(Path("/nonexistent/scenarios.json")),
            SCOPE.DEFAULT_MAX_SCENARIO,
        )

    def test_max_scenario_corrupt_manifest_raises(self) -> None:
        # 清单存在但结构异常时抛错，不能用 37 掩盖。
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "bad.json"
            bad.write_text('{"scenarios": "not-a-list"}', encoding="utf-8")
            with self.assertRaises(SCOPE.ScenarioScopeError):
                SCOPE.max_scenario_from_json(bad)

    # ---- github 输出格式：只发下游消费项，避免无用项 ----

    def test_github_format_emits_scenarios_and_publish_only(self) -> None:
        import contextlib
        import io

        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            SCOPE._emit(
                {
                    "scenarios": "29",
                    "scenario_limited": True,
                    "validation_mode": "scope",
                    "publish_ci": False,
                },
                "github",
            )
        lines = buf.getvalue().splitlines()
        self.assertIn("scenarios=29", lines)
        self.assertIn("publish_ci=false", lines)
        # 未使用的 scenario_limited / validation_mode 不得进入 CI 输出。
        self.assertNotIn("scenario_limited", "\n".join(lines))
        self.assertNotIn("validation_mode", "\n".join(lines))


if __name__ == "__main__":
    unittest.main()
