import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("bash") and shutil.which("timeout"), "需要 bash 与 timeout")
class ZnFixtureReadinessTest(unittest.TestCase):
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
printf '%s' "$count" > "$SRX_READY_COUNT"
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
