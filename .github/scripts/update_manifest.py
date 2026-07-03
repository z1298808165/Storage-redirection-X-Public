#!/usr/bin/env python3
"""Update the static app update manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def release_url(repository: str, tag: str) -> str:
    return f"https://github.com/{repository}/releases/tag/{tag}"


def asset_url(repository: str, tag: str, asset_name: str) -> str:
    return f"https://github.com/{repository}/releases/download/{tag}/{asset_name}"


def load_manifest(path: Path) -> dict:
    if not path.exists():
        return {"schema": 1, "repository": "", "stable": None, "beta": None, "releases": []}
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", default="update.json", help="manifest path")
    parser.add_argument("--repository", required=True, help="GitHub repository in owner/repo form")
    parser.add_argument("--channel", required=True, choices=("stable", "beta"), help="release channel to update")
    parser.add_argument("--version", required=True, help="semantic version without leading v")
    parser.add_argument("--tag", required=True, help="GitHub release tag")
    parser.add_argument("--title", default="", help="release title")
    parser.add_argument("--release-url", default="", help="override release page URL")
    parser.add_argument("--apk-asset", default="", help="manager APK asset name")
    parser.add_argument("--module-asset", default="", help="module ZIP asset name")
    args = parser.parse_args()

    manifest_path = Path(args.manifest)
    manifest = load_manifest(manifest_path)
    manifest["schema"] = 1
    manifest["repository"] = args.repository

    entry = {
        "version": args.version,
        "tag": args.tag,
        "title": args.title or f"Storage Redirect X {args.tag}",
        "url": args.release_url or release_url(args.repository, args.tag),
        "repository": args.repository,
        "prerelease": args.channel == "beta",
    }
    if args.apk_asset:
        entry["downloadUrl"] = asset_url(args.repository, args.tag, args.apk_asset)
    if args.module_asset:
        entry["moduleUrl"] = asset_url(args.repository, args.tag, args.module_asset)

    manifest[args.channel] = entry
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
