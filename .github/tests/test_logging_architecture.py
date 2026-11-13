import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from test_caller_attribution_boundaries import extract_fn


ROOT = Path(__file__).resolve().parents[2]


def section(source: str, start: str, end: str) -> str:
    """截取从 `start` 到其后首个 `end` 之间的片段，用于把断言限定在单个函数/循环内。"""
    return source[source.index(start) : source.index(end, source.index(start))]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def read_fuse_redirect_core() -> str:
    """读取 `src/fuse_redirect` 的 mod.rs 与 callbacks.rs（mod.rs 在前）。

    FUSE 回调与目录项枚举已拆到 callbacks.rs，守卫需要看到合并后的内容，
    否则按函数名取片段时会因文件边界而失败。只合并这两个文件，
    避免把 config/policy 等无关模块带进来影响「不得出现」类断言。
    """
    base = ROOT / "src" / "fuse_redirect"
    return "".join(
        (base / name).read_text(encoding="utf-8") for name in ("mod.rs", "callbacks.rs")
    )


def read_daemon_mount_module() -> str:
    """读取 `src/daemon_mount*.rs` 的全部内容。

    诊断与快照已拆到 daemon_mount_diag.rs，守卫需要看到合并后的内容，
    否则抽取函数或断言文本时会因文件边界而失败。
    """
    files = sorted((ROOT / "src").glob("daemon_mount*.rs"))
    return "".join(path.read_text(encoding="utf-8") for path in files)


def read_paths_module() -> str:
    """读取 `src/platform/paths*.rs` 的全部内容。

    paths 已按职责拆出 paths_alias / paths_roots / paths_rules / paths_safety 同级
    模块，守卫测试需要看到合并后的内容，否则抽取函数时会因文件边界而失败。
    """
    platform_dir = ROOT / "src" / "platform"
    files = sorted(platform_dir.glob("paths*.rs"))
    return "\n".join(path.read_text(encoding="utf-8") for path in files)


class LoggingArchitectureTest(unittest.TestCase):
    def test_redirected_mapping_fuse_requests_are_logged_before_inotify_observation(self) -> None:
        callbacks = read_fuse_redirect_core()
        policy = read("src/fuse_redirect/policy.rs")
        open_section = callbacks[callbacks.index("    fn open(") : callbacks.index("    fn read(", callbacks.index("    fn open("))]
        self.assertIn("open_flags_write(flags.0)", open_section)
        self.assertIn("open_flags_create(flags.0)", open_section)
        self.assertIn("let is_mutation", open_section)
        self.assertIn("policy.emit_monitor_mutation", open_section)
        self.assertIn("event_role=request", policy)
        self.assertIn("self.resolve_mapping(&display_path).is_some()", policy)
        self.assertIn('"path_mapping"', policy)
        self.assertIn("fuse_open_operation_name(flags.0)", callbacks)
        helpers = read("src/fuse_redirect/helpers.rs")
        self.assertIn("pub(super) fn open_flags_create", helpers)
        self.assertIn('"open:create"', helpers)
        self.assertIn("destination_existed", callbacks)
        self.assertIn('if destination_existed', callbacks)
        self.assertIn('"write"', callbacks)
        self.assertIn('"create"', callbacks)

    def test_monitor_ui_coalesces_exact_backend_and_keeps_mapping_owner(self) -> None:
        webui = read("assets/zygisk_module/webroot/js/app.js")
        self.assertIn("entry?.backendPath || entry?.landingPath", webui)
        self.assertIn('entry?.extras?.source === "path_mapping"', webui)
        self.assertIn('entry?.extras?.source === "read_only_path"', webui)
        self.assertIn('source === "path_mapping" && identifyMethod === "daemon_inotify"', webui)
        self.assertIn("mergeFuseCreateWriteDuplicate", webui)
        screen = read("app/src/main/java/org/srx/manager/ui/screen/LogsScreen.kt")
        self.assertIn('entry.source == "path_mapping" && entry.identifyMethod == "daemon_inotify"', screen)

    def test_unconfigured_apps_use_monitor_only_passthrough_without_redirecting_writes(self) -> None:
        config = read("src/config.rs")
        self.assertIn("pub fn should_capture_unconfigured_app", config)
        specialize = read("src/lifecycle/specialize_pre.rs")
        self.assertIn("let is_monitor_only = self.should_monitor && !self.should_redirect;", specialize)
        self.assertIn("StorageBackendMode::Fuse", specialize)
        request = read("src/lifecycle/companion_request.rs")
        self.assertIn("pub is_monitor_only: bool", request)
        self.assertIn('get("monitor_only")', request)
        policy = read("src/fuse_redirect/policy.rs")
        backend = policy[policy.index("fn backend_decision") : policy.index("fn matches_any")]
        # 2026-10-21 起沙盒命中（仅限配置了 sandboxed_paths 的应用）优先于监视
        # 直通短路：未配置应用的沙盒索引为空，is_sandbox 恒 false，监视直通
        # 语义不变；监视分支仍须先于其余映射类 Redirect 分支。
        self.assertLess(
            backend.index("let is_sandbox = self.is_mapping_mode_only"),
            backend.index("if self.is_monitor_only"),
        )
        self.assertLess(
            backend.index("if self.is_monitor_only"),
            backend.rindex("BackendKind::Redirect"),
        )
        self.assertIn('"fuse_monitor_only"', policy)
        constructor_start = policy.index("impl RedirectPolicy")
        constructor = policy[constructor_start : policy.index("        let mut path_mappings", constructor_start)]
        self.assertIn("config.is_passthrough_host", constructor)
        self.assertIn("config.is_monitor_only", constructor)
        self.assertIn("is_mapping_root_passthrough", constructor)
        self.assertIn("config.is_mapping_mode_only", constructor)
        self.assertIn("event_role=request", policy)

    @unittest.skipUnless(shutil.which("rustc"), "需要 rustc")
    def test_monitor_only_eligibility_preserves_app_id_and_package_exclusions_for_all_users(self):
        policy = read("src/redirect/policy.rs")
        predicates = "\n".join("pub " + extract_fn(policy, name) for name in (
            "is_system_writer_package", "is_system_writer_candidate", "is_media_provider_candidate",
            "is_file_monitor_bridge_package", "is_file_monitor_ui_package",
        ))
        constants = "\n".join(line for line in policy.splitlines() if line.startswith("const "))
        uid_source = (ROOT / "src/platform/uid.rs").as_posix()
        fixture = f'''
mod platform {{ include!("{uid_source}"); }}
mod redirect {{ pub mod policy {{ {constants}\n{predicates} }} }}
{extract_fn(read("src/config.rs"), "should_capture_unconfigured_app")}
fn main() {{
    for user in [0, 1, 10, 999, 21473] {{
        for app_id in [10000, 10123, 89999] {{
            let uid = user * platform::ANDROID_USER_ID_OFFSET + app_id;
            assert!(should_capture_unconfigured_app("com.example.app", uid, true));
            assert!(!should_capture_unconfigured_app("com.example.app", uid, false));
            for package in ["", "com.storage.redirect.x", "com.google.android.providers.media.module",
                "com.android.providers.media.module", "com.android.providers.media",
                "com.android.providers.downloads", "com.android.mtp", "com.android.externalstorage",
                "com.android.providers.downloads.ui", "com.android.documentsui",
                "com.android.photopicker", "com.example.filemanager", "com.example.fileexplorer",
                "com.example.myfiles"] {{
                assert!(!should_capture_unconfigured_app(package, uid, true), "{{package}} {{uid}}");
            }}
        }}
        for app_id in [0, 1000, 9999, 90000, 99000, 99999] {{
            assert!(!should_capture_unconfigured_app("com.example.app", user * 100000 + app_id, true));
        }}
    }}
    for uid in [-1, -10000, -10123, i32::MIN] {{
        assert!(!should_capture_unconfigured_app("com.example.app", uid, true));
    }}
}}
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "monitor_eligibility.rs"
            source.write_text(fixture, encoding="utf-8")
            binary = root / "monitor_eligibility"
            subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", str(source), "-o", str(binary)],
                           check=True, capture_output=True, text=True, timeout=30)
            subprocess.run([str(binary)], check=True, capture_output=True, text=True, timeout=10)

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
        # 高频 OEM Error 日志会在半分钟内耗尽 8000 行，主缓冲快照需 20000 行
        # 才能覆盖「复现后 1-2 分钟内导出」的窗口。
        self.assertIn("-t 20000", script)
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
        # App 端 legacy 内嵌导出已删除（最低兼容模块 v1.2.55，脚本入口由模块自带）；
        # 不变量改为：WebUI 内嵌导出器与模块诊断脚本必须使用同一组有界采集窗口，
        # 保证不同导出面产出的诊断包可相互对照。
        for path in (
            "assets/zygisk_module/service.d/diagnostic_archive.sh",
            "assets/zygisk_module/webroot/js/api.js",
        ):
            source = read(path)
            self.assertIn("-t 10000", source)
            self.assertIn("-t 20000", source)
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

    def test_fuse_unavailable_reason_is_reported_per_mount(self) -> None:
        """FUSE 视图没建起来时必须留下可判定原因。

        scoped 会话退役后只剩共享宿主一种载体，宿主接入失败就直接降级成 namespace
        bind。降级本身是对的，但如果只写一句 "unavailable"，用户设备上出现不支持时
        无从判断是设备没有 `/dev/fuse`、配置里没有规则、策略没登记进宿主，还是接入
        本身失败。因此要求：原因类型集中定义、挂载结论里带上原因、两条路径都打一行
        带设备事实的结论日志。
        """
        session = read("src/fuse_session.rs")
        for reason in (
            "NoSharedHost",
            "AttachGateClosed",
            "PolicyRegistrationFailed",
            "NoPolicyRules",
            "AttachFailed",
        ):
            self.assertIn(reason, session)
        self.assertIn("no_shared_host_session", session)
        self.assertIn("host_attach_failed", session)
        shared = read("src/fuse_redirect/scoped_mount.rs")
        self.assertIn("unavailable_reason", shared)
        self.assertIn("scoped_mount_failed_namespace_fallback", shared)
        for path in ("src/daemon_mount.rs", "src/lifecycle/companion_mount.rs"):
            source = read(path)
            self.assertIn("fuse view unavailable reason=", source, path)
            self.assertIn("fallback=namespace_bind", source, path)
            self.assertIn("fuse_device=", source, path)
            self.assertIn("unavailable_reason: host_skip.map(", source, path)
            # 回退重挂的旧机制已随 scoped 退役删除，不能重新长出第二套降级实现。
            self.assertNotIn("fn apply_mount_namespace_fallback(", source, path)

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
            "pub fn capture_explicit_mount_identity(",
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
            "for offset in 0..plans.len() {",
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
        # 写入已下沉到 fuse_session 共享模块，两侧经委托调用共享同一写入点。
        session = read("src/fuse_session.rs")
        self.assertIn('"fingerprint={}\\n"', session)
        self.assertIn("crate::fuse_session::write_mount_state(", mount)
        mount_state = read("src/lifecycle/companion_mount/mount_state.rs")
        self.assertIn("crate::fuse_session::write_mount_state(", mount_state)

    def test_doctor_reports_cross_layer_identity(self) -> None:
        """doctor 必须一次给齐五层身份，而不是只报告挂载账本。"""
        daemon = read_daemon_mount_module()
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

    def test_retry_deadlines_do_not_restore_fixed_polling(self) -> None:
        pending = read("src/mount/own_private_pending.rs")
        self.assertIn("pub fn next_due_delay_ms() -> Option<i64>", pending)
        self.assertIn("backoff_ms(rounds).saturating_sub(age_ms)", pending)
        self.assertIn("wake_daemon();", pending)
        self.assertNotIn("SCAN_INTERVAL_MS", pending)
        daemon = read("src/daemon.rs")
        self.assertIn("own_private_pending::next_due_delay_ms()", daemon)
        self.assertIn("own_private_pending::set_daemon_wake_eventfd(control_wake_fd)", daemon)
        self.assertIn("RegularAppMonitor::next_wakeup_deadline_ms", daemon)
        wait = section(daemon, "fn wait_for_file_monitor_events(", "fn request_file_monitor_refresh(")
        self.assertIn("next_wakeup_deadline_ms()", wait)
        self.assertNotIn("event_fd() < 0 && sync.request_event_fd < 0", wait)

    def test_config_event_reader_stops_on_poll_error_events(self) -> None:
        watcher = read("src/config/watcher.rs")
        reader = section(watcher, "fn event_reader_loop(", "fn publish_change(")
        error_branch = section(reader, "if (poll_fd.revents & (", "if ready == 0")
        for flag in ("POLLNVAL", "POLLERR", "POLLHUP"):
            self.assertIn(f"libc::{flag}", error_branch)
        self.assertIn("break;", error_branch)
        self.assertNotIn("continue;", error_branch)
        # 退出必须降级，且不让下次热路径在同一个坏 fd 上反复重建线程。
        cleanup = reader[reader.index("publish_change(kind);"):]
        self.assertIn("EVENTS_LIVE.store(false, Ordering::Release)", cleanup)
        self.assertIn("INOTIFY_FD.compare_exchange(fd, -1", cleanup)
        self.assertIn("(poll_fd.revents & libc::POLLIN) == 0", reader)
        self.assertIn("drain_changed_packages()", reader)

    def test_config_watch_mask_includes_moved_to(self) -> None:
        """配置目录的 inotifyd 掩码必须含 `y`（IN_MOVED_TO）。

        配置的原子保存写法是「写临时文件 → rename 覆盖目标」，这条路径只产生
        m（MOVED_FROM，路径是临时文件名）和 y（MOVED_TO，路径才是 global.json）。
        只监听 `wcmdn` 时两半都收不到：m 的文件名对不上 `.json` 过滤，y 压根不上报，
        于是「保存了配置但开关不生效」。A17 实测：掩码 wcmdn 下 rename 只吐
        `m <dir> stage.txt`；扩到 wcmdny 才有 `y <dir> renamed.json`，开关从
        「40 秒无反应」变成 2 秒内生效。WebUI 的 restoreConfigSnapshot 正是
        `mv "$stage/global.json" "$config/global.json"` 落盘。

        不加 `e`（IN_ATTRIB）：chmod/touch 只改元数据，不代表配置内容变化。
        """
        events = read("assets/zygisk_module/service.d/config_events.sh")
        watch_lines = [
            line for line in events.splitlines() if "inotifyd - " in line and "CONFIG" in line
        ]
        self.assertTrue(watch_lines, "未找到配置目录的 inotifyd 监听")
        for line in watch_lines:
            self.assertIn(":wcmdny", line, line)
            self.assertNotIn(":wcmdn ", line, line)

    def test_srxctl_does_not_require_one_shot_snapshot_pids(self) -> None:
        """`srxctl ensure-collectors` 不得把一次性快照的 pid 当作「必须在位」。

        media_state / app_status 快照是后台一次性任务，跑完即退。把它们列进存活条件，
        每次配置保存都会被判成「采集器缺失」并重启整个 `service.sh`——白中断采集几秒，
        还会短暂出现两套事件循环。常驻的只有 debug 采集器（`.running_collector.pid`）。
        """
        srxctl = read("assets/zygisk_module/bin/srxctl")
        body = section(srxctl, "start_collectors_if_needed() {", "service collectors restart requested")
        self.assertIn(".running_collector.pid", body)
        for one_shot in (".media_state_collector.pid", ".app_status_snapshot.pid"):
            self.assertNotIn(one_shot, body, f"{one_shot} 是一次性任务，不能作为存活条件")

    def test_process_listing_uses_absolute_ps_path(self) -> None:
        """带 `-o` 的 `ps` 调用必须写 `/system/bin/ps`。

        模块脚本有两条启动路径：开机 `busybox sh service.sh`（ash）与
        `srxctl ensure-collectors` 的 `sh service.sh`（mksh）。busybox 的 ash 把
        applet 当内建命令，`ps` 解析成 **busybox 版**，而它只认小写 `-o pid`：
        `ps -A -o PID,ARGS` 直接 `bad -o argument 'PID'` 且 rc=1。错误被
        `2>/dev/null` 吞掉之后，唯一表现就是「选不出进程、什么都没杀」。
        A17 实测：busybox 路径下 `victims` 恒空、每轮开关泄漏一个 logcat；
        同一份代码在 mksh 路径下正常。绝对路径让两种 shell 都落到 toybox。
        """
        files = [
            "assets/zygisk_module/service.d/common.sh",
            "assets/zygisk_module/service.d/media_state.sh",
            "assets/zygisk_module/service.d/app_status.sh",
            "assets/zygisk_module/service.d/diagnostic_archive.sh",
            "assets/zygisk_module/uninstall.sh",
        ]
        offenders = []
        for path in files:
            for lineno, line in enumerate(read(path).splitlines(), 1):
                stripped = line.strip()
                if stripped.startswith("#") or "echo " in stripped:
                    continue
                if re.search(r"(?<!/system/bin/)\bps\s+-[A-Za-z]*\s*-o\b", stripped):
                    offenders.append(f"{path}:{lineno}: {stripped}")
        self.assertEqual(offenders, [], "这些 ps 调用没有用绝对路径:\n" + "\n".join(offenders))

    def test_service_scripts_have_no_duplicate_function_definitions(self) -> None:
        """同一个脚本文件内不得重复定义同名函数。

        shell 里后定义的函数会静默覆盖前一个，没有 warning。2026-11-11 精简 reap 时就
        踩了：把 `reap_tagged_processes` 合并进 `reap_stale_logcat_collectors` 之后，
        文件后面还留着旧的薄封装 `reap_stale_logcat_collectors() { reap_tagged_processes ... }`
        ——它覆盖了新实现，而 `reap_tagged_processes` 已删，于是运行时 `job not found`、
        reap 完全失效。`sh -n` 通过、原有断言也通过（只检查了前一个定义），只有这条能拦。

        跨文件同名是允许的：`uninstall.sh` 独立执行、不 source `common.sh`，它自带
        `stop_background_process` / `stop_collector_by_pid_file` 的副本。
        """
        pattern = re.compile(r"^([a-z_][a-z0-9_]*)\(\) \{", re.MULTILINE)
        for path in sorted((ROOT / "assets" / "zygisk_module").rglob("*.sh")):
            names = pattern.findall(path.read_text(encoding="utf-8"))
            duplicates = sorted({name for name in names if names.count(name) > 1})
            self.assertEqual(
                duplicates, [], f"{path.name} 内重复定义函数: {duplicates}"
            )

    def test_default_collectors_do_not_subscribe_to_native_hot_tags(self) -> None:
        collectors = read("assets/zygisk_module/service.d/log_collectors.sh") + read(
            "assets/zygisk_module/service.d/debug_collectors.sh"
        )
        self.assertNotIn("FileMonitorOp:I", collectors)
        self.assertNotIn("Stats:I", collectors)
        self.assertNotIn("StorageRedirect:V", collectors)
        self.assertEqual(collectors.count("logcat -T 1"), 1)

    def test_debug_collector_stop_kills_subshell_before_reaping(self) -> None:
        """停止详细日志采集器必须「SIGKILL 子 shell → 按 tag 收 logcat」。

        子 shell 是 mksh 且阻塞在管道 `read` 上，SIGTERM 会被延后到该次读返回才处理，
        而关闭 verbose 后 SRX 行可能长时间没有一条：发过 SIGTERM 在运行期等价于没杀，
        随后状态被写成 off 就再也没有路径重试。只按 tag reap 同样无效——子 shell 会在
        1 秒内重建 logcat。
        """
        debug = read("assets/zygisk_module/service.d/debug_collectors.sh")
        common = read("assets/zygisk_module/service.d/common.sh")
        stop = section(debug, "stop_debug_logcat_collector() {", "start_debug_collectors() {")
        self.assertIn('stop_background_process "$pid" KILL', stop)
        self.assertIn("reap_stale_logcat_collectors KILL", stop)
        # 顺序固定：先让子 shell 死掉，再收管道里的 logcat。
        self.assertLess(
            stop.index('stop_background_process "$pid" KILL'),
            stop.index("reap_stale_logcat_collectors KILL"),
        )
        self.assertEqual(stop.count('stop_background_process "$pid"'), 1)
        reap = section(common, "reap_stale_logcat_collectors() {", "log -p i -t Boot")
        self.assertIn('reap_signal="${1:-KILL}"', reap)
        self.assertIn("kill_with_signal", reap)
        # 采集进程清单必须一次 `ps` 取全表：逐个读 `/proc/<pid>/cmdline` 时，目标进程若在
        # 读取过程中消失会让读端自旋不返回，而本函数跑在配置事件循环里。
        self.assertIn("ps -A -o PID,ARGS", reap)
        self.assertNotIn("/proc/$proc_dir/cmdline", reap)
        # 前缀匹配而非子串：`awk`/`ps` 自己的 argv 里也会出现该标记。
        self.assertIn("index(args, tag) == 1", reap)
        self.assertLess(reap.index("victims=$(/system/bin/ps -A"), reap.index("for victim in $victims"))
        # 递归停止不得用全局变量存目标 pid：递归会覆盖它，父 shell 从此收不到信号。
        stopper = section(common, "stop_background_process() {", "stop_collector_by_pid_file() {")
        self.assertNotIn('target_pid="$1"', stopper)
        self.assertIn("kill_with_signal \"$1\"", stopper)

    def test_uninstall_uses_same_collector_stop_semantics(self) -> None:
        """卸载脚本里的停止逻辑是副本，必须与运行时同款，否则卸载会留下 logcat。"""
        uninstall = read("assets/zygisk_module/uninstall.sh")
        common = read("assets/zygisk_module/service.d/common.sh")
        tag = re.search(r'SRX_COLLECTOR_TAG="([^"]+)"', common)
        self.assertIsNotNone(tag)
        self.assertIn(f'SRX_COLLECTOR_TAG="{tag.group(1)}"', uninstall)
        self.assertIn("ps -A -o PID,ARGS", uninstall)
        self.assertIn("index(args, tag) == 1", uninstall)
        self.assertNotIn("/proc/$proc_dir/cmdline", uninstall)
        self.assertIn('stop_background_process "$pid" KILL', uninstall)
        self.assertNotIn('target_pid="$1"', uninstall)

    def test_uninstall_skips_pid_files_from_previous_boot(self) -> None:
        """卸载脚本也只能清理本 boot 启动的采集器。

        pid 号会被复用：跨重启后 pid 文件里的数字可能是无关进程，照它 SIGKILL 就是误杀。
        判据用 `logs/.boot_ok` 与当前 boot_id 比较（A14 实测两分支：跨重启时无关进程
        存活且打出提示，同一 boot 内正常清理）。读不到当前 boot_id 时保持原行为。
        """
        uninstall = read("assets/zygisk_module/uninstall.sh")
        self.assertIn('recorded_boot_id=$(cat "$MODDIR/.boot_ok" 2>/dev/null)', uninstall)
        self.assertIn('[ "$current_boot_id" != "$recorded_boot_id" ]', uninstall)
        guard = uninstall.index('if [ "$pid_files_from_previous_boot" -eq 0 ]; then')
        self.assertLess(uninstall.index("pid_files_from_previous_boot=0"), guard)
        self.assertGreater(
            uninstall.index('stop_collector_by_pid_file "$RUNNING_COLLECTOR_PID_FILE"'),
            guard,
        )
        # 回收 logcat 不受该判断约束：只按 tag 精确定位，pid 号复用不会误伤。
        self.assertGreater(uninstall.index("reap_collector_processes", guard), guard)

    def test_debug_collector_reader_splits_with_quoted_pipe(self) -> None:
        """读循环必须用引号化的 `|` 模式拆分，否则采集器永远不落盘。

        Android 的 `/system/bin/sh` 是 mksh（R59 起带 ksh 风格扩展模式），裸写的
        `${x%%|*}` 会被当成「空模式或任意」剥掉整个字符串、`${x#*|}` 一个字符都不剥，
        于是 `kind` 恒为空、下面的 `case` 一个分支都进不去：采集器活着、读到数据
        （wchar>0）却从不写 `running.log` / `app_status.log`。引号化后 mksh 与 POSIX
        shell 都按字面量 `|` 处理（A14 实机对照：未加引号 0 字节，加引号 100 行全部落盘）。
        """
        debug = read("assets/zygisk_module/service.d/debug_collectors.sh")
        self.assertIn('kind=${tagged_line%%"|"*}', debug)
        self.assertIn('line=${tagged_line#*"|"}', debug)
        self.assertNotIn("${tagged_line%%|*}", debug)
        self.assertNotIn("${tagged_line#*|}", debug)
        # threadtime 把 tag 补齐到 8 列并单列一个 ":"，短 tag 时不能把冒号拼进消息。
        self.assertIn('if ($7 == ":") msg_start = 8', debug)
        self.assertIn("line_text = join_from(msg_start)", debug)

    def test_boot_clears_stale_collector_pid_files(self) -> None:
        """本 boot 首次运行服务时必须清掉上一次开机留下的采集器 pid 与状态文件。

        pid 号会被复用：拿旧 pid 判活会静默跳过采集器启动，拿旧 pid 停止会杀掉无关进程
        （A14 实测：`state=on` + 旧 pid 存活 → 整个 boot 没有采集器；关闭开关时 SIGKILL
        打到复用的 pid）。daemon 的 pid 文件不在此列，它自带 comm 校验。
        """
        boot = read("assets/zygisk_module/service.d/boot.sh")
        body = boot[boot.index("boot_guard_wait() {") :]
        self.assertIn('[ ! -f "$LOGS_DIR/boot_${boot_id}.marker" ]', body)
        rm_block = body[body.index("rm -f \"$MONITOR_COLLECTOR_PID_FILE\"") : body.index("timeout=180")]
        for pid_file in (
            "MONITOR_COLLECTOR_PID_FILE",
            "CONFIG_EVENT_COLLECTOR_PID_FILE",
            "PACKAGE_EVENT_COLLECTOR_PID_FILE",
            "RUNNING_COLLECTOR_PID_FILE",
            "MEDIA_STATE_COLLECTOR_PID_FILE",
            "APP_STATUS_COLLECTOR_PID_FILE",
            "APP_STATUS_SNAPSHOT_PID_FILE",
            "STATS_COLLECTOR_PID_FILE",
            "APP_ERROR_RING_PID_FILE",
            "APP_ERROR_WINDOW_PID_FILE",
            "DEBUG_COLLECTORS_STATE_FILE",
        ):
            self.assertIn(f'"${pid_file}"', rm_block)
        self.assertNotIn("srx_daemon.pid", rm_block)
        # 清理必须发生在启动采集器之前。
        self.assertLess(body.index("rm -f \"$MONITOR_COLLECTOR_PID_FILE\""), body.index("start_log_collectors"))

    def test_event_driven_snapshots_do_not_block_config_event_loop(self) -> None:
        """两个一次性快照采集器必须后台执行，不能让 `dumpsys` 按住配置事件循环。

        media 快照里的 `dumpsys meminfo` / `logcat -d` 在 A17 上实测 47-58 秒（A14 上
        8-10 秒），app 快照的 `cmd package list packages -U` 另有 8 秒。它们同步跑在
        `start_debug_collectors` 里，等于每次翻转详细日志开关都阻塞事件循环近一分钟，
        期间所有配置事件排队——现场表现就是「开关按了没反应」。
        快照只写自己的日志文件、不依赖调用方上下文，后台化不改变语义；pid 落盘是为了
        让下一轮 `start_debug_collectors` 的「先停旧」能收掉还没返回的上一轮。
        """
        cases = (
            ("assets/zygisk_module/service.d/media_state.sh", "start_media_state_collector() {",
             "append_media_state_snapshot", "MEDIA_STATE_COLLECTOR_PID_FILE"),
            ("assets/zygisk_module/service.d/app_status.sh", "start_app_status_snapshot_collector() {",
             "append_app_status_snapshot", "APP_STATUS_SNAPSHOT_PID_FILE"),
        )
        for path, header, snapshot_call, pid_file in cases:
            source = read(path)
            marker = f'echo "$!" > "${pid_file}"'
            body = section(source, header, marker)
            self.assertIn(") </dev/null >/dev/null 2>&1 &", body, path)
            self.assertLess(
                body.index(snapshot_call),
                body.index(") </dev/null >/dev/null 2>&1 &"),
                f"{path}: 快照调用必须位于后台子 shell 内",
            )
            self.assertIn(marker, source, path)

        # 三段启动之间不得夹任何等待：这条链路由配置事件驱动，等待就是排队。
        starters = section(
            read("assets/zygisk_module/service.d/debug_collectors.sh"),
            "start_debug_collectors() {",
            "start_app_status_snapshot_collector",
        )
        for forbidden in ("sleep", "wait "):
            self.assertNotIn(forbidden, starters, "start_debug_collectors 不得阻塞")

    def test_collector_stop_terminates_child_processes_without_argv_marking(self) -> None:
        """停止采集器必须能收掉子进程，且不得靠改 argv[0] 打标记。

        两件事都是实测定下来的：
        1. `/proc/<pid>/task/<pid>/children` 在 A14 雷电与 A17 AvdLite 上都不存在，
           旧实现遇到就跳过整段递归，管道里的 inotifyd / awk 会留成孤儿——所以要退化为
           `ps` 的 PPID 列，并且**先取清单再杀**（先杀父会让子进程被 init 收养）。
        2. `/system/bin/inotifyd` 是 toybox 的符号链接，toybox 靠 argv[0] 分派 applet；
           用 `exec -a` 改掉 argv[0] 后它直接起不来，配置事件整条链路静默失效
           （2026-11-11 在 A17 上踩到，且表面上像「进程起来又停掉」）。
        """
        common = read("assets/zygisk_module/service.d/common.sh")
        events = read("assets/zygisk_module/service.d/config_events.sh")
        stopper = section(common, "stop_background_process() {", "stop_collector_by_pid_file() {")
        self.assertIn('child_pids=$(/system/bin/ps -A -o PID,PPID', stopper)
        self.assertIn("$2 == root", stopper)
        self.assertLess(stopper.index("child_pids=$(/system/bin/ps -A"), stopper.index('kill_with_signal "$1"'))
        self.assertIn('for child_pid in $child_pids', stopper)
        # toybox applet 不得被打标记
        self.assertNotIn("exec -a", events)
        self.assertNotIn("SRX_CONFIG_EVENT_TAG", common)
        self.assertNotIn("SRX_PACKAGE_EVENT_TAG", common)
        # 按 pid 文件停止时默认 KILL（TERM 会被阻塞中的 read 延后）
        by_pid = section(common, "stop_collector_by_pid_file() {", "reap_stale_logcat_collectors() {")
        self.assertIn('stop_background_process "$pid" "${2:-KILL}"', by_pid)

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

        # stats 必须存放在模块目录之外的持久目录；路径定义已收拢到 module_paths，
        # daemon 侧只保留常量别名，因此锚定常量定义与引用两侧。
        self.assertIn(
            '"/data/adb/storage.redirect.x/stats"', read("src/platform/module_paths.rs")
        )
        self.assertIn('"/data/adb/storage.redirect.x/.stats.tmp"', read("src/platform/module_paths.rs"))
        self.assertIn(
            '"/data/adb/storage.redirect.x/.stats.reset.ok"',
            read("src/platform/module_paths.rs"),
        )
        self.assertIn("module_paths::STATS_FILE", daemon)
        self.assertIn("module_paths::STATS_TEMP_FILE", daemon)
        self.assertIn("module_paths::STATS_RESET_ACK_FILE", daemon)
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
        fuse = read_fuse_redirect_core()
        watcher = read("src/config/watcher.rs")
        self.assertIn("fn forget(&self", fuse)
        self.assertIn("lookup_counts: HashMap<u64, u64>", fuse)
        self.assertIn("dir_entry_refs: HashMap<u64, u64>", fuse)
        self.assertIn("dirs: HashMap<u64, Arc<[DirEntry]>>", fuse)
        self.assertIn("remove_unreferenced_inode", fuse)
        # 配置变更热路径不得再做系统调用：inotify fd 由**单个**阻塞线程独占读取
        # （空闲零唤醒），结论发布到待取槽，热路径只做一次原子判断再取走。这里是每次
        # 被拦截操作都会走到的路径，任何「每次操作读一次 fd」的实现都会按流量放大。
        self.assertIn("fn event_reader_loop(", watcher)
        self.assertIn("libc::poll(&mut poll_fd, 1, -1)", watcher)
        self.assertNotIn("CHANGE_DEBOUNCE_MS", watcher)
        self.assertIn("static PENDING_READY: AtomicBool", watcher)
        hot = watcher[
            watcher.index("pub fn poll_changed_with_packages") :
            watcher.index("pub fn poll_changed_unthrottled_with_packages")
        ]
        self.assertNotIn("libc::read", hot)
        self.assertIn("take_published_change()", hot)
        # 只有事件驱动的等待方（daemon 主循环）直接读 fd，且必须不节流：它已经阻塞在
        # 该 fd 上，把事件挡回去会立刻再次被唤醒，退化成忙循环。
        self.assertIn("fn drain_changed_packages() -> ChangeKind", watcher)

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
        paths = read_paths_module()
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
        self.assertIn("public_owner_roots", reconfigure)
        self.assertIn("MAX_PUBLIC_OWNER_REPAIR_DIRS: usize = 32768", monitor)
        # 1 秒周期扫描已移除（2026-10-21 用户拍板）：周期入口与节流常量不得回潮，
        # 建树一次性扫描 + inotify 事件驱动修复 + missing 重试是仅存通道。
        self.assertNotIn("repair_public_owner_roots_if_due", monitor)
        self.assertNotIn("PUBLIC_OWNER_REPAIR_INTERVAL_MS", monitor)
        self.assertNotIn("last_public_owner_repair_ms", monitor)
        reset = monitor[monitor.index("fn reset") : monitor.index("fn repair_public_owner_root")]
        self.assertIn("self.public_owner_roots.clear()", reset)
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

    def test_public_path_package_attribution_survives_daemon_filtering(self) -> None:
        source_hint = read("src/monitor/source_hint.rs")
        monitor = read("src/monitor.rs")
        events = read("src/daemon_monitor/events.rs")
        app_ui = read("app/src/main/java/org/srx/manager/ui/screen/LogsScreen.kt")
        web_ui = read("assets/zygisk_module/webroot/js/app.js")

        self.assertIn("infer_public_path_package_name", source_hint)
        self.assertNotIn("public_path_package_alias_paths", source_hint)
        self.assertIn("不把 `.gs/<包名>/...` 的证据向上投射", source_hint)
        self.assertIn("RECENT_PUBLIC_PATH_PACKAGE_ALIAS_WINDOW_MS", source_hint)
        self.assertIn('if window[0] != ".gs"', source_hint)
        self.assertIn("uid >= ANDROID_APP_UID_START", source_hint)
        self.assertIn('if op == "inotify"', monitor)
        self.assertIn('"public_path_package", "high"', monitor)
        self.assertIn('identity.identify_method == "public_path_package"', events)
        self.assertIn('identify_method: "public_path_package"', events)
        self.assertIn('"public_path_package" => Some("public_path_package")', read("src/monitor/hint_file.rs"))
        self.assertIn('"public_path_package" -> "隐藏路径中的应用包名"', app_ui)
        self.assertIn('public_path_package: "隐藏路径中的应用包名"', web_ui)
        self.assertIn("isAttributedFilesystemObservation", web_ui)



    def test_monitor_clear_waits_for_daemon_ack_and_archive_scans_backups(self) -> None:
        paths = read("src/platform/module_paths.rs")
        daemon = read("src/log_daemon.rs")
        ctl = read("assets/zygisk_module/bin/srxctl")
        webui = read("assets/zygisk_module/webroot/js/app.js")
        archive = read("assets/zygisk_module/service.d/diagnostic_archive.sh")

        self.assertIn("MONITOR_CLEAR_ACK_FILE", paths)
        self.assertIn('const CONTROL_CLEAR_MONITOR_PREFIX: &str = "clear-monitor:"', daemon)
        self.assertIn("self.clear_monitor(Some(token))", daemon)
        self.assertIn("fs::rename(ack_tmp, MONITOR_CLEAR_ACK_FILE)", daemon)
        self.assertIn("MONITOR_CLEAR_ACK_FILE=", ctl)
        self.assertIn('control "clear-monitor:$request_id"', ctl)
        self.assertIn('= "$request_id"', ctl)
        self.assertIn('control clear-monitor; then', ctl)
        self.assertIn("logLoadGeneration", webui)
        self.assertIn('State.logLoadGeneration += 1', webui)
        self.assertIn('await loadLogs({ afterClear: true })', webui)
        self.assertIn('for monitor_log in "$LOGS_DIR/file_monitor.log" "$LOGS_DIR"/file_monitor.log.[0-9]*; do', archive)

    def test_daemon_monitor_module_uses_a_stable_rustfmt_path(self) -> None:
        daemon = read("src/bin/srx_daemon.rs")
        monitor = read("src/bin/monitor/mod.rs")

        self.assertIn('#[path = "monitor/mod.rs"]', daemon)
        self.assertNotIn("mod monitor {", daemon)
        self.assertIn('#[path = "../../monitor/hint_file.rs"]', monitor)
        self.assertIn('#[path = "../../monitor/source_hint.rs"]', monitor)


if __name__ == "__main__":
    unittest.main()
