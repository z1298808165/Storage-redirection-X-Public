#!/usr/bin/env python3
"""解析 CI 单场景范围标记，统一确定测试流范围与验证模式。

提交信息里的「单场景 29」「单场景 29,34」（全角逗号也接受）或
``workflow_dispatch`` 的 ``scenarios`` 输入都能收窄范围。解析结果上移到
CI 的 ``prepare`` job 输出，使 dispatch inputs 与提交标记成为唯一来源，
下游 ``test-flow`` 与发布相关 job 统一引用，不再在 job 内就地二次解析。

设计要点：

- 非法范围（非数字、越界、重复、空元素、分隔符后缺数字、标记后无编号）必须显式
  抛错，绝不静默回退到全量。这是「非法范围不得静默跑全量」的硬保证：标记写错时
  整条 CI 直接失败，而不是悄悄把 37 个场景全跑一遍。
- 派发输入或提交标记写 ``all``（不区分大小写）按现有输入约定表示全量，不报错。
- 没有标记也没有 dispatch 输入时，范围保持 ``all``、验证模式 ``full``、
  且仍走完整发布流程（publish_ci=true）。
- 一旦范围受限，发布开关关闭（publish_ci=false），下游发布相关 job 必须跳过，
  既不发布 CI 资产，也不更新 update.json，也不会额外触发一次全量 CI。
- 真实清单（``.github/tests/storage-redirect-scenarios.json``）存在但结构异常时
  必须抛错，绝不用默认 37 掩盖解析失败；只在清单文件确实缺失时才回退默认上限。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

DEFAULT_MAX_SCENARIO = 37
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


def max_scenario_from_json(path: Path | None = None) -> int:
    """从场景清单读取合法上限。

    仅在清单文件确实缺失时回退默认上限；清单存在但无法解析或结构异常时抛错，
    绝不静默用默认 37 掩盖解析失败。
    """
    if path is None:
        path = Path(__file__).resolve().parents[1] / "tests" / "storage-redirect-scenarios.json"
    if not path.exists():
        return DEFAULT_MAX_SCENARIO
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
    return max(ids)


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
    return "".join(scope_chars)


def _full_result(source: str) -> dict:
    return {
        "scenarios": "all",
        "scenario_limited": False,
        "validation_mode": "full",
        "publish_ci": True,
        "source": source,
    }


def _limited_result(numbers: list[int], source: str) -> dict:
    return {
        "scenarios": ",".join(str(number) for number in numbers),
        "scenario_limited": True,
        "validation_mode": "scope",
        "publish_ci": False,
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

    # 派发输入优先；"all" 按现有输入约定表示全量。
    if override and override.strip():
        override_text = override.strip()
        if override_text.lower() == "all":
            return _full_result("dispatch inputs")
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
            return _full_result("commit message")
        numbers = parse_raw_scope(
            extracted, max_scenario=max_scenario, source="commit message"
        )
        return _limited_result(numbers, "commit message")

    return _full_result("none")


def _emit(data: dict, fmt: str) -> None:
    if fmt == "github":
        # 只输出下游实际消费的两项，避免无用项（scenario_limited/validation_mode
        # 未被任何 job 引用，不进入 CI 输出）。
        print(f"scenarios={data['scenarios']}")
        print(f"publish_ci={'true' if data['publish_ci'] else 'false'}")
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
