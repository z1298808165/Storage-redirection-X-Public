import shutil
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("bash") and shutil.which("zip"), "需要 bash 与 zip")
class ZnPackagingTest(unittest.TestCase):
    def test_manifest_targets_the_packaged_abi_and_is_uncompressed_lf(self):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            binary = scratch / "binary"
            binary.write_bytes(b"fixture")
            for abi in ("arm64-v8a", "x86_64"):
                with self.subTest(abi=abi):
                    archive = scratch / f"{abi}.zip"
                    subprocess.run(
                        ["bash", ".github/scripts/package_module.sh", "2.3.0-ci.8", "23008",
                         str(binary), str(binary), str(archive), abi],
                        cwd=ROOT, check=True, stdout=subprocess.DEVNULL,
                    )
                    with zipfile.ZipFile(archive) as package:
                        manifest = package.read("zn_modules.txt")
                        self.assertEqual(manifest, f"path=/system_ext/bin/hyos_spawner companion zygisk/{abi}.so\n".encode())
                        self.assertIn(f"zygisk/{abi}.so", package.namelist())
                        self.assertEqual(package.getinfo("zn_modules.txt").compress_type, zipfile.ZIP_STORED)
