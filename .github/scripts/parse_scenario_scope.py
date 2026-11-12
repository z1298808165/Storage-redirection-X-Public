#!/usr/bin/env python3
"""解析 CI 单场景范围标记，统一确定测试流范围与验证模式。

提交信息里的「单场景 29」「单场景 29,34」（全角逗号也接受）或
``workflow_dispatch`` 的 ``scenarios`` 输入都能收窄范围。解析结果上移到
CI 的 ``prepare`` job 输出，使 dispatch inputs 与提交标记成为唯一来源，
下游 ``test-flow`` 与发布相关 job 统一引用，不再在 job 内就地二次解析。

设计要点：

- 非法范围（非数字、越界、重复、空元素、分隔符后缺数字、标记后无编号）必须显式
  抛错，绝不静默回退到全量。这是「非法范围不得静默跑全量」的硬保证：标记写错时
  整条 CI 直接失败，而不是悄悄把全部场景跑一遍。
- 派发输入或提交标记写 ``all``（不区分大小写）按现有输入约定表示全量，不报错。
- 没有标记也没有 dispatch 输入时，范围保持 ``all``、验证模式 ``full``、
  且仍走完整发布流程（publish_ci=true）。
- 一旦范围受限，发布开关关闭（publish_ci=false），下游发布相关 job 必须跳过，
  既不发布 CI 资产，也不更新 update.json，也不会额外触发一次全量 CI。
- 真实清单（``.github/tests/storage-redirect-scenarios.json``）存在但结构异常时
  必须抛错，绝不用默认上限掩盖解析失败；只在清单文件确实缺失时才回退默认上限。
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

DEFAULT_MAX_SCENARIO = 42
# 标记「单场景」只在后接空白/冒号/数字时才算范围标记，避免把正文里
# 「单场景运行能力」这类 incidental 提及误判为范围标记。
MARKER_KEYWORD = re.compile(r"单场景[：:]?(?=[\s0-9])")
ITEM_SEPARATOR = re.compile(r"[，,]")
# 标记后合法范围的前导数字串；用于在「单场景 3,29 描述」这类标题里
# 只取范围、忽略后续描述文字，同时仍对「29,abc」这类非法延续报错。
SCOPE_RUN = re.compile(r"[0-9]+(?:[,，][0-9]+)*")

# _extract_scope_from_message 的哨兵：标记存在且范围为全量。
_MARKER_ALL = "__ALL__"


class ScenarioScopeError(ValueError):
    """场景范围非法时抛出，禁止静默回退到全量。"""


def _scenario_ids(path: Path | None = None) -> list[int]:
    """读取场景清单里的全部合法 id。

    仅在清单文件确实缺失时回退为空列表（调用方自会用默认上限兜底）；清单存在但
    无法解析或结构异常时抛错，绝不静默用默认值掩盖解析失败。
    """
    if path is None:
        path = Path(__file__).resolve().parents[1] / "tests" / "storage-redirect-scenarios.json"
    if not path.exists():
        return []
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ScenarioScopeError(f"场景清单读取失败：{exc}") from exc
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ScenarioScopeError(f"场景清单 JSON 解析失败：{exc}") from exc
    scenarios = data.get("scenarios") if isinstance(data, dict) else None
    if not isinstance(scenarios, list):
        raise ScenarioScopeError("场景清单结构异常：scenarios 不是列表")
    ids = [item.get("id") for item in scenarios if isinstance(item, dict)]
    ids = [i for i in ids if isinstance(i, int)]
    if not ids:
        raise ScenarioScopeError("场景清单未包含任何合法 id")
    return ids


def max_scenario_from_json(path: Path | None = None) -> int:
    """从场景清单读取合法上限。"""
    ids = _scenario_ids(path)
    return max(ids) if ids else DEFAULT_MAX_SCENARIO


def scenario_total_from_json(path: Path | None = None) -> int:
    """从场景清单读取场景总数，用于全量范围的超时预算。

    不能沿用 ``max_scenario_from_json``：编号可以不连续，全量实际要跑的是条目数，
    拿最大 id 当条数会在跳号时高估预算、在编号重排时低估。
    """
    ids = _scenario_ids(path)
    return len(set(ids)) if ids else DEFAULT_MAX_SCENARIO


def parse_raw_scope(raw: str, *, max_scenario: int, source: str) -> list[int]:
    """把原始范围串解析为去重、升序、范围合法的场景编号列表。

    任何不合法都抛 ``ScenarioScopeError``，调用方据此让 CI 直接失败。
    """
    parts = ITEM_SEPARATOR.split(raw)
    # 严格拒绝空元素（如 "29,,34" 或 "29," 或 ",29"），不允许静默丢弃后只跑部分场景。
    if any(part.strip() == "" for part in parts):
        raise ScenarioScopeError(
            f"{source} 场景范围含空元素（如多余逗号），必须为形如 29 或 29,34 的连续编号"
        )
    items = [part.strip() for part in parts]
    if not items:
        raise ScenarioScopeError(f"{source} 场景范围为空")
    seen: set[int] = set()
    result: list[int] = []
    for item in items:
        if not re.fullmatch(r"[0-9]+", item):
            raise ScenarioScopeError(f"{source} 含非法场景编号 {item!r}，只能为正整数")
        number = int(item)
        if number < 1 or number > max_scenario:
            raise ScenarioScopeError(
                f"{source} 场景编号 {number} 超出合法范围 1-{max_scenario}"
            )
        if number in seen:
            raise ScenarioScopeError(f"{source} 场景编号重复：{number}")
        seen.add(number)
        result.append(number)
    return sorted(result)


def _extract_scope_from_message(message: str) -> str | None:
    """从提交信息中抽取范围字符串。

    返回 ``None`` 表示没有「单场景」标记；返回 ``_MARKER_ALL`` 表示标记为全量；
    其余情况返回清洗后的数字范围串。标记存在但缺编号/格式非法时直接抛错。
    """
    match = MARKER_KEYWORD.search(message)
    if match is None:
        return None
    pos = match.end()
    # 跳过标记与数字之间的空白（"单场景 29"）。
    while pos < len(message) and message[pos] in " \t":
        pos += 1
    if pos >= len(message):
        raise ScenarioScopeError("提交信息含「单场景」标记但缺少场景编号")
    # "all" 按全量处理（如「单场景 all」），与派发输入约定一致。
    if message[pos:].lower().startswith("all") and (
        pos + 3 >= len(message) or not message[pos + 3].isalnum()
    ):
        return _MARKER_ALL
    if not message[pos].isdigit():
        raise ScenarioScopeError(
            "提交信息含「单场景」标记但缺少合法场景编号（正确写法如 单场景 29 或 单场景 29,34）"
        )
    # 逐字符抽取范围数字与分隔符，遇非范围字符即视为描述性后缀并停止；
    # 分隔符后必须紧跟数字，否则视为非法范围（如 29,abc / 29, / 29,,34）。
    scope_chars: list[str] = []
    i = pos
    while i < len(message):
        ch = message[i]
        if ch.isdigit():
            scope_chars.append(ch)
            i += 1
        elif ch in "，,":
            if i + 1 >= len(message) or not message[i + 1].isdigit():
                raise ScenarioScopeError(
                    "场景范围格式非法：分隔符后必须紧跟数字（如 29,34）"
                )
            scope_chars.append(",")
            i += 1
        else:
            break
    # 「单场景 990s」「单场景 30 秒」这类正文里的耗时描述不是范围标记：990 本就
    # 超出合法范围，按标记处理会让整轮 CI 在 prepare 阶段直接 fail（run
    # 40327967419 实证：提交正文写「单场景 990s、6,38 1078s」，prepare 报
    # 「场景编号 990 超出合法范围 1-40」，五个版本的测试全部没跑）。只认紧跟数字
    # 的单位字符，带空格的不算（「单场景 39,40 秒级验证」仍按范围解析）。
    if i < len(message) and message[i] in ("s", "S", "秒"):
        return None
    return "".join(scope_chars)


# 超时预算的硬上限（分钟）。job 级上限与 GitHub job 默认上限 360 分钟（6 小时）
# 对齐；步骤级留 15 分钟差值，保证 job 永远大于步骤，超时判定不会同时落在两级、
# 让 action 来不及执行 `adb emu kill` 收尾。
TIMEOUT_CEILING_MINUTES = 360
STEP_TIMEOUT_CEILING_MINUTES = TIMEOUT_CEILING_MINUTES - 15

# 预算不是拍脑袋的固定系数，而是按下面这条链推导出来的，任何一环变动都必须
# 让本组常量跟着变（test_scenario_scope.py 的 test_budget_chain_matches_test_flow_script
# 会从脚本两侧读常量交叉校验）：
#
#   基础设置预算  →  场景看门狗  →  步骤级  →  job 级
#
# 1. 基础设置预算：run-android-test-flow.sh 的 SETUP_BUDGET_SECONDS，覆盖场景循环
#    之前的一切准备（设备健康检查、模块安装、包服务恢复）。实测跨 5 个成功 run、
#    35 个分片，耗时 118-477 秒，最坏是 A13/A14 的 Magisk 打补丁 + framework 重启
#    路径；取 900 秒（15 分钟）约 1.9 倍余量。
SETUP_BUDGET_MINUTES = 15
# 2. 场景看门狗：run-android-test-flow.sh 里 `900 + 90 * n` 秒，即 15 + 1.5n 分钟。
WATCHDOG_BASE_MINUTES = 15
WATCHDOG_PER_SCENARIO_MINUTES = 1.5
# 3. 步骤级还必须容下 action 自身的开销：模拟器启动 + 脚本启动前的动作 + 收尾 kill。
#    实测（2 个 run × 10 个分片）启动 31-53 秒、启动前动作 9-26 秒，合计 42-78 秒，
#    给 3 分钟是 2.3 倍余量。
#
#    本余量**不覆盖** `emulator-boot-timeout`（A13-A16 1500 秒 / A17 1800 秒）：
#    启动慢到那个量级时 action 会先自己失败（boot timeout），轮不到 step 预算。
#    反过来，只要启动超过这 3 分钟（3 分钟到 boot 上限之间是 A13-A16 的 22 分钟，
#    这条带其实**不窄**），step 就会先于看门狗触发；再叠加「场景阶段挂死」，才会在
#    step 硬杀时丢掉看门狗的模拟器清场与残留快照——判定结论本身不受影响，只是少了
#    现场。
#    这是有意的取舍，两个方向的代价都量化过：
#      - 把余量抬到 30 分钟能覆盖 A17 的 boot 上限，但场景数超过约 190 后 step 先触顶，
#        覆盖又被侵蚀回去，等于只在当前规模下有效；
#      - 而它让「runner 被拖住」时的 step 兜底从 93 分钟推迟到 120 分钟——每一次真实
#        挂起都变慢，换来的只是极罕见组合下的一次清场。
#    所以这里选快兜底：3 分钟对实测启动 42-78 秒是 2.3-4.3 倍余量，够用。
STEP_MARGIN_MINUTES = 3
# 步骤级预算＝基础设置 + 看门狗 + 余量。这样 step 大于 setup + watchdog（前提是
# 模拟器启动不超过上面的余量，那条例外见 3.），
# 看门狗才能在自己的预算耗尽时执行清理，而不是被 step 抢先硬杀。
#
# 代价（有意的取舍，必须知道）：step 里还包含 android-emulator-runner 自己的收尾，
# 而它的 `adb emu kill` 没有超时——历史上曾把 job 挂在 Terminate Emulator 上
# 25-38 分钟（run 40257794067 / 40261786446 实测：action 收尾的 `adb emu kill`
# 没有超时，模拟器异常时无限挂起）。收紧 step 能让这类
# 收尾挂起更快被杀，但会打破上面的嵌套、让看门狗的清理分支跑不到。这里选择保住
# 嵌套：失败路径的清理由脚本自己带超时完成，不再依赖 action 收尾；剩下的暴露面
# 只有「场景全过、action 收尾却挂住」这一种，最长等到 step 上限（全量 93 分钟，
# job 108 分钟内），比原先 45/50 分钟长，但只在极罕见的情况下才会触到。
# 每场景增量沿用看门狗的 1.5 分钟，而不是另设更大系数——实测单场景
# 16-32 秒，看门狗本身已有 2.8 倍以上余量，再叠加只会把上限推高而没有收益。
STEP_BASE_MINUTES = (
    SETUP_BUDGET_MINUTES + WATCHDOG_BASE_MINUTES + STEP_MARGIN_MINUTES
)
STEP_PER_SCENARIO_MINUTES = WATCHDOG_PER_SCENARIO_MINUTES
# 上面这条推导只在 6 小时硬上限还容得下时成立：step 封顶 345 分钟后，看门狗仍
# 按 1.5n 继续增长，到 210 个场景时「基础设置 15 + 看门狗 330」已经追平 345 分钟的
# step 上限，嵌套关系彻底失效（step 会重新抢在看门狗之前触发）。当前常量下
# n ≤ 209 严格成立。
# 余量也会在最后一步收窄：step 在 208 个场景触顶，n ≤ 208 时还留着完整的 3 分钟，
# 到 n = 209 只剩 1.5 分钟。所以**实际应在 200 个场景之前就改矩阵分片**——把场景拆到
# 多个 test-flow job，让每个分片各自跑完整链——而不是继续抬高上限：360 分钟是
# GitHub 的墙，抬不动。
# test_scenario_scope.py 会穷举出这个阈值并断言它为 209（改常量时必须同步）。
# 步骤之外的单次作业开销：checkout、下载运行时、SDK/系统镜像预热、产物上传。
JOB_OVERHEAD_MINUTES = 15


def _test_flow_timeout_minutes(count: int, total: int) -> int:
    """按场景数估算 test-flow 步骤的超时预算（分钟）。

    步骤级超时是「runner 被拖住」的最终兜底：编排脚本的看门狗只覆盖场景脚本
    本体，模拟器异常时它前面的设备健康检查与模块/APK 安装同样可能无限挂起
    （run 40272962460 实证：五个版本全部停在 Run Android test-flow scenarios、
    33 分钟无进展，取消请求也无法立即生效）。

    ``count <= 0`` 表示全量范围，此时必须按清单真实条目数计预算，不能回退固定值：
    场景数会随项目增长，写死会让全量跑在场景加到一定数量后被误杀。
    """
    effective = total if count <= 0 else count
    return min(
        STEP_TIMEOUT_CEILING_MINUTES,
        math.ceil(STEP_BASE_MINUTES + effective * STEP_PER_SCENARIO_MINUTES),
    )


def _test_flow_job_timeout_minutes(step_minutes: int) -> int:
    """job 级预算＝步骤预算 + 作业开销，保证 job 严格大于步骤。

    三层预算必须满足 ``job > step > setup + 脚本看门狗``。此前 job 级写死
    45/50 分钟，比 40 场景的 75 分钟看门狗还小，导致看门狗永不触发、永远被
    job 硬杀在 45/50 分钟，拿不到 ``scenario_exit == 124`` 分支里的模拟器
    清场与残留进程快照。
    """
    return min(TIMEOUT_CEILING_MINUTES, step_minutes + JOB_OVERHEAD_MINUTES)


def _timeout_budget(count: int, total: int) -> dict:
    """成对产出步骤级与 job 级超时预算。"""
    step_minutes = _test_flow_timeout_minutes(count, total)
    return {
        "test_flow_timeout_minutes": step_minutes,
        "test_flow_job_timeout_minutes": _test_flow_job_timeout_minutes(step_minutes),
    }


def _full_result(source: str, total: int) -> dict:
    return {
        "scenarios": "all",
        "scenario_limited": False,
        "validation_mode": "full",
        "publish_ci": True,
        **_timeout_budget(0, total),
        "source": source,
    }


def _limited_result(numbers: list[int], source: str) -> dict:
    return {
        "scenarios": ",".join(str(number) for number in numbers),
        "scenario_limited": True,
        "validation_mode": "scope",
        "publish_ci": False,
        # 受限范围下场景数已知，total 不参与计算（仅全量分支需要）。
        **_timeout_budget(len(numbers), len(numbers)),
        "source": source,
    }


def resolve_scope(
    message: str,
    override: str,
    *,
    max_scenario: int | None = None,
) -> dict:
    """从提交信息与 dispatch 输入统一解析范围。

    返回带 ``scenarios``（``all`` 或逗号列表）、``scenario_limited``（bool）、
    ``validation_mode``（``full``/``scope``）、``publish_ci``（bool）与 ``source`` 的字典。
    """
    if max_scenario is None:
        max_scenario = max_scenario_from_json()
    total = scenario_total_from_json()

    # 派发输入优先；"all" 按现有输入约定表示全量。
    if override and override.strip():
        override_text = override.strip()
        if override_text.lower() == "all":
            return _full_result("dispatch inputs", total)
        numbers = parse_raw_scope(
            override_text, max_scenario=max_scenario, source="dispatch inputs"
        )
        return _limited_result(numbers, "dispatch inputs")

    # 提交信息标记「单场景 N」/「单场景 N,M」。
    extracted = (
        _extract_scope_from_message(message) if message and message.strip() else None
    )
    if extracted is not None:
        if extracted is _MARKER_ALL:
            return _full_result("commit message", total)
        numbers = parse_raw_scope(
            extracted, max_scenario=max_scenario, source="commit message"
        )
        return _limited_result(numbers, "commit message")

    return _full_result("none", total)


def _emit(data: dict, fmt: str) -> None:
    if fmt == "github":
        print(f"scenarios={data['scenarios']}")
        print(f"publish_ci={'true' if data['publish_ci'] else 'false'}")
        # 超时预算成对输出：步骤级由「Run Android test-flow scenarios」引用，
        # job 级由 test-flow 的 timeout-minutes 引用。两者都随场景数自适应，
        # 场景增加时预算自动跟上，不需要再手改 workflow。
        print(f"test_flow_timeout_minutes={data['test_flow_timeout_minutes']}")
        print(
            f"test_flow_job_timeout_minutes={data['test_flow_job_timeout_minutes']}"
        )
    else:
        print(json.dumps(data, ensure_ascii=False, indent=2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--message", default="", help="提交信息（或 PR 标题）")
    parser.add_argument("--override", default="", help="workflow_dispatch scenarios 输入")
    parser.add_argument(
        "--max-scenario",
        type=int,
        default=None,
        help="合法场景上限，默认读取场景清单 JSON",
    )
    parser.add_argument("--format", choices=("json", "github"), default="json")
    args = parser.parse_args()
    try:
        data = resolve_scope(args.message, args.override, max_scenario=args.max_scenario)
    except ScenarioScopeError as exc:
        print(f"场景范围解析失败：{exc}", file=sys.stderr)
        raise SystemExit(1)
    _emit(data, args.format)


if __name__ == "__main__":
    main()
