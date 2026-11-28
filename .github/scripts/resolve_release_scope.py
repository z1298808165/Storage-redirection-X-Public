#!/usr/bin/env python3
"""判定一次发布到底改动了 App、模块，还是两者。

CI 用它决定跑哪些 job：只改 App 就不编模块、不跑场景；只改模块就不编 App。

判定输入有两个来源，优先级从高到低：

1. 提交正文里的 ``Release-Scope: app|module|both|none``（人工覆盖，由 validate-commit.ps1 校验取值）；
2. 与基线 ref 之间的改动路径。

判不出来的路径一律落 ``both``——多跑一轮构建只是浪费几分钟，漏跑会发布出与改动不一致的构件。
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

SCOPES = ("app", "module", "both", "none")

# 只影响 App 构件的路径。
APP_PREFIXES = ("app/", "gradle/")
APP_FILES = {
    "build.gradle.kts",
    "settings.gradle.kts",
    "gradle.properties",
    "gradle/libs.versions.toml",
}
# 只影响模块 zip 的路径（webroot 已随 WebUI 移除）。
MODULE_PREFIXES = ("src/", "native/", "vendor/", "java_src/", "assets/zygisk_module/")
MODULE_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "rust-toolchain.toml",
}
# 发布流程与文档：不改变任何构件，不需要构建。
OTHER_PREFIXES = (".github/", "docs/", "scripts/", "tests/", "temp/")
OTHER_FILES = {"README.md", "AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md", "COPYING", "LICENSE"}
IGNORED_FILES = {"update.json"}


def change_component(path: str) -> str:
    """单个路径归属：``app`` / ``module`` / ``other`` / ``ignore``。"""
    if path in IGNORED_FILES:
        return "ignore"
    if path in APP_FILES or path.startswith(APP_PREFIXES):
        return "app"
    if path in MODULE_FILES or path.startswith(MODULE_PREFIXES):
        return "module"
    if path in OTHER_FILES or path.startswith(OTHER_PREFIXES):
        return "other"
    # 判不出来就按「两边都可能受影响」处理。
    return "unknown"


def resolve_scope(paths: list[str]) -> str:
    components = {change_component(path) for path in paths}
    if "unknown" in components:
        return "both"
    touched = components & {"app", "module"}
    if touched == {"app"}:
        return "app"
    if touched == {"module"}:
        return "module"
    if touched == {"app", "module"}:
        return "both"
    return "none"


def parse_trailer(message: str) -> str | None:
    """从提交正文里取 ``Release-Scope:`` 覆盖值；没有或取值非法时返回 None。"""
    for line in message.splitlines():
        key, separator, value = line.partition(":")
        if not separator or key.strip().lower() != "release-scope":
            continue
        candidate = value.strip().lower()
        if candidate in SCOPES:
            return candidate
    return None


def changed_paths(base: str, head: str) -> list[str]:
    if base:
        args = ["diff", "--name-only", f"{base}..{head}"]
    else:
        # 没有基线（首次运行）时只能看单个提交，宁可判成 both。
        args = ["show", "--format=", "--name-only", head]
    result = subprocess.run(
        ["git", *args], check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def commit_message(ref: str) -> str:
    result = subprocess.run(
        ["git", "log", "-1", "--format=%B", ref],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return result.stdout


def scope_outputs(scope: str) -> dict[str, str]:
    return {
        "scope": scope,
        "app_changed": "true" if scope in ("app", "both") else "false",
        "module_changed": "true" if scope in ("module", "both") else "false",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="", help="基线 ref（上一版发布/CI 构建），留空则只看单个提交")
    parser.add_argument("--head", default="HEAD", help="当前 ref")
    parser.add_argument("--message", default="", help="提交正文；留空则从 git 读取 head 的正文")
    parser.add_argument(
        "--format", choices=("text", "github"), default="text", help="github 输出写入 $GITHUB_OUTPUT"
    )
    args = parser.parse_args()

    message = args.message or commit_message(args.head)
    override = parse_trailer(message)
    scope = override or resolve_scope(changed_paths(args.base, args.head))
    outputs = scope_outputs(scope)

    if args.format == "github":
        target = os.environ.get("GITHUB_OUTPUT")
        lines = "".join(f"{key}={value}\n" for key, value in outputs.items())
        if target:
            with Path(target).open("a", encoding="utf-8") as handle:
                handle.write(lines)
        else:
            sys.stdout.write(lines)
        return

    for key, value in outputs.items():
        sys.stdout.write(f"{key}={value}\n")


if __name__ == "__main__":
    main()
