#!/usr/bin/env python3
"""Validate user-facing notes before publishing a formal release."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


COMPONENT_HEADINGS = {"模块更新", "App 更新", "其它更新"}
DETAIL_HEADINGS = {
    "修复了什么问题",
    "增加了什么功能",
    "功能变化",
    "新功能怎么使用",
    "注意事项",
    "其它",
}
FORBIDDEN_PATTERNS = (
    (re.compile(r"(?i)\bwarnings?\b"), "不得记录 warning 清理过程"),
    (re.compile(r"(?:停止|移除|取消).{0,12}(?:文件)?跟踪"), "不得记录文件跟踪维护过程"),
    (re.compile(r"(?:尝试|重试)"), "不得记录尝试或重试过程"),
    (
        re.compile(r"(?:失败|提交|版本|CI|测试).{0,16}(?:回退|撤销)|(?:回退|撤销).{0,16}(?:提交|修复|尝试|CI|测试|版本)"),
        "不得记录回退或撤销过程",
    ),
    (
        re.compile(r"(?i)(?:\bCI\b|测试流|测试过程|workflow|Actions|scenario|artifact).{0,16}(?:过程|门禁|矩阵|失败|验证|构建|发布)"),
        "不得记录 CI 或测试执行过程",
    ),
    (re.compile(r"(?:\[(?:skip ci|ci skip|no ci)\]|仅验证CI)", re.I), "不得包含 CI 跳过标记"),
    (re.compile(r"^### 提交列表$", re.M), "不得包含提交列表"),
    (re.compile(r"^\*\*完整变更对比\*\*", re.M), "不得包含完整变更对比"),
    (re.compile(r"(?m)^- `?[0-9a-f]{7,40}`?\s"), "不得逐条列出提交"),
)


def validation_errors(markdown: str, version: str = "") -> list[str]:
    errors: list[str] = []
    expected_title = f"# Storage Redirect X v{version}" if version else ""
    first_line = markdown.splitlines()[0].strip() if markdown.splitlines() else ""
    if expected_title and first_line != expected_title:
        errors.append(f"首行必须是 {expected_title}")

    for pattern, message in FORBIDDEN_PATTERNS:
        if pattern.search(markdown):
            errors.append(message)

    headings = re.findall(r"^(#{2,3})\s+(.+?)\s*$", markdown, flags=re.M)
    components = [title for level, title in headings if level == "##"]
    if not components:
        errors.append("至少需要一个组件分区")
    for title in components:
        if title not in COMPONENT_HEADINGS:
            errors.append(f"不支持的组件分区：{title}")
    for level, title in headings:
        if level == "###" and title not in DETAIL_HEADINGS:
            errors.append(f"不支持的内容分区：{title}")

    section_pattern = re.compile(
        r"^###\s+(.+?)\s*$\n(.*?)(?=^#{2,3}\s+|\Z)",
        flags=re.M | re.S,
    )
    for match in section_pattern.finditer(markdown):
        if not re.search(r"(?m)^- \S", match.group(2)):
            errors.append(f"内容分区没有条目：{match.group(1)}")

    return list(dict.fromkeys(errors))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--file", required=True)
    parser.add_argument("--version", default="")
    args = parser.parse_args()

    path = Path(args.file)
    markdown = path.read_text(encoding="utf-8")
    errors = validation_errors(markdown, args.version)
    if errors:
        details = "\n".join(f"- {error}" for error in errors)
        raise SystemExit(f"Formal release notes validation failed:\n{details}")


if __name__ == "__main__":
    main()
