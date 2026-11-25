"""验证扫描回调参数、文件描述符作用域和数据库行身份。"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from test_mount_metadata_scope import function_body


ROOT = Path(__file__).resolve().parents[2]


def method(source, signature):
    start = source.index(signature)
    return source[start:source.index("{", start)] + function_body(source, signature)


@unittest.skipUnless(shutil.which("javac") and shutil.which("java"), "requires JDK")
class MediaScanBindingTest(unittest.TestCase):
    def test_scanner_slots_scope_and_row_identity(self):
        folder = ROOT / "java_src/org/srx/hook"
        hooker = (folder / "Hooker.java").read_text(encoding="utf8")
        records = (folder / "MediaScanFiles.java").read_text(encoding="utf8")
        fixture = (Path(__file__).parent / "harness/MediaScanFixture.java").read_text(encoding="utf8")
        marker = "/* 注入生产方法 */"
        self.assertEqual(fixture.count(marker), 1, "扫描测试模板必须恰有一个生产方法插入点")
        methods = [
            method(hooker, "public Object mediaScanCallback("),
            method(hooker, "private static Object[] replaceArgument("),
            method(records, "static String backing("),
            method(records, "private String identity("),
            method(records, "private String verifiedBacking("),
        ]
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "MediaScanFixture.java"
            source.write_text(fixture.replace(marker, "\n".join(methods)), encoding="utf8")
            subprocess.run(["javac", "-d", directory, str(source)], check=True, timeout=45)
            subprocess.run(["java", "-ea", "-cp", directory, "MediaScanFixture"], check=True, timeout=30)


if __name__ == "__main__":
    unittest.main()
