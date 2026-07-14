#!/usr/bin/env python3
"""Validate the version transition before starting a normal CI build."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

import resolve_build_version


SKIP_CI_PATTERN = re.compile(r"\[(?:skip ci|ci skip|no ci)\]", re.I)
WORKFLOW_SKIP_PREFIXES = (
    "Releases:",
    "CI：更新更新清单",
    "发布：更新更新清单",
)


def semantic_version(version: str) -> tuple[int, int, int]:
    return resolve_build_version.validate_base_version(version)


def stable_manifest_version(path: Path = Path("update.json")) -> str | None:
    if not path.exists():
        return None
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    stable = manifest.get("stable")
    if not isinstance(stable, dict):
        return None
    version = stable.get("version")
    return version if isinstance(version, str) else None


def should_skip_ci(message: str) -> bool:
    subject = message.splitlines()[0].strip() if message.splitlines() else ""
    return bool(SKIP_CI_PATTERN.search(message)) or subject.startswith(WORKFLOW_SKIP_PREFIXES)


def transition_errors(base_version: str, stable_version: str | None, build_count: int) -> list[str]:
    errors: list[str] = []
    if stable_version and semantic_version(base_version) <= semantic_version(stable_version):
        errors.append(
            f"Cargo.toml version {base_version} is not newer than released version {stable_version}; "
            "bump the target patch version before the next CI build"
        )
    if build_count < 1:
        errors.append(f"CI build count must be positive, got: {build_count}")
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("json", "github"), default="json")
    args = parser.parse_args()

    message = resolve_build_version.run_git(["log", "-1", "--pretty=%B"], check=False)
    if should_skip_ci(message):
        print("CI version transition check skipped because the target commit does not trigger CI.")
        return

    base_version = resolve_build_version.read_current_cargo_version()
    build_count = resolve_build_version.resolve_build_count(base_version, include_dirty=False)
    stable_version = stable_manifest_version()
    errors = transition_errors(base_version, stable_version, build_count)
    if errors:
        raise SystemExit("CI version transition validation failed:\n- " + "\n- ".join(errors))

    resolved_version = f"{base_version}-ci.{build_count}"
    resolved_code = resolve_build_version.version_code(base_version, build_count, release=False)
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
