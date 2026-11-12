#!/usr/bin/env python3
"""根据提交时由 AI Agent 生成的用户说明渲染中文 CI/Release 更新日志。"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

MAX_SECTION_ITEMS = 20
# CI 构建日志面向测试用户，只保留可感知的结论：每节条目上限比正式发布更紧，
# 并额外加一道全局上限，避免「模块/App/其它」三块各堆二十条把页面撑爆
# （2.2.0-ci.1 实测：112 条、5491 字符，全是「适用范围/限制」这类内部元数据）。
MAX_CI_SECTION_ITEMS = 12
MAX_CI_TOTAL_ITEMS = 24
COMPONENTS = ("module", "app", "other")
COMPONENT_PRIORITY = {"module": 0, "app": 1, "other": 2}
SECTIONS = ("fixed", "features", "changes", "usage", "notes")
AUTO_MANIFEST_PREFIXES = (
    "CI：更新更新清单",
    "发布：更新更新清单",
)
MACHINE_TRAILER_PATTERN = re.compile(
    r"^(?:AI-Review-(?:Agent|Tree|Report|Summary):|Signed-off-by:|Co-authored-by:)",
    flags=re.I,
)
BODY_FIELD_PATTERN = re.compile(r"^(变更|用户影响|范围|限制|验证)[：:]\s*(.*)$")
# 致谢类字段：提交正文里记录外部思路来源与贡献者，必须同样出现在 CI 与发布日志里。
# 它们不是普通正文，不能并进上一个字段（否则会被「验证」吞掉，日志里永远看不到）。
CREDIT_FIELD_PATTERN = re.compile(r"^(致谢|思路来源|贡献者|上游来源|上游作者)[：:]\s*(.*)$")
# 说明性元数据字段：既不进更新日志，也不能并进上一个更新日志字段。
# `文档同交：` 用来声明「本笔的配套文档无法独立成笔」的原因（见 AGENTS.md 提交信息规范），
# 与 validate-commit.ps1 的同名判据对应。
STANDALONE_META_FIELD_PATTERN = re.compile(r"^文档同交[：:]")
CREDIT_PEOPLE_LABELS = ("贡献者", "上游作者")
CREDIT_SOURCE_LABELS = ("思路来源", "上游来源")
# 致谢里的链接必须显式写成 Markdown 链接：GitHub 的自动链接会把紧跟在 URL 之后的
# 中文标点（尤其全角括号）并进链接，`感谢 People-11（https://github.com/People-11），来源：上游`
# 点开就成了 `…/People-11），来源：上游` 这样的错误地址（2.2.1 更新日志实测）。
CREDIT_URL_PATTERN = re.compile(r"https?://[^\s，、；：（）()\[\]<>。]+")
CREDIT_LINKED_NAME_PATTERN = re.compile(
    r"([^\s，、；：（）()\[\]<>]+)\s*[（(]\s*(https?://[^\s，、；：（）()\[\]<>。]+)\s*[）)]"
)
# 依赖锁定常写成 `仓库.git@<sha>`：这种地址本身点不开（GitHub 上 404），
# 改写成提交页地址才能落实「链接可用」的要求。
CREDIT_GIT_PIN_PATTERN = re.compile(r"^(https?://[^/\s]+/[^/\s]+/[^/\s]+?)(?:\.git)?@([0-9a-f]{7,40})$")
CREDITS_HEADING = "### 致谢"
MAX_CREDITS = 10
# CI 构建面向测试用户，致谢只保留最近的几条，避免把日志尾部拉长。
MAX_CI_CREDITS = 5


@dataclass(frozen=True)
class CommitInfo:
    sha: str
    subject: str
    body: str
    fields: dict[str, str]


def run_git(args: list[str], allow_fail: bool = False) -> str:
    result = subprocess.run(
        ["git", *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    if result.returncode != 0:
        if allow_fail:
            return ""
        raise SystemExit(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout.strip()


def current_ref() -> str:
    return os.environ.get("GITHUB_SHA") or run_git(["rev-parse", "HEAD"])


def release_version(tag: str) -> tuple[int, int, int] | None:
    match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", tag)
    return tuple(int(part) for part in match.groups()) if match else None


def find_previous_ci_ref(current: str) -> str:
    candidates: list[tuple[int, str]] = []
    refs = run_git(
        [
            "for-each-ref",
            "--merged",
            f"{current}^",
            "--format=%(creatordate:unix)%09%(refname:short)",
            "refs/tags/ci-build-*",
            "refs/tags/v*",
        ],
        allow_fail=True,
    )
    for line in refs.splitlines():
        timestamp, separator, tag = line.strip().partition("\t")
        if separator and tag:
            candidates.append((int(timestamp or "0"), tag))
    if candidates:
        return max(candidates)[1]
    return run_git(["rev-parse", f"{current}^"], allow_fail=True)


def select_previous_release_tag(tags: list[str], version: str) -> str:
    current_version = release_version(f"v{version}")
    candidates = [
        (parsed, tag)
        for tag in tags
        if (parsed := release_version(tag)) is not None
        and (current_version is None or parsed < current_version)
    ]
    return max(candidates, default=((0, 0, 0), ""))[1]


def find_previous_release_ref(version: str) -> str:
    tags = run_git(["tag", "--list", "v*"], allow_fail=True).splitlines()
    return select_previous_release_tag(tags, version)


def rev_range(previous: str, current: str) -> str:
    return f"{previous}..{current}" if previous else current


def summary_input(subject: str, body: str) -> str:
    """过滤审核凭据，保留 Agent 写入的用户说明。"""
    body_lines = []
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped or MACHINE_TRAILER_PATTERN.match(stripped):
            continue
        body_lines.append(stripped)
    return "\n".join([subject.strip(), *body_lines[:20]])


def parse_body_fields(body: str) -> dict[str, str]:
    fields: dict[str, list[str]] = {}
    current_field = ""
    for raw_line in body.splitlines():
        line = raw_line.strip()
        if not line or MACHINE_TRAILER_PATTERN.match(line):
            continue
        match = CREDIT_FIELD_PATTERN.match(line)
        if match:
            fields.setdefault(match.group(1), []).append(match.group(2).strip())
            # 致谢字段是独立条目：随后的正文行不能再续到上一个字段里。
            current_field = ""
            continue
        # `文档同交：` 与致谢字段同类：说明性元数据，不是更新日志字段，也不能并进上一个
        # 更新日志字段（否则放在 `范围：` 之后会被吞掉，日志里出现莫名内容）。
        if STANDALONE_META_FIELD_PATTERN.match(line):
            current_field = ""
            continue
        match = BODY_FIELD_PATTERN.match(line)
        if match:
            current_field = match.group(1)
            fields.setdefault(current_field, []).append(match.group(2).strip())
            continue
        if current_field and not line.startswith(("AI-Review-", "Signed-off-by:", "Co-authored-by:")):
            fields.setdefault(current_field, []).append(line)
    return {key: " ".join(value).strip() for key, value in fields.items() if " ".join(value).strip()}


def is_auto_manifest_subject(subject: str) -> bool:
    return subject.startswith(AUTO_MANIFEST_PREFIXES)


def commit_infos(previous: str, current: str) -> list[CommitInfo]:
    output = run_git(
        ["log", "--pretty=format:%H%x1f%s%x1f%b%x1e", rev_range(previous, current)],
        allow_fail=True,
    )
    commits: list[CommitInfo] = []
    for raw in output.split("\x1e"):
        parts = raw.strip("\n").split("\x1f", 2)
        if len(parts) < 2 or is_auto_manifest_subject(parts[1].strip()):
            continue
        subject = parts[1].strip()
        body = parts[2].strip() if len(parts) > 2 else ""
        commits.append(
            CommitInfo(
                sha=parts[0][:12],
                subject=subject,
                body=body,
                fields=parse_body_fields(body),
            )
        )
    return commits


def changed_files(previous: str, current: str) -> list[str]:
    args = ["diff", "--name-only", f"{previous}..{current}"] if previous else ["show", "--format=", "--name-only", current]
    return [line.strip() for line in run_git(args, allow_fail=True).splitlines() if line.strip()]


def commit_changed_files(commit: CommitInfo) -> list[str]:
    output = run_git(
        ["diff-tree", "--root", "--no-commit-id", "--name-only", "-r", commit.sha],
        allow_fail=True,
    )
    return [line.strip() for line in output.splitlines() if line.strip()]


def change_components(path: str) -> set[str]:
    if path.startswith("app/"):
        return {"app"}
    if path.startswith("assets/zygisk_module/webroot/"):
        return {"module"}
    if path.startswith((".github/", "docs/", "scripts/")) or path in {
        "AGENTS.md",
        "CLAUDE.md",
        "CONTRIBUTING.md",
        "README.md",
    }:
        return {"other"}
    if path == "update.json":
        return set()
    return {"module"}


def commit_kind(subject: str) -> str:
    match = re.match(r"^\s*([^（(：:]+)(?:\([^)]*\))?[：:]", subject)
    prefix = match.group(1).strip().lower() if match else ""
    return {
        "修复": "fix",
        "回退": "fix",
        "功能": "feature",
        "新增": "feature",
        "界面": "feature",
        "重构": "change",
        "性能": "change",
        "维护": "change",
        "依赖": "change",
        "测试": "process",
        "文档": "process",
        "构建": "process",
        "CI": "process",
        "发布": "process",
    }.get(prefix, "change")


def subject_description(subject: str) -> str:
    description = re.sub(r"^\s*[^（(：:]+(?:\([^)]*\))?[：:]\s*", "", subject)
    description = re.sub(r"\s*(?:\[(?:skip ci|ci skip|no ci)\]|仅验证CI)\s*$", "", description, flags=re.I)
    return description.strip(" 。；;，,\t")


def user_summary(commit: CommitInfo, mode: str = "release") -> str:
    change = commit.fields.get("用户影响") or commit.fields.get("变更") or subject_description(commit.subject)
    change = change.strip(" 。；;，,")
    if not change:
        return ""
    if mode == "ci":
        # CI 构建日志面向测试用户：只保留一句用户可读的结论。「适用范围/限制」
        # 是给后续排查用的提交元数据，拼进来会把每条拉长一倍还全是内部词汇。
        return change
    parts = [change]
    scope = commit.fields.get("范围")
    limit = commit.fields.get("限制")
    if scope:
        parts.append(f"适用范围：{scope}")
    if limit:
        parts.append(f"限制：{limit}")
    return "；".join(part.strip(" 。；;，,") for part in parts if part.strip(" 。；;，,"))


def section_for_commit(commit: CommitInfo, mode: str) -> str | None:
    kind = commit_kind(commit.subject)
    if kind == "fix":
        return "fixed"
    if kind == "feature":
        return "features"
    if kind == "process":
        # 测试、文档、构建、CI 调整对使用者没有可感知变化，两种模式都不进日志；
        # 完整改动始终可以从对比链接查看。
        return None
    if kind == "change":
        return "changes"
    if mode == "ci":
        return "changes"
    return None


def collect_analysis(mode: str, files: list[str], commits: list[CommitInfo]) -> dict[str, dict[str, list[dict[str, object]]]]:
    file_set = set(files)
    # 只收模块与 App（WebUI 随模块目录计）：CI、测试流与文档调整对使用者
    # 没有可感知变化，两类日志都不收录（2.2.0-ci.1 之前 CI 日志里混有大量
    # 「其它更新」，实测占了三分之一篇幅）。
    analysis = {
        component: {section: [] for section in SECTIONS}
        for component in COMPONENTS
        if component != "other"
    }
    emitted_summaries: dict[str, set[str]] = {section: set() for section in SECTIONS}
    for commit in commits:
        summary = user_summary(commit, mode)
        if not summary:
            continue
        section = section_for_commit(commit, mode)
        if section is None:
            continue
        commit_files = [path for path in commit_changed_files(commit) if path in file_set]
        for component in sorted(COMPONENTS, key=lambda value: COMPONENT_PRIORITY[value]):
            if component not in analysis:
                continue
            component_files = [path for path in commit_files if component in change_components(path)]
            if not component_files:
                continue
            key = summary_key(summary)
            if key in emitted_summaries[section]:
                continue
            add_unique(
                analysis[component][section],
                {"text": summary, "files": component_files},
            )
            emitted_summaries[section].add(key)
    return analysis


def add_unique(items: list[dict[str, object]], item: dict[str, object]) -> None:
    text = str(item["text"]).strip(" 。；;，,")
    if not text:
        return
    if not any(existing["text"] == text for existing in items):
        items.append({"text": text, "files": item["files"]})


def summary_key(text: str) -> str:
    """构造跨组件去重用的稳定文本键。"""
    return re.sub(r"\s+", "", text).strip("。；;，,")


def credit_url_target(url: str) -> str:
    """返回致谢链接真正应该指向的地址，`仓库.git@<sha>` 转成可点击的提交页。"""
    match = CREDIT_GIT_PIN_PATTERN.match(url)
    return f"{match.group(1)}/commit/{match.group(2)}" if match else url


def markdown_credit_links(text: str) -> str:
    """把致谢文本里的链接统一改写成 Markdown 链接。

    `名称（URL）` 与裸 URL 在 GitHub 上依赖自动链接，紧随其后的中文标点会被当成
    URL 的一部分，读者点开就跳到错误地址。改写成 `[名称](URL)` 后链接边界由括号
    确定，后面的中文标点留在链接外。已经是 Markdown 链接的片段原样保留。
    """

    def linked(match: re.Match[str]) -> str:
        url = match.group(2)
        return f"[{match.group(1)}]({credit_url_target(url)})"

    text = CREDIT_LINKED_NAME_PATTERN.sub(linked, text)

    def wrap_bare(match: re.Match[str]) -> str:
        url = match.group(0)
        if match.start() >= 2 and text[match.start() - 2 : match.start()] == "](":
            return url
        return f"[{url}]({credit_url_target(url)})"

    return CREDIT_URL_PATTERN.sub(wrap_bare, text)


def credit_line(commit: CommitInfo) -> str:
    """把一笔提交里的贡献者与来源拼成一行致谢文本，没有贡献信息时返回空串。"""
    people = [commit.fields[label] for label in CREDIT_PEOPLE_LABELS if commit.fields.get(label)]
    sources = [commit.fields[label] for label in CREDIT_SOURCE_LABELS if commit.fields.get(label)]
    notes = [commit.fields["致谢"]] if commit.fields.get("致谢") else []
    if not people and not sources and not notes:
        return ""
    if not people:
        # 没有具体贡献者时把裸链接（常见写法）补上"来源："，否则日志里只剩一串 URL。
        parts = [f"来源：{source}" for source in sources]
        parts.extend(note if not note.lower().startswith(("http", "git@")) else f"来源：{note}" for note in notes)
        return markdown_credit_links("；".join(parts))
    text = "感谢 " + "、".join(people)
    if sources:
        text += "，来源：" + "；".join(sources)
    if notes:
        text += "，" + "；".join(notes)
    return markdown_credit_links(text)


def credit_range_start(current: str) -> str:
    """正式 Release 的致谢跨整个发布周期累积。

    致谢可能写进任意一笔提交，而正式发布的对比基准可能只覆盖最后一程——只按对比
    范围取会让早期贡献在发布日志里凭空消失。这里以最近的 `v*` 正式发布 tag 为下界，
    正式发布后自然清零。CI 构建日志不走本函数：CI 是对比区间的差异记录，致谢只认
    对比范围内的提交（见 write_changelog）。仓库是浅克隆或尚未打过 tag 时返回空串，
    退回对比范围。
    """
    tag = run_git(["describe", "--tags", "--abbrev=0", "--match", "v*", current], allow_fail=True)
    if not tag:
        return ""
    # 正式发布流程先建草稿（tag 指向当前提交）再生成正文，此时最近 tag 就是本版本
    # 自身，按它取下界会得到空范围、致谢整段消失。这种情况退到再上一个 tag。
    head = run_git(["rev-parse", f"{current}^{{commit}}"], allow_fail=True)
    tagged = run_git(["rev-parse", f"{tag}^{{commit}}"], allow_fail=True)
    if head and head == tagged:
        return run_git(["describe", "--tags", "--abbrev=0", "--match", "v*", f"{tag}^"], allow_fail=True)
    return tag


def collect_credits(commits: list[CommitInfo], mode: str = "release") -> list[str]:
    """按时间倒序收集去重后的致谢行。"""
    limit = MAX_CI_CREDITS if mode == "ci" else MAX_CREDITS
    lines: list[str] = []
    seen: set[str] = set()
    for commit in commits:
        text = credit_line(commit).strip(" 。；;，,")
        # 历史提交正文里把外部建议写成「私下建议」，公开日志里建议就是建议，
        # 输出端统一归一措辞，不追溯改写提交原文。
        text = text.replace("私下建议", "建议")
        if not text:
            continue
        key = summary_key(text)
        if key in seen:
            continue
        seen.add(key)
        lines.append(text)
        if len(lines) >= limit:
            break
    return lines


def limit_analysis(analysis: dict[str, dict[str, list[dict[str, object]]]], mode: str = "release") -> None:
    per_section = MAX_CI_SECTION_ITEMS if mode == "ci" else MAX_SECTION_ITEMS
    for sections in analysis.values():
        for section, items in sections.items():
            if len(items) > per_section:
                sections[section] = items[:per_section]
    if mode != "ci":
        return
    total = sum(len(items) for sections in analysis.values() for items in sections.values())
    if total <= MAX_CI_TOTAL_ITEMS:
        return
    # 全局上限按组件优先级（模块 → App → 其它）与节顺序截断，保证最相关的
    # 条目留到最后一批也先被保留。
    remaining = MAX_CI_TOTAL_ITEMS
    for component in COMPONENTS:
        for section in SECTIONS:
            items = analysis.get(component, {}).get(section, [])
            if not items:
                continue
            if remaining <= 0:
                analysis[component][section] = []
                continue
            analysis[component][section] = items[:remaining]
            remaining -= len(analysis[component][section])


def render_analysis(
    mode: str,
    version: str,
    previous: str,
    current: str,
    analysis: dict[str, dict[str, list[dict[str, object]]]],
    credits: list[str] | None = None,
) -> str:
    if mode == "release":
        lines = [f"# Storage Redirect X v{version}"]
    else:
        lines = [
            "## CI 构建更新日志",
            "",
            f"- 当前版本：`{version or current[:7]}`",
            f"- 对比基准：上一版 CI 或 Release 构建 `{previous or '初始提交'}`",
            f"- 当前提交：`{current[:12]}`",
        ]
    headings = {
        "fixed": "### 修复了什么问题",
        "features": "### 增加了什么功能",
        "changes": "### 功能变化",
        "usage": "### 新功能怎么使用",
        "notes": "### 注意事项",
    }
    component_headings = {"module": "## 模块更新", "app": "## App 更新", "other": "## 其它更新"}
    for component in COMPONENTS:
        sections = analysis.get(component, {})
        if not any(sections.get(section) for section in SECTIONS):
            continue
        lines.extend(["", component_headings[component]])
        for section in SECTIONS:
            items = sections.get(section, [])
            if not items:
                continue
            lines.extend(["", headings[section]])
            lines.extend(f"- {item['text']}。" for item in items)
    if credits:
        # 致谢独立成段放在全局末尾：它不属于任何组件，也不该被组件分区上限截断。
        lines.extend(["", CREDITS_HEADING, ""])
        lines.extend(f"- {text}。" for text in credits)
    if mode == "ci":
        # 不再罗列提交列表：使用者关心的是结论，逐笔提交交给对比链接。
        repo = os.environ.get("GITHUB_REPOSITORY", "")
        if repo:
            compare = f"https://github.com/{repo}/{('compare/' + previous + '...' + current) if previous else ('commit/' + current)}"
            lines.extend(["", f"**完整变更对比**: {compare}"])
    if mode == "release" and len(lines) == 1:
        raise SystemExit("更新日志生成失败：对比范围内没有可读取的用户说明。")
    return "\n".join(lines) + "\n"


def write_changelog(mode: str, version: str, current: str, output: Path) -> None:
    previous = find_previous_release_ref(version) if mode == "release" else find_previous_ci_ref(current)
    files = changed_files(previous, current)
    commits = commit_infos(previous, current)
    analysis = collect_analysis(mode, files, commits)
    limit_analysis(analysis, mode)
    # 致谢取数范围按模式区分：CI 构建日志是对比区间的差异记录，致谢也只认对比
    # 范围内的提交——按整个发布周期累积会让上一版 CI 已展示过的旧条目在后续
    # 每版 CI 里重复出现（ci.11 曾因此带出 ci.9/ci.10 已发布的致谢）。
    # 正式 Release 是版本周期汇总，保持周期累积不变。
    if mode == "ci":
        credits = collect_credits(commits, mode)
    else:
        credit_base = credit_range_start(current)
        earlier = commit_infos(credit_base, current) if credit_base and credit_base != previous else []
        credits = collect_credits([*commits, *earlier], mode)
    output.write_text(render_analysis(mode, version, previous, current, analysis, credits), encoding="utf-8")


def ensure_credits(path: Path, mode: str = "release") -> None:
    """给人写的发布说明补齐致谢段。

    正式 Release 允许用 `docs/release-notes/v<版本>.md` 覆盖自动生成的正文，覆盖之后
    生成器算出的致谢就丢了。这里在覆盖之后兜住：正文已有 `### 致谢` 就不动（尊重人工
    措辞），没有则按发布周期内的提交汇总追加，保证外部思路来源始终出现在更新日志里。
    """
    markdown = path.read_text(encoding="utf-8")
    if CREDITS_HEADING in markdown:
        return
    current = current_ref()
    base = credit_range_start(current)
    credits = collect_credits(commit_infos(base, current), mode)
    if not credits:
        return
    lines = markdown.rstrip("\n").splitlines()
    lines.extend(["", CREDITS_HEADING, ""])
    lines.extend(f"- {text}。" for text in credits)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=["ci", "release"], required=True)
    parser.add_argument("--version", default="")
    parser.add_argument("--output", default="")
    parser.add_argument("--ensure-credits", default="")
    args = parser.parse_args()
    if args.ensure_credits:
        ensure_credits(Path(args.ensure_credits), args.mode)
        return
    if not args.output:
        raise SystemExit("必须提供 --output，或使用 --ensure-credits 补充致谢")
    write_changelog(args.mode, args.version, current_ref(), Path(args.output))


if __name__ == "__main__":
    main()
