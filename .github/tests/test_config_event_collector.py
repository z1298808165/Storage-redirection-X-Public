import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SOURCE = Path(__file__).resolve().parents[2] / "assets/zygisk_module/service.d/config_events.sh"


@unittest.skipUnless(shutil.which("bash"), "需要 Bash")
class ConfigEventCollectorTest(unittest.TestCase):
    def test_reconciles_app_config_after_global_event(self) -> None:
        for events, expected_refreshes in (
            (["global.json"], []),
            (["global.json", "com.example.global.json", "com.example.global.json.tmp"], ["force"]),
        ):
            with self.subTest(events=events), tempfile.TemporaryDirectory() as directory:
                result = subprocess.run(
                    ["bash", "-c", r'''
                        set -eu
                        . "$1"
                        LOGS_DIR=$2
                        CONFIG_DIR=$2/config
                        APPS_CONFIG_DIR=$CONFIG_DIR/apps
                        CONFIG_STATE_FILE=$2/state
                        CONFIG_EVENT_COLLECTOR_PID_FILE=$2/pid
                        shift 2
                        events=("$@")
                        inotifyd() {
                            for name in "${events[@]}"; do
                                case "$name" in
                                    global.json) watch=$3 ;;
                                    *) watch=$2 ;;
                                esac
                                case "${watch##*:}" in
                                    *y*) printf 'y\t%s\t%s\n' "${watch%:*}" "$name" ;;
                                esac
                            done
                        }
                        build_config_state_file() { : > "$1"; }
                        refresh_uid_map() { printf '%s\n' "$1"; }
                        sleep() { exit 0; }
                        start_config_event_collector
                        wait "$(cat "$CONFIG_EVENT_COLLECTOR_PID_FILE")"
                    ''', "test-config-events", str(SOURCE), directory, *events],
                    capture_output=True, text=True, timeout=10, check=True,
                )
                self.assertEqual(result.stdout.splitlines(), expected_refreshes)
