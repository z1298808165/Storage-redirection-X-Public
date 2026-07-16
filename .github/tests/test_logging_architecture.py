import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


class LoggingArchitectureTest(unittest.TestCase):
    def test_diagnostic_logcat_snapshot_precedes_slow_collection(self) -> None:
        script = read("assets/zygisk_module/service.d/diagnostic_archive.sh")
        main_flow = script.index('update_progress 1 init "正在准备日志包"')
        initial_capture = script.index("collect_initial_logcat_state", main_flow)
        slow_collection = script.index("collect_basic_files", main_flow)
        self.assertLess(initial_capture, slow_collection)
        self.assertIn("-t 10000", script)
        self.assertIn("-t 8000", script)
        self.assertIn("-b crash -d -v threadtime", script)
        self.assertIn('-T "$LOGCAT_CAPTURE_START"', script)
        self.assertIn("tail -n 3000", script)

    def test_legacy_exporters_use_the_same_bounded_windows(self) -> None:
        for path in (
            "app/src/main/java/org/srx/manager/data/RootFileStore.kt",
            "assets/zygisk_module/webroot/js/api.js",
        ):
            source = read(path)
            self.assertLess(source.index("-t 10000"), source.index("cp -p"))
            self.assertIn("-t 8000", source)
            self.assertIn("logcat-buffers.txt", source)
            self.assertIn("logcat-capture.txt", source)
            self.assertIn("tail -n 3000", source)

    def test_default_collectors_do_not_subscribe_to_native_hot_tags(self) -> None:
        collectors = read("assets/zygisk_module/service.d/log_collectors.sh") + read(
            "assets/zygisk_module/service.d/debug_collectors.sh"
        )
        self.assertNotIn("FileMonitorOp:I", collectors)
        self.assertNotIn("Stats:I", collectors)
        self.assertNotIn("StorageRedirect:V", collectors)
        self.assertEqual(collectors.count("logcat -T 1"), 1)

    def test_private_writer_owns_monitor_and_stats(self) -> None:
        daemon = read("src/log_daemon.rs")
        logging = read("src/logging.rs")
        companion_stats = read("src/lifecycle/companion_mount/stats.rs")
        control = read("assets/zygisk_module/bin/srxctl")
        self.assertIn('b"storage.redirect.x.logd"', daemon)
        self.assertIn('b"storage.redirect.x.logd"', logging)
        self.assertIn('"Stats", "+1"', companion_stats)
        self.assertNotIn("O_TRUNC", companion_stats)
        self.assertIn("control clear-monitor", control)

    def test_private_log_socket_allows_supported_root_domains(self) -> None:
        policy = read("assets/zygisk_module/sepolicy.rule")
        senders = (
            "zygote",
            "appdomain",
            "mediaprovider",
            "mediaprovider_app",
            "system_server",
        )
        for target in ("magisk", "su", "ksu"):
            for sender in senders:
                self.assertIn(
                    f"allow {sender} {target} unix_dgram_socket sendto", policy
                )
            self.assertIn(
                f"allow {target} {target} unix_dgram_socket sendto", policy
            )


if __name__ == "__main__":
    unittest.main()
