#!/usr/bin/env python3
"""Resolve SRX build version names and Android version codes."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


BUILD_VERSION_BASELINE_PATH = Path(".github/build-version-baseline.json")


def run_git(args: list[str], check: bool = True) -> str:
    result = subprocess.run(
        ["git", *args],
        check=check,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return result.stdout.strip()


def read_cargo_version_from_text(text: str) -> str | None:
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', text)
    return match.group(1) if match else None


def read_current_cargo_version() -> str:
    version = read_cargo_version_from_text(Path("Cargo.toml").read_text(encoding="utf-8"))
    if not version:
        raise SystemExit("Unable to read package version from Cargo.toml")
    return version


def validate_base_version(version: str) -> tuple[int, int, int]:
    parts = version.split(".")
    if len(parts) != 3 or any(not part.isdigit() for part in parts):
        raise SystemExit(f"Cargo.toml version must be MAJOR.MINOR.PATCH, got: {version}")
    return int(parts[0]), int(parts[1]), int(parts[2])


def is_worktree_dirty() -> bool:
    return bool(run_git(["status", "--porcelain"], check=False))


def published_manifest_build_count(current_version: str) -> int | None:
    manifest_path = Path("update.json")
    if not manifest_path.exists():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    beta = manifest.get("beta")
    if not isinstance(beta, dict):
        return None
    version = beta.get("version")
    if not isinstance(version, str):
        return None
    match = re.fullmatch(re.escape(current_version) + r"-ci\.(\d+)", version)
    return int(match.group(1)) if match else None


def read_build_count_baseline(current_version: str) -> int | None:
    if not BUILD_VERSION_BASELINE_PATH.exists():
        return None
    try:
        baseline = json.loads(BUILD_VERSION_BASELINE_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    build_counts = baseline.get("buildCounts")
    if not isinstance(build_counts, dict):
        return None
    count = build_counts.get(current_version)
    if isinstance(count, int) and count > 0:
        return count
    if isinstance(count, str) and count.isdigit():
        return int(count)
    return None


def write_build_count_baseline(base_version: str, build_count: int) -> None:
    if build_count < 1:
        raise SystemExit(f"Build count must be positive, got: {build_count}")
    data: dict[str, object]
    if BUILD_VERSION_BASELINE_PATH.exists():
        try:
            loaded = json.loads(BUILD_VERSION_BASELINE_PATH.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            loaded = {}
        data = loaded if isinstance(loaded, dict) else {}
    else:
        data = {}

    build_counts = data.get("buildCounts")
    if not isinstance(build_counts, dict):
        build_counts = {}
    previous = build_counts.get(base_version)
    previous_count = previous if isinstance(previous, int) else 0
    if isinstance(previous, str) and previous.isdigit():
        previous_count = int(previous)
    build_counts[base_version] = max(previous_count, build_count)

    ordered_counts = {
        version: build_counts[version]
        for version in sorted(build_counts, key=lambda item: tuple(int(part) for part in item.split(".") if part.isdigit()))
    }
    data = {
        "schema": 1,
        "buildCounts": ordered_counts,
    }
    BUILD_VERSION_BASELINE_PATH.parent.mkdir(parents=True, exist_ok=True)
    BUILD_VERSION_BASELINE_PATH.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")


def parse_ci_version(version: str) -> tuple[str, int]:
    match = re.fullmatch(r"(\d+\.\d+\.\d+)-ci\.(\d+)", version)
    if not match:
        raise SystemExit(f"CI version must look like MAJOR.MINOR.PATCH-ci.N, got: {version}")
    return match.group(1), int(match.group(2))


def resolve_build_count(current_version: str, include_dirty: bool) -> int:
    """按构建次数解析下一个 CI 序号。

    序号只取决于该基础版本已经产出过多少个构建，与提交数量无关：
    以基线文件与已发布清单中记录的最高 `N` 为准，下一次构建加 1。
    同一个基础版本首次构建时两者都没有记录，从 1 开始。
    `include_dirty` 只影响本地：工作区干净说明当前提交对应的构建已经产出，
    直接复用记录中的最高序号，避免仅重新打包就推进版本；有未提交改动时才算作新构建。
    """
    recorded = [
        count
        for count in (
            read_build_count_baseline(current_version),
            published_manifest_build_count(current_version),
        )
        if count is not None
    ]
    highest = max(recorded) if recorded else 0

    if include_dirty and not is_worktree_dirty() and highest > 0:
        return highest

    return highest + 1


def version_code(base_version: str, build_count: int, release: bool) -> int:
    major, minor, patch = validate_base_version(base_version)
    base_code = major * 1_000_000 + minor * 10_000 + patch * 100
    if release:
        return base_code
    if build_count < 1:
        raise SystemExit(f"CI build count must be positive, got: {build_count}")
    return base_code - 100 + min(build_count, 99)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--include-dirty", action="store_true", help="count local uncommitted changes as the next build")
    parser.add_argument("--release", action="store_true", help="resolve release version without ci suffix")
    parser.add_argument("--record-version", help="record a resolved MAJOR.MINOR.PATCH-ci.N as the next build baseline")
    parser.add_argument("--format", choices=("json", "github"), default="json")
    args = parser.parse_args()

    if args.record_version:
        base_version, build_count = parse_ci_version(args.record_version)
        write_build_count_baseline(base_version, build_count)
        return

    base_version = read_current_cargo_version()
    build_count = resolve_build_count(base_version, include_dirty=args.include_dirty)
    resolved_version = base_version if args.release else f"{base_version}-ci.{build_count}"
    resolved_code = version_code(base_version, build_count, release=args.release)
    data = {
        "base_version": base_version,
        "build_count": build_count,
        "version": resolved_version,
        "version_code": resolved_code,
    }

    if args.format == "github":
        for key, value in data.items():
            print(f"{key}={value}")
    else:
        print(json.dumps(data, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
