import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("bash") and shutil.which("timeout"), "需要 bash 与 timeout")
class ZnFixtureReadinessTest(unittest.TestCase):
    def test_case_failure_keeps_diagnostics_and_failure_count(self):
        source = (ROOT / ".github/tests/run-zn-runtime-fixture.sh").read_text(encoding="utf-8")
        function = source[source.index("run_case() {"):source.index("run_case passthrough")]
        script = r'''
set -eu
fixture=fixture
library=/module.so
uid=10192
module=/module
source_dir=/source
passed=0
failed=0
fixture() { echo 'zn fixture open: Permission denied' >&2; return 122; }
timeout() { test "$1" = 5; shift; "$@"; }
grep() { return 1; }
ls() { echo labels >&2; return 1; }
tail() { echo mount_decisions >&2; return 1; }
dmesg() { echo avc_denial >&2; return 1; }
''' + function + '\nrun_case monitor_only 0 private test.app /request /evidence\nprintf "passed=%s failed=%s\\n" "$passed" "$failed"\n'
        result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=5)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertEqual("passed=0 failed=1\n", result.stdout)
        for record in ("Permission denied", "FAIL [zn_monitor_only]", "labels", "mount_decisions", "avc_denial"):
            self.assertIn(record, result.stderr)

    def test_user_zero_fixture_ignores_other_installed_users(self):
        source = (ROOT / ".github/tests/run-zn-runtime-fixture.sh").read_text(encoding="utf-8")
        query = next(line for line in source.splitlines() if line.startswith('uid="$(cmd package'))
        script = '''set -eu
app=me.fakerqu.test.storageredirect
cmd() {
  if [ "$*" = "package list packages --user 0 -U $app" ]; then
    printf 'package:%s uid:10192\\n' "$app"
  else
    printf 'package:%s uid:10192,1010192\\n' "$app"
  fi
}
'''
        result = subprocess.run(["bash", "-c", script + query + '\nprintf "%s" "$uid"'], capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "10192")
        for name in ("run-storage-redirect-scenarios.ps1", "run-storage-redirect-scenarios.sh"):
            source = (ROOT / ".github/tests" / name).read_text(encoding="utf-8")
            self.assertIn("cmd package list packages --user 0 -U", source)
            self.assertNotIn("cmd package list packages -U", source)

    def test_provider_must_answer_before_the_fixture_starts(self):
        source = (ROOT / ".github/tests/run-zn-runtime-fixture.sh").read_text(encoding="utf-8")
        start = source.index("wait_for_media_provider() {")
        function = source[start:source.index("\n}", start) + 2]
        for attempts, query_fails, expected in (
            (1, False, 0),
            (3, False, 0),
            (3, True, 0),
            (10000, False, 124),
        ):
            with self.subTest(attempts=attempts, query_fails=query_fails), tempfile.TemporaryDirectory() as directory:
                scratch = Path(directory)
                scripts = {
                    "timeout": (
                        "#!/bin/sh\nshift\n"
                        f'exec "{shutil.which("timeout")}" 0.3 "$@"\n'
                    ),
                    "sleep": '#!/bin/sh\nexec /bin/sleep 0.01\n',
                    "content": """#!/bin/sh
test "$1" = query && test "$2" = --uri && test "$3" = content://media/external_primary/file || exit 2
count=$(cat "$SRX_READY_COUNT")
count=$((count + 1))
printf '%s' "$count" > "$SRX_READY_COUNT.tmp" && mv "$SRX_READY_COUNT.tmp" "$SRX_READY_COUNT"
if test "$count" -ge "$SRX_READY_ATTEMPTS"; then
  printf '%s\n' 'No result found'
else
  printf '%s\n' 'IllegalArgumentException: provider is starting'
  test "$SRX_READY_QUERY_FAILS" = 0
fi
""",
                }
                for name, text in scripts.items():
                    path = scratch / name
                    path.write_text(text, encoding="utf-8")
                    path.chmod(0o755)
                counter = scratch / "count"
                counter.write_text("0", encoding="utf-8")
                env = dict(
                    os.environ,
                    PATH=f"{scratch}{os.pathsep}{os.environ['PATH']}",
                    SRX_READY_COUNT=str(counter),
                    SRX_READY_ATTEMPTS=str(attempts),
                    SRX_READY_QUERY_FAILS=str(int(query_fails)),
                )
                result = subprocess.run(
                    ["bash", "-c", function + "\nwait_for_media_provider"],
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                self.assertEqual(result.returncode, expected, result.stderr)
                count = int(counter.read_text())
                if expected == 0:
                    self.assertEqual(count, attempts)
                else:
                    self.assertGreater(count, 0)
                    self.assertLess(count, attempts)


if __name__ == "__main__":
    unittest.main()
