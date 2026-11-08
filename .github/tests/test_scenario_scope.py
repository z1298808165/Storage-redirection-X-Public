"""parse_scenario_scope 的真实解析、无标记全量与非法范围硬失败测试。"""

import importlib.util
import json
import math
import re
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

FLOW_SCRIPT = Path(__file__).resolve().parent / "run-android-test-flow.sh"


def read_flow_script() -> str:
    return FLOW_SCRIPT.read_text(encoding="utf-8")


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

    def test_step_timeout_budget_follows_scenario_count(self) -> None:
        # 步骤级超时预算按场景数自适应：窄跑要快速失败，全量留足余量。
        # 预算＝基础设置 15 + 看门狗 15+1.5n + 余量 3（见 parse_scenario_scope.py
        # 的推导链注释）；全量必须按清单真实条目数计，不能回退固定值——写死会在
        # 场景增长后被误杀。
        self.assertEqual(SCOPE.resolve_scope("", "29")["test_flow_timeout_minutes"], 35)
        self.assertEqual(
            SCOPE.resolve_scope("", "29,34")["test_flow_timeout_minutes"], 36
        )
        total = SCOPE.scenario_total_from_json()
        self.assertEqual(
            SCOPE.resolve_scope("", "")["test_flow_timeout_minutes"],
            min(
                SCOPE.STEP_TIMEOUT_CEILING_MINUTES,
                math.ceil(
                    SCOPE.STEP_BASE_MINUTES
                    + total * SCOPE.STEP_PER_SCENARIO_MINUTES
                ),
            ),
        )
        # 预算必须随场景数单调不减，避免场景多的窄跑比场景少的更早被误杀。
        self.assertLess(
            SCOPE.resolve_scope("", "29")["test_flow_timeout_minutes"],
            SCOPE.resolve_scope("", "29,34,39")["test_flow_timeout_minutes"],
        )

    def test_budget_chain_matches_test_flow_script(self) -> None:
        # 步骤级预算由「基础设置预算 + 脚本看门狗 + 余量」推导，两侧常量必须一致：
        # 任何一边单独改动都会让 step 抢先于看门狗触发，清理分支再次失效。
        flow = read_flow_script()
        setup_seconds = int(
            re.search(r'SETUP_BUDGET_SECONDS="\$\{SETUP_BUDGET_SECONDS:-(\d+)\}"', flow).group(1)
        )
        watchdog = re.search(
            r"scenario_watchdog=\$\(\((\d+) \+ scenario_count \* (\d+)\)\)", flow
        )
        self.assertIsNotNone(watchdog, "看门狗算式已变更，请同步本守卫")
        watchdog_base, watchdog_per = (int(watchdog.group(1)), int(watchdog.group(2)))
        self.assertEqual(SCOPE.SETUP_BUDGET_MINUTES * 60, setup_seconds)
        self.assertEqual(SCOPE.WATCHDOG_BASE_MINUTES * 60, watchdog_base)
        self.assertEqual(SCOPE.WATCHDOG_PER_SCENARIO_MINUTES * 60, watchdog_per)
        # 场景数在两处是**独立实现**：解析脚本按 JSON 的 id 去重计数，编排脚本按
        # `grep -c '"id"'` 按行计数。两者不一致会让 step 预算与看门狗用不同的 n，
        # 直接破坏嵌套关系，所以这里按 grep 的语义（逐行匹配）复算一遍。
        scenario_json = (
            Path(__file__).resolve().parents[1] / "tests" / "storage-redirect-scenarios.json"
        ).read_text(encoding="utf-8")
        grep_semantics = sum(1 for line in scenario_json.splitlines() if '"id"' in line)
        self.assertEqual(
            SCOPE.scenario_total_from_json(),
            grep_semantics,
            "解析脚本的去重计数与编排脚本的 grep 计数不一致，请统一场景清单格式",
        )
        # 对任意场景数，步骤级都必须容下「基础设置跑满 + 看门狗跑满」，
        # 且差值还要容得下 action 自身的模拟器启动与脚本启动前动作（实测 42-78 秒）。
        for count in (1, 2, 5, 40, 60):
            step_seconds = SCOPE._test_flow_timeout_minutes(count, count) * 60
            worst_case = setup_seconds + watchdog_base + count * watchdog_per
            self.assertGreater(step_seconds, worst_case, f"{count} 个场景")
            self.assertGreaterEqual(
                step_seconds - worst_case,
                SCOPE.STEP_MARGIN_MINUTES * 60,
                f"{count} 个场景：step 给模拟器启动/收尾的余量不足 {SCOPE.STEP_MARGIN_MINUTES} 分钟",
            )
            job_minutes = SCOPE._test_flow_job_timeout_minutes(step_seconds // 60)
            self.assertGreater(job_minutes * 60, step_seconds, f"{count} 个场景")

    def test_budget_chain_holds_up_to_shard_split_threshold(self) -> None:
        # 嵌套关系（step > 基础设置 + 看门狗）只在 6 小时硬上限还容得下时成立。
        # 超过这个规模就必须改矩阵分片，而不是继续抬预算——360 分钟是 GitHub 的墙。
        # 这里穷举出阈值，防止有人调大系数后阈值悄悄跌到「正常规模也容不下」。
        flow = read_flow_script()
        setup_seconds = int(
            re.search(r'SETUP_BUDGET_SECONDS="\$\{SETUP_BUDGET_SECONDS:-(\d+)\}"', flow).group(1)
        )
        watchdog = re.search(
            r"scenario_watchdog=\$\(\((\d+) \+ scenario_count \* (\d+)\)\)", flow
        )
        watchdog_base, watchdog_per = (int(watchdog.group(1)), int(watchdog.group(2)))
        limit = 0
        for count in range(1, 400):
            step_seconds = SCOPE._test_flow_timeout_minutes(count, count) * 60
            if step_seconds > setup_seconds + watchdog_base + count * watchdog_per:
                limit = count
            else:
                break
        self.assertEqual(
            limit,
            209,
            "嵌套成立的最大场景数应为 209；改动预算常量后请同步 parse_scenario_scope.py 的说明",
        )

    def test_job_timeout_budget_nests_around_step_and_watchdog(self) -> None:
        # 四层预算必须严格嵌套：job > step > (基础设置 + 脚本看门狗) > 单场景。
        # job 若不大于 step，步骤内的模拟器收尾会被 job 级截断；旧值 45/50 甚至
        # 小于 40 场景的 75 分钟看门狗，导致看门狗永不触发、清理分支跑不到。
        total = SCOPE.scenario_total_from_json()
        for scope in ("", "29", "29,34", "29,34,39"):
            data = SCOPE.resolve_scope("", scope)
            step = data["test_flow_timeout_minutes"]
            job = data["test_flow_job_timeout_minutes"]
            self.assertGreater(job, step, scope)
            self.assertLessEqual(job, SCOPE.TIMEOUT_CEILING_MINUTES, scope)
            self.assertLessEqual(step, SCOPE.STEP_TIMEOUT_CEILING_MINUTES, scope)
        # 上限必须真正生效：场景数再涨也不会突破 GitHub job 的 6 小时墙。
        self.assertEqual(
            SCOPE._test_flow_timeout_minutes(10_000, total),
            SCOPE.STEP_TIMEOUT_CEILING_MINUTES,
        )
        self.assertEqual(
            SCOPE._test_flow_job_timeout_minutes(SCOPE.STEP_TIMEOUT_CEILING_MINUTES),
            SCOPE.TIMEOUT_CEILING_MINUTES,
        )

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

    def test_message_duration_suffix_is_not_scope(self) -> None:
        # 正文里的耗时描述（数字紧跟单位）不是范围标记：按标记解析会因 990 超范围
        # 直接 fail，把整轮 CI 卡在 prepare（run 40327967419 实证）。
        for message in (
            "看门狗预算 单场景 990s",
            "预算（单场景 990s、6,38 1078s→1080s）",
            "单场景 30秒",
        ):
            with self.subTest(message=message):
                data = SCOPE.resolve_scope(message, "")
                self.assertEqual(data["scenarios"], "all")
                self.assertFalse(data["scenario_limited"])

    def test_message_scope_followed_by_spaced_unit_still_parses(self) -> None:
        # 单位前有空格时仍是范围标记：不能因为放宽上一条而吞掉真实窄跑意图。
        data = SCOPE.resolve_scope("单场景 39,40 秒级验证", "")
        self.assertEqual(data["scenarios"], "39,40")

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
            SCOPE.resolve_scope("", "41")

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
        self.assertEqual(expected, 40)

    def test_max_scenario_missing_manifest_uses_default(self) -> None:
        self.assertEqual(
            SCOPE.max_scenario_from_json(Path("/nonexistent/scenarios.json")),
            SCOPE.DEFAULT_MAX_SCENARIO,
        )

    def test_max_scenario_corrupt_manifest_raises(self) -> None:
        # 清单存在但结构异常时抛错，不能用默认上限掩盖。
        import tempfile

        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
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
                    "test_flow_timeout_minutes": 17,
                    "test_flow_job_timeout_minutes": 32,
                },
                "github",
            )
        lines = buf.getvalue().splitlines()
        self.assertIn("scenarios=29", lines)
        self.assertIn("publish_ci=false", lines)
        # 超时预算由 test-flow 的 Run 步骤与 job 级 timeout-minutes 直接引用，
        # 两者都必须进 CI 输出。
        self.assertIn("test_flow_timeout_minutes=17", lines)
        self.assertIn("test_flow_job_timeout_minutes=32", lines)
        # 未使用的 scenario_limited / validation_mode 不得进入 CI 输出。
        self.assertNotIn("scenario_limited", "\n".join(lines))
        self.assertNotIn("validation_mode", "\n".join(lines))


if __name__ == "__main__":
    unittest.main()
