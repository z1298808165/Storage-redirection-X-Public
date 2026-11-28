import importlib.util
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).parents[1] / "scripts" / "resolve_release_scope.py"
SPEC = importlib.util.spec_from_file_location("resolve_release_scope", SCRIPT_PATH)
RESOLVE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(RESOLVE)


class ChangeComponentTest(unittest.TestCase):
    def test_app_paths(self) -> None:
        for path in (
            "app/src/main/java/org/srx/manager/MainActivity.kt",
            "gradle/libs.versions.toml",
            "build.gradle.kts",
        ):
            self.assertEqual("app", RESOLVE.change_component(path), path)

    def test_module_paths(self) -> None:
        for path in (
            "src/daemon.rs",
            "native/CMakeLists.txt",
            "vendor/lsplant/src/hook.cc",
            "Cargo.toml",
            "assets/zygisk_module/service.sh",
        ):
            self.assertEqual("module", RESOLVE.change_component(path), path)

    def test_pipeline_and_docs_do_not_trigger_builds(self) -> None:
        for path in (
            ".github/workflows/ci.yml",
            "docs/build-process.md",
            "scripts/build-local-app.ps1",
            "README.md",
            "update.json",
        ):
            self.assertIn(RESOLVE.change_component(path), ("other", "ignore"), path)

    def test_unknown_path_falls_back_to_both(self) -> None:
        self.assertEqual("both", RESOLVE.resolve_scope(["something-new/file.xyz"]))


class ResolveScopeTest(unittest.TestCase):
    def test_single_component_changes(self) -> None:
        self.assertEqual("app", RESOLVE.resolve_scope(["app/foo.kt"]))
        self.assertEqual("module", RESOLVE.resolve_scope(["src/daemon.rs"]))

    def test_both_components_changed(self) -> None:
        self.assertEqual("both", RESOLVE.resolve_scope(["app/foo.kt", "src/daemon.rs"]))

    def test_docs_only_needs_no_build(self) -> None:
        self.assertEqual("none", RESOLVE.resolve_scope(["docs/a.md", ".github/workflows/ci.yml"]))

    def test_update_manifest_is_ignored(self) -> None:
        self.assertEqual("none", RESOLVE.resolve_scope(["update.json"]))

    def test_empty_diff_is_none(self) -> None:
        self.assertEqual("none", RESOLVE.resolve_scope([]))


class TrailerTest(unittest.TestCase):
    def test_trailer_overrides_paths(self) -> None:
        message = "修复(CI)：调整\n\n变更：x\nRelease-Scope: module\n"
        self.assertEqual("module", RESOLVE.parse_trailer(message))

    def test_invalid_trailer_is_ignored(self) -> None:
        self.assertIsNone(RESOLVE.parse_trailer("变更：x\nRelease-Scope: everything\n"))

    def test_missing_trailer(self) -> None:
        self.assertIsNone(RESOLVE.parse_trailer("修复：只改文档\n"))

    def test_scope_outputs_map_to_booleans(self) -> None:
        self.assertEqual(
            {"scope": "both", "app_changed": "true", "module_changed": "true"},
            RESOLVE.scope_outputs("both"),
        )
        self.assertEqual(
            {"scope": "app", "app_changed": "true", "module_changed": "false"},
            RESOLVE.scope_outputs("app"),
        )
        self.assertEqual(
            {"scope": "none", "app_changed": "false", "module_changed": "false"},
            RESOLVE.scope_outputs("none"),
        )


if __name__ == "__main__":
    unittest.main()
