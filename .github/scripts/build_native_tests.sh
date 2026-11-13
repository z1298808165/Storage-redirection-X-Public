#!/usr/bin/env bash
set -euo pipefail

target="${1:?missing Android target}"
output="${2:?missing output directory}"
mkdir -p "$output"
cargo test -p srx_core --target "$target" --no-run --message-format=json > "$output/artifacts.jsonl"
cargo build -p zn-runtime-fixture --target "$target" --release --message-format=json >> "$output/artifacts.jsonl"
# 只复制本次 Cargo 明确报告的可执行文件，避免通配符选中缓存中的旧测试。
python3 - "$output" <<'PY'
import json
import shutil
import sys
from pathlib import Path

output = Path(sys.argv[1])
artifacts = {}
for line in (output / "artifacts.jsonl").read_text().splitlines():
    item = json.loads(line)
    if item.get("reason") != "compiler-artifact" or not item.get("executable"):
        continue
    name = item["target"]["name"]
    if item["profile"]["test"] and name in ("srx_core", "srx_daemon"):
        artifacts[name + "-tests"] = item["executable"]
    elif name == "zn-runtime-fixture":
        artifacts[name] = item["executable"]
if set(artifacts) != {"srx_core-tests", "srx_daemon-tests", "zn-runtime-fixture"}:
    raise SystemExit(f"Incomplete native test artifacts: {sorted(artifacts)}")
for name, source in artifacts.items():
    shutil.copyfile(source, output / name)
(output / "artifacts.jsonl").unlink()
PY
