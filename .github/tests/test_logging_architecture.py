import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def section(source: str, start: str, end: str) -> str:
    """截取从 `start` 到其后首个 `end` 之间的片段，用于把断言限定在单个函数/循环内。"""
    return source[source.index(start) : source.index(end, source.index(start))]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


class LoggingArchitectureTest(unittest.TestCase):
    def test_monitor_watches_precede_slow_public_owner_scan(self) -> None:
        # 公共目录扫描期间仍须消费事件，避免新目录内的覆盖写入漏记。
        source = read("src/daemon_monitor.rs")
        rebuild = source[
            source.index("    pub fn reconfigure(") : source.index("    fn retry_missing_watch_roots(")
        ]
        scan_start = rebuild.index("self.repair_public_owner_root(root)")
        self.assertLess(rebuild.index("self.add_watch_root(root)"), scan_start)
        self.assertLess(rebuild.index("self.expand_watch_tree_from("), scan_start)
        scan = source[
            source.index("    fn repair_existing_public_tree(") : source.index("    fn add_watch_node(")
        ]
        self.assertIn("scanned_entries.is_multiple_of(64)", scan)
        self.assertIn("self.drain_events();", scan)

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
        self.assertIn('diagnostic_archive_version=8', script)
        self.assertIn('diagnostic-summary.json', script)
        self.assertIn('collect_diagnostic_summary', script)
        self.assertIn("collect_fuse_state", script)
        self.assertIn('fuse/cache-performance.txt', script)
        self.assertIn('test-flow-backend-diagnostic.txt', read(".github/tests/run-storage-redirect-scenarios.sh"))
        self.assertIn('fuse/mount_state', script)

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
            self.assertIn("fuse/cache-performance.txt", source)
            self.assertIn("fuse/mount-state-index.txt", source)
            self.assertIn("diagnostic-summary.json", source)

    def test_fuse_cache_sampling_includes_peak_entries(self) -> None:
        source = read("src/fuse_redirect/perf.rs")
        self.assertIn("dir_cache_peak_entries", source)
        self.assertIn("peak_entries={}", source)
        self.assertIn("dir_cache_capacity", source)
        self.assertIn("max_capacity={}", source)
        self.assertIn("dir_cache_bytes", source)
        self.assertIn("byte_budget={}", source)
        self.assertIn("oversize={}", source)

    def test_backend_diagnostic_artifact_captures_selection_contract(self) -> None:
        source = read(".github/tests/run-storage-redirect-scenarios.sh")
        self.assertIn("test-flow-backend-diagnostic.txt", source)
        self.assertIn(".fuse_capability", source)
        self.assertIn("backend_effective", source)
        self.assertIn("fuse_dir_cache_(config|sample)|perf_snapshot component=fuse", source)
        # 选择契约（backend_effective / selection_reason / capability）收敛在单一实现里：
        # 两条挂载路径此前各有一份逐字重复的判定，真机排查时同一请求在两处给出不同的
        # selection_reason，把「能力未放行」与「规则本来不需要 FUSE 根」混成同一句话。
        shared = read("src/fuse_redirect/scoped_mount.rs")
        self.assertIn("pub fn conclude_scoped_mount", shared)
        self.assertIn("backend_effective", shared)
        self.assertIn("selection_reason=", shared)
        self.assertIn("capability=", shared)
        for site in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            source = read(site)
            self.assertIn("conclude_scoped_mount(ScopedMountReport {", source)
            # 站点不得再自行记录能力结果，否则能力语义又会分叉。
            self.assertNotIn("record_fuse_capability_result", source)

    def test_mount_intent_cleanup_uses_process_starttime(self) -> None:
        intent = read("src/mount_intent.rs")
        daemon = read("src/daemon.rs")
        self.assertIn("pub fn prune_stale", intent)
        self.assertIn("process_start_time_ticks", intent)
        self.assertIn("mount_intent::prune_stale", daemon)

    def test_namespace_fallback_reexpands_wildcard_rules_after_fuse_failure(self) -> None:
        config = read("src/fuse_redirect/config.rs")
        daemon = read("src/daemon_mount.rs")
        companion = read("src/lifecycle/companion_mount.rs")
        self.assertIn("expand_namespace_fallback_rules", config)
        self.assertIn("expand_namespace_fallback_rules", daemon)
        self.assertIn("expand_namespace_fallback_rules", companion)
        # 后端结论串只出现在单一实现里；两条挂载路径通过 needs_namespace_fallback 消费它。
        shared = read("src/fuse_redirect/scoped_mount.rs")
        self.assertIn("scoped_mount_failed_namespace_fallback", shared)
        for source in (daemon, companion):
            self.assertIn("needs_namespace_fallback", source)

    def test_fuse_capability_failures_are_bucketed_per_scope(self) -> None:
        """能力熔断不得是全设备单点：分桶、多应用升格、退避自愈三者缺一不可。

        旧实现只有一个全局 fail_count，达到预算就把整机写成 unavailable，此后所有应用一起退回
        namespace，而且因为再也不会尝试挂载，成功结果无从记录——只能靠重启恢复。真机上实测到
        「单个应用的失败把全体拖下水」与「10 秒内 failed/ready 反复横跳」两种表现。
        """
        config = read("src/fuse_redirect/config.rs")
        self.assertIn("scope_failures", config)
        self.assertIn("fn bump_scope_failure", config)
        self.assertIn("fn scope_budget_exhausted", config)
        # 设备级升格必须要求多个不同应用各自失败，否则单个坏应用就能关掉全设备 FUSE。
        self.assertIn("DEVICE_UNAVAILABLE_MIN_SCOPES", config)
        self.assertIn("snapshot.last_failed_scope != scope", config)
        # unavailable 必须可自愈：退避截止时间 + 到期放行一次探测。
        self.assertIn("fn retry_window_open", config)
        self.assertIn("RETRY_BACKOFF_BASE_MS", config)
        self.assertIn("RETRY_BACKOFF_MAX_MS", config)
        self.assertIn("snapshot.retry_at_ms = now_ms.saturating_add", config)
        # 规划层必须按应用判定，而不是读全局状态。
        planning = config[
            config.index("pub fn scoped_fuse_mount_roots_for_request") :
            config.index("pub fn fuse_device_present")
        ]
        self.assertIn("scoped_mount_allowed_for_scope(request.package_name()", planning)

    def test_teardown_failure_does_not_gate_mounts(self) -> None:
        """收尾（unmount）失败不得消耗挂载预算。

        摘不掉旧挂载说明不了下一次挂载会失败；历史上 `scoped_session_end_error` 与挂载失败共用
        同一个预算，因此一次收尾异常就能把全设备 FUSE 关掉，属于口径错配。
        """
        config = read("src/fuse_redirect/config.rs")
        self.assertIn('record_fuse_teardown_failure("scoped_session_end_error")', config)
        self.assertNotIn(
            'record_fuse_capability_result(false, "scoped_session_end_error")', config
        )
        teardown = config[
            config.index("pub fn record_fuse_teardown_failure") :
            config.index("/// 供诊断输出使用的能力快照摘要。")
        ]
        self.assertNotIn("FuseCapability::Unavailable", teardown)
        self.assertNotIn("record_fuse_capability_result", teardown)

    def test_mount_fallback_expansion_is_scoped_per_app(self) -> None:
        """通配规则收敛判定必须带上应用，不能读全局状态。

        过去 `expand_mount_fallbacks_for_mode` 只看设备级能力，于是任意一个应用的失败都会改写
        其它应用的规则展开方式。
        """
        for site in ("src/redirect/router.rs", "src/redirect/writer.rs"):
            call = re.search(
                r"expand_mount_fallbacks_for_mode\(\s*config\.storage_backend_mode\(\),\s*[a-z_]+",
                read(site),
            )
            self.assertIsNotNone(call, f"{site} 必须把应用作用域传给收敛判定")
        config = read("src/fuse_redirect/config.rs")
        self.assertIn("pub fn expand_mount_fallbacks_for_mode(mode: StorageBackendMode, scope: &str)", config)

    def test_unknown_capability_does_not_gate_on_app_side_dev_fuse(self) -> None:
        """`Unknown` 不得退回应用侧 `/dev/fuse` 节点探测。

        这个判定同样会在应用进程里执行，而应用视角的 `/dev/fuse` 在 HyperOS 上是
        `crw------- root root`，连 stat 都被 SELinux 拒绝。据此否定会让应用永远规划不出 FUSE 根、
        只能退回 namespace，而且永远等不到那个能解锁的失败计数。只有快照完全不存在时才退回探测。
        """
        config = read("src/fuse_redirect/config.rs")
        gate = config[
            config.index("pub fn scoped_mount_allowed_for_scope") :
            config.index("/// 自动后端使用的 fallback 路径决策")
        ]
        self.assertIn("FuseCapability::Unknown => snapshot.present || fuse_device_present()", gate)
        self.assertNotIn("FuseCapability::Unknown => fuse_device_present()", gate)

    def test_mount_identity_is_recorded_on_both_mount_paths(self) -> None:
        """两条挂载路径都必须登记挂载身份账本。

        账本原本只在 daemon 路径写过（`mount_identity` 只编译在 bin 里），companion 路径只写
        挂载状态文件。于是走 companion 的应用在恢复流程与 `doctor` 里没有任何归属判据，只能
        退化成「按挂载源判定」这个较弱判据——真机上实测同一次挂载在两条路径上留下不同结果。
        """
        ledger = read("src/mount_ledger.rs")
        for entry in (
            "pub fn record_mount_identity(",
            "pub fn capture_mount_identity(",
            "pub fn namespace_identity(",
            "pub fn load(",
            "pub fn save(",
        ):
            self.assertIn(entry, ledger)
        # 共享内核必须被 lib 与 bin 同时声明，否则 companion 路径又拿不到它。
        self.assertIn("mod mount_ledger;", read("src/lib.rs"))
        self.assertIn("../mount_ledger.rs", read("src/bin/srx_daemon.rs"))
        # 恢复决策层只做再导出 + 判定，不得再留一份落盘格式。
        identity = read("src/mount_identity.rs")
        self.assertIn("pub use crate::mount_ledger::*;", identity)
        self.assertNotIn("fn encode(", identity)
        self.assertNotIn("fn decode(", identity)
        # 两条挂载路径都必须登记，并带上各自标识以便日志区分。
        for path, tag in (
            ("src/daemon_mount.rs", "daemon"),
            ("src/lifecycle/companion_mount.rs", "companion"),
        ):
            call = re.search(r'record_mount_identity\(\s*"%s"' % tag, read(path))
            self.assertIsNotNone(call, f"{path} 必须登记挂载身份账本")

    def test_reconcile_remount_is_idempotent(self) -> None:
        """reconcile 重挂必须有幂等判据，否则对已收敛的应用反复叠加挂载层。

        重挂不会先摘除旧挂载栈，而对同一个进程重复挂载会在其命名空间里叠出多层：应用自身
        specialize 挂一次、Prewarm 挂一次、随后两轮 Full 再各挂一次，顶层目录上因此压着
        2~3 层模块挂载。叠加层的 `root` 解析基准不同，应用读到的目录内容随层数变化
        （真机表现：微信 `Android/media/com.tencent.mm/Lumenchat/plugins` 时而读到真实目录、
        时而读到空壳），层数也没有上限。

        守卫锁定：判据存在且被调用；幂等跳过排在其它分支之前；显式请求（Forced）不受跳过影响。
        """
        daemon = read("src/daemon.rs")
        self.assertIn("fn should_skip_as_current(&self)", daemon)
        self.assertIn("is_mount_current", daemon)
        loop = section(
            daemon,
            "for (index, plan) in plans.iter().enumerate() {",
            "\n    if should_log_reconcile_summary(",
        )
        # 跳过必须排在 Prewarm/MissingOnly 分支之前，否则那些分支会先放行。
        self.assertLess(
            loop.index("plan.should_skip_as_current()"),
            loop.index("should_run_in_prewarm()"),
        )
        # 显式请求要能强制重挂，不能被幂等判据吞掉。
        self.assertIn("mode != ReconcileMode::Forced && plan.should_skip_as_current()", loop)
        self.assertIn("ReconcileMode::Forced", daemon)

        # 判据本身：状态健康 + 记录的配置指纹与当前一致。
        mount = read("src/daemon_mount.rs")
        self.assertIn("pub fn has_current_mount_state(request: &MountRequest) -> bool", mount)
        current = section(mount, "pub fn has_current_mount_state(", "\n/// 读取挂载状态文件里记录的配置指纹。")
        self.assertIn("has_mount_state_internal(request, true)", current)
        self.assertIn("config_fingerprint()", current)
        self.assertIn("mount_state_fingerprint(request)", current)

        # 指纹必须由两个写入点都写进状态文件，否则一侧写的状态永远判为「需要重挂」。
        self.assertIn('"fingerprint={}\\n"', mount)
        companion_state = read("src/lifecycle/companion_mount/mount_state.rs")
        self.assertIn('"fingerprint={}\\n"', companion_state)

    def test_doctor_reports_cross_layer_identity(self) -> None:
        """doctor 必须一次给齐五层身份，而不是只报告挂载账本。"""
        daemon = read("src/daemon_mount.rs")
        self.assertIn("pub fn doctor_report(args: &[String]) -> i32", daemon)
        snapshot = daemon[
            daemon.index("fn print_cross_layer_snapshot") :
            daemon.index("pub fn doctor_report(args: &[String]) -> i32")
        ]
        for layer in (
            "== capability ==",
            "== java_hook ==",
            "== app_config ==",
            "== processes ==",
            "== path ==",
        ):
            self.assertIn(layer, snapshot)
        self.assertIn("fuse_capability_summary()", snapshot)
        self.assertIn("scoped_mount_allowed_for_scope(", snapshot)
        # 必须在输出里点明「启动后查 maps 查不到」是常见现象，否则会重复误判为未注入。
        self.assertIn("module_mapped_now=false", snapshot)
        main = read("src/bin/srx_daemon.rs")
        self.assertIn("daemon_mount::doctor_report(&doctor_args)", main)

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

    def test_module_update_keeps_existing_runtime_stats(self) -> None:
        customize = read("assets/zygisk_module/customize.sh")
        api = read("assets/zygisk_module/webroot/js/api.js")
        daemon = read("src/log_daemon.rs")
        srxctl = read("assets/zygisk_module/bin/srxctl")
        post_fs = read("assets/zygisk_module/post-fs-data.sh")
        uninstall = read("assets/zygisk_module/uninstall.sh")

        # stats 必须存放在模块目录之外的持久目录
        self.assertIn('"/data/adb/storage.redirect.x/stats"', daemon)
        self.assertIn('"/data/adb/storage.redirect.x/.stats.tmp"', daemon)
        self.assertIn('"/data/adb/storage.redirect.x/.stats.reset.ok"', daemon)
        self.assertNotIn('"/data/adb/modules/storage.redirect.x/stats"', daemon)

        # post-fs-data.sh 必须为持久目录做 mkdir
        self.assertIn("mkdir -p /data/adb/storage.redirect.x", post_fs)

        # srxctl reset-stats fallback 必须写到持久目录
        self.assertIn('"/data/adb/storage.redirect.x"', srxctl)
        self.assertNotIn('"$MODDIR/stats"', srxctl)
        self.assertNotIn('"$MODDIR/.stats.tmp"', srxctl)
        self.assertNotIn('"$MODDIR/.stats.reset.ok"', srxctl)

        # WebUI 必须从持久目录读取
        self.assertIn('"/data/adb/storage.redirect.x/stats"', api)
        self.assertNotIn('MODULE_DIR + "/stats"', api)

        # customize.sh 升级时做一次性迁移，不再做 backup/restore
        self.assertIn("migrate_stats_to_persistent_dir", customize)
        migrate_call = customize.index("\nmigrate_stats_to_persistent_dir\n")
        unzip_call = customize.index('unzip -o "$ZIPFILE"')
        self.assertLess(migrate_call, unzip_call)
        self.assertNotIn("backup_existing_stats", customize)
        self.assertNotIn("restore_existing_stats", customize)

        # uninstall.sh 必须通过安全守卫清理持久目录
        self.assertIn("safe_remove_known_path", uninstall)
        self.assertIn("safe_remove_known_path /data/adb/storage.redirect.x", uninstall)
        self.assertIn("/data/adb/storage.redirect.x)", uninstall)

    def test_webui_reads_all_rotated_logs_and_renders_in_batches(self) -> None:
        app = read("assets/zygisk_module/webroot/js/app.js")
        api = read("assets/zygisk_module/webroot/js/api.js")
        self.assertIn("Api.readFileWithBackups(FILE_MONITOR_LOG)", app)
        self.assertNotIn("Api.readFile(FILE_MONITOR_LOG)", app)
        self.assertIn('for f in "$base".*', api)
        self.assertIn('sort -rn', api)
        self.assertIn('case "$suffix" in', api)
        self.assertIn("appendNextLogBatch", app)
        self.assertIn("logRenderLimit: 80", app)
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
        fuse = read("src/fuse_redirect/mod.rs")
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

    def test_native_fixed_capacity_caches_evict_incrementally(self) -> None:
        paths = read("src/platform/paths.rs")
        monitor = read("src/config/inspect.rs")
        cache = read("src/platform/lru_cache.rs")
        raw = read("src/config/raw_scan.rs")

        self.assertIn("struct PathNormalizeCache", paths)
        self.assertIn("LruCache", paths)
        self.assertNotIn("cache.clear();\n        }\n        cache.insert(path", paths)
        self.assertIn("struct MonitorPathMatchCache", monitor)
        self.assertIn("cache.prepare_version(config_version)", monitor)
        self.assertIn("LruCache", monitor)
        self.assertIn("while self.entries.len() > self.capacity", cache)
        self.assertIn("cache.remove(0)", raw)
        capacity_branch = raw[raw.index("if cache.len() >= RAW_CACHE_CAP") : raw.index("cache.push(entry)")]
        self.assertNotIn("cache.clear()", capacity_branch)

    def test_daemon_monitor_reuses_config_and_filter_decisions(self) -> None:
        config = read("src/config/inspect.rs")
        monitor = read("src/daemon_monitor.rs")
        events = read("src/daemon_monitor/events.rs")

        reconfigure = monitor[
            monitor.index("pub fn reconfigure") : monitor.index("fn retry_missing_watch_roots")
        ]
        emit = events[events.index("pub(super) fn emit_monitor_event") : events.index("fn watch_package_identity")]
        duplicate = monitor[
            monitor.index("fn should_skip_duplicate") : monitor.index("fn trim_recent_events")
        ]

        self.assertIn("get_daemon_monitor_config_snapshot", config)
        self.assertEqual(1, reconfigure.count("get_daemon_monitor_config_snapshot()"))
        self.assertNotIn("get_monitor_app_specs", reconfigure)
        self.assertNotIn("get_public_owner_repair_app_specs", reconfigure)
        self.assertNotIn("should_filter_monitor_record", emit)
        self.assertEqual(2, duplicate.count('format!("{}|create|{}|{}"'))
        self.assertNotIn(
            'let create_key = format!("{}|create|{}|{}", package_name, path, from_path);\n        if operation_name',
            duplicate,
        )

    def test_public_owner_repair_does_not_consume_audit_watches(self) -> None:
        monitor = read("src/daemon_monitor.rs")
        reconfigure = monitor[
            monitor.index("pub fn reconfigure") : monitor.index("fn retry_missing_watch_roots")
        ]
        retry = monitor[
            monitor.index("fn retry_missing_watch_roots") : monitor.index("fn ensure_fd")
        ]
        repair = monitor[
            monitor.index("fn repair_public_owner_root") : monitor.index("fn add_watch_root")
        ]

        self.assertIn('if root.source == "public_owner"', reconfigure)
        self.assertIn("self.repair_public_owner_root(root)", reconfigure)
        self.assertIn("self.repair_public_owner_roots_if_due()", reconfigure)
        self.assertIn("public_owner_roots", reconfigure)
        self.assertIn("fn repair_public_owner_roots_if_due", monitor)
        self.assertIn("PUBLIC_OWNER_REPAIR_INTERVAL_MS", monitor)
        self.assertIn("MAX_PUBLIC_OWNER_REPAIR_DIRS: usize = 32768", monitor)
        reset = monitor[monitor.index("fn reset") : monitor.index("fn repair_public_owner_root")]
        self.assertIn("self.public_owner_roots.clear()", reset)
        self.assertIn("self.last_public_owner_repair_ms = 0", reset)
        self.assertIn('if root.source == "public_owner"', retry)
        self.assertIn("self.repair_public_owner_root(&root)", retry)
        self.assertIn("self.repair_existing_public_tree(&node)", repair)

    def test_read_only_exclusions_keep_parent_mount_read_only(self) -> None:
        apply = read("src/mount/apply.rs")
        aliases = read("src/mount/alias.rs")
        branch = apply[
            apply.index("let preserve_data_media_backend") : apply.index(
                "if is_read_only_mounted", apply.index("let preserve_data_media_backend")
            )
        ]
        preserving = aliases[
            aliases.index("bind_read_only_mount_with_storage_aliases_preserving_backend") : aliases.index(
                "fn path_exists"
            )
        ]

        self.assertIn("bind_read_only_mount_with_storage_aliases_preserving_backend", branch)
        self.assertNotIn("bind_read_write_mount_with_storage_aliases", branch)
        self.assertIn("is_data_media_backend_alias", preserving)
        self.assertIn("bind_mount_read_only", preserving)

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
