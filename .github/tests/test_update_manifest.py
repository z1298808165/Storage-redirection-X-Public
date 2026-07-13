import importlib.util
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).parents[1] / "scripts" / "update_manifest.py"
SPEC = importlib.util.spec_from_file_location("update_manifest", SCRIPT_PATH)
UPDATE_MANIFEST = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(UPDATE_MANIFEST)


class UpdateManifestTest(unittest.TestCase):
    def test_sanitize_release_notes_drops_commit_details(self) -> None:
        markdown = """
## 模块更新
- 修复模块。

## App 更新
- 修复 App。

### 提交列表
- `abc1234` 修复

**完整变更对比**: https://github.com/example/repo/compare/a...b
"""

        sanitized = UPDATE_MANIFEST.sanitize_release_notes(markdown)

        self.assertIn("## 模块更新", sanitized)
        self.assertIn("## App 更新", sanitized)
        self.assertNotIn("提交列表", sanitized)
        self.assertNotIn("完整变更对比", sanitized)


if __name__ == "__main__":
    unittest.main()
