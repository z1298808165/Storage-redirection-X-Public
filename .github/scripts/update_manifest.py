#!/usr/bin/env python3
"""Update the static app update manifest.

清单从 schema 2 起把「检测版本」与两个组件版本分开：

- 顶层 ``version``：检测版本，等价于「第几次发布」。旧客户端只读这个字段，所以它必须
  **每次发布都严格递增**，且不低于两个组件版本。
- ``app.version`` / ``module.version``：App 与模块各自的版本线，互不相干（App 走 UTC
  日期时间 ``yyyyMMddHHmmss``，模块跟 Cargo.toml）。本次没有重编的组件沿用上一版清单里记录的值，不需要额外
  的状态文件。
- ``downloadUrl`` / ``moduleUrl``：兼容字段，分别是 ``app.url`` / ``module.url`` 的镜像，
  只描述本次发布自带的构件；本次没有该构件时留空。
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

SCHEMA_VERSION = 2

VERSION_PATTERN = re.compile(r"(?:^|[^0-9])v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?")


def release_url(repository: str, tag: str) -> str:
    return f"https://github.com/{repository}/releases/tag/{tag}"


def asset_url(repository: str, tag: str, asset_name: str) -> str:
    return f"https://github.com/{repository}/releases/download/{tag}/{asset_name}"


def version_key(value: str) -> tuple[int, int, int, tuple[tuple[int, object], ...]] | None:
    """把版本串解析成可比较的键；解析不出返回 None。

    预发布段按语义化版本处理：数字标识符按数值比较且低于字母标识符，
    前缀相同时更短的预发布段更小（2.3.2-ci.9 < 2.3.2-ci.10 < 2.3.2）。
    """
    match = VERSION_PATTERN.search(value.strip())
    if match is None:
        return None
    pre_release = match.group(4) or ""
    parts: list[tuple[int, object]] = []
    for part in pre_release.split("."):
        if part.isdigit():
            parts.append((0, int(part)))
        elif part:
            parts.append((1, part))
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)), tuple(parts))


def compare_versions(left: str, right: str) -> int:
    """比较两个版本串；任一解析不出时按字符串比较（保持确定性，不抛异常）。"""
    left_key = version_key(left)
    right_key = version_key(right)
    if left_key is None or right_key is None:
        return (left > right) - (left < right)
    if left_key[0:3] != right_key[0:3]:
        return (left_key[0:3] > right_key[0:3]) - (left_key[0:3] < right_key[0:3])
    left_pre, right_pre = left_key[3], right_key[3]
    if not left_pre and not right_pre:
        return 0
    if not left_pre:
        return 1
    if not right_pre:
        return -1
    for left_part, right_part in zip(left_pre, right_pre):
        if left_part == right_part:
            continue
        # (0, 数字) 与 (1, 文本) 的首元素不同时，数字标识符更小。
        if left_part[0] != right_part[0]:
            return -1 if left_part[0] < right_part[0] else 1
        return -1 if left_part[1] < right_part[1] else 1
    return (len(left_pre) > len(right_pre)) - (len(left_pre) < len(right_pre))


def max_version(versions: list[str]) -> str:
    winner = versions[0]
    for version in versions[1:]:
        if compare_versions(version, winner) > 0:
            winner = version
    return winner


def load_manifest(path: Path) -> dict:
    if not path.exists():
        return {"schema": SCHEMA_VERSION, "repository": "", "stable": None, "beta": None, "releases": []}
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, dict):
        raise ValueError("update manifest must be a JSON object")
    data.setdefault("schema", 1)
    data.setdefault("repository", "")
    data.setdefault("stable", None)
    data.setdefault("beta", None)
    data.setdefault("releases", [])
    return data


def previous_component_version(previous: dict | None, component: str) -> str:
    """上一版清单里记录的组件版本。

    schema 2 之前每次发布都会重编两个组件，所以 v1 条目的 ``version`` 就是当时的组件版本。
    """
    if not isinstance(previous, dict):
        return ""
    nested = previous.get(component)
    if isinstance(nested, dict):
        recorded = str(nested.get("version", "")).strip()
        if recorded:
            return recorded
    # 条目本身不带 schema 字段：只有完全没有任何组件字段的 v1 条目，才把顶层检测版本
    # 当作当时的组件版本；带了组件字段却缺这一侧，说明当时没有该组件的信息，不能顶替。
    if previous.get("app") is None and previous.get("module") is None:
        return str(previous.get("version", "")).strip()
    return ""


def build_component(version: str, url: str) -> dict:
    component = {"version": version}
    if url:
        component["url"] = url
    return component


def sanitize_release_notes(markdown: str) -> str:
    normalized = markdown.replace('\r\n', '\n').replace('\r', '\n').strip()
    if not normalized:
        return ''
    commit_heading = re.search(r'(?im)^#{1,6}\s*提交列表\s*$', normalized)
    if commit_heading:
        normalized = normalized[: commit_heading.start()].rstrip()
    normalized = re.sub(
        r'(?im)^\*\*完整变更对比\*\*\s*:\s*https?://\S+\s*$',
        '',
        normalized,
    ).strip()
    return normalized


def _is_legacy_entry(previous: dict | None) -> bool:
    """上一版是否为「两个组件共用检测版本」的旧格式（或根本没有上一版）。"""
    if not isinstance(previous, dict):
        return True
    return previous.get("app") is None and previous.get("module") is None


def build_entry(args: argparse.Namespace, previous: dict | None) -> dict:
    app_version = (args.app_version or "").strip() or previous_component_version(previous, "app")
    module_version = (args.module_version or "").strip() or previous_component_version(previous, "module")
    # 首次发布（或上一版是 v1 条目）没有组件版本可沿用：两个组件都等于检测版本。
    # 上一版已经是 schema 2 却缺某一侧时，说明当时没有该组件的信息，宁可留空也不要
    # 拿检测版本顶替——客户端会把空版本当作「本次没有该组件」。
    if not app_version and _is_legacy_entry(previous):
        app_version = args.version
    if not module_version and _is_legacy_entry(previous):
        module_version = args.version
    # 检测版本是旧客户端唯一的判定依据：既要严格递增（否则老客户端看不到这次发布），
    # 也必须保持 X.Y.Z 形态——旧客户端用 semver 正则解析它，解析不出来等于永远没有更新。
    if version_key(args.version) is None:
        raise SystemExit(
            f"检测版本 {args.version} 不是 X.Y.Z[-预发布] 形态："
            "旧客户端按 semver 解析它，日期时间版本只能用在 App/模块自己的版本线上。"
        )
    # 组件版本可以和检测版本不同空间（App 用 yyyyMMddHHmm），只比较同一空间的那些。
    comparable = [value for value in (app_version, module_version) if version_key(value) is not None]
    if comparable:
        highest_component = max_version(comparable)
        if compare_versions(args.version, highest_component) < 0:
            raise SystemExit(
                f"检测版本 {args.version} 低于组件版本 {highest_component}："
                "检测版本不得低于同一版本空间内的组件版本。"
            )
    previous_detected = str((previous or {}).get("version", "")).strip() if isinstance(previous, dict) else ""
    previous_tag = str((previous or {}).get("tag", "")).strip() if isinstance(previous, dict) else ""
    if previous_detected:
        comparison = compare_versions(args.version, previous_detected)
        if comparison < 0:
            raise SystemExit(
                f"检测版本 {args.version} 低于上一版 {previous_detected}："
                "检测版本一旦回退，装过新版的客户端就再也收不到后续更新。"
            )
        # 同一轮 CI 重跑会算出同一个检测版本与同一个 tag：这是幂等重写，不是新一轮发布。
        if comparison == 0 and previous_tag and previous_tag != args.tag:
            raise SystemExit(
                f"检测版本 {args.version} 与上一版 {previous_detected} 相同但 tag 不同"
                f"（{previous_tag} → {args.tag}）：旧客户端只比检测版本，"
                "第二次发布不会被任何旧客户端发现。"
            )

    apk_url = asset_url(args.repository, args.tag, args.apk_asset) if args.apk_asset else ""
    module_url = asset_url(args.repository, args.tag, args.module_asset) if args.module_asset else ""

    entry = {
        "version": args.version,
        "tag": args.tag,
        "title": args.title or f"Storage Redirect X {args.tag}",
        "url": args.release_url or release_url(args.repository, args.tag),
        "repository": args.repository,
        "prerelease": args.channel == "beta",
        "app": build_component(app_version, apk_url),
        "module": build_component(module_version, module_url),
        # 兼容字段：旧客户端只认 downloadUrl/moduleUrl，且只描述本次发布自带的构件。
        "downloadUrl": apk_url,
        "moduleUrl": module_url,
    }
    if args.release_notes_file:
        notes_path = Path(args.release_notes_file)
        notes = sanitize_release_notes(notes_path.read_text(encoding="utf-8"))
        if notes:
            entry["releaseNotes"] = notes
    return entry


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", default="update.json", help="manifest path")
    parser.add_argument("--repository", required=True, help="GitHub repository in owner/repo form")
    parser.add_argument("--channel", required=True, choices=("stable", "beta"), help="release channel to update")
    parser.add_argument("--version", required=True, help="检测版本：旧客户端读这个字段，必须单调递增")
    parser.add_argument("--tag", required=True, help="GitHub release tag")
    parser.add_argument("--title", default="", help="release title")
    parser.add_argument("--release-url", default="", help="override release page URL")
    parser.add_argument("--apk-asset", default="", help="manager APK asset name")
    parser.add_argument("--module-asset", default="", help="module ZIP asset name")
    parser.add_argument("--app-version", default="", help="本轮重编的 App 版本；缺省沿用上一版")
    parser.add_argument("--module-version", default="", help="本轮重编的模块版本；缺省沿用上一版")
    parser.add_argument("--release-notes-file", default="", help="Markdown release notes file")
    args = parser.parse_args()

    manifest_path = Path(args.manifest)
    manifest = load_manifest(manifest_path)
    previous = manifest.get(args.channel)

    entry = build_entry(args, previous)

    manifest["schema"] = SCHEMA_VERSION
    manifest["repository"] = args.repository
    manifest[args.channel] = entry
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
