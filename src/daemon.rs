use crate::config::{SettingsHub, watcher};

#[path = "daemon/media_hook_heal.rs"]
mod media_hook_heal;
use crate::daemon_monitor::RegularAppMonitor;
use crate::daemon_mount::{
    MountOperation, MountRequest, execute_mount_request, has_healthy_mount_state, has_mount_state,
    prune_stale_mount_states,
};
use crate::logging::Logger;
use crate::platform;
use crate::redirect_policy as policy;
use crate::runtime_control;
use std::collections::HashSet;
use std::fs as std_fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const RECONCILE_INTERVAL_MS: u64 = 1000;
const PERIODIC_RECONCILE_INTERVAL_MS: i64 = 3_000;
const CONFIG_FINGERPRINT_FALLBACK_INTERVAL_MS: i64 = 10_000;
const FILE_MONITOR_POLL_MS: u64 = 100;
/// 降级路径单轮最多连续排空的次数，避免挤占同一循环内的 reconcile。
const FALLBACK_DRAIN_ROUNDS: usize = 4;
const FILE_MONITOR_SYNC_TIMEOUT_MS: i64 = 2_000;
const INITIAL_RECONCILE_ROUNDS: usize = 3;
const PREWARM_RECONCILE_ROUNDS: usize = 1;
const PREWARM_MAX_REQUESTS: usize = 16;
const ANDROID_APP_UID_START: i32 = 10000;
const UNINTERRUPTIBLE_SKIP_LOG_STEP: u64 = 32;

static UNINTERRUPTIBLE_SKIP_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

/// daemon 主循环与文件监视线程之间的配置同步状态。
///
/// 早期实现让双方各自以 10ms 步长轮询原子计数，既浪费唤醒又让重建请求最多白等一个
/// 轮询周期。这里改成原子计数负责传值、条件变量负责唤醒：进度由监视线程通知等待方，
/// 重建请求由等待方通知监视线程，两侧都不再忙等。
struct FileMonitorSync {
    configured_version: AtomicU64,
    requested_rebuild: AtomicU64,
    completed_rebuild: AtomicU64,
    /// 仅用于配合下面两个条件变量，不承载业务数据。
    signal_lock: Mutex<()>,
    /// 监视线程完成一轮配置同步后唤醒等待方。
    progress_signal: Condvar,
    /// 等待方登记重建请求后唤醒监视线程。
    request_signal: Condvar,
}

impl FileMonitorSync {
    fn lock_signal(&self) -> std::sync::MutexGuard<'_, ()> {
        self.signal_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// 通知等待方本轮配置同步已经推进。
    fn notify_progress(&self) {
        let _guard = self.lock_signal();
        self.progress_signal.notify_all();
    }

    /// 登记重建请求后唤醒监视线程。
    fn notify_request(&self) {
        let _guard = self.lock_signal();
        self.request_signal.notify_all();
    }

    /// 等待监视线程推进一轮，最多等待 `timeout`。
    ///
    /// 通知方在持有 `signal_lock` 时才发出通知，因此这里必须先拿锁再复检原子计数，
    /// 否则会漏掉在检查与等待之间发生的唤醒。
    fn wait_progress_until(&self, deadline: Instant, is_done: impl Fn() -> bool) -> bool {
        let mut guard = self.lock_signal();
        loop {
            if is_done() {
                return true;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (next_guard, _) = self
                .progress_signal
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|error| error.into_inner());
            guard = next_guard;
        }
    }

    /// 等待新的重建请求，最多等待 `timeout`。
    ///
    /// 超时返回也要继续下一轮，监视线程仍需按轮询周期 drain 事件。
    fn wait_new_request(&self, timeout: Duration) {
        let guard = self.lock_signal();
        if self.requested_rebuild.load(Ordering::Acquire)
            > self.completed_rebuild.load(Ordering::Acquire)
        {
            return;
        }
        let _ = self
            .request_signal
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|error| error.into_inner());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconcileMode {
    Prewarm,
    Full,
    MissingOnly,
}

pub fn main_entry() -> i32 {
    Logger::init(Some("srx_daemon"));
    if let Err(error) = crate::log_daemon::start() {
        log::error!("private log writer start failed error={}", error);
        return 1;
    }
    log::info!("daemon start");

    if !runtime_control::is_module_runtime_enabled() {
        log::info!("daemon exit reason=runtime_disabled");
        return 0;
    }

    let config = SettingsHub::instance();
    if !config.init(None) {
        log::warn!("daemon config init failed");
        return 1;
    }
    crate::fuse_redirect::config::refresh_fuse_capability_snapshot("daemon_start");
    policy::refresh_shared_uid_cache();
    let config_watch_fd = watcher::init(crate::platform::module_paths::CONFIG_DIR);
    if config_watch_fd < 0 {
        log::warn!("daemon config watcher unavailable, using fingerprint polling");
    }

    let mut last_version = 0;
    let mut last_fingerprint_check_ms = crate::platform::paths::monotonic_ms();
    let mut last_periodic_reconcile_ms = crate::platform::paths::monotonic_ms();
    let mut round: usize = 0;
    let mut pending_full_reconcile = false;
    let file_monitor_sync = start_file_monitor_thread();
    let mut fallback_file_monitor = file_monitor_sync.is_none().then(RegularAppMonitor::new);
    loop {
        if !runtime_control::is_module_runtime_enabled() {
            log::info!("daemon stop reason=runtime_disabled");
            return 0;
        }
        let before = config.config_version();
        let did_reload = reload_config_for_daemon(config, &mut last_fingerprint_check_ms);
        let current = config.config_version();
        let control_reconcile = crate::log_daemon::take_reconcile_request();
        let periodic_reconcile = should_periodic_reconcile(&mut last_periodic_reconcile_ms);
        let should_reconcile = round < INITIAL_RECONCILE_ROUNDS
            || did_reload
            || current != last_version
            || current != before
            || control_reconcile.is_some()
            || pending_full_reconcile
            || periodic_reconcile;
        if let Some(file_monitor) = fallback_file_monitor.as_mut() {
            file_monitor.reconfigure(config, false);
        }
        if should_reconcile {
            wait_for_file_monitor_version(file_monitor_sync.as_ref(), current);
            policy::refresh_shared_uid_cache();
            let mode = if control_reconcile.is_some() || pending_full_reconcile {
                pending_full_reconcile = false;
                ReconcileMode::Full
            } else if should_prewarm_reconcile(round, did_reload, current, last_version, before) {
                pending_full_reconcile = true;
                ReconcileMode::Prewarm
            } else if periodic_reconcile {
                ReconcileMode::MissingOnly
            } else {
                ReconcileMode::Full
            };
            let mounts_changed = reconcile_running_apps(current, mode);
            if let Some(request) = control_reconcile.as_deref() {
                log::info!(
                    "running app remount completed request={} applied={}",
                    request,
                    mounts_changed
                );
            }
            if mounts_changed {
                if let Some(sync) = file_monitor_sync.as_ref() {
                    request_file_monitor_rebuild(sync);
                } else if let Some(file_monitor) = fallback_file_monitor.as_mut() {
                    file_monitor.reconfigure(config, true);
                }
            }
            last_version = current;
        }
        if let Some(file_monitor) = fallback_file_monitor.as_mut() {
            // 无独立监视线程的降级路径：本循环还要承担 reconcile，因此不无限排空，
            // 但也不能只读一轮就睡 RECONCILE_INTERVAL_MS，否则突发写入会大量积压。
            for _ in 0..FALLBACK_DRAIN_ROUNDS {
                if !file_monitor.drain_events() {
                    break;
                }
            }
        }
        round = round.saturating_add(1);
        thread::sleep(Duration::from_millis(RECONCILE_INTERVAL_MS));
    }
}

fn start_file_monitor_thread() -> Option<Arc<FileMonitorSync>> {
    let sync = Arc::new(FileMonitorSync {
        configured_version: AtomicU64::new(0),
        requested_rebuild: AtomicU64::new(0),
        completed_rebuild: AtomicU64::new(0),
        signal_lock: Mutex::new(()),
        progress_signal: Condvar::new(),
        request_signal: Condvar::new(),
    });
    let thread_sync = Arc::clone(&sync);
    let spawn_result = thread::Builder::new()
        .name("srx-file-monitor".to_string())
        .spawn(move || {
            let config = SettingsHub::instance();
            let mut file_monitor = RegularAppMonitor::new();
            while runtime_control::is_module_runtime_enabled() {
                let requested_rebuild = thread_sync.requested_rebuild.load(Ordering::Acquire);
                let force_rebuild =
                    requested_rebuild > thread_sync.completed_rebuild.load(Ordering::Acquire);
                file_monitor.reconfigure(config, force_rebuild);
                thread_sync
                    .configured_version
                    .store(file_monitor.configured_version(), Ordering::Release);
                if force_rebuild {
                    thread_sync
                        .completed_rebuild
                        .store(requested_rebuild, Ordering::Release);
                }
                thread_sync.notify_progress();
                // 达到单轮预算时队列里仍有事件，立即进入下一轮继续排空，不等轮询间隔，
                // 避免把"防止饿死重建"变成"事件延迟一个周期"。
                if !file_monitor.drain_events() {
                    thread_sync.wait_new_request(Duration::from_millis(FILE_MONITOR_POLL_MS));
                }
            }
            log::info!("daemon file monitor stop reason=runtime_disabled");
        });
    if let Err(error) = spawn_result {
        log::warn!("daemon file monitor thread start failed error={}", error);
        return None;
    }
    Some(sync)
}

fn wait_for_file_monitor_version(sync: Option<&Arc<FileMonitorSync>>, version: u64) {
    let Some(sync) = sync else {
        return;
    };
    let deadline = Instant::now() + Duration::from_millis(FILE_MONITOR_SYNC_TIMEOUT_MS as u64);
    let synced = sync.wait_progress_until(deadline, || {
        sync.configured_version.load(Ordering::Acquire) >= version
    });
    if !synced {
        log::warn!(
            "daemon file monitor config sync timeout expected={:x} actual={:x}",
            version,
            sync.configured_version.load(Ordering::Acquire)
        );
    }
}

fn request_file_monitor_rebuild(sync: &FileMonitorSync) {
    let requested = sync.requested_rebuild.fetch_add(1, Ordering::AcqRel) + 1;
    sync.notify_request();
    let deadline = Instant::now() + Duration::from_millis(FILE_MONITOR_SYNC_TIMEOUT_MS as u64);
    let rebuilt = sync.wait_progress_until(deadline, || {
        sync.completed_rebuild.load(Ordering::Acquire) >= requested
    });
    if !rebuilt {
        // 超时说明监视线程这轮没能跟上，主循环会在下一轮 reconcile 再次登记请求。
        log::warn!(
            "daemon file monitor rebuild sync timeout requested={} completed={}",
            requested,
            sync.completed_rebuild.load(Ordering::Acquire)
        );
    }
}

fn should_periodic_reconcile(last_reconcile_ms: &mut i64) -> bool {
    let now_ms = crate::platform::paths::monotonic_ms();
    should_periodic_reconcile_at(last_reconcile_ms, now_ms)
}

fn should_periodic_reconcile_at(last_reconcile_ms: &mut i64, now_ms: i64) -> bool {
    if now_ms.saturating_sub(*last_reconcile_ms) < PERIODIC_RECONCILE_INTERVAL_MS {
        return false;
    }
    *last_reconcile_ms = now_ms;
    true
}

fn should_prewarm_reconcile(
    round: usize,
    did_reload: bool,
    current: u64,
    last_version: u64,
    before: u64,
) -> bool {
    round < PREWARM_RECONCILE_ROUNDS || did_reload || current != last_version || current != before
}

fn reload_config_for_daemon(config: &SettingsHub, last_fingerprint_check_ms: &mut i64) -> bool {
    if watcher::poll_changed() {
        *last_fingerprint_check_ms = crate::platform::paths::monotonic_ms();
        return config.reload_force();
    }

    let now_ms = crate::platform::paths::monotonic_ms();
    if now_ms.saturating_sub(*last_fingerprint_check_ms) < CONFIG_FINGERPRINT_FALLBACK_INTERVAL_MS {
        return false;
    }

    *last_fingerprint_check_ms = now_ms;
    let before = config.config_version();
    let _ = config.reload_if_changed();
    config.config_version() != before
}

fn reconcile_running_apps(config_version: u64, mode: ReconcileMode) -> bool {
    let started_ms = crate::platform::paths::monotonic_ms();
    prune_stale_mount_states();
    crate::mount_intent::prune_stale();
    if crate::fuse_redirect::config::fuse_capability()
        != crate::fuse_redirect::config::FuseCapability::Available
    {
        crate::fuse_redirect::config::refresh_fuse_capability_snapshot("reconcile_probe");
    }
    let mut seen = HashSet::new();
    let mut applied = 0usize;
    let mut disabled = 0usize;
    let mut skipped = 0usize;
    let mut deferred = 0usize;
    let mut plans = Vec::new();
    let mut media_processes = Vec::new();
    let mut media_like_names: Vec<String> = Vec::new();
    let config_snapshot = SettingsHub::instance().get_daemon_reconcile_config_snapshot();

    for proc in list_app_processes() {
        // /proc 目录项本身按 pid 唯一，pid 足以去重，无需再拼接包名分配字符串。
        if !seen.insert(proc.pid) {
            continue;
        }
        // MediaProvider 走 hook 而非挂载，会被 should_skip_process 跳过；
        // 这里借本轮已有的枚举结果记下它，避免自愈逻辑重复扫描 /proc。
        if media_hook_heal::is_media_provider_process(&proc.package_name) {
            media_processes.push((proc.pid, proc.uid));
        } else if proc.package_name.contains("providers.media")
            || proc.package_name.contains("process.media")
        {
            // 名字看着像 MediaProvider 却没被判定命中：记下原始包名，
            // 用于区分「MediaProvider 没在跑」与「判定没认出它」。
            media_like_names.push(proc.package_name.clone());
        }
        if should_skip_process(&proc) {
            skipped += 1;
            continue;
        }

        let request = build_request(&proc, config_version, &config_snapshot);
        plans.push(ReconcilePlan::new(
            request,
            mode == ReconcileMode::MissingOnly,
        ));
    }

    media_hook_heal::heal_if_needed(SettingsHub::instance(), &media_processes, &media_like_names);

    if mode == ReconcileMode::Prewarm {
        plans.sort_by_key(|plan| plan.priority());
    }

    for (index, plan) in plans.iter().enumerate() {
        if mode == ReconcileMode::Prewarm
            && (index >= PREWARM_MAX_REQUESTS || !plan.should_run_in_prewarm())
        {
            deferred += 1;
            continue;
        }
        if mode == ReconcileMode::MissingOnly && !plan.should_run_in_missing_only() {
            skipped += 1;
            continue;
        }
        match plan.request.operation {
            MountOperation::Reload => {
                if execute_mount_request(&plan.request) {
                    if !plan.has_mount_state && has_mount_state(&plan.request) {
                        crate::runtime_stats::record_runtime_activation();
                    }
                    applied += 1;
                }
            }
            MountOperation::Disable => {
                if plan.has_mount_state && execute_mount_request(&plan.request) {
                    disabled += 1;
                } else if !plan.has_mount_state {
                    skipped += 1;
                }
            }
        }
    }

    log::info!(
        "daemon reconcile mode={:?} version={:x} planned={} applied={} disabled={} skipped={} deferred={} ms={}",
        mode,
        config_version,
        plans.len(),
        applied,
        disabled,
        skipped,
        deferred,
        crate::platform::paths::monotonic_ms().saturating_sub(started_ms)
    );
    applied > 0 || disabled > 0
}

struct ReconcilePlan {
    request: MountRequest,
    has_mount_state: bool,
}

impl ReconcilePlan {
    fn new(request: MountRequest, check_mount_targets: bool) -> Self {
        let has_mount_state = if check_mount_targets {
            has_healthy_mount_state(&request)
        } else {
            has_mount_state(&request)
        };
        Self {
            request,
            has_mount_state,
        }
    }

    fn should_run_in_prewarm(&self) -> bool {
        self.request.operation == MountOperation::Reload || self.has_mount_state
    }

    fn should_run_in_missing_only(&self) -> bool {
        self.request.operation == MountOperation::Reload && !self.has_mount_state
    }

    fn priority(&self) -> u8 {
        match (self.request.operation, self.has_mount_state) {
            (MountOperation::Reload, false) => 0,
            (MountOperation::Reload, true) => 1,
            (MountOperation::Disable, true) => 2,
            (MountOperation::Disable, false) => 3,
        }
    }
}

fn build_request(
    proc: &AppProcess,
    config_version: u64,
    snapshot: &crate::config::DaemonReconcileConfigSnapshot,
) -> MountRequest {
    let (
        operation,
        user_id,
        redirect_target,
        allowed_real_paths,
        excluded_real_paths,
        path_mappings,
        sandboxed_paths,
        read_only_paths,
        is_mapping_mode_only,
    ) = match snapshot.resolve_profile(&proc.package_name, proc.uid) {
        Some(resolved) => (
            MountOperation::Reload,
            resolved.user_id,
            resolved.redirect_target,
            resolved.allowed_real_paths,
            resolved.excluded_real_paths,
            resolved.path_mappings,
            resolved.sandboxed_paths,
            resolved.read_only_paths,
            resolved.is_mapping_mode_only,
        ),
        None => (
            MountOperation::Disable,
            platform::user_id_from_uid(proc.uid),
            String::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        ),
    };

    MountRequest {
        operation,
        pid: proc.pid,
        uid: proc.uid,
        package_name: proc.package_name.clone(),
        app_data_dir: format!("/data/user/{}/{}", user_id, proc.package_name),
        redirect_target,
        allowed_real_paths,
        excluded_real_paths,
        path_mappings,
        sandboxed_paths,
        read_only_paths,
        is_mapping_mode_only,
        storage_backend_mode: snapshot.storage_backend_mode,
        is_file_monitor_enabled: snapshot.is_file_monitor_enabled,
        config_version,
    }
}

fn should_skip_process(proc: &AppProcess) -> bool {
    if proc.pid <= 0 || proc.uid < ANDROID_APP_UID_START {
        return true;
    }
    if proc.is_uninterruptible {
        log_uninterruptible_skip(proc);
        return true;
    }
    if platform::is_isolated_uid(proc.uid) {
        return true;
    }
    if policy::is_system_writer_package(&proc.package_name)
        || policy::is_shared_uid_process(proc.uid)
    {
        return true;
    }
    false
}

#[derive(Clone)]
struct AppProcess {
    pid: i32,
    uid: i32,
    package_name: String,
    is_uninterruptible: bool,
}

fn list_app_processes() -> Vec<AppProcess> {
    let mut processes = Vec::new();
    let Ok(entries) = std_fs::read_dir("/proc") else {
        return processes;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        // /proc 每轮都有数百个目录项，这里只借用文件名判断，不再为每个目录项分配 String。
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.is_empty() || !name.bytes().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        // 先读 status 取 uid：/proc 中绝大多数是内核线程与系统进程。
        // MediaProvider 在部分 Android 版本启动时会先以系统 UID 建立进程，
        // 再切换到应用 UID；因此不能在读取 cmdline 前用应用 UID 门槛过滤，
        // 否则 daemon 会漏掉未安装 Java hook 的 Provider，无法触发自愈重启。
        let Some((uid, is_uninterruptible)) = read_process_status(pid) else {
            continue;
        };
        let Some(package_name) = read_process_package(pid) else {
            continue;
        };
        if uid < ANDROID_APP_UID_START && !policy::is_media_provider_package(&package_name) {
            continue;
        }
        processes.push(AppProcess {
            pid,
            uid,
            package_name,
            is_uninterruptible,
        });
    }

    processes
}

fn read_process_package(pid: i32) -> Option<String> {
    let data = std_fs::read(format!("/proc/{}/cmdline", pid)).ok()?;
    let first = data.split(|ch| *ch == 0).next()?;
    let raw = std::str::from_utf8(first).ok()?.trim();
    if raw.is_empty() || raw.starts_with('/') || !raw.contains('.') {
        return None;
    }
    let package = raw.split(':').next().unwrap_or(raw).trim();
    if package.is_empty() || !package.contains('.') {
        return None;
    }
    Some(package.to_string())
}

fn read_process_status(pid: i32) -> Option<(i32, bool)> {
    let status = std_fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    let mut uid = None;
    let mut is_uninterruptible = false;
    let mut state_found = false;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = rest.split_whitespace().next()?.parse::<i32>().ok();
        } else if let Some(state) = line.strip_prefix("State:") {
            is_uninterruptible = state.trim_start().starts_with('D');
            state_found = true;
        } else {
            continue;
        }
        // status 后续还有几十行内存与信号字段，两个字段都拿到后不必继续扫描。
        if uid.is_some() && state_found {
            break;
        }
    }
    uid.map(|uid| (uid, is_uninterruptible))
}

fn log_uninterruptible_skip(proc: &AppProcess) {
    let count = UNINTERRUPTIBLE_SKIP_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 8 || count.is_multiple_of(UNINTERRUPTIBLE_SKIP_LOG_STEP) {
        log::warn!(
            "daemon skip uninterruptible process pid={} pkg={} n={}",
            proc.pid,
            proc.package_name,
            count
        );
    }
}
