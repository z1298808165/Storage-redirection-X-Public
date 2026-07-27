import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT_PATH = Path(__file__).parents[1] / "scripts" / "resolve_build_version.py"
SPEC = importlib.util.spec_from_file_location("resolve_build_version", SCRIPT_PATH)
RESOLVER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(RESOLVER)


class ResolveBuildCountTest(unittest.TestCase):
    def resolve(self, baseline, manifest, include_dirty=False, dirty=False):
        with (
            mock.patch.object(RESOLVER, "read_build_count_baseline", return_value=baseline),
            mock.patch.object(RESOLVER, "published_manifest_build_count", return_value=manifest),
            mock.patch.object(RESOLVER, "is_worktree_dirty", return_value=dirty),
        ):
            return RESOLVER.resolve_build_count("1.2.59", include_dirty=include_dirty)

    def test_first_build_starts_at_one(self) -> None:
        self.assertEqual(self.resolve(baseline=None, manifest=None), 1)

    def test_next_build_increments_recorded_count(self) -> None:
        self.assertEqual(self.resolve(baseline=81, manifest=81), 82)

    def test_highest_recorded_count_wins(self) -> None:
        self.assertEqual(self.resolve(baseline=15, manifest=81), 82)
        self.assertEqual(self.resolve(baseline=81, manifest=15), 82)

    def test_commit_history_does_not_affect_count(self) -> None:
        with mock.patch.object(RESOLVER, "run_git") as run_git:
            self.assertEqual(self.resolve(baseline=81, manifest=None), 82)
        run_git.assert_not_called()

    def test_clean_worktree_reuses_recorded_count_locally(self) -> None:
        self.assertEqual(self.resolve(baseline=81, manifest=None, include_dirty=True, dirty=False), 81)

    def test_dirty_worktree_counts_as_next_build_locally(self) -> None:
        self.assertEqual(self.resolve(baseline=81, manifest=None, include_dirty=True, dirty=True), 82)

    def test_clean_worktree_without_record_starts_at_one(self) -> None:
        self.assertEqual(self.resolve(baseline=None, manifest=None, include_dirty=True, dirty=False), 1)


class BuildCountBaselineTest(unittest.TestCase):
    def test_record_version_keeps_highest_count(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            baseline_path = Path(directory) / "build-version-baseline.json"
            with mock.patch.object(RESOLVER, "BUILD_VERSION_BASELINE_PATH", baseline_path):
                RESOLVER.write_build_count_baseline("1.2.59", 82)
                RESOLVER.write_build_count_baseline("1.2.59", 80)
                data = json.loads(baseline_path.read_text(encoding="utf-8"))
                self.assertEqual(data["buildCounts"]["1.2.59"], 82)
                self.assertEqual(RESOLVER.read_build_count_baseline("1.2.59"), 82)


if __name__ == "__main__":
    unittest.main()
