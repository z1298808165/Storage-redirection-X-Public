use crate::config::{SettingsHub, watcher};
use crate::daemon_monitor::RegularAppMonitor;
use crate::daemon_mount::{MountOperation, MountRequest, execute_mount_request, has_mount_state};
use crate::logging::Logger;
use crate::platform;
use crate::redirect_policy as policy;
use crate::runtime_control;
use std::collections::HashSet;
use std::fs as std_fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const RECONCILE_INTERVAL_MS: u64 = 1000;
const PERIODIC_RECONCILE_INTERVAL_MS: i64 = 3_000;
const CONFIG_FINGERPRINT_FALLBACK_INTERVAL_MS: i64 = 10_000;
const INITIAL_RECONCILE_ROUNDS: usize = 3;
const PREWARM_RECONCILE_ROUNDS: usize = 1;
const PREWARM_MAX_REQUESTS: usize = 16;
const ANDROID_APP_UID_START: i32 = 10000;
const UNINTERRUPTIBLE_SKIP_LOG_STEP: u64 = 32;

static UNINTERRUPTIBLE_SKIP_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconcileMode {
    Prewarm,
    Full,
    MissingOnly,
}

pub fn main_entry() -> i32 {
    Logger::init(Some("srx_daemon"));
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
    let mut file_monitor = RegularAppMonitor::new();
    loop {
        if !runtime_control::is_module_runtime_enabled() {
            log::info!("daemon stop reason=runtime_disabled");
            return 0;
        }
        let before = config.config_version();
        let did_reload = reload_config_for_daemon(config, &mut last_fingerprint_check_ms);
        let current = config.config_version();
        let periodic_reconcile = should_periodic_reconcile(&mut last_periodic_reconcile_ms);
        let should_reconcile = round < INITIAL_RECONCILE_ROUNDS
            || did_reload
            || current != last_version
            || current != before
            || pending_full_reconcile
            || periodic_reconcile;
        if should_reconcile {
            policy::refresh_shared_uid_cache();
            let mode = if pending_full_reconcile {
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
            reconcile_running_apps(current, mode);
            last_version = current;
        }
        file_monitor.reconfigure(config);
        file_monitor.drain_events();
        round = round.saturating_add(1);
        thread::sleep(Duration::from_millis(RECONCILE_INTERVAL_MS));
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

fn reconcile_running_apps(config_version: u64, mode: ReconcileMode) {
    let started_ms = crate::platform::paths::monotonic_ms();
    let mut seen = HashSet::new();
    let mut applied = 0usize;
    let mut disabled = 0usize;
    let mut skipped = 0usize;
    let mut deferred = 0usize;
    let mut plans = Vec::new();

    for proc in list_app_processes() {
        let key = format!("{}:{}", proc.pid, proc.package_name);
        if !seen.insert(key) {
            continue;
        }
        if should_skip_process(&proc) {
            skipped += 1;
            continue;
        }

        let request = build_request(&proc, config_version);
        plans.push(ReconcilePlan::new(request));
    }

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
}

struct ReconcilePlan {
    request: MountRequest,
    has_mount_state: bool,
}

impl ReconcilePlan {
    fn new(request: MountRequest) -> Self {
        let has_mount_state = has_mount_state(&request);
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

fn build_request(proc: &AppProcess, config_version: u64) -> MountRequest {
    let config = SettingsHub::instance();
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
    ) = match config.get_resolved_user_profile_snapshot(&proc.package_name, proc.uid) {
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
        is_fuse_daemon_redirect_enabled: config.is_fuse_daemon_redirect_enabled(),
        is_file_monitor_enabled: config.is_file_monitor_enabled(),
        config_version,
    }
}

fn should_skip_process(proc: &AppProcess) -> bool {
    if proc.pid <= 0 || proc.uid < ANDROID_APP_UID_START {
        return true;
    }
    if is_process_uninterruptible(proc.pid) {
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
}

fn list_app_processes() -> Vec<AppProcess> {
    let mut processes = Vec::new();
    let Ok(entries) = std_fs::read_dir("/proc") else {
        return processes;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let Some(package_name) = read_process_package(pid) else {
            continue;
        };
        let Some(uid) = read_process_uid(pid) else {
            continue;
        };
        processes.push(AppProcess {
            pid,
            uid,
            package_name,
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

fn read_process_uid(pid: i32) -> Option<i32> {
    let status = std_fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        let uid_text = rest.split_whitespace().next()?;
        return uid_text.parse::<i32>().ok();
    }
    None
}

fn is_process_uninterruptible(pid: i32) -> bool {
    let Ok(status) = std_fs::read_to_string(format!("/proc/{}/status", pid)) else {
        return false;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .map(|state| state.trim_start().starts_with('D'))
        .unwrap_or(false)
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
