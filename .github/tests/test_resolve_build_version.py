import importlib.util
import unittest
from pathlib import Path
from unittest import mock


SCRIPT_PATH = Path(__file__).parents[1] / "scripts" / "resolve_build_version.py"
SPEC = importlib.util.spec_from_file_location("resolve_build_version", SCRIPT_PATH)
RESOLVER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(RESOLVER)


class ResolveBuildVersionTest(unittest.TestCase):
    def test_version_start_commit_uses_one_history_query(self) -> None:
        history = """commit:first
+version = "1.2.58"
commit:format-only
-version = "1.2.58"
+version  =  "1.2.58"
commit:next
-version  =  "1.2.58"
+version = "1.2.59"
commit:current
-version = "1.2.59"
+version = "1.2.58"
"""

        with mock.patch.object(RESOLVER, "run_git", return_value=history) as run_git:
            self.assertEqual(RESOLVER.version_start_commit("1.2.58"), "current")

        run_git.assert_called_once()

    def test_count_non_auto_commits_reads_subjects_once(self) -> None:
        subjects = "\n".join(("修复：普通提交", "CI：更新更新清单 1.2.59-ci.1", "测试：补充验证"))

        with mock.patch.object(RESOLVER, "run_git", return_value=subjects) as run_git:
            self.assertEqual(RESOLVER.count_non_auto_commits("start..HEAD"), 2)

        run_git.assert_called_once_with(
            ["log", "--first-parent", "--format=%s", "start..HEAD"],
            check=False,
        )

    def test_latest_manifest_commit_reads_history_once(self) -> None:
        history = "newer\t修复：普通提交\nmanifest\tCI：更新更新清单 1.2.59-ci.8"

        with mock.patch.object(RESOLVER, "run_git", return_value=history) as run_git:
            self.assertEqual(RESOLVER.latest_ci_manifest_commit("1.2.59"), "manifest")

        run_git.assert_called_once()


if __name__ == "__main__":
    unittest.main()
