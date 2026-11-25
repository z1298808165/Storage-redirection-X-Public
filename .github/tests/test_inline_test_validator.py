import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class InlineTestValidatorTest(unittest.TestCase):
    def test_staged_and_revision_checks_cover_build_script_but_not_external_tests(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            env = {**os.environ, "GIT_INDEX_FILE": str(Path(tmp) / "index"),
                   "GIT_OBJECT_DIRECTORY": tmp,
                   "GIT_ALTERNATE_OBJECT_DIRECTORIES": subprocess.check_output(
                       ["git", "rev-parse", "--path-format=absolute", "--git-path", "objects"], cwd=ROOT, text=True).strip()}

            def git(*args, data=None):
                return subprocess.run(["git", *args], cwd=ROOT, env=env, text=True, input=data,
                                      capture_output=True, check=True).stdout.strip()

            git("read-tree", "HEAD")
            probe = git("hash-object", "-w", "--stdin", data="#[test]\nfn validator_probe() {}\n")
            for path in ("build.rs", "tests/build-script/external.rs"):
                git("update-index", "--add", "--cacheinfo", f"100644,{probe},{path}")
            for args in ([], ["-Ref", git("write-tree")]):
                result = subprocess.run(["pwsh", "-NoProfile", "-File", "scripts/validate-inline-tests.ps1", *args],
                                        cwd=ROOT, env=env, text=True, capture_output=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("build.rs::validator_probe", result.stderr)
                self.assertNotIn("external.rs::validator_probe", result.stderr)


if __name__ == "__main__":
    unittest.main()
