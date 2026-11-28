import argparse
import importlib.util
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).parents[1] / "scripts" / "update_manifest.py"
SPEC = importlib.util.spec_from_file_location("update_manifest", SCRIPT_PATH)
UPDATE_MANIFEST = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(UPDATE_MANIFEST)

REPOSITORY = "example/repo"
TAG = "ci-build-1"


def make_args(**overrides: object) -> argparse.Namespace:
    values = {
        "repository": REPOSITORY,
        "channel": "beta",
        "version": "2.3.2-ci.8",
        "tag": TAG,
        "title": "",
        "release_url": "",
        "apk_asset": "",
        "module_asset": "",
        "app_version": "",
        "module_version": "",
        "release_notes_file": "",
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def previous_entry(**overrides: object) -> dict:
    entry = {
        "version": "2.3.2-ci.7",
        "tag": "ci-build-0",
        "app": {"version": "2.3.2-ci.7", "url": "https://example/apk"},
        "module": {"version": "2.3.2-ci.7", "url": "https://example/zip"},
        "downloadUrl": "https://example/apk",
        "moduleUrl": "https://example/zip",
    }
    entry.update(overrides)
    return entry


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

    def test_pre_release_versions_order_numerically(self) -> None:
        ordered = ["2.3.2-ci.9", "2.3.2-ci.10", "2.3.2", "2.4.0-ci.1"]
        for earlier, later in zip(ordered, ordered[1:]):
            self.assertLess(
                UPDATE_MANIFEST.compare_versions(earlier, later),
                0,
                f"{earlier} 应当小于 {later}",
            )
        self.assertEqual(0, UPDATE_MANIFEST.compare_versions("2.3.2", "2.3.2"))

    def test_app_only_release_keeps_module_version(self) -> None:
        entry = UPDATE_MANIFEST.build_entry(
            make_args(
                apk_asset="storage-redirect-x-manager-v2.3.2-ci.8.apk",
                app_version="2.3.2-ci.8",
            ),
            previous_entry(),
        )

        self.assertEqual("2.3.2-ci.8", entry["version"])
        self.assertEqual("2.3.2-ci.8", entry["app"]["version"])
        self.assertEqual("2.3.2-ci.7", entry["module"]["version"])
        self.assertNotIn("url", entry["module"])
        self.assertEqual("", entry["moduleUrl"])
        self.assertTrue(entry["downloadUrl"].endswith("storage-redirect-x-manager-v2.3.2-ci.8.apk"))

    def test_module_only_release_keeps_app_version(self) -> None:
        entry = UPDATE_MANIFEST.build_entry(
            make_args(
                apk_asset="",
                module_asset="storage.redirect.x-v2.3.2-ci.8.zip",
                module_version="2.3.2-ci.8",
            ),
            previous_entry(),
        )

        self.assertEqual("2.3.2-ci.8", entry["version"])
        self.assertEqual("2.3.2-ci.7", entry["app"]["version"])
        self.assertEqual("2.3.2-ci.8", entry["module"]["version"])
        self.assertEqual("", entry["downloadUrl"])
        self.assertTrue(entry["moduleUrl"].endswith("storage.redirect.x-v2.3.2-ci.8.zip"))

    def test_v1_entry_is_used_as_component_baseline(self) -> None:
        legacy_previous = {
            "version": "2.3.2-ci.7",
            "downloadUrl": "https://example/apk",
            "moduleUrl": "https://example/zip",
        }

        entry = UPDATE_MANIFEST.build_entry(
            make_args(apk_asset="manager.apk", app_version="2.3.2-ci.8"),
            legacy_previous,
        )

        self.assertEqual("2.3.2-ci.7", entry["module"]["version"])

    def test_both_components_rebuilt_share_one_version(self) -> None:
        entry = UPDATE_MANIFEST.build_entry(
            make_args(
                apk_asset="manager.apk",
                module_asset="module.zip",
                app_version="2.3.2-ci.8",
                module_version="2.3.2-ci.8",
            ),
            previous_entry(),
        )

        self.assertEqual(entry["app"]["version"], entry["module"]["version"])
        self.assertEqual("2.3.2-ci.8", entry["version"])
        self.assertTrue(entry["downloadUrl"])
        self.assertTrue(entry["moduleUrl"])

    def test_detected_version_must_be_higher_than_previous(self) -> None:
        with self.assertRaises(SystemExit):
            UPDATE_MANIFEST.build_entry(
                make_args(version="2.3.2-ci.7", app_version="2.3.2-ci.7"),
                previous_entry(),
            )

    def test_v2_entry_missing_module_does_not_carry_detection_version(self) -> None:
        # 条目带了组件字段却缺模块侧：说明当时没有模块信息，不能把检测版本当模块版本沿用。
        previous = previous_entry()
        previous.pop("module")
        entry = UPDATE_MANIFEST.build_entry(make_args(app_version="20261128081516"), previous)

        self.assertEqual("20261128081516", entry["app"]["version"])
        self.assertEqual("", entry["module"]["version"])

    def test_same_tag_rerun_rewrites_the_same_entry(self) -> None:
        # 同一轮 CI 重跑：检测版本与 tag 都不变，属于幂等重写而不是新一轮发布。
        entry = UPDATE_MANIFEST.build_entry(
            make_args(version="2.3.2-ci.7", tag="ci-build-0"),
            previous_entry(),
        )

        self.assertEqual("2.3.2-ci.7", entry["version"])
        self.assertEqual("ci-build-0", entry["tag"])

    def test_same_version_with_new_tag_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            UPDATE_MANIFEST.build_entry(
                make_args(version="2.3.2-ci.7", tag="ci-build-1"),
                previous_entry(),
            )

    def test_components_run_on_independent_version_lines(self) -> None:
        entry = UPDATE_MANIFEST.build_entry(
            make_args(
                version="3.0.0",
                apk_asset="manager.apk",
                app_version="3.0.0",
            ),
            previous_entry(),
        )

        self.assertEqual("3.0.0", entry["app"]["version"])
        self.assertEqual("2.3.2-ci.7", entry["module"]["version"])
        self.assertEqual("3.0.0", entry["version"])

    def test_app_version_can_use_datetime_line(self) -> None:
        entry = UPDATE_MANIFEST.build_entry(
            make_args(
                version="2.3.2-ci.8",
                apk_asset="manager.apk",
                app_version="202611281259",
            ),
            previous_entry(),
        )

        self.assertEqual("202611281259", entry["app"]["version"])
        self.assertEqual("2.3.2-ci.7", entry["module"]["version"])
        # 检测版本仍留在 semver 空间，旧客户端才解析得出来。
        self.assertEqual("2.3.2-ci.8", entry["version"])

    def test_datetime_detected_version_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            UPDATE_MANIFEST.build_entry(
                make_args(version="202611281259", app_version="202611281259"),
                previous_entry(),
            )

    def test_detected_version_below_component_version_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            UPDATE_MANIFEST.build_entry(
                make_args(version="2.3.2-ci.8", module_version="2.3.2-ci.9"),
                previous_entry(),
            )


if __name__ == "__main__":
    unittest.main()
