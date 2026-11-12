"""解析测试环境的最新稳定版资产及上游公布的 SHA-256。"""

import json
import re
import subprocess
import sys


ASSETS = {
    "zygisk-next": ("LSPosed/ZygiskNext", r"Zygisk-Next-[0-9.]+-[0-9]+-[0-9a-f]+-release\.zip"),
    "kernelsu": ("tiann/KernelSU", r"KernelSU_v[0-9.]+_[0-9]+-release\.apk"),
}
for api, android in (("35", 15), ("36", 16), ("37.0", 17)):
    ASSETS[f"avdlite-{api}"] = (
        "PuerNya/AvdLite", rf"AvdLiteWithoutKsu-Android{android}-api{re.escape(api)}-4k\.zip",
    )


def select_asset(release: dict, name: str) -> dict[str, str]:
    repository, pattern = ASSETS[name]
    if not isinstance(release, dict) or release.get("draft") or release.get("prerelease"):
        raise ValueError("release must be published and stable")
    tag = release.get("tag_name", "")
    if not isinstance(tag, str) or not re.fullmatch(r"[A-Za-z0-9._-]+", tag):
        raise ValueError("release has an invalid tag")
    if name.startswith("avdlite-") and tag != "images":
        raise ValueError("AvdLite images must come from the images release")
    assets = release.get("assets")
    if not isinstance(assets, list):
        raise ValueError("release assets are missing")
    matches = [
        asset for asset in assets
        if isinstance(asset, dict)
        and isinstance(asset.get("name"), str)
        and re.fullmatch(pattern, asset["name"])
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one release asset, found {len(matches)}")
    asset = matches[0]
    digest = asset.get("digest")
    if not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("release asset does not publish a SHA-256 digest")
    name = asset["name"]
    url = f"https://github.com/{repository}/releases/download/{tag}/{name}"
    if asset.get("browser_download_url") != url:
        raise ValueError("asset URL does not match its GitHub release")
    return {"version": tag, "asset": name, "sha256": digest[7:], "url": url}


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ASSETS:
        print("usage: resolve_test_release.py " + "|".join(ASSETS), file=sys.stderr)
        return 2
    name = sys.argv[1]
    repository = ASSETS[name][0]
    release = "tags/images" if name.startswith("avdlite-") else "latest"
    try:
        result = subprocess.run(
            ["gh", "api", f"repos/{repository}/releases/{release}", "--header", "Cache-Control: no-cache"],
            check=True, capture_output=True, text=True, encoding="utf-8", timeout=30,
        )
        asset = select_asset(json.loads(result.stdout), name)
    except subprocess.CalledProcessError as error:
        print(error.stderr.strip() or "GitHub release lookup failed", file=sys.stderr)
        return 1
    except (OSError, subprocess.TimeoutExpired, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    for key, value in asset.items():
        print(f"{key}={value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
