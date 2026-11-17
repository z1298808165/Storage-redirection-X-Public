import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
CONFIG_EVENTS = REPO_ROOT / "assets/zygisk_module/service.d/config_events.sh"


class ConfigEventsMultiUserUidTest(unittest.TestCase):
    def test_config_change_expands_shared_uids_for_every_user(self):
        if os.name == "nt":
            self.skipTest("run this Bash integration test in Linux or WSL")

        bash = shutil.which("bash")
        if bash is None:
            self.skipTest("bash is required to source the Android shell functions")

        with tempfile.TemporaryDirectory(prefix="srx-config-events-") as temp_dir:
            temp_path = Path(temp_dir)
            uid_map = temp_path / "system_writer_uids.list"
            uid_map.write_text(
                "# package:uid\n"
                "com.example.changed:10123\n"
                "com.example.shared0:10123\n"
                "com.example.changed:1010123\n"
                "com.example.shared10:1010123\n",
                encoding="utf-8",
            )
            old_state = temp_path / "old.state"
            old_state.write_text(
                "com.example.changed|1|10|1\n"
                "com.example.shared0|1|10|1\n"
                "com.example.shared10|1|10|1\n",
                encoding="utf-8",
            )
            new_state = temp_path / "new.state"
            new_state.write_text(
                "com.example.changed|2|10|1\n"
                "com.example.shared0|1|10|1\n"
                "com.example.shared10|1|10|1\n",
                encoding="utf-8",
            )

            env = os.environ.copy()
            env.update(
                {
                    "CONFIG_EVENTS": str(CONFIG_EVENTS),
                    "SYSTEM_WRITER_UIDS_FILE": str(uid_map),
                    "LOGS_DIR": temp_dir,
                    "OLD_STATE": str(old_state),
                    "NEW_STATE": str(new_state),
                }
            )
            result = subprocess.run(
                [
                    bash,
                    "-c",
                    '. "$CONFIG_EVENTS"\n'
                    'log() { printf \'%s\\n\' "$*"; }\n'
                    'handle_config_changes "com.example.changed" "$OLD_STATE" "$NEW_STATE"',
                ],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=True,
            )

            queued = re.findall(r"pkg=([^ ]+)", result.stdout)
            self.assertEqual(
                queued,
                [
                    "com.example.changed",
                    "com.example.shared0",
                    "com.example.shared10",
                ],
            )


if __name__ == "__main__":
    unittest.main()
