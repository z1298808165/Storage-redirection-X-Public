#!/usr/bin/env python3
"""Resolve SRX build version names and Android version codes."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


AUTO_MANIFEST_PREFIXES = (
    "CI：更新更新清单",
    "发布：更新更新清单",
)
BUILD_COUNT_OFFSETS = {
    # Previous CI builds for Cargo.toml 1.2.56 already published 1.2.57-ci.284.
    # Continue that visible version line without reserving Cargo.toml 1.2.58 early.
    "1.2.57": 285,
}
LEGACY_CI_VERSION_CODE_OVERRIDES = {
    "1.2.57",
}


def run_git(args: list[str], check: bool = True) -> str:
    result = subprocess.run(
        ["git", *args],
        check=check,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
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


def version_at_commit(commit: str) -> str | None:
    try:
        text = run_git(["show", f"{commit}:Cargo.toml"])
    except subprocess.CalledProcessError:
        return None
    return read_cargo_version_from_text(text)


def current_head_version() -> str | None:
    try:
        text = run_git(["show", "HEAD:Cargo.toml"])
    except subprocess.CalledProcessError:
        return None
    return read_cargo_version_from_text(text)


def version_start_commit(current_version: str) -> str | None:
    commits_text = run_git(["rev-list", "--first-parent", "--reverse", "HEAD", "--", "Cargo.toml"], check=False)
    commits = [line for line in commits_text.splitlines() if line]
    previous_version: str | None = None
    start: str | None = None
    for commit in commits:
        version = version_at_commit(commit)
        if version == current_version and previous_version != current_version:
            start = commit
        previous_version = version
    return start


def is_auto_manifest_commit(commit: str) -> bool:
    subject = run_git(["log", "-1", "--pretty=%s", commit])
    return subject.startswith(AUTO_MANIFEST_PREFIXES)


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


def latest_ci_manifest_commit(current_version: str) -> str | None:
    commits_text = run_git(["rev-list", "--first-parent", "HEAD"], check=False)
    for commit in [line for line in commits_text.splitlines() if line]:
        subject = run_git(["log", "-1", "--pretty=%s", commit])
        if subject.startswith(f"CI：更新更新清单 {current_version}-ci."):
            return commit
    return None


def count_non_auto_commits(range_expr: str) -> int:
    commits_text = run_git(["rev-list", "--first-parent", "--reverse", range_expr], check=False)
    commits = [line for line in commits_text.splitlines() if line]
    return sum(1 for commit in commits if not is_auto_manifest_commit(commit))


def resolve_build_count(current_version: str, include_dirty: bool) -> int:
    head_version = current_head_version()
    start = None if head_version != current_version else version_start_commit(current_version)
    offset = BUILD_COUNT_OFFSETS.get(current_version, 0)
    historical_count = 0
    if start:
        historical_count = count_non_auto_commits(f"{start}..HEAD")

    manifest_count = published_manifest_build_count(current_version)
    count = historical_count
    if manifest_count is not None:
        last_manifest_commit = latest_ci_manifest_commit(current_version)
        if last_manifest_commit:
            pending_count = count_non_auto_commits(f"{last_manifest_commit}..HEAD")
            count = max(manifest_count - offset, 0) + pending_count

    if include_dirty and is_worktree_dirty():
        if head_version != current_version:
            count = 0
        count += 1

    return max(count, 1) + offset


def version_code(base_version: str, build_count: int, release: bool) -> int:
    major, minor, patch = validate_base_version(base_version)
    base_code = major * 1_000_000 + minor * 10_000 + patch * 100
    if release:
        return base_code
    if base_version in LEGACY_CI_VERSION_CODE_OVERRIDES:
        return base_code - 1
    if build_count < 1 or build_count > 99:
        raise SystemExit("CI build count must be between 1 and 99. Bump Cargo.toml version before continuing.")
    return base_code - 100 + build_count


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--include-dirty", action="store_true", help="count local uncommitted changes as the next build")
    parser.add_argument("--release", action="store_true", help="resolve release version without ci suffix")
    parser.add_argument("--format", choices=("json", "github"), default="json")
    args = parser.parse_args()

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
