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

    def test_diagnostic_control_rejects_unsafe_paths_without_legacy_fallback(self) -> None:
        control = read("assets/zygisk_module/bin/srxctl")
        self.assertIn('is_managed_temp_path "$stage" || return 64', control)
        self.assertIn('is_managed_temp_path "$archive" || return 64', control)

        for path in (
            "app/src/main/java/org/srx/manager/data/RootFileStore.kt",
            "assets/zygisk_module/webroot/js/api.js",
        ):
            source = read(path)
            self.assertIn("rc=2", source)
            self.assertIn("127", source)
            self.assertNotIn("eq 64", source)

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
        runtime_stats = read("src/runtime_stats.rs")
        companion_stats = read("src/lifecycle/companion_mount/stats.rs")
        specialize_post = read("src/lifecycle/specialize_post.rs")
        hook_stats = read("src/hook/stats.rs")
        control = read("assets/zygisk_module/bin/srxctl")
        self.assertIn('b"storage.redirect.x.logd"', daemon)
        self.assertIn('b"storage.redirect.x.logd"', logging)
        self.assertIn('STATS_TAG, "+1"', runtime_stats)
        self.assertIn("record_runtime_activation", companion_stats)
        self.assertIn("record_runtime_activation", specialize_post)
        self.assertNotIn("increment_global_redirect_count", hook_stats)
        self.assertNotIn("is_debug_logging_enabled", runtime_stats)
        self.assertIn('const STATS_SCHEMA: &str = "2"', daemon)
        self.assertIn('"runtime_activations"', daemon)
        self.assertIn("persist_runtime_activations", daemon)
        self.assertIn("fs::rename(STATS_TEMP_FILE, STATS_FILE)", daemon)
        self.assertIn('const CONTROL_RESET_STATS: &str = "reset-stats"', daemon)
        self.assertIn("CONTROL_RESET_STATS => self.reset_stats()", daemon)
        self.assertIn("STATS_RESET_ACK_FILE", daemon)
        self.assertNotIn("O_TRUNC", companion_stats)
        self.assertIn("control clear-monitor", control)
        self.assertIn("control reset-stats", control)

    def test_webui_reads_bounded_log_tail_and_cleans_bridge_failures(self) -> None:
        app = read("assets/zygisk_module/webroot/js/app.js")
        api = read("assets/zygisk_module/webroot/js/api.js")
        self.assertIn("Api.readFileTail(FILE_MONITOR_LOG, 500)", app)
        self.assertNotIn("Api.readFile(FILE_MONITOR_LOG)", app)
        tail = api[api.index("async readFileTail") : api.index("async writeFile", api.index("async readFileTail"))]
        self.assertNotIn("this.readFile(path)", tail)
        bridge = api[api.index("const finish =") : api.index("// 3. Fallback")]
        self.assertEqual(2, bridge.count(".catch((error) => finish("))
        self.assertIn("finish(1, \"\", fallbackError?.message", bridge)

    def test_bulk_webui_config_writes_use_one_staged_manifest(self) -> None:
        api = read("assets/zygisk_module/webroot/js/api.js")
        bulk = api[api.index("async writeAppConfigs") : api.index("async deleteAppConfig")]
        restore = api[api.index("async restoreConfigSnapshot") : api.index("async stopModule")]
        self.assertIn("this.writeStagedFiles(stage, files)", bulk)
        self.assertNotIn("this.writeRawFile(", bulk)
        self.assertIn("this.writeStagedFiles(stage, files)", restore)
        self.assertNotIn("this.writeRawFile(", restore)

    def test_native_hot_paths_keep_bounded_cache_and_polling(self) -> None:
        fuse = read("src/fuse_redirect.rs")
        watcher = read("src/config/watcher.rs")
        self.assertIn("fn forget(&self", fuse)
        self.assertIn("lookup_counts: HashMap<u64, u64>", fuse)
        self.assertIn("dir_entry_refs: HashMap<u64, u64>", fuse)
        self.assertIn("dirs: HashMap<u64, Arc<[DirEntry]>>", fuse)
        self.assertIn("remove_unreferenced_inode", fuse)
        self.assertIn("const POLL_INTERVAL_MS", watcher)
        self.assertIn("LAST_POLL_MS", watcher)
        self.assertIn("compare_exchange", watcher)

    def test_manager_log_refresh_uses_one_filter_snapshot(self) -> None:
        repository = read("app/src/main/java/org/srx/manager/data/SrxRepository.kt")
        view_model = read("app/src/main/java/org/srx/manager/ui/SrxViewModel.kt")
        snapshot = repository[
            repository.index("suspend fun readLogSnapshot") : repository.index("suspend fun clearLogs")
        ]
        refresh = view_model[
            view_model.index("fun refreshLogs()") : view_model.index("fun refreshFileMonitorFilters()")
        ]

        self.assertEqual(1, snapshot.count("readFileMonitorFilters()"))
        self.assertIn("MonitorLogSnapshot(entries, filters)", snapshot)
        self.assertIn("repository.readLogSnapshot()", refresh)
        self.assertNotIn("repository.readFileMonitorFilters()", refresh)

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
