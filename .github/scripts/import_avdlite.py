"""核对 AvdLite 来源与启动报告，再用资产内的导入器创建本轮 AVD。"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


def validate_bundle(archive: zipfile.ZipFile, api: str) -> None:
    manifest = json.loads(archive.read("manifest.json"))
    image = manifest["image"]
    if manifest["ksu"] is not None or image["api"] != api or image["page_size"] != "4k" or image["abi"] != "x86_64":
        raise ValueError("expected a matching AvdLite x86_64 4K image without KernelSU")
    if json.loads(archive.read("verification.json")).get("status") != "passed":
        raise ValueError("AvdLite image has no passed boot verification")
    for name in archive.namelist():
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or "\\" in name:
            raise ValueError(f"unsafe bundle entry: {name}")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="avdlite-", dir=os.environ["RUNNER_TEMP"]) as directory:
        bundle = Path(directory)
        with zipfile.ZipFile(os.environ["AVDLITE_CACHE_ZIP"]) as archive:
            validate_bundle(archive, os.environ["ANDROID_API_LEVEL"])
            archive.extractall(bundle)
        avd_home = Path.home() / ".android/avd"
        avd_home.mkdir(parents=True, exist_ok=True)
        name = os.environ["AVD_NAME"]
        subprocess.run([
            sys.executable, str(bundle / "import-avd.py"), "--sdk", os.environ["ANDROID_HOME"],
            "--avd-home", str(avd_home), "--name", name,
        ], check=True)
        with (avd_home / f"{name}.avd/config.ini").open("a", encoding="utf-8") as config:
            config.write("disk.dataPartition.size=4G\nvm.heapSize=1024\nhw.keyboard=yes\n")
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
            output.write(f"ANDROID_AVD_HOME={avd_home}\n")


if __name__ == "__main__":
    main()
