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
COMPONENTS = ("module", "app", "other")
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


def user_summary(commit: CommitInfo) -> str:
    change = commit.fields.get("用户影响") or commit.fields.get("变更") or subject_description(commit.subject)
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
    if kind == "change":
        return "changes"
    if mode == "ci":
        return "changes"
    return None


def collect_analysis(mode: str, files: list[str], commits: list[CommitInfo]) -> dict[str, dict[str, list[dict[str, object]]]]:
    file_set = set(files)
    analysis = {
        component: {section: [] for section in SECTIONS}
        for component in COMPONENTS
        if mode == "ci" or component != "other"
    }
    for commit in commits:
        summary = user_summary(commit)
        if not summary:
            continue
        section = section_for_commit(commit, mode)
        if section is None:
            continue
        commit_files = [path for path in commit_changed_files(commit) if path in file_set]
        for component in COMPONENTS:
            if component not in analysis:
                continue
            component_files = [path for path in commit_files if component in change_components(path)]
            if not component_files:
                continue
            add_unique(
                analysis[component][section],
                {"text": summary, "files": component_files},
            )
    return analysis


def add_unique(items: list[dict[str, object]], item: dict[str, object]) -> None:
    text = str(item["text"]).strip(" 。；;，,")
    if not text:
        return
    if not any(existing["text"] == text for existing in items):
        items.append({"text": text, "files": item["files"]})


def limit_analysis(analysis: dict[str, dict[str, list[dict[str, object]]]]) -> None:
    for sections in analysis.values():
        for section, items in sections.items():
            if len(items) > MAX_SECTION_ITEMS:
                sections[section] = items[:MAX_SECTION_ITEMS]


def render_analysis(mode: str, version: str, previous: str, current: str, analysis: dict[str, dict[str, list[dict[str, object]]]]) -> str:
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
    if mode == "ci":
        commits = commit_infos(previous, current)
        if commits:
            lines.extend(["", "### 提交列表"])
            lines.extend(f"- `{commit.sha}` {commit.subject}" for commit in commits)
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
    limit_analysis(analysis)
    output.write_text(render_analysis(mode, version, previous, current, analysis), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=["ci", "release"], required=True)
    parser.add_argument("--version", default="")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    write_changelog(args.mode, args.version, current_ref(), Path(args.output))


if __name__ == "__main__":
    main()
