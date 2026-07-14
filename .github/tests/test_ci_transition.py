import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS_DIR = Path(__file__).parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))
SCRIPT_PATH = SCRIPTS_DIR / "validate_ci_transition.py"
SPEC = importlib.util.spec_from_file_location("validate_ci_transition", SCRIPT_PATH)
CI_TRANSITION = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = CI_TRANSITION
SPEC.loader.exec_module(CI_TRANSITION)


class CiTransitionTest(unittest.TestCase):
    def test_rejects_released_version_but_allows_large_ci_count(self) -> None:
        self.assertTrue(CI_TRANSITION.transition_errors("1.2.57", "1.2.57", 20))
        self.assertEqual(CI_TRANSITION.transition_errors("1.2.58", "1.2.57", 100), [])
        self.assertEqual(CI_TRANSITION.transition_errors("1.2.58", "1.2.57", 1), [])

    def test_large_ci_count_keeps_formal_version_code_higher(self) -> None:
        resolver = CI_TRANSITION.resolve_build_version
        self.assertEqual(resolver.version_code("1.2.58", 99, release=False), 1_025_799)
        self.assertEqual(resolver.version_code("1.2.58", 100, release=False), 1_025_799)
        self.assertEqual(resolver.version_code("1.2.58", 101, release=False), 1_025_799)
        self.assertEqual(resolver.version_code("1.2.58", 101, release=True), 1_025_800)

    def test_detects_commits_that_do_not_trigger_ci(self) -> None:
        self.assertTrue(CI_TRANSITION.should_skip_ci("维护：更新规则 [skip ci]"))
        self.assertTrue(CI_TRANSITION.should_skip_ci("Releases: 发布 1.2.58"))
        self.assertFalse(CI_TRANSITION.should_skip_ci("修复：稳定保存链路"))

    def test_reads_stable_manifest_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "update.json"
            path.write_text(
                json.dumps({"stable": {"version": "1.2.57"}}),
                encoding="utf-8",
            )
            self.assertEqual(CI_TRANSITION.stable_manifest_version(path), "1.2.57")


if __name__ == "__main__":
    unittest.main()
