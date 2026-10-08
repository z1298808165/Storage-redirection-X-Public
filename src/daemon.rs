use crate::config::{SettingsHub, watcher};

#[path = "daemon/media_hook_heal.rs"]
mod media_hook_heal;
use crate::daemon_monitor::RegularAppMonitor;
use crate::daemon_mount::{
    MountOperation, MountRequest, cleanup_all_mount_states, execute_mount_request,
    has_healthy_mount_state, has_mount_state, invalidate_pre_registered_host_policies,
    prune_stale_mount_states,
};
use crate::logging::Logger;
use crate::platform;
use crate::redirect_policy as policy;
use crate::runtime_control;
use std::collections::HashSet;
use std::fs::{self as std_fs, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 周期兜底 reconcile 的间隔。应用启动由 companion 主动挂载 + register-policy 定点
/// 登记，配置变更由 inotify 监听，开机由全量预登记兜底——这些都不依赖周期轮询。
/// 周期轮询只剩「补挂 companion 未处理的进程 / 宿主重建后重新接入 / 清理退出残留」
/// 等补偿职责，把间隔从 3 秒放宽到 30 秒可显著降低常驻唤醒与 /proc 扫描功耗，
/// 补偿类场景最坏多等一个周期（30 秒），不影响事件驱动主路径的即时性。
const PERIODIC_RECONCILE_INTERVAL_MS: i64 = 30_000;
/// 状态文件清理只防「已死进程的记录无限累积」，不参与挂载正确性；
/// 独立节流，不随周期 reconcile 一起触发，避免空闲时每个周期都扫描状态目录和 /proc。
const PRUNE_INTERVAL_MS: i64 = 30_000;
const CONFIG_FINGERPRINT_FALLBACK_INTERVAL_MS: i64 = 10_000;
/// 降级路径单轮最多连续排空的次数，避免挤占同一循环内的 reconcile。
const FALLBACK_DRAIN_ROUNDS: usize = 4;
/// 全量 reconcile 的单轮预算。超过后把剩余计划留给下一轮，给配置/控制事件让路。
/// 每轮只执行一个计划是刻意为之：挂载执行同步阻塞主循环（实测单个 40ms~3.5s），
/// 一轮内连做多个会把 companion 的控制请求挤出关键窗口——应用挂载确认后 FUSE
/// 后端尚未就绪，写入真实/映射目标直接 ENOENT/EROFS（真机 A/B 实测：单轮 3 个
/// 挂载共 3.3 秒的场景 4/9/12/15 复现失败，单轮 1 个则全绿）。
const RECONCILE_BATCH_MAX_PLANS: usize = 1;
/// 续跑等待上限。批次未完成时主循环仍要尽快续跑，但 0 毫秒的 poll 等于全速自旋：
/// 未配置应用的计划既不 applied 也不 current，一批 27 个计划要在 poll(0) 下以
/// 每轮一次 /proc 全量扫描的速度无限排空，烧 CPU 且反复触碰挂载决策。配置、宿主
/// 与控制事件本身会立即唤醒 poll；200ms 只兜"排空剩余计划"，足够快且封顶自旋。
const RECONCILE_CONTINUE_WAIT_MS: libc::c_int = 200;
const FILE_MONITOR_SYNC_TIMEOUT_MS: i64 = 2_000;
const INITIAL_RECONCILE_ROUNDS: usize = 3;
const PREWARM_RECONCILE_ROUNDS: usize = 1;
const PREWARM_MAX_REQUESTS: usize = 16;
const ANDROID_APP_UID_START: i32 = 10000;
const UNINTERRUPTIBLE_SKIP_LOG_STEP: u64 = 32;
/// 周期 reconcile 摘要在计数没有变化时的记录间隔（每轮 30 秒，约 50 分钟）。
const RECONCILE_SUMMARY_LOG_HEARTBEAT: u64 = 100;

static UNINTERRUPTIBLE_SKIP_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

/// 上一轮 reconcile 观察到的 MediaProvider 进程 pid 集合（排序后）。
///
/// 集合变化即 MediaProvider 发生重启/换代：宿主命名空间里的 media 视图绑定仍指向
/// 已死亡的旧 FUSE 连接，必须清空宿主策略预登记指纹表，让本轮预登记全量重跑并
/// 重新绑定新连接。集合稳定时不清空，配合预登记指纹实现稳态零 fork。
static LAST_MEDIA_PROVIDER_PIDS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// 开机全量预登记是否已在 MediaProvider 就绪后补登记过一次。
///
/// 开机全量预登记发生在 daemon 启动早期，此时 MediaProvider 尚未重建应用私有目录、
/// SELinux 标签也未就位，沙箱目录 mkdir 会失败，冷启动应用只能回退 scoped。等
/// MediaProvider 进程出现后补登记一次（幂等，已成功者跳过），覆盖首次失败的应用。
static PRE_REGISTER_ALL_AFTER_MEDIA_READY: AtomicBool = AtomicBool::new(false);

/// 周期 reconcile 摘要的记录状态。
///
/// 每轮都按 info 记录摘要会让调试日志以秒级速度增长，把真正需要排查的挂载事件
/// 挤出 tail 窗口。这里只在摘要计数变化时记录，长期不变时按心跳间隔补充一次。
struct ReconcileSummaryLogState {
    signature: String,
    count: u64,
}

static RECONCILE_SUMMARY_LOG_STATE: Mutex<ReconcileSummaryLogState> =
    Mutex::new(ReconcileSummaryLogState {
        signature: String::new(),
        count: 0,
    });

fn should_log_reconcile_summary(
    mode: ReconcileMode,
    config_version: u64,
    planned: usize,
    applied: usize,
    disabled: usize,
    skipped: usize,
    deferred: usize,
) -> bool {
    let signature = format!(
        "{:?}:{:x}:{}:{}:{}:{}:{}",
        mode, config_version, planned, applied, disabled, skipped, deferred
    );
    let mut state = RECONCILE_SUMMARY_LOG_STATE
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if state.signature != signature {
        state.signature = signature;
        return true;
    }
    state.count = state.count.saturating_add(1);
    state.count.is_multiple_of(RECONCILE_SUMMARY_LOG_HEARTBEAT)
}

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
    /// 兼容同步等待方的请求通知；事件线程主要依赖下面的 eventfd。
    request_signal: Condvar,
    /// eventfd 是监视线程的事件源，避免为重建请求周期性超时唤醒。
    request_event_fd: i32,
}

impl Drop for FileMonitorSync {
    fn drop(&mut self) {
        if self.request_event_fd >= 0 {
            // SAFETY: eventfd 由该同步对象创建并独占持有，Drop 后不再有等待线程使用它。
            unsafe { libc::close(self.request_event_fd) };
        }
    }
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
        if self.request_event_fd >= 0 {
            let value = 1u64.to_ne_bytes();
            // SAFETY: eventfd 接受固定 8 字节计数值；fd 由 FileMonitorSync 持有至 daemon 结束。
            unsafe {
                libc::write(
                    self.request_event_fd,
                    value.as_ptr() as *const libc::c_void,
                    value.len(),
                );
            }
        }
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
    /// 显式请求触发的强制重挂。
    ///
    /// 与 `Full` 的区别是**不做幂等跳过**：诊断与测试流要的就是「立刻按当前配置重挂一遍」，
    /// 若这里也跳过，显式请求会静默变成空操作。
    Forced,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ReconcilePlanIdentity {
    package_name: String,
    pid: i32,
    uid: i32,
    start_time_ticks: Option<u64>,
}

#[derive(Default)]
struct ReconcileCursor {
    last_attempted: Option<ReconcilePlanIdentity>,
}

impl ReconcileCursor {
    fn next_start_index(&self, identities: &[ReconcilePlanIdentity]) -> usize {
        if identities.is_empty() {
            return 0;
        }
        self.last_attempted.as_ref().map_or(0, |last| {
            identities
                .iter()
                .position(|identity| identity == last)
                .map_or_else(
                    || {
                        identities
                            .iter()
                            .position(|identity| identity > last)
                            .unwrap_or(0)
                    },
                    |index| (index + 1) % identities.len(),
                )
        })
    }

    fn advance(&mut self, plan: &ReconcilePlan) {
        self.last_attempted = Some(plan.identity());
    }
}

struct DaemonInstanceLock {
    _file: File,
}

impl DaemonInstanceLock {
    fn acquire() -> io::Result<Option<Self>> {
        let lock_path =
            std::path::Path::new(crate::platform::module_paths::DAEMON_INSTANCE_LOCK_FILE);
        if let Some(parent) = lock_path.parent() {
            std_fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(crate::platform::module_paths::DAEMON_INSTANCE_LOCK_FILE)?;
        // SAFETY: as_raw_fd() 来源于 file 持有的有效文件描述符；非阻塞独占锁的持有
        // 时间与 DaemonInstanceLock 生命周期一致。
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(errno) if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK)
        {
            return Ok(None);
        }
        Err(error)
    }
}

/// 安装 daemon 主进程的 panic 钩子。
///
/// release 构建 `panic = "abort"`，`catch_unwind` 无法截获 panic，service.sh 又把
/// stderr 丢到 `/dev/null`，主进程 panic 不会留下任何记录（宿主子进程有
/// `install_host_panic_hook`，主进程没有）。钩子在 abort 之前把 panic 位置写到独立
/// 文件，便于崩溃后定位根因；写到独立文件而非 running.log，是因为 running.log 由
/// collector 进程按 socket 事件维护，直接写会与其文件偏移竞争。
fn install_daemon_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crate::platform::module_paths::DAEMON_PANIC_LOG)
        {
            let _ = std::io::Write::write_fmt(
                &mut file,
                format_args!("daemon panic at {}: {}\n", location, message),
            );
        }
    }));
}

pub fn main_entry() -> i32 {
    Logger::init(Some("srx_daemon"));
    install_daemon_panic_hook();
    let _instance_lock = match DaemonInstanceLock::acquire() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            log::info!("daemon exit reason=already_running");
            return 0;
        }
        Err(error) => {
            log::error!("daemon instance lock acquire failed error={}", error);
            return 1;
        }
    };
    if let Err(error) = crate::log_daemon::start() {
        log::error!("private log writer start failed error={}", error);
        return 1;
    }
    log::info!("daemon start");

    if !runtime_control::is_module_runtime_enabled() {
        cleanup_all_mount_states();
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

    // 建立共享宿主 FUSE 会话。失败只记录并继续，不影响主循环与既有 scoped 路径；
    // 后续 reconcile 会在宿主子进程死亡后按需恢复，而不是继续使用失效句柄。
    if !crate::fuse_host::ensure_global() {
        log::warn!("fuse host session unavailable, scoped path remains active");
    } else {
        // 宿主就绪后立即全量预登记所有已配置应用：让应用冷启动时无需等待 reconcile
        // 轮询到该进程，就能从宿主快照确认 uid 并接入共享会话，避免 scoped 竞态与
        // 启动窗口。宿主未就绪时跳过（此时预登记必然失败，逐个触发等待反而拖慢启动）。
        pre_register_all_configured_apps(config, config.config_version());
    }

    let mut last_version = 0;
    let mut last_fingerprint_check_ms = crate::platform::paths::monotonic_ms();
    let mut last_periodic_reconcile_ms = crate::platform::paths::monotonic_ms();
    let mut round: usize = 0;
    let mut pending_full_reconcile = false;
    let mut reconcile_cursor = ReconcileCursor::default();
    let mut last_host_generation = crate::fuse_host::generation();
    // SAFETY: eventfd 只创建内核对象并返回 fd，不接触调用方内存；标志位均为合法常量。
    let control_wake_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    crate::log_daemon::set_daemon_wake_eventfd(control_wake_fd);
    let file_monitor_sync = start_file_monitor_thread();
    let mut fallback_file_monitor = file_monitor_sync.is_none().then(RegularAppMonitor::new);
    loop {
        if !runtime_control::is_module_runtime_enabled() {
            cleanup_all_mount_states();
            log::info!("daemon stop reason=runtime_disabled");
            return 0;
        }
        let before = config.config_version();
        let (did_reload, changed_packages, full_change) =
            reload_config_for_daemon(config, &mut last_fingerprint_check_ms);
        let current = config.config_version();
        let control_reconcile = crate::log_daemon::take_reconcile_request();
        let policy_register = crate::log_daemon::take_policy_register_request();
        let periodic_reconcile = should_periodic_reconcile(&mut last_periodic_reconcile_ms);
        let host_generation = crate::fuse_host::generation();
        let host_changed = host_generation != last_host_generation;
        if host_changed {
            last_host_generation = host_generation;
        }
        let should_reconcile = host_changed
            || round < INITIAL_RECONCILE_ROUNDS
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
            // 监视器重建是后台最终一致任务，不能阻塞配置变化对应的挂载应用。
            // 旧路径在这里同步等待最多 2 秒，正是 MT 开关后规则迟迟不生效的主要来源。
            policy::refresh_shared_uid_cache();
            let mode = if control_reconcile.is_some() {
                pending_full_reconcile = false;
                ReconcileMode::Forced
            } else if host_changed {
                pending_full_reconcile = true;
                ReconcileMode::Full
            } else if full_change {
                // global/filter 变化必须全量，但同批 apps 包会在计划排序时优先处理。
                ReconcileMode::Full
            } else if changed_packages.is_some() {
                // 单个 apps/<package>.json 变化只处理该包；若此前已有未完成全量批次，
                // 保留 pending 标记，待该增量事件完成后继续剩余全量计划。
                ReconcileMode::Full
            } else if pending_full_reconcile {
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
            let (mounts_changed, reconcile_incomplete) = reconcile_running_apps(
                current,
                mode,
                changed_packages.as_deref(),
                full_change,
                &mut reconcile_cursor,
            );
            if reconcile_incomplete {
                // 仅全量路径可以安全在下一轮继续；apps/<package>.json 增量路径已限制为
                // 目标包，不把未完成批次升级成全量重挂。
                pending_full_reconcile = true;
            }
            if let Some(request) = control_reconcile.as_deref() {
                if reconcile_incomplete {
                    log::info!(
                        "running app remount batch deferred request={} applied={}",
                        request,
                        mounts_changed
                    );
                } else {
                    log::info!(
                        "running app remount completed request={} applied={}",
                        request,
                        mounts_changed
                    );
                }
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
        if let Some(token) = policy_register.as_deref() {
            register_policy_for_pid_token(token, current);
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
        let host_exited = wait_for_daemon_events(
            watcher::event_fd(),
            pending_full_reconcile,
            crate::fuse_host::pidfd(),
            control_wake_fd,
        );
        if host_exited {
            let cleared = crate::fuse_host::clear_if_dead_with_reason("pidfd");
            pending_full_reconcile = true;
            log::warn!(
                "daemon shared fuse host exit event observed cleared={} recovery scheduled",
                cleared
            );
        }
    }
}

/// 阻塞等待配置事件；超时后返回主循环执行低频宿主/状态兜底。
///
/// inotify fd 可读时下一轮会立即解析增量配置；watcher 不可用时退回有限超时，
/// 保留 fingerprint fallback 和周期自愈，不会因事件源失效而永久停摆。
fn wait_for_daemon_events(
    config_watch_fd: i32,
    reconcile_pending: bool,
    host_pidfd: i32,
    control_wake_fd: i32,
) -> bool {
    let timeout_ms: libc::c_int = if reconcile_pending {
        RECONCILE_CONTINUE_WAIT_MS
    } else {
        30_000
    };
    if config_watch_fd < 0 && host_pidfd < 0 && control_wake_fd < 0 {
        if timeout_ms > 0 {
            thread::sleep(Duration::from_millis(timeout_ms as u64));
        }
        return false;
    }
    let mut fds = [
        libc::pollfd {
            fd: config_watch_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: host_pidfd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: control_wake_fd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // 任选 fd 缺席（负值）时 poll 会忽略对应条目；只在两者都缺席时才缩短轮询窗口。
    let nfds = if config_watch_fd >= 0 || control_wake_fd >= 0 {
        3
    } else {
        2
    };
    // SAFETY: fds 指向已初始化 pollfd 数组；超时只是低频兜底，不持有 Rust 借用跨调用。
    let result = unsafe { libc::poll(fds.as_mut_ptr(), nfds, timeout_ms) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            return false;
        }
        log::warn!(
            "daemon event poll failed error={} config_fd={} host_pidfd={} control_fd={}",
            error,
            config_watch_fd,
            host_pidfd,
            control_wake_fd
        );
        return false;
    }
    let host_revents = fds[1].revents;
    if host_pidfd >= 0 && host_revents != 0 {
        log::info!(
            "daemon host pidfd event poll_result={} pidfd={} revents={:#x}",
            result,
            host_pidfd,
            host_revents
        );
    }
    if control_wake_fd >= 0 && (fds[2].revents & libc::POLLIN) != 0 {
        let mut value = 0u64;
        // SAFETY: eventfd read buffer固定为8字节；daemon 独占该唤醒 fd。
        unsafe {
            libc::read(
                control_wake_fd,
                &mut value as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
    host_pidfd >= 0
        && fds[1].fd == host_pidfd
        && (host_revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0
}

fn start_file_monitor_thread() -> Option<Arc<FileMonitorSync>> {
    // SAFETY: eventfd 只创建内核对象并返回 fd，不接触调用方内存；标志位均为合法常量。
    let request_event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    let sync = Arc::new(FileMonitorSync {
        configured_version: AtomicU64::new(0),
        requested_rebuild: AtomicU64::new(0),
        completed_rebuild: AtomicU64::new(0),
        signal_lock: Mutex::new(()),
        progress_signal: Condvar::new(),
        request_signal: Condvar::new(),
        request_event_fd,
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
                    wait_for_file_monitor_events(&thread_sync, &file_monitor);
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

fn wait_for_file_monitor_events(sync: &FileMonitorSync, monitor: &RegularAppMonitor) {
    if monitor.event_fd() < 0 && sync.request_event_fd < 0 {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut fds = [
        libc::pollfd {
            fd: monitor.event_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: sync.request_event_fd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // inotify 是主事件源，eventfd 唤醒配置重建；30 秒超时只作为 runtime disable 的兜底。
    // SAFETY: fds 是本栈帧上的合法 pollfd 数组，poll 只在其中写入 revents；
    // nfds_t 转换不会截断（数组长度恒为 2）。
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 30_000) };
    if result < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
        return;
    }
    if fds[1].revents & libc::POLLIN != 0 && sync.request_event_fd >= 0 {
        let mut value = 0u64;
        // SAFETY: eventfd read buffer固定为8字节，fd由同步状态持有。
        unsafe {
            libc::read(
                sync.request_event_fd,
                &mut value as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

fn request_file_monitor_rebuild(sync: &FileMonitorSync) {
    // 监视树重建只影响后续文件监视，不影响当前应用挂载策略；异步排队后立即返回，
    // 避免把一次 MT 配置切换再阻塞在最多 2 秒的监视器 ACK 上。
    sync.requested_rebuild.fetch_add(1, Ordering::AcqRel);
    sync.notify_request();
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

fn reload_config_for_daemon(
    config: &SettingsHub,
    last_fingerprint_check_ms: &mut i64,
) -> (bool, Option<Vec<String>>, bool) {
    match watcher::poll_changed_with_packages() {
        watcher::ChangeKind::Apps(packages) => {
            *last_fingerprint_check_ms = crate::platform::paths::monotonic_ms();
            return (config.reload_force(), Some(packages), false);
        }
        watcher::ChangeKind::Full(priority_packages) => {
            *last_fingerprint_check_ms = crate::platform::paths::monotonic_ms();
            return (config.reload_force(), Some(priority_packages), true);
        }
        watcher::ChangeKind::None => {}
    }

    let now_ms = crate::platform::paths::monotonic_ms();
    if now_ms.saturating_sub(*last_fingerprint_check_ms) < CONFIG_FINGERPRINT_FALLBACK_INTERVAL_MS {
        return (false, None, false);
    }

    *last_fingerprint_check_ms = now_ms;
    let before = config.config_version();
    let _ = config.reload_if_changed();
    (
        config.config_version() != before,
        None,
        config.config_version() != before,
    )
}

/// 按 [`PRUNE_INTERVAL_MS`] 节流执行三类过期记录清理。
///
/// reconcile 是常驻循环里唯一的空闲活动，清理要扫状态目录并对每个记录查一次
/// `/proc/<pid>/stat`；被清理的对象本身不参与挂载正确性，延迟一个窗口没有影响，
/// 因此把这部分开销从每 3 秒一次降到每 30 秒一次。主循环是单线程调用，
/// 这里只用原子量记录上次执行时间即可。
fn prune_stale_states_throttled() {
    static LAST_PRUNE_MS: AtomicI64 = AtomicI64::new(0);
    let now_ms = crate::platform::paths::monotonic_ms();
    if now_ms.saturating_sub(LAST_PRUNE_MS.load(Ordering::Relaxed)) < PRUNE_INTERVAL_MS {
        return;
    }
    LAST_PRUNE_MS.store(now_ms, Ordering::Relaxed);
    prune_stale_mount_states();
    crate::mount_intent::prune_stale();
    crate::mount_identity::prune_stale();
}

fn reconcile_running_apps(
    config_version: u64,
    mode: ReconcileMode,
    changed_packages: Option<&[String]>,
    full_change: bool,
    cursor: &mut ReconcileCursor,
) -> (bool, bool) {
    // 共享宿主死亡时先尝试恢复；失败则让各挂载请求继续走 scoped FUSE 回退。
    // 该动作只在 reconcile 入口执行一次，避免每个应用计划重复 fork 宿主。
    let _host_ready = crate::fuse_host::ensure_global();
    let started_ms = crate::platform::paths::monotonic_ms();
    prune_stale_states_throttled();
    let mut seen = HashSet::new();
    let mut applied = 0usize;
    let mut disabled = 0usize;
    let mut skipped = 0usize;
    let mut deferred = 0usize;
    let mut batch_requests = 0usize;
    let mut reconcile_incomplete = false;
    let mut plans = Vec::new();
    let mut media_processes = Vec::new();
    let mut media_like_names: Vec<String> = Vec::new();
    let config_snapshot = SettingsHub::instance().get_daemon_reconcile_config_snapshot();

    for proc in list_app_processes() {
        // /proc 目录项本身按 pid 唯一，pid 足以去重，无需再拼接包名分配字符串。
        if !seen.insert(proc.pid) {
            continue;
        }
        if !full_change
            && let Some(packages) = changed_packages
            && !packages.iter().any(|package| package == &proc.package_name)
        {
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

    // 单个 apps/<package>.json 增量事件不改变 MediaProvider 进程集合；跳过媒体自愈和
    // PID 换代判定，避免一次 MT 配置切换清空全局宿主预登记缓存并触碰媒体视图。
    let media_related_change = full_change
        || changed_packages.is_none()
        || changed_packages.is_some_and(|packages| {
            packages.iter().any(|package| {
                media_hook_heal::is_media_provider_process(package)
                    || package.contains("providers.media")
                    || package.contains("process.media")
            })
        });
    let media_ready = if media_related_change {
        media_hook_heal::heal_if_needed(
            SettingsHub::instance(),
            &media_processes,
            &media_like_names,
        );
        // MediaProvider 换代检测必须先于预登记循环：本轮就要用重绑后的视图登记策略。
        let mut media_pids: Vec<i32> = media_processes.iter().map(|(pid, _)| *pid).collect();
        media_pids.sort_unstable();
        let mut last_media_pids = LAST_MEDIA_PROVIDER_PIDS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *last_media_pids != media_pids {
            *last_media_pids = media_pids;
            drop(last_media_pids);
            invalidate_pre_registered_host_policies();
        }
        !media_processes.is_empty()
    } else {
        false
    };

    // 开机全量预登记在 boot 早期执行时，MediaProvider 尚未重建应用私有目录、SELinux
    // 标签未就位，沙箱目录 mkdir 会失败（冷启动应用回退 scoped）。等 MediaProvider
    // 进程出现后补登记一次，覆盖首次失败的应用；幂等由 pre_register_host_policy 的
    // 指纹命中保证，已成功者跳过，稳态零开销。
    if media_ready && !PRE_REGISTER_ALL_AFTER_MEDIA_READY.swap(true, Ordering::AcqRel) {
        pre_register_all_configured_apps(SettingsHub::instance(), config_version);
    }

    let incremental_change = changed_packages.is_some() && !full_change;
    let uses_batch_cursor = !incremental_change && mode != ReconcileMode::Forced;

    // companion 与 daemon 可能同时为同一应用发起挂载。全量路径先登记本轮 Auto 应用策略，
    // 让 companion 能从宿主快照确认 uid 后直接接入共享会话。单包增量路径由快速热更新或
    // 完整挂载流程各自完成一次策略注册，跳过这里，避免同一 MT 切换重复触碰宿主控制通道。
    if !incremental_change {
        for plan in &plans {
            if crate::daemon_mount::pre_register_host_policy(&plan.request)
                == crate::daemon_mount::PreRegisterOutcome::Registered
            {
                log::debug!(
                    "daemon pre-registered fuse host policy pid={} uid={} pkg={}",
                    plan.request.pid,
                    plan.request.uid,
                    plan.request.package_name
                );
            }
        }
    }

    plans.sort_by_key(|plan| plan.identity());
    if full_change {
        if let Some(priority_packages) = changed_packages {
            plans.sort_by_key(|plan| {
                (
                    if priority_packages
                        .iter()
                        .any(|package| package == &plan.request.package_name)
                    {
                        0u8
                    } else {
                        1u8
                    },
                    plan.identity(),
                )
            });
        }
    } else if mode == ReconcileMode::Prewarm {
        plans.sort_by_key(|plan| (plan.priority(), plan.identity()));
    }

    let start_index = if uses_batch_cursor && !(full_change && changed_packages.is_some()) {
        cursor.next_start_index(
            &plans
                .iter()
                .map(ReconcilePlan::identity)
                .collect::<Vec<_>>(),
        )
    } else {
        0
    };
    for offset in 0..plans.len() {
        let index = (start_index + offset) % plans.len();
        let plan = &plans[index];
        // 增量配置必须先尝试共享宿主 fast update：companion 可能已经把状态文件指纹
        // 更新为新配置，若先走幂等跳过，会把“策略尚未登记”误判成 current，宿主仍持有旧
        // UID 策略，随后应用访问映射路径只得到 ENOENT。fast update 成功后无需重挂；失败
        // 才继续下面的幂等判定与完整恢复路径。
        if incremental_change && crate::daemon_mount::try_fast_update_shared_host(&plan.request) {
            applied += 1;
            continue;
        }
        // 幂等跳过必须排在完整挂载分支之前：配置未变且挂载健康时，任何模式下的重挂都只会
        // 叠加挂载层。`Forced`（显式请求）例外——诊断与测试流要的就是无条件重挂。
        if mode != ReconcileMode::Forced && plan.should_skip_as_current() {
            skipped += 1;
            continue;
        }
        if mode == ReconcileMode::Prewarm
            && (offset >= PREWARM_MAX_REQUESTS || !plan.should_run_in_prewarm())
        {
            deferred += 1;
            continue;
        }
        if mode == ReconcileMode::MissingOnly && !plan.should_run_in_missing_only() {
            skipped += 1;
            continue;
        }
        if !incremental_change
            && mode != ReconcileMode::Forced
            && batch_requests >= RECONCILE_BATCH_MAX_PLANS
        {
            deferred += 1;
            reconcile_incomplete = true;
            continue;
        }
        batch_requests += 1;
        // 无论后续快速更新或完整挂载成功与否，都把当前计划记为已尝试；失败由
        // reconcile_incomplete 保证下一轮重试，但不会再次占住本轮唯一 slot。
        if uses_batch_cursor {
            cursor.advance(plan);
        }
        // 共享宿主 fast update 已在幂等判定之前执行；走到这里表示宿主快速更新失败，
        // 后续完整清理/重建路径负责恢复挂载和策略。
        // 恢复动作由账本归属与端点健康共同决定：被判定为"不摘除"或"已收敛"的命名空间
        // 不能继续注入，否则就是在死连接上叠加新的挂载层。这里在真正执行前取一次监督结论，
        // 既作为执行门禁，也把判定依据写进日志。
        if let Some(snapshot) = crate::daemon_mount::supervise_mount_request(&plan.request)
            && !snapshot.last_action.allows_inject()
        {
            log::warn!(
                "daemon reconcile skip inject {}",
                crate::fuse_supervisor::render_namespace(&snapshot)
            );
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
                } else if uses_batch_cursor {
                    reconcile_incomplete = true;
                }
            }
            MountOperation::Disable => {
                if plan.has_mount_state {
                    if execute_mount_request(&plan.request) {
                        disabled += 1;
                    } else if uses_batch_cursor {
                        reconcile_incomplete = true;
                    }
                } else {
                    skipped += 1;
                }
            }
        }
    }

    if should_log_reconcile_summary(
        mode,
        config_version,
        plans.len(),
        applied,
        disabled,
        skipped,
        deferred,
    ) {
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
        // 监督计数只在确实发生过恢复行为时输出，避免每轮都刷同样的零值。
        let summary = crate::fuse_supervisor::SupervisorSummary::snapshot();
        if summary.has_activity() {
            log::info!("daemon supervisor {}", summary.render());
        }
    }
    (applied > 0 || disabled > 0, reconcile_incomplete)
}

struct ReconcilePlan {
    request: MountRequest,
    start_time_ticks: Option<u64>,
    has_mount_state: bool,
    /// 状态健康且记录的配置指纹与当前配置一致：本轮配置已经落地。
    is_mount_current: bool,
}

impl ReconcilePlan {
    fn new(request: MountRequest, check_mount_targets: bool) -> Self {
        let has_mount_state = if check_mount_targets {
            has_healthy_mount_state(&request)
        } else {
            has_mount_state(&request)
        };
        let is_mount_current = crate::daemon_mount::has_current_mount_state(&request);
        let start_time_ticks = crate::platform::process_start_time_ticks(request.pid);
        Self {
            request,
            start_time_ticks,
            has_mount_state,
            is_mount_current,
        }
    }

    fn identity(&self) -> ReconcilePlanIdentity {
        ReconcilePlanIdentity {
            package_name: self.request.package_name.clone(),
            pid: self.request.pid,
            uid: self.request.uid,
            start_time_ticks: self.start_time_ticks,
        }
    }

    fn should_run_in_prewarm(&self) -> bool {
        self.request.operation == MountOperation::Reload || self.has_mount_state
    }

    fn should_run_in_missing_only(&self) -> bool {
        self.request.operation == MountOperation::Reload && !self.has_mount_state
    }

    /// 幂等跳过：该应用的挂载已按当前配置建立，重挂只会叠加挂载层。
    ///
    /// `Full` 轮次过去没有这条判据，对每个运行中的应用无条件重挂。开机时应用自身 specialize
    /// 挂一次、Prewarm 挂一次、随后两轮 Full 再各挂一次，同一个进程的命名空间里因此叠出
    /// 2~3 层模块挂载；叠加层的 `root` 解析基准不同，应用读到的目录内容随层数变化。这是
    /// 「目录内容时而正确时而错误」与「挂载层数无上限增长」的共同根因。
    ///
    /// 只在配置指纹变化、挂载目标消失或 FUSE 子进程死亡时才重挂——那正是需要重挂的情形。
    fn should_skip_as_current(&self) -> bool {
        self.is_mount_current
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
    let is_monitor_only = snapshot
        .resolve_profile(&proc.package_name, proc.uid)
        .is_none()
        && crate::config::should_capture_unconfigured_app(
            &proc.package_name,
            proc.uid,
            snapshot.is_file_monitor_enabled,
        );
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
        None if is_monitor_only => (
            MountOperation::Reload,
            platform::user_id_from_uid(proc.uid),
            platform::paths::storage_user_root_for_user(platform::user_id_from_uid(proc.uid)),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
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

    let mut request = MountRequest {
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
        is_monitor_only,
        storage_backend_mode: if is_monitor_only {
            crate::config::StorageBackendMode::Fuse
        } else {
            snapshot.storage_backend_mode
        },
        is_file_monitor_enabled: snapshot.is_file_monitor_enabled,
        config_version,
        policy_fingerprint: 0,
    };
    request.policy_fingerprint = crate::fuse_redirect::request_policy_fingerprint(&request);
    request
}

/// 读 `/data/system/packages.list` 解析包名到 uid 的映射。
///
/// 开机全量预登记需要为未运行的应用解析 uid（宿主策略按 uid 注册），packages.list
/// 是系统维护的权威映射。找不到（应用未安装或已被卸载）时跳过，应用真正启动后仍会
/// 由 reconcile 逐轮预登记或 companion 的 register-policy 定点登记补上。
fn read_package_uid(package_name: &str) -> Option<i32> {
    let content = std_fs::read_to_string("/data/system/packages.list").ok()?;
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some(package_name) {
            return fields.next().and_then(|value| value.parse::<i32>().ok());
        }
    }
    None
}

/// 开机一次性预登记所有已配置应用的宿主策略。
///
/// 只写策略、不创建挂载，不依赖应用进程是否运行。它与 reconcile 的逐轮预登记、
/// companion 的 register-policy 定点登记构成三层覆盖：开机全量兜底 + 运行期轮询 +
/// 冷启动定点，使应用任何时刻启动都能从宿主快照确认 uid 并直接接入共享会话，
/// 不再因「宿主快照还没这个 uid」回退 scoped、也不再经历 scoped 挂载竞态。
fn pre_register_all_configured_apps(config: &SettingsHub, config_version: u64) {
    let snapshot = config.get_daemon_reconcile_config_snapshot();
    for package_name in snapshot.configured_package_names() {
        let Some(uid) = read_package_uid(package_name) else {
            continue;
        };
        if uid < ANDROID_APP_UID_START {
            continue;
        }
        let proc = AppProcess {
            pid: 0,
            uid,
            package_name: package_name.clone(),
            is_uninterruptible: false,
        };
        let request = build_request(&proc, config_version, &snapshot);
        if request.operation != MountOperation::Reload {
            continue;
        }
        crate::daemon_mount::pre_register_host_policy(&request);
    }
}

/// 处理 companion 的 `register-policy:<pkg>:<pid>` 请求：定点完成宿主策略预登记。
///
/// companion 发起挂载时 daemon 可能还没轮询到该应用；周期 reconcile 最坏 3s+，
/// 应用侧裸视图窗口随之拉长。这里按 pid 定位进程后走与 reconcile 完全相同的
/// build_request + pre_register_host_policy 路径，只登记不挂载，单次开销毫秒级。
fn register_policy_for_pid_token(token: &str, config_version: u64) {
    let Some((package_name, pid)) = token
        .rsplit_once(':')
        .and_then(|(package, pid)| pid.parse::<i32>().ok().map(|pid| (package, pid)))
    else {
        log::warn!("register-policy token invalid token={}", token);
        return;
    };
    let Some(proc) = list_app_processes()
        .into_iter()
        .find(|proc| proc.pid == pid && proc.package_name == package_name)
    else {
        log::debug!(
            "register-policy process gone pkg={} pid={}",
            package_name,
            pid
        );
        return;
    };
    if should_skip_process(&proc) {
        return;
    }
    let snapshot = SettingsHub::instance().get_daemon_reconcile_config_snapshot();
    let request = build_request(&proc, config_version, &snapshot);
    let outcome = crate::daemon_mount::pre_register_host_policy(&request);
    log::info!(
        "register-policy applied pkg={} pid={} outcome={:?} fingerprint={:016x}",
        request.package_name,
        request.pid,
        outcome,
        request.policy_fingerprint
    );
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
        // 该计数按日志节流统计全部被跳过的进程，不是单个进程被跳过的次数。
        log::warn!(
            "daemon skip uninterruptible process pid={} pkg={} skipped_total={}",
            proc.pid,
            proc.package_name,
            count
        );
    }
}
