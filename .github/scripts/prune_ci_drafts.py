#!/usr/bin/env python3
"""清理废弃的 CI 草稿 Release。

草稿 Release 是 module/app 构建产物的唯一副本，任务级重跑
（`gh run rerun --failed`）要靠它续接发布，所以本轮草稿**永远保留**——
哪怕本轮失败，那也是唯一还能发出去的资产来源。其它 run 的草稿按两条规则处理：

1. 本轮**已成功发布**时，版本不高于本轮、且创建时间早于本轮的草稿都已被取代：
   发布成功后 `update-manifest` 会推进版本基线，这些序号不会再被任何一轮使用，
   立即删除，避免同版本号不同内容的废弃副本长期留在后台。
2. 版本**严格低于**本轮、且创建时间早于本轮的草稿同样按废弃处理，与是否发布
   无关：本轮已经在构建同一基础版本里更大的序号，更老那一轮对应的代码已被
   取代，续发只会发出一个序号更小、清单也不会回退的旧包。
3. 本轮没有发布成功时，其余草稿保留 24 小时续发窗口（单轮 CI 上限 2 小时，
   留足人工判断与重跑时间），超过窗口仍未被接走的视为废弃。

同一个版本号可能同时存在多个失败草稿（连续几轮都失败，序号不推进），因此判定
必须逐个 Release 独立进行，不能按版本号去重或只处理最先遇到的那一个。

版本号从 Release 标题解析，取不到时回落到资产名；两者都解析不出来则只按
24 小时窗口处理，不做「已取代」判定——宁可在后台多留一天，也不误删可能还要
续发的草稿。
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timezone

CI_TAG_PREFIX = "ci-build-"
CI_VERSION_PATTERN = re.compile(r"(\d+)\.(\d+)\.(\d+)-ci\.(\d+)")
RERUN_WINDOW_SECONDS = 24 * 60 * 60
RELEASES_PER_PAGE = 100

Version = tuple[int, int, int, int]


@dataclass(frozen=True)
class Decision:
    """单个草稿的处置结论。action 取 `delete` 或 `keep`。"""

    action: str
    reason: str


def parse_ci_version(text: str | None) -> Version | None:
    """从任意文本中提取 `MAJOR.MINOR.PATCH-ci.N`，用于版本先后比较。"""
    if not text:
        return None
    match = CI_VERSION_PATTERN.search(text)
    if not match:
        return None
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)), int(match.group(4)))


def release_version(release: dict) -> Version | None:
    """Release 的版本：优先标题，其次标签，最后从资产名反推。

    旧实现在标题里也可能不带版本（草稿创建时只写了「构建与测试进行中」），
    资产名 `storage.redirect.x-v<version>.zip` 是可靠的兜底来源。
    """
    for key in ("name", "tag_name"):
        version = parse_ci_version(release.get(key))
        if version is not None:
            return version
    for asset in release.get("assets") or []:
        version = parse_ci_version(asset.get("name"))
        if version is not None:
            return version
    return None


def parse_timestamp(text: str | None) -> datetime | None:
    """解析 GitHub 返回的 ISO 8601 时间戳；无法解析时返回 None。"""
    if not text:
        return None
    try:
        return datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return None


def decide(
    release: dict,
    *,
    current_tag: str,
    current_version: Version | None,
    current_run_at: datetime | None,
    published: bool,
    now: datetime,
) -> Decision:
    """判定一个 Release 该删还是该留。

    `published` 表示本轮发布环节是否成功；只有成功时才允许用「已被本轮取代」
    立即清理，否则一律等满 24 小时，保住失败轮次的续发窗口。
    """
    tag = release.get("tag_name") or ""
    if tag == current_tag:
        return Decision("keep", "current run draft")
    if not tag.startswith(CI_TAG_PREFIX):
        return Decision("keep", "not a ci-build release")
    if not release.get("draft"):
        return Decision("keep", "already published")

    created = parse_timestamp(release.get("created_at"))
    age = (now - created).total_seconds() if created else None
    version = release_version(release)
    comparable = (
        version is not None
        and current_version is not None
        and created is not None
        and current_run_at is not None
        and created < current_run_at
    )

    if comparable and version < current_version:
        # 本轮已在同一基础版本里构建更大的序号：更老那一轮的代码已被取代。
        return Decision("delete", "superseded by newer build")

    if comparable and version == current_version and published:
        # 同版本号不同内容：本轮已发布，旧副本没有任何用途。
        return Decision("delete", "superseded by this run")

    if age is not None and age >= RERUN_WINDOW_SECONDS:
        return Decision("delete", "stale beyond rerun window")

    return Decision("keep", "within rerun window")


def plan(
    releases: list[dict],
    *,
    current_tag: str,
    current_version: Version | None,
    current_run_at: datetime | None,
    published: bool,
    now: datetime,
) -> list[tuple[str, Decision]]:
    """对整批草稿逐个判定，返回 (tag, 结论) 列表。

    同一个版本号可能有多份失败草稿，所以这里不做任何按版本去重：每条 Release
    独立判定，保证「多个同名废弃草稿」也能一次清干净。
    """
    return [
        (
            release.get("tag_name") or "",
            decide(
                release,
                current_tag=current_tag,
                current_version=current_version,
                current_run_at=current_run_at,
                published=published,
                now=now,
            ),
        )
        for release in releases
    ]


def run_gh(args: list[str]) -> str:
    result = subprocess.run(
        ["gh", *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        raise SystemExit(f"gh {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def fetch_draft_releases(repository: str) -> list[dict]:
    output = run_gh(
        [
            "api",
            f"repos/{repository}/releases?per_page={RELEASES_PER_PAGE}",
        ]
    )
    try:
        releases = json.loads(output)
    except json.JSONDecodeError as error:
        raise SystemExit(f"unable to parse release list: {error}") from error
    return [release for release in releases if release.get("draft")]


def delete_release(repository: str, tag: str) -> None:
    # 不加 --cleanup-tag：草稿不创建 tag，已有 tag 也可能被当作诊断锚点引用。
    run_gh(["release", "delete", tag, "--repo", repository, "--yes"])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, help="owner/repo")
    parser.add_argument("--current-tag", required=True, help="本轮 CI 的 Release 标签")
    parser.add_argument("--current-version", required=True, help="本轮版本号，如 2.2.0-ci.1")
    parser.add_argument(
        "--current-run-created",
        default="",
        help="本轮 workflow run 的创建时间（ISO 8601），用于区分更早/更晚的轮次",
    )
    parser.add_argument(
        "--published",
        action="store_true",
        help="本轮发布环节已成功；允许按「已被本轮取代」立即清理",
    )
    parser.add_argument("--dry-run", action="store_true", help="只打印结论，不实际删除")
    args = parser.parse_args()

    current_version = parse_ci_version(args.current_version)
    if current_version is None:
        raise SystemExit(f"--current-version must look like MAJOR.MINOR.PATCH-ci.N, got: {args.current_version}")
    current_run_at = parse_timestamp(args.current_run_created)
    if current_run_at is None:
        print(
            "warning: --current-run-created missing or unparsable; "
            "supersede check disabled, only the rerun window applies",
            file=sys.stderr,
        )

    now = datetime.now(timezone.utc)
    releases = fetch_draft_releases(args.repository)
    decisions = plan(
        releases,
        current_tag=args.current_tag,
        current_version=current_version,
        current_run_at=current_run_at,
        published=args.published,
        now=now,
    )
    deleted = 0
    for tag, decision in decisions:
        print(f"{decision.action}\t{tag or '<no tag>'}\t{decision.reason}")
        if decision.action != "delete":
            continue
        if args.dry_run:
            continue
        delete_release(args.repository, tag)
        deleted += 1

    print(f"draft releases scanned: {len(releases)}, deleted: {deleted}")


if __name__ == "__main__":
    main()
