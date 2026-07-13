#!/usr/bin/env python3
"""Generate Chinese changelogs for CI artifacts and GitHub Releases."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

MAX_PATCH_CHARS = 180_000
MAX_SECTION_ITEMS = 8
GENERIC_SUMMARIES = {"CI", "ci", "更新", "修复", "调整", "优化", "文档", "测试"}
AUTO_MANIFEST_PREFIXES = (
    "CI：更新更新清单",
    "发布：更新更新清单",
)
CLASSIFICATION_PATCH_EXCLUDED_PREFIXES = (
    ".github/",
    "docs/",
    "scripts/",
)
CLASSIFICATION_PATCH_EXCLUDED_FILES = {
    "AGENTS.md",
    "CONTRIBUTING.md",
    "README.md",
    "update.json",
}


@dataclass(frozen=True)
class CommitInfo:
    sha: str
    subject: str
    body: str
    summary: str
    kind: str


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
        line = line.strip()
        if not line:
            continue
        parts = line.split("\t", 1)
        if len(parts) != 2:
            continue
        timestamp, tag = parts
        candidates.append((int(timestamp or "0"), tag))

    if candidates:
        candidates.sort(reverse=True)
        return candidates[0][1]

    previous_commit = run_git(["rev-parse", f"{current}^"], allow_fail=True)
    return previous_commit


def find_previous_release_ref(current: str) -> str:
    tags = run_git(
        ["tag", "--merged", f"{current}^", "--sort=-version:refname", "--list", "v*"],
        allow_fail=True,
    )
    if not tags:
        return ""
    return tags.splitlines()[0]


def rev_range(previous: str, current: str) -> str:
    if previous:
        return f"{previous}..{current}"
    return current


def commit_infos(previous: str, current: str) -> list[CommitInfo]:
    log_range = rev_range(previous, current)
    output = run_git(["log", "--pretty=format:%H%x1f%s%x1f%b%x1e", log_range], allow_fail=True)
    commits: list[CommitInfo] = []
    for raw in output.split("\x1e"):
        raw = raw.strip("\n")
        if not raw:
            continue
        parts = raw.split("\x1f", 2)
        if len(parts) < 2:
            continue
        sha = parts[0][:7]
        subject = parts[1].strip()
        if is_auto_manifest_subject(subject):
            continue
        body = parts[2].strip() if len(parts) > 2 else ""
        summary = summarize_commit_text(subject)
        commits.append(
            CommitInfo(
                sha=sha,
                subject=subject,
                body=body,
                summary=summary,
                kind=classify_commit(subject, summary),
            )
        )
    return commits


def is_auto_manifest_subject(subject: str) -> bool:
    return subject.startswith(AUTO_MANIFEST_PREFIXES)


def commit_lines(commits: list[CommitInfo]) -> list[str]:
    return [f"- `{commit.sha}` {commit.summary}" for commit in commits]


def changed_files(previous: str, current: str) -> list[str]:
    if previous:
        args = ["diff", "--name-only", f"{previous}..{current}"]
    else:
        args = ["show", "--format=", "--name-only", current]
    output = run_git(args, allow_fail=True)
    return [line.strip() for line in output.splitlines() if line.strip()]


def changed_patch(previous: str, current: str) -> str:
    if previous:
        args = [
            "diff",
            "--unified=0",
            "--no-ext-diff",
            f"{previous}..{current}",
            "--",
            ".",
            ":(exclude)vendor/**",
            ":(exclude)target/**",
        ]
    else:
        args = [
            "show",
            "--format=",
            "--unified=0",
            "--no-ext-diff",
            current,
            "--",
            ".",
            ":(exclude)vendor/**",
            ":(exclude)target/**",
        ]
    return run_git(args, allow_fail=True)[:MAX_PATCH_CHARS]


def commit_changed_files(commit: CommitInfo) -> list[str]:
    output = run_git(
        ['diff-tree', '--no-commit-id', '--name-only', '-r', commit.sha],
        allow_fail=True,
    )
    return [line.strip() for line in output.splitlines() if line.strip()]


def change_components(path: str) -> set[str]:
    if path.startswith('app/'):
        return {'app'}
    if path.startswith('assets/zygisk_module/webroot/'):
        return {'module'}
    if path.startswith(('.github/', 'docs/', 'scripts/')) or path in {
        'AGENTS.md',
        'CLAUDE.md',
        'CONTRIBUTING.md',
        'README.md',
    }:
        return {'other'}
    if path == 'update.json':
        return set()
    return {'module'}


def patch_path_affects_classification(path: str) -> bool:
    if path in CLASSIFICATION_PATCH_EXCLUDED_FILES:
        return False
    if path.startswith(CLASSIFICATION_PATCH_EXCLUDED_PREFIXES):
        return False
    if "/test/" in path or path.startswith("app/src/test/"):
        return False
    return True


def classification_patch(patch: str) -> str:
    chunks: list[str] = []
    include_current_file = True
    for line in patch.splitlines():
        if line.startswith("diff --git "):
            match = re.match(r"^diff --git a/(.+?) b/(.+)$", line)
            path = match.group(2) if match else ""
            include_current_file = patch_path_affects_classification(path)
        if include_current_file:
            chunks.append(line)
    return "\n".join(chunks)


def summarize_commit_text(text: str) -> str:
    normalized = " ".join(line.strip() for line in text.splitlines() if line.strip())
    normalized = re.sub(
        r"^(feat|fix|docs|ci|chore|refactor|perf|test|build)(\([^)]*\))?:\s*",
        "",
        normalized,
        flags=re.I,
    )
    normalized = re.sub(
        r"^(修复|功能|新增|文档|测试|发布|构建|优化|重构|维护|依赖|CI)(\([^)]*\))?[：:]\s*",
        "",
        normalized,
        flags=re.I,
    )
    replacements = [
        ("attribute system media writes to caller apps", "将系统媒体代写归因到真实调用应用"),
        ("caller", "调用方"),
        ("system media writes", "系统媒体代写"),
        ("changelog", "更新日志"),
        ("release", "正式发布"),
        ("ci", "CI"),
        ("config", "配置"),
        ("workflow", "工作流"),
    ]
    result = normalized
    for source, target in replacements:
        result = re.sub(re.escape(source), target, result, flags=re.I)
    return result or "未填写提交说明"


def classify_commit(subject: str, summary: str) -> str:
    prefix_match = re.match(r"^\s*([A-Za-z]+|[\u4e00-\u9fff]+)(?:\([^)]*\))?[：:]", subject)
    if prefix_match:
        prefix = prefix_match.group(1).lower()
        prefix_map = {
            "fix": "fix",
            "bugfix": "fix",
            "hotfix": "fix",
            "修复": "fix",
            "feat": "feature",
            "feature": "feature",
            "功能": "feature",
            "新增": "feature",
            "docs": "docs",
            "doc": "docs",
            "文档": "docs",
            "ci": "ci",
            "build": "ci",
            "构建": "ci",
            "release": "ci",
            "发布": "ci",
            "test": "test",
            "tests": "test",
            "测试": "test",
            "perf": "fix",
            "优化": "fix",
            "refactor": "internal",
            "重构": "internal",
            "chore": "internal",
            "维护": "internal",
            "依赖": "dependency",
        }
        if prefix in prefix_map:
            return prefix_map[prefix]

    lowered = subject.lower()
    if subject.strip().upper() == "CI" or any(word in lowered for word in ("workflow", "artifact", "release")):
        return "ci"

    fix_markers = (
        "修复",
        "避免",
        "降低",
        "减少",
        "补齐",
        "补全",
        "校正",
        "清理",
        "兼容",
        "处理",
        "稳定",
        "放行",
        "保留",
        "禁止",
        "严格",
        "恢复",
        "兜底",
    )
    feature_markers = ("新增", "增加", "支持", "引入", "启用", "提供", "允许", "接入", "产出")
    docs_markers = ("文档", "说明", "README", "docs/")
    test_markers = ("测试", "覆盖", "fixture")

    if summary.startswith(fix_markers) or any(marker in summary for marker in fix_markers[:8]):
        return "fix"
    if summary.startswith(feature_markers):
        return "feature"
    if any(marker in summary for marker in docs_markers):
        return "docs"
    if any(marker in summary for marker in test_markers):
        return "test"
    return "internal"


def add_unique(items: list[str], sentence: str) -> None:
    sentence = sentence.strip()
    if not sentence:
        return
    if not sentence.startswith("- "):
        sentence = f"- {sentence}"
    if not sentence.endswith(("。", "！", "？")):
        sentence = f"{sentence.rstrip('；;,.，、 ')}。"
    if sentence not in items:
        items.append(sentence)


def compact_phrase(text: str) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    return text.strip("。；;,.，、 ")


def meaningful_phrase(text: str) -> bool:
    text = compact_phrase(text)
    return bool(text and text not in GENERIC_SUMMARIES and len(text) > 2)


def join_limited(items: list[str], limit: int = 4) -> str:
    phrases = [compact_phrase(item) for item in items if meaningful_phrase(item)]
    shown = phrases[:limit]
    result = "、".join(shown)
    if len(phrases) > limit:
        suffix = " 等" if re.search(r"[A-Za-z0-9]$", result) else "等"
        result = f"{result}{suffix}"
    return result


def action_area(action: str, area: str) -> str:
    separator = " " if re.match(r"^[A-Za-z0-9]", area) else ""
    return f"{action}{separator}{area}"


def is_user_facing_feature(commit: CommitInfo, _files: list[str], _patch: str) -> bool:
    text = "\n".join([commit.subject, commit.summary])
    feature_markers = (
        "设置",
        "开关",
        "配置项",
        "配置键",
        "全局配置",
        "用户配置",
        "配置",
        "页面",
        "入口",
        "菜单",
        "按钮",
        "模板",
        "导入",
        "导出",
        "备份",
        "恢复",
        "缩放",
        "路径浏览器",
        "日志包",
        "自动保存",
        "详细日志",
        "管理 App",
        "管理界面",
        "WebUI",
    )
    return any(marker in text for marker in feature_markers)


def detect_area(text: str) -> str:
    area_rules = [
        (
            (
                "MediaProvider",
                "媒体",
                "系统代写",
                "writer hook",
                "SAF",
                "DocumentsUI",
                "PhotoPicker",
                "Hooker.java",
            ),
            "媒体代写与系统存储链路",
        ),
        (
            (
                "FUSE",
                "FuseFix",
                "fuse",
                "挂载",
                "mount",
                "通配符",
                "只读",
                "映射",
                "路径映射",
                "公共映射",
            ),
            "FUSE、挂载和路径映射",
        ),
        (
            ("文件监视", "文件监控", "监视记录", "日志导出", "采集器", "source_hint"),
            "文件监控与日志",
        ),
        (
            ("WebUI", "webui", "界面", "缩放", "bottomNav", "app.js", "miuix"),
            "WebUI/管理界面",
        ),
        (
            ("管理 App", "MainActivity", "SettingsScreen", "Compose", "Miuix", "APK"),
            "管理 App",
        ),
        (("配置", "raw 配置", "模板", "global.json", "apps/", "config"), "配置解析与模板"),
        (("重定向", "redirect", "caller", "调用方", "归因", "路径策略", "router"), "重定向策略与调用方识别"),
        (("CI", "workflow", "artifact", "Release", "产物", "更新日志"), "CI/Release 发布流程"),
        (("测试", "单元测试", "fixture"), "测试覆盖"),
        (("文档", "README", "docs/"), "文档说明"),
    ]
    for keywords, area in area_rules:
        if any(keyword in text for keyword in keywords):
            return area
    return "核心逻辑"


def append_grouped_commit_sections(
    sections: dict[str, list[str]],
    commits: list[CommitInfo],
    files: list[str],
    patch: str,
) -> None:
    fix_groups: dict[str, list[str]] = {}
    feature_groups: dict[str, list[str]] = {}

    for commit in commits:
        area = detect_area(f"{commit.subject}\n{commit.summary}")
        if commit.kind == "fix":
            fix_groups.setdefault(area, []).append(commit.summary)
        elif commit.kind == "feature" and is_user_facing_feature(commit, files, patch):
            feature_groups.setdefault(area, []).append(commit.summary)

    for area, summaries in fix_groups.items():
        details = join_limited(summaries)
        if not details:
            continue
        if len(summaries) == 1:
            add_unique(sections["fixed"], f"{action_area('修复', area)}相关问题：{details}")
        else:
            add_unique(sections["fixed"], f"{action_area('修复', area)}的一组问题：{details}")

    for area, summaries in feature_groups.items():
        details = join_limited(summaries)
        if details:
            add_unique(sections["features"], f"{action_area('新增或增强', area)}能力：{details}")


def extract_config_keys(patch: str) -> list[str]:
    added_lines = [
        line[1:]
        for line in patch.splitlines()
        if line.startswith("+") and not line.startswith("+++")
    ]
    source = "\n".join(added_lines) if added_lines else patch
    keys = set(
        re.findall(
            r"['\"`]([a-z][a-z0-9_]*(?:_enabled|_paths|_mappings|_template_id|_filters|_scale|_mode|_config|_version))['\"`]",
            source,
        )
    )
    noisy = {"version", "format_version"}
    return sorted(key for key in keys if key not in noisy)[:6]


def append_contextual_sections(
    sections: dict[str, list[str]],
    files: list[str],
    patch: str,
) -> None:
    has_substantive_change = bool(sections["fixed"] or sections["features"])
    has_runtime = any(
        path.startswith(("src/hook/", "src/mount", "src/fuse", "src/lifecycle", "src/redirect/", "src/daemon", "java_src/", "native/"))
        for path in files
    )
    has_config = any(path.startswith("src/config") or path.startswith("docs/config-fixtures/") for path in files)

    if has_substantive_change and has_runtime:
        add_unique(sections["notes"], "本次涉及运行时 hook、挂载或系统存储链路，升级后建议重点验证文件保存、相册读取、SAF 导出和已配置应用的读写路径")

    if has_substantive_change and has_config:
        add_unique(sections["notes"], "配置解析或默认值有调整时，已有配置会继续按兼容逻辑读取；运行中的应用可能需要重启后才完全使用新规则")


def append_feature_usage(
    sections: dict[str, list[str]],
    commits: list[CommitInfo],
    files: list[str],
    patch: str,
) -> None:
    if not sections["features"]:
        return

    for commit in commits:
        if commit.kind != "feature" or not is_user_facing_feature(commit, files, patch):
            continue
        text = f"{commit.subject}\n{commit.summary}"
        if "配置操作即时保存" in text or "自动保存" in text or "app_config_auto_save" in text:
            add_unique(sections["usage"], "在 WebUI 设置页打开 `配置操作即时保存` 后，进入应用配置页直接修改配置即可；关闭时仍沿用手动点击 `保存` 的旧流程")
        if "详细日志" in text or "verbose_logging_enabled" in text:
            add_unique(sections["usage"], "需要排障时在设置页的“模块设置”中打开 `详细日志`，定位结束后关闭即可立即停止相关记录")
        if "管理界面缩放" in text or "界面缩放" in text or "page_scale" in text or "缩放" in text:
            add_unique(sections["usage"], "在 WebUI 或管理界面的设置页调整界面缩放比例；保存后刷新页面或重新打开管理界面即可使用新的显示比例")


def limit_section(items: list[str]) -> list[str]:
    if len(items) <= MAX_SECTION_ITEMS:
        return items
    omitted = len(items) - (MAX_SECTION_ITEMS - 1)
    return [
        *items[: MAX_SECTION_ITEMS - 1],
        f"- 另有 {omitted} 条相关变化已保留在下方提交列表中，完整细节以提交列表和完整变更对比为准。",
    ]


def classify_changes(files: list[str], commits: list[CommitInfo], patch: str) -> dict[str, list[str]]:
    domain_patch = classification_patch(patch)
    domain_files = [path for path in files if patch_path_affects_classification(path)]
    sections = {
        "fixed": [],
        "features": [],
        "usage": [],
        "notes": [],
    }

    append_grouped_commit_sections(sections, commits, files, domain_patch)
    append_feature_usage(sections, commits, files, domain_patch)
    append_contextual_sections(sections, domain_files, domain_patch)

    for key, values in sections.items():
        sections[key] = limit_section(values)

    return sections


def compare_url(previous: str, current: str) -> str:
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    if not repo:
        return ""
    if previous:
        return f"https://github.com/{repo}/compare/{previous}...{current}"
    return f"https://github.com/{repo}/commit/{current}"


def write_changelog(mode: str, version: str, previous: str, current: str, output: Path) -> None:
    commits = commit_infos(previous, current)
    patch = changed_patch(previous, current)

    title = "CI 构建更新日志" if mode == "ci" else "Release 更新日志"
    baseline = "上一版 CI 或 Release 构建" if mode == "ci" else "上一版 Release 构建"
    current_label = version or current[:7]
    previous_label = previous or "初始提交"
    url = compare_url(previous, current)

    lines = [
        f"## {title}",
        "",
        f"- 当前版本：`{current_label}`",
        f"- 对比基准：{baseline} `{previous_label}`",
        f"- 当前提交：`{current[:7]}`",
    ]
    detail_titles = [
        ("fixed", "### 修复了什么问题"),
        ("features", "### 增加了什么功能"),
        ("usage", "### 新功能怎么使用"),
        ("notes", "### 注意事项"),
    ]
    component_titles = [
        ('module', '## 模块更新'),
        ('app', '## App 更新'),
        ('other', '## 其它更新'),
    ]
    component_commits = {key: [] for key, _ in component_titles}
    for commit in commits:
        components = set().union(*(change_components(path) for path in commit_changed_files(commit)))
        for component in components:
            component_commits[component].append(commit)

    for component, component_heading in component_titles:
        selected_commits = component_commits[component]
        if not selected_commits:
            continue
        selected_files = sorted(
            {
                path
                for commit in selected_commits
                for path in commit_changed_files(commit)
                if component in change_components(path)
            }
        )
        sections = classify_changes(selected_files, selected_commits, patch)
        if not any(sections.values()):
            sections['notes'] = [f"- {commit.summary}。" for commit in selected_commits]
        lines.extend(["", component_heading])
        for key, heading in detail_titles:
            items = sections[key]
            if items:
                lines.extend(["", heading, *items])

    commit_list = commit_lines(commits)
    if commit_list:
        lines.extend(["", "### 提交列表", *commit_list])
    if url:
        lines.extend(["", f"**完整变更对比**: {url}"])

    output.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=["ci", "release"], required=True)
    parser.add_argument("--version", default="")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    current = current_ref()
    if args.mode == "release":
        previous = find_previous_release_ref(current)
    else:
        previous = find_previous_ci_ref(current)
    write_changelog(args.mode, args.version, previous, current, Path(args.output))


if __name__ == "__main__":
    main()
