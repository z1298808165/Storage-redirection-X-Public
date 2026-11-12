"""查询 Linux x86_64 SDK 的稳定版 revision，或核对已安装包的版本。"""

import re
import sys
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path


def latest_revision(xml: bytes, package: str) -> str:
    revisions = []
    for entry in ET.fromstring(xml).findall("remotePackage"):
        channel = entry.find("channelRef")
        revision = entry.find("revision")
        if (entry.get("path") != package or entry.get("obsolete") == "true"
                or channel is None or channel.get("ref") != "channel-0"
                or revision is None or revision.find("preview") is not None):
            continue
        if not any(archive.findtext("host-os") in (None, "linux")
                   and archive.findtext("host-arch") in (None, "x64", "x86_64")
                   for archive in entry.findall("archives/archive")):
            continue
        revisions.append(tuple(int(revision.findtext(part, "0")) for part in ("major", "minor", "micro")))
    if not revisions:
        raise ValueError(f"no stable revision found for {package}")
    return ".".join(map(str, max(revisions)))


def verify_revision(properties: Path, expected: str) -> None:
    match = re.search(r"(?m)^Pkg\.Revision\s*=\s*([0-9]+(?:\.[0-9]+){0,2})\s*$", properties.read_text())
    if match is None:
        raise ValueError(f"missing SDK revision: {properties}")
    actual = match[1].split(".")
    actual += ["0"] * (3 - len(actual))
    if ".".join(actual) != expected:
        raise ValueError(f"SDK revision changed during setup: expected {expected}, got {match[1]}")


def main() -> int:
    try:
        if len(sys.argv) == 4 and sys.argv[1] == "verify":
            verify_revision(Path(sys.argv[2]), sys.argv[3])
        elif len(sys.argv) == 1:
            request = urllib.request.Request(
                "https://dl.google.com/android/repository/repository2-3.xml", headers={"Cache-Control": "no-cache"},
            )
            with urllib.request.urlopen(request, timeout=30) as response:
                revision = latest_revision(response.read(), "emulator")
            print(f"emulator={revision}")
        else:
            raise ValueError("usage: android_sdk_revision.py [verify SOURCE_PROPERTIES REVISION]")
    except (OSError, ValueError, ET.ParseError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
