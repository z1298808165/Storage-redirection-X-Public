"""验证共享宿主接入子进程的失败出口会立即回包，保留死连接分类。"""

import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def extract_fn(source: str, name: str) -> str:
    start = source.index(f"fn {name}(")
    brace = source.index("{", start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class HostAttachResultTest(unittest.TestCase):
    def test_failure_exits_send_one_result(self) -> None:
        """执行生产代码，覆盖命名空间、克隆、挂载、复核和回包失败。"""
        rustc = shutil.which("rustc")
        if rustc is None:
            self.skipTest("环境缺少 rustc，执行验证已跳过")
        source = (ROOT / "src/fuse_host.rs").read_text(encoding="utf8")
        functions = "\n".join(
            extract_fn(source, name)
            for name in ("host_attach_child_main", "send_host_attach_result")
        )
        dispatch = re.search(
            r"let result = host_attach_child_main\(view, target_root\);\s*"
            r"let ok = send_host_attach_result\(ready_sockets\[1\], result\) && result == 0;",
            source,
        )
        self.assertIsNotNone(dispatch, "接入子进程必须使用统一结果回包")
        template = (
            ROOT / ".github/tests/harness/fuse_host_attach_result.rs"
        ).read_text(encoding="utf8")
        harness = template.replace("// __PRODUCTION_FUNCTIONS__", functions).replace(
            "// __PRODUCTION_DISPATCH__", dispatch.group(0)
        )
        temp_root = ROOT / "temp"
        temp_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix="host-attach-result-", dir=temp_root
        ) as tmp:
            path = Path(tmp) / "harness.rs"
            binary = Path(tmp) / ("result.exe" if os.name == "nt" else "result")
            path.write_text(harness, encoding="utf8")
            for command in (
                [rustc, "--edition=2024", str(path), "-o", str(binary)],
                [str(binary)],
            ):
                completed = subprocess.run(
                    command, capture_output=True, text=True, encoding="utf8"
                )
                self.assertEqual(
                    completed.returncode, 0, completed.stdout + completed.stderr
                )


if __name__ == "__main__":
    unittest.main()
