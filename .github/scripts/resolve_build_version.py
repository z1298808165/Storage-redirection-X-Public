#!/usr/bin/env python3
"""Resolve SRX build version names and Android version codes."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


BUILD_VERSION_BASELINE_PATH = Path(".github/build-version-baseline.json")
AUTO_MANIFEST_PREFIXES = (
    "CI：更新更新清单",
    "发布：更新更新清单",
)


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


def current_head_version() -> str | None:
    try:
        text = run_git(["show", "HEAD:Cargo.toml"])
    except subprocess.CalledProcessError:
        return None
    return read_cargo_version_from_text(text)


def version_start_commit(current_version: str) -> str | None:
    history = run_git(
        ["log", "--first-parent", "--reverse", "--format=commit:%H", "-p", "--", "Cargo.toml"],
        check=False,
    )
    commit: str | None = None
    start: str | None = None
    added_current_version = False
    removed_current_version = False

    def record_version_start() -> None:
        nonlocal start
        if added_current_version and not removed_current_version:
            start = commit

    for line in history.splitlines():
        if line.startswith("commit:"):
            record_version_start()
            commit = line.removeprefix("commit:").strip()
            added_current_version = False
            removed_current_version = False
            continue
        if line.startswith("+") and read_cargo_version_from_text(line.removeprefix("+")) == current_version:
            added_current_version = True
        elif line.startswith("-") and read_cargo_version_from_text(line.removeprefix("-")) == current_version:
            removed_current_version = True
    record_version_start()
    return start


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


def latest_ci_manifest_commit(current_version: str) -> str | None:
    commits_text = run_git(["log", "--first-parent", "--format=%H%x09%s", "HEAD"], check=False)
    for line in commits_text.splitlines():
        commit, _, subject = line.partition("\t")
        if subject.startswith(f"CI：更新更新清单 {current_version}-ci."):
            return commit
    return None


def count_non_auto_commits(range_expr: str) -> int:
    subjects_text = run_git(["log", "--first-parent", "--format=%s", range_expr], check=False)
    return sum(1 for subject in subjects_text.splitlines() if subject and not subject.startswith(AUTO_MANIFEST_PREFIXES))


def resolve_build_count(current_version: str, include_dirty: bool) -> int:
    head_version = current_head_version()
    start = None if head_version != current_version else version_start_commit(current_version)
    historical_count = 0
    if start:
        historical_count = count_non_auto_commits(f"{start}..HEAD")

    manifest_count = published_manifest_build_count(current_version)
    count = historical_count
    if manifest_count is not None:
        last_manifest_commit = latest_ci_manifest_commit(current_version)
        if last_manifest_commit:
            pending_count = count_non_auto_commits(f"{last_manifest_commit}..HEAD")
            count = manifest_count + pending_count

    if include_dirty and is_worktree_dirty():
        if head_version != current_version:
            count = 0
        count += 1

    resolved_count = max(count, 1)
    baseline_count = read_build_count_baseline(current_version)
    if baseline_count is not None:
        resolved_count = max(resolved_count, baseline_count + 1)

    return resolved_count


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
