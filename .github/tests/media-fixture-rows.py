"""按夹具路径和本轮应用的创建回执核对及清理 MediaStore 条目。"""

import argparse
import re
import shlex
import subprocess
import sys
from pathlib import Path


def created_uris(lines):
    created = re.compile(
        r"^PASS \[mediastore_create_(?:image|video|audio|file|download|image_relative_data|then_file_overwrite)\] "
        r"[^\r\n]*? \(\d+ms\) uri=(content://media/(?:external|external_primary)/"
        r"(?:audio/media|images/media|video/media|file|downloads)/[1-9][0-9]*)(?=, |$)"
    )
    return dict.fromkeys(match[1] for line in lines if (match := created.match(line)))


def device_shell(adb, args):
    result = subprocess.run(
        [*adb, "shell", shlex.join(args)], capture_output=True, text=True,
        encoding="utf-8", errors="replace", timeout=30,
    )
    if result.returncode or result.stderr.strip():
        raise RuntimeError(f"ADB 退出码 {result.returncode}：{result.stderr.strip()} {result.stdout.strip()}")
    return result.stdout.strip()


def fixture_rows(action, serial, results=None):
    adb = ["adb", "-s", serial] if serial else ["adb"]
    uid = device_shell(adb, ["id", "-u"])
    if uid not in ("0", "2000"):
        raise RuntimeError(f"无法确认 ADB 调用身份：uid={uid}")
    # MediaProvider 拒绝没有关联包名的 UID 0。adbd 已提权时也必须由 shell 调用。
    caller = ["su", "2000"] if uid == "0" else []

    def content(op, uri, *args):
        return device_shell(adb, [*caller, "content", op, "--user", "0", "--uri", uri, *args])

    def query(uri, *where):
        reply = content("query", uri, "--projection", "_id", *where)
        if reply == "No result found.":
            return []
        if not re.fullmatch(r"Row: \d+ _id=[1-9]\d*(?:\nRow: \d+ _id=[1-9]\d*)*", reply):
            raise ValueError(f"无法核对媒体索引 {uri}：{reply}")
        return [line.rsplit("=", 1)[1] for line in reply.splitlines()]

    def delete(uri, *where):
        reply = content("delete", uri, *where)
        if reply:
            raise ValueError(f"删除媒体条目 {uri} 失败：{reply}")

    errors = []
    if results is not None:
        for uri in created_uris(results.read_text(encoding="utf-8-sig").splitlines()):
            try:
                delete(uri)
                if query(uri):
                    raise ValueError(f"删除后仍有媒体条目：{uri}")
            except (OSError, subprocess.TimeoutExpired, RuntimeError, ValueError) as error:
                errors.append(str(error))
    collection = "content://media/external_primary/file"
    parent = "/storage/emulated/0/Pictures/SrtReadOnlyMedia"
    for path in (parent + "/srt_read_only_media.jpg", parent):
        where = ("--where", f"_data='{path}'")
        ids = query(collection, *where)
        if ids and action == "check":
            raise ValueError(f"只读图片夹具与原有索引冲突：{path}")
        for row_id in ids:
            delete(f"{collection}/{row_id}", *where)
        if ids and query(collection, *where):
            raise ValueError(f"删除后仍有夹具索引：{path}")
    if errors:
        raise RuntimeError("\n".join(errors))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check", "clean"])
    parser.add_argument("serial")
    parser.add_argument("results", type=Path, nargs="?")
    args = parser.parse_args()
    if args.action == "check" and args.results:
        parser.error("检查模式不接受待清理的创建回执")
    try:
        fixture_rows(args.action, args.serial, args.results)
    except (OSError, subprocess.TimeoutExpired, RuntimeError, ValueError) as error:
        sys.exit(str(error))
