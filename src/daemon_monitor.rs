#[path = "daemon_monitor/events.rs"]
mod events;
#[path = "daemon_monitor/inotify.rs"]
mod inotify;
#[path = "daemon_monitor/roots.rs"]
mod roots;

use crate::config::SettingsHub;
use crate::platform::inotify::Event;
use crate::platform::paths;
use events::{
    MonitorEventPaths, emit_monitor_event, monitor_operation_from_mask,
    repair_monitored_backend_owner, resolve_monitor_identity, should_filter_display_path,
    should_skip_ambiguous_allowed_real_path_event, should_skip_ambiguous_read_only_path_event,
    should_skip_public_root_event_identity,
};
use roots::{
    build_private_owner_repair_roots, build_public_owner_repair_root, build_watch_roots,
    dedup_roots, is_under_any_root, map_record_from_path, select_watch_start,
    should_descend_into_child, should_record_display_path, sort_roots_by_monitor_priority,
};
use std::collections::{HashMap, HashSet, VecDeque};

const DUPLICATE_EVENT_WINDOW_MS: i64 = 1500;
const MISSING_ROOT_RETRY_MS: i64 = 1000;
const MAX_RECENT_EVENTS: usize = 512;
const DEFAULT_MAX_WATCHES: usize = 8192;
/// 防止设备把 inotify 配额调得过大后，监视器一次性递归展开整个共享存储。
const MAX_WATCHES_CEILING: usize = 32768;
/// 单轮 `drain_events` 最多处理的事件数。
///
/// 事件掩码包含 `IN_MODIFY`，持续写入会按写调用产生事件。若一直读到 EAGAIN 才返回，
/// 监视线程会长时间停在排空循环内，使配置重建、溢出补偿和版本推进被无限推迟。
/// 达到预算即返回，让调用方有机会处理重建，剩余事件留在内核队列下一轮继续读。
const MAX_EVENTS_PER_DRAIN: usize = 4096;
/// reconfigure 关闭旧 inotify fd 前的排空轮次上限，每轮再受 [`MAX_EVENTS_PER_DRAIN`] 约束。
///
/// 排空不能无限进行：大监视树（数千 watch）叠加共享 FUSE 宿主、MediaProvider 或持续
/// 写入风暴时，内核事件队列永远排不空，无界排空会把监视线线程整个卡死在 reconfigure
/// 内——配置版本与重建计数从此冻结（真机复现：监视线程 100% 系统时间空转、场景级
/// `daemon file monitor config sync timeout` 连续超时）。超过上限后丢弃剩余事件：它们随
/// 旧 fd 一起失效本来就是可接受的损失，新监视树建立后的既有文件扫描会兜底补齐。
const MAX_PRE_RESET_DRAIN_ROUNDS: usize = 4;
/// 两次溢出补偿全量扫描之间的最小间隔。
///
/// 溢出多由写入风暴引起，而补偿扫描要遍历全树并做 owner 修复。不限流的话风暴期间
/// 会反复触发全扫，反而延长处理时间、加剧溢出。
const OVERFLOW_RESYNC_MIN_INTERVAL_MS: i64 = 30_000;
const MAX_PUBLIC_OWNER_REPAIR_DIRS: usize = 32768;
/// 公共 owner 修复不安装 inotify watch，使用有界周期扫描覆盖运行期间新建的目录。
const PUBLIC_OWNER_REPAIR_INTERVAL_MS: i64 = 1000;

/// 死监视条目对账周期。inotify 队列溢出会丢弃 IN_IGNORED/IN_DELETE 事件，
/// `watch_nodes` 中对应条目从此无人删除；内核复用 wd 后新目录的节点又推进
/// 同一个 Vec，条目逐代堆积（ishtar 实测：繁忙设备 1.5 小时积到每 wd 平均
/// ~12 个节点，监视树内存 121MB，而全新树只需 15MB）。按分钟级对账即可把
/// 滞留控制在一个小窗口内。
const PRUNE_DEAD_WATCHES_INTERVAL_MS: i64 = 60_000;
/// 公共 owner 修复扫描在目录数没有变化时的记录间隔（扫描每 1 秒一轮，约 5 分钟）。
const PUBLIC_OWNER_REPAIR_LOG_HEARTBEAT: u64 = 300;
/// 递归展开监视树时最多访问的目录数，与 [`MAX_PUBLIC_OWNER_REPAIR_DIRS`] 对齐。
const MAX_EXISTING_TREE_REPAIR_DIRS: usize = 32768;
const PUBLIC_OWNER_EXISTING_WATCH_DEPTH: usize = 2;

fn runtime_max_watches() -> usize {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .map(|limit| limit.min(MAX_WATCHES_CEILING))
        .unwrap_or(DEFAULT_MAX_WATCHES)
}

#[derive(Clone)]
struct WatchRoot {
    package_name: String,
    backend_root: String,
    display_root: String,
    record_display_root: String,
    record_from_root: String,
    excluded_roots: Vec<String>,
    source: &'static str,
}

/// 读取本进程 inotify fd 的内核侧 live wd 清单。
///
/// `/proc/self/fdinfo/<fd>` 的 `inotify wd:` 行就是内核 watch 表的真实内容，
/// 任何因队列溢出丢失 IN_IGNORED 造成的用户态账本漂移都会在这里现形。
/// 行格式为 `inotify wd:<n> ino:<hex> ...`，只取冒号后的首个空白分隔字段。
fn read_live_watch_fds(fd: i32) -> Option<HashSet<i32>> {
    let content = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}")).ok()?;
    let mut live = HashSet::new();
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("inotify wd:") else {
            continue;
        };
        let number = rest.trim().split_whitespace().next().unwrap_or("");
        if let Ok(wd) = number.parse::<i32>() {
            live.insert(wd);
        }
    }
    Some(live)
}

/// 对一组 watcher 执行任意路径（目录或文件）的 owner 修复。
///
/// 修复本身按 (source, 路径) 决定作用域，与包名无关；同一目录上多个 watcher
/// 的修复调用次数与合并前一致，行为保持不变。
fn repair_monitored_backend_owner_for_watchers_dir(
    watchers: &[WatchWatcher],
    display_path: &str,
    backend_path: &str,
) {
    for watcher in watchers {
        repair_monitored_backend_owner(
            watcher.source,
            &watcher.package_name,
            display_path,
            backend_path,
        );
    }
}

/// watcher 集合驻留表：同一棵子树内所有目录的 watcher 集合与根完全一致，
/// 按集合内容驻留后全树每个不同的根只存一份 Arc。直接 Arc::make_mut 做合并
/// 会因「子树共享着同一份 Arc」在每次合并时深拷贝整个集合，25036 个目录各自
/// 持有 ~20 项的副本（ishtar 实测 30MB），驻留才是正确的共享方式。
fn intern_watchers(set: Vec<WatchWatcher>) -> std::sync::Arc<Vec<WatchWatcher>> {
    static INTERN: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<Vec<WatchWatcher>, std::sync::Arc<Vec<WatchWatcher>>>,
        >,
    > = std::sync::OnceLock::new();
    let mut table = INTERN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(shared) = table.get(&set) {
        return shared.clone();
    }
    let shared = std::sync::Arc::new(set.clone());
    table.insert(set, shared.clone());
    shared
}

/// 跨节点共享的不可变字符串驻留表。
/// 监视树容量上限内的每个节点都携带包名与记录根等相同字符串，按值存储会让
/// 上万个节点重复持有相同的堆分配；驻留后同一字符串全表只存一份，节点间
/// 克隆退化为指针拷贝。
fn intern_shared(value: &str) -> std::sync::Arc<str> {
    static INTERN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::sync::Arc<str>, ()>>,
    > = std::sync::OnceLock::new();
    let mut table = INTERN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some((existing, _)) = table.get_key_value(value) {
        return existing.clone();
    }
    let shared: std::sync::Arc<str> = value.into();
    table.insert(shared.clone(), ());
    shared
}

/// 单个监视根（包视角）落在同一目录上的 watcher 身份。
///
/// 同一物理目录（同一 wd）会被多个应用的监视根各自展开：模板配置让若干应用
/// 共享 Download/DCIM 等公共目录，旧结构按 (包,目录) 各建一个节点并各自克隆
/// 一份目录路径串，节点与字符串按共享包数成倍膨胀（ishtar 实测 ~6.6 倍，
/// 监视树内存 121MB）。合并后每目录一个节点，包级信息收进 watchers。
#[derive(Clone, PartialEq, Eq, Hash)]
struct WatchWatcher {
    package_name: std::sync::Arc<str>,
    record_display_root: std::sync::Arc<str>,
    record_from_root: std::sync::Arc<str>,
    excluded_roots: std::sync::Arc<[String]>,
    source: &'static str,
}

/// 单一物理目录的监视条目。
///
/// `watchers` 通常只有一个；同一目录被多个包根覆盖时合并为多个 watcher，
/// 目录路径串每目录只存一份。同一 wd 上不同 backend_dir（极少见的别名路径
/// 指向同一 inode）仍是不同节点，保证事件路径推导不串视图。
#[derive(Clone)]
struct WatchNode {
    backend_dir: String,
    display_dir: String,
    /// 共享的 watcher 集合：同一棵子树内所有目录的 watcher 集合与根完全一致，
    /// Arc 共享让子节点继承退化为指针拷贝（ishtar 实测：25036 个目录若各自
    /// 克隆 ~20 项 watcher 列表要 30MB，共享后整棵树每根只存一份）。
    watchers: std::sync::Arc<Vec<WatchWatcher>>,
}

struct WatchStart {
    backend_dir: String,
    display_dir: String,
}

pub struct RegularAppMonitor {
    fd: i32,
    config_version: u64,
    watch_nodes: HashMap<i32, Vec<WatchNode>>,
    max_watches: usize,
    recent_event_ms: HashMap<String, i64>,
    recent_event_order: VecDeque<String>,
    missing_watch_roots: Vec<WatchRoot>,
    public_owner_roots: Vec<WatchRoot>,
    missing_roots: usize,
    capacity_limited: bool,
    needs_rebuild: bool,
    /// inotify 队列溢出后待执行的一次全量补偿扫描。
    overflow_resync: bool,
    last_rebuild_ms: i64,
    /// `inotify_add_watch` 非预期 errno 的累计次数，用于按 2 的幂限频告警。
    add_watch_error_count: u32,
    /// 上次登记溢出补偿全量扫描的时间，用于限流。
    last_overflow_resync_ms: i64,
    /// 配置未变化而直接沿用现有监视树的次数，用于限频输出排查日志。
    unchanged_reconfigure_count: u32,
    last_public_owner_repair_ms: i64,
    /// 上次死监视条目对账时间，用于节流。
    last_prune_dead_ms: i64,
    /// 各公共 owner 根上次扫描到的目录数，用于只在覆盖范围变化时记录扫描摘要。
    public_owner_repair_log_dirs: HashMap<String, usize>,
    /// 公共 owner 修复扫描的记录次数，用于按心跳间隔补充摘要。
    public_owner_repair_log_count: u64,
}

impl RegularAppMonitor {
    pub fn new() -> Self {
        Self {
            fd: -1,
            config_version: 0,
            watch_nodes: HashMap::new(),
            max_watches: runtime_max_watches(),
            recent_event_ms: HashMap::new(),
            recent_event_order: VecDeque::new(),
            missing_watch_roots: Vec::new(),
            public_owner_roots: Vec::new(),
            missing_roots: 0,
            capacity_limited: false,
            needs_rebuild: true,
            overflow_resync: false,
            last_rebuild_ms: 0,
            add_watch_error_count: 0,
            last_overflow_resync_ms: 0,
            unchanged_reconfigure_count: 0,
            last_public_owner_repair_ms: 0,
            last_prune_dead_ms: 0,
            public_owner_repair_log_dirs: HashMap::new(),
            public_owner_repair_log_count: 0,
        }
    }

    pub fn should_retry_missing_roots(&self) -> bool {
        !self.capacity_limited
            && self.missing_roots > 0
            && paths::monotonic_ms().saturating_sub(self.last_rebuild_ms) >= MISSING_ROOT_RETRY_MS
    }

    pub fn configured_version(&self) -> u64 {
        self.config_version
    }

    /// 返回监视器 inotify fd，供 daemon 事件循环阻塞等待文件事件。
    pub fn event_fd(&self) -> i32 {
        self.fd
    }

    pub fn reconfigure(&mut self, config: &SettingsHub, force: bool) {
        self.refresh_max_watches();
        let version = config.config_version();
        if !force && !self.needs_rebuild && self.config_version == version {
            if self.should_retry_missing_roots() {
                self.retry_missing_watch_roots();
            }
            self.repair_public_owner_roots_if_due();
            self.prune_dead_watches_if_due();
            // 排查用：按 2 的幂限频记录一次「配置未变化、沿用现有监视树」。
            // 汇总日志缺失时需要区分两种情形：reconfigure 每轮都在走这条捷径
            // （说明监视树是早前建立的、或从未建立），还是根本没被调用。
            self.unchanged_reconfigure_count = self.unchanged_reconfigure_count.saturating_add(1);
            if self.unchanged_reconfigure_count.is_power_of_two() {
                log::info!(
                    "daemon monitor unchanged n={} watches={} missing={} version={:x}",
                    self.unchanged_reconfigure_count,
                    self.watch_nodes.len(),
                    self.missing_roots,
                    self.config_version
                );
            }
            return;
        }

        // 缺失的根目录可能触发周期性重建。关闭旧 inotify fd 前先排空队列事件，
        // 避免重建时丢失上一轮循环中观测到的创建事件。
        // 排空受 [`MAX_PRE_RESET_DRAIN_ROUNDS`] 约束：事件风暴下队列永远排不空，
        // 无界等待会饿死整个监视线程（版本冻结、重建滞后），必须允许带着未读事件
        // 进入重建——新监视树的既有文件扫描会补齐这部分状态。
        let mut pre_reset_drain_rounds = 0usize;
        while pre_reset_drain_rounds < MAX_PRE_RESET_DRAIN_ROUNDS && self.drain_events() {
            pre_reset_drain_rounds += 1;
        }
        self.reset();
        self.config_version = version;
        self.last_rebuild_ms = paths::monotonic_ms();
        self.needs_rebuild = false;

        let snapshot = config.get_daemon_monitor_config_snapshot();
        if snapshot.app_specs.is_empty() {
            // 排查用：监视树未建立时必须能区分「没有可监视的应用配置」与其它原因，
            // 否则汇总日志缺失时无法判断是提前返回还是根本没被调用。
            log::info!(
                "daemon monitor skip reason=no_app_specs file_monitor={} version={:x}",
                snapshot.is_file_monitor_enabled,
                version
            );
            return;
        }
        if !self.ensure_fd() {
            self.needs_rebuild = true;
            log::warn!(
                "daemon monitor skip reason=inotify_fd_unavailable specs={} version={:x}",
                snapshot.app_specs.len(),
                version
            );
            return;
        }

        let mut roots = Vec::new();
        for spec in &snapshot.app_specs {
            if spec.is_enabled
                && let Some(root) = build_public_owner_repair_root(spec)
            {
                roots.push(root);
            }
            if snapshot.is_file_monitor_enabled {
                roots.extend(build_private_owner_repair_roots(spec));
                roots.extend(build_watch_roots(spec));
            }
        }
        dedup_roots(&mut roots);
        sort_roots_by_monitor_priority(&mut roots);
        self.public_owner_roots = roots
            .iter()
            .filter(|root| root.source == "public_owner")
            .cloned()
            .collect();

        let mut applied_roots = 0usize;
        let mut expansion_roots = Vec::new();
        let mut missing_watch_roots = Vec::new();
        for root in &roots {
            if root.source == "public_owner" {
                // 公共目录修复可能遍历大树，先安装全部事件监视，再执行该扫描。
                continue;
            }
            if let Some(node) = self.add_watch_root(root) {
                applied_roots = applied_roots.saturating_add(1);
                expansion_roots.push(node);
            } else {
                self.missing_roots = self.missing_roots.saturating_add(1);
                missing_watch_roots.push(root.clone());
            }
            if self.watch_nodes.len() >= self.max_watches {
                self.capacity_limited = true;
                break;
            }
        }

        // 溢出补偿扫描需要对全部来源重新执行 owner 修复，不能只覆盖 private_owner。
        let overflow_resync = std::mem::take(&mut self.overflow_resync);
        if !self.capacity_limited {
            for node in expansion_roots {
                let repair_existing_files = overflow_resync
                    || node
                        .watchers
                        .iter()
                        .any(|watcher| watcher.source == "private_owner");
                let recurse_existing_tree = overflow_resync
                    || node
                        .watchers
                        .iter()
                        .any(|watcher| watcher.source != "public_owner");
                self.expand_watch_tree_from(node, repair_existing_files, recurse_existing_tree);
                if self.capacity_limited {
                    break;
                }
            }
        }
        // 先消费安装期间的目录创建事件，及时为新子目录登记 watch。
        self.drain_events();
        for root in &roots {
            if root.source != "public_owner" {
                continue;
            }
            if self.repair_public_owner_root(root) {
                applied_roots = applied_roots.saturating_add(1);
            } else {
                self.missing_roots = self.missing_roots.saturating_add(1);
                missing_watch_roots.push(root.clone());
            }
        }
        self.missing_watch_roots = missing_watch_roots;
        self.last_public_owner_repair_ms = paths::monotonic_ms();

        log::info!(
            "daemon monitor roots={} applied={} missing={} watches={} capacity_limited={} version={:x}",
            roots.len(),
            applied_roots,
            self.missing_roots,
            self.watch_nodes.len(),
            self.capacity_limited,
            self.config_version
        );
    }

    fn retry_missing_watch_roots(&mut self) {
        self.last_rebuild_ms = paths::monotonic_ms();
        if self.missing_watch_roots.is_empty() || self.capacity_limited {
            return;
        }
        if !self.ensure_fd() {
            self.needs_rebuild = true;
            return;
        }

        let previous_missing = self.missing_watch_roots.len();
        let mut still_missing = Vec::new();
        let mut applied_roots = 0usize;
        let mut repaired_public_root = false;
        let mut expansion_roots = Vec::new();
        let mut roots = std::mem::take(&mut self.missing_watch_roots).into_iter();
        while let Some(root) = roots.next() {
            if root.source == "public_owner" {
                if self.repair_public_owner_root(&root) {
                    applied_roots = applied_roots.saturating_add(1);
                    repaired_public_root = true;
                } else {
                    still_missing.push(root);
                }
                continue;
            }
            if self.watch_nodes.len() >= self.max_watches {
                self.mark_capacity_limited();
                still_missing.push(root);
                still_missing.extend(roots);
                break;
            }
            if let Some(node) = self.add_watch_root(&root) {
                applied_roots = applied_roots.saturating_add(1);
                expansion_roots.push(node);
            } else {
                still_missing.push(root);
            }
        }

        if !self.capacity_limited {
            for node in expansion_roots {
                let repair_existing_files = node
                    .watchers
                    .iter()
                    .any(|watcher| watcher.source == "private_owner");
                let recurse_existing_tree = node
                    .watchers
                    .iter()
                    .any(|watcher| watcher.source != "public_owner");
                self.expand_watch_tree_from(node, repair_existing_files, recurse_existing_tree);
                if self.capacity_limited {
                    break;
                }
            }
        }
        self.missing_roots = still_missing.len();
        self.missing_watch_roots = still_missing;
        if repaired_public_root {
            self.last_public_owner_repair_ms = paths::monotonic_ms();
        }
        if applied_roots > 0 || self.missing_roots != previous_missing {
            log::info!(
                "daemon monitor retry missing previous={} applied={} remaining={} watches={} capacity_limited={} version={:x}",
                previous_missing,
                applied_roots,
                self.missing_roots,
                self.watch_nodes.len(),
                self.capacity_limited,
                self.config_version
            );
        }
    }

    /// 排空 inotify 事件队列。
    ///
    /// 返回 `true` 表示因达到单轮预算而提前返回、内核队列中仍有事件待处理，调用方
    /// 应立即再次调用而不要先等待轮询间隔，否则剩余事件会被推迟一个轮询周期。
    pub fn drain_events(&mut self) -> bool {
        if self.fd < 0 {
            return false;
        }

        // inotify_event 需要 4 字节对齐；内核保证每个事件总长度是 sizeof(int) 的倍数，
        // 因此缓冲区起始 4 字节对齐后，后续每个事件也满足对齐要求。
        let mut buffer = inotify::InotifyBuf::<{ 16 * 1024 }>::new();
        let mut handled = 0usize;
        loop {
            if handled >= MAX_EVENTS_PER_DRAIN {
                // 队列里可能仍有事件，但必须让出控制权，让调用方有机会处理重建。
                log::info!(
                    "daemon monitor drain budget reached n={}, resume next round",
                    handled
                );
                return true;
            }
            let n = inotify::read_into(self.fd, &mut buffer.0);
            if n < 0 {
                let errno = inotify::last_errno();
                if errno == libc::EINTR {
                    continue;
                }
                if errno != libc::EAGAIN && errno != libc::EWOULDBLOCK {
                    log::warn!("daemon monitor read failed errno={}", errno);
                    self.needs_rebuild = true;
                }
                break;
            }
            if n == 0 {
                break;
            }

            let total = n as usize;
            inotify::for_each_event(&buffer.0[..total], |event| {
                self.handle_event(event);
                handled = handled.saturating_add(1);
            });
        }
        false
    }

    fn ensure_fd(&mut self) -> bool {
        if self.fd >= 0 {
            return true;
        }
        let fd = inotify::init_nonblocking();
        if fd < 0 {
            log::warn!(
                "daemon monitor inotify init failed errno={}",
                inotify::last_errno()
            );
            return false;
        }
        self.fd = fd;
        true
    }

    /// 包级增量重建：只重建 `packages` 内应用的监视根，其余包的 watch 原地保留。
    ///
    /// 全量重建会把所有应用的目录树（真机实测 9862~24909 个目录）整棵重走，
    /// 每次应用启用/配置变化都产生数十秒 100% 单核阵发。监视树按包独立构成，
    /// 变化包的旧节点直接放弃（内核 watch 保留原位，事件因节点缺失被丢弃，
    /// `inotify_add_watch` 对同一目录幂等返回相同 wd，重建时自动复用），只需
    /// 重加变化包的根并展开其子树。`public_owner` 根与包配置无关，保持不动。
    pub fn reconfigure_changed_packages(&mut self, config: &SettingsHub, packages: &[String]) {
        self.refresh_max_watches();
        let version = config.config_version();
        self.config_version = version;
        self.last_rebuild_ms = paths::monotonic_ms();
        self.needs_rebuild = false;

        let snapshot = config.get_daemon_monitor_config_snapshot();
        if snapshot.app_specs.is_empty() {
            return;
        }
        if !self.ensure_fd() {
            self.needs_rebuild = true;
            return;
        }

        // 只移除变化包的 watcher：同目录上其它未变化包的 watcher 原地保留，
        // 整目录所有 watcher 都被移除时才放弃该节点（内核 watch 保留原位，
        // 事件因节点缺失被丢弃，`inotify_add_watch` 幂等复用 wd）。
        self.watch_nodes.retain(|_, nodes| {
            nodes.retain_mut(|node| {
                let mut kept = node.watchers.as_ref().clone();
                kept.retain(|watcher| {
                    !packages
                        .iter()
                        .any(|package| package.as_str() == watcher.package_name.as_ref())
                });
                if kept.len() == node.watchers.len() {
                    return true;
                }
                if kept.is_empty() {
                    return false;
                }
                node.watchers = intern_watchers(kept);
                true
            });
            !nodes.is_empty()
        });

        let mut roots = Vec::new();
        for spec in &snapshot.app_specs {
            if !packages.iter().any(|package| package == &spec.package_name) {
                continue;
            }
            if snapshot.is_file_monitor_enabled {
                roots.extend(build_private_owner_repair_roots(spec));
                roots.extend(build_watch_roots(spec));
            }
        }
        dedup_roots(&mut roots);
        sort_roots_by_monitor_priority(&mut roots);

        let mut applied_roots = 0usize;
        let mut expansion_roots = Vec::new();
        let mut missing_watch_roots = Vec::new();
        for root in &roots {
            if let Some(node) = self.add_watch_root(root) {
                applied_roots = applied_roots.saturating_add(1);
                expansion_roots.push(node);
            } else {
                self.missing_roots = self.missing_roots.saturating_add(1);
                missing_watch_roots.push(root.clone());
            }
            if self.watch_nodes.len() >= self.max_watches {
                self.mark_capacity_limited();
                break;
            }
        }
        if !self.capacity_limited {
            for node in expansion_roots {
                let repair_existing_files = node
                    .watchers
                    .iter()
                    .any(|watcher| watcher.source == "private_owner");
                let recurse_existing_tree = node
                    .watchers
                    .iter()
                    .any(|watcher| watcher.source != "public_owner");
                self.expand_watch_tree_from(node, repair_existing_files, recurse_existing_tree);
                if self.capacity_limited {
                    break;
                }
            }
        }
        // 先消费安装期间产生的目录创建事件，及时为新子目录登记 watch。
        self.drain_events();
        self.missing_watch_roots.extend(missing_watch_roots);
        log::info!(
            "daemon monitor packages={} roots={} applied={} missing={} watches={} capacity_limited={} version={:x}",
            packages.len(),
            roots.len(),
            applied_roots,
            self.missing_roots,
            self.watch_nodes.len(),
            self.capacity_limited,
            self.config_version
        );
    }

    fn reset(&mut self) {
        if self.fd >= 0 {
            inotify::close_fd(self.fd);
        }
        self.fd = -1;
        self.watch_nodes.clear();
        self.missing_watch_roots.clear();
        self.public_owner_roots.clear();
        self.missing_roots = 0;
        self.capacity_limited = false;
        self.last_public_owner_repair_ms = 0;
        self.public_owner_repair_log_dirs.clear();
        self.public_owner_repair_log_count = 0;
    }

    fn repair_public_owner_root(&mut self, root: &WatchRoot) -> bool {
        let Some(start) = select_watch_start(root) else {
            return false;
        };
        let node = WatchNode {
            backend_dir: start.backend_dir,
            display_dir: start.display_dir,
            watchers: intern_watchers(vec![WatchWatcher {
                package_name: intern_shared(&root.package_name),
                record_display_root: intern_shared(&root.record_display_root),
                record_from_root: intern_shared(&root.record_from_root),
                excluded_roots: root.excluded_roots.clone().into(),
                source: root.source,
            }]),
        };
        repair_monitored_backend_owner_for_watchers_dir(
            &node.watchers,
            &node.display_dir,
            &node.backend_dir,
        );
        self.repair_existing_public_tree(&node);
        true
    }

    fn repair_public_owner_roots_if_due(&mut self) {
        if self.public_owner_roots.is_empty() {
            return;
        }
        let now = paths::monotonic_ms();
        if now.saturating_sub(self.last_public_owner_repair_ms) < PUBLIC_OWNER_REPAIR_INTERVAL_MS {
            return;
        }
        self.last_public_owner_repair_ms = now;
        for root in self.public_owner_roots.clone() {
            self.repair_public_owner_root(&root);
        }
    }

    /// 按节流周期对账内核 live wd 集合，剪除已死亡的监视条目。
    fn prune_dead_watches_if_due(&mut self) {
        if self.watch_nodes.is_empty() {
            return;
        }
        let now = paths::monotonic_ms();
        if now.saturating_sub(self.last_prune_dead_ms) < PRUNE_DEAD_WATCHES_INTERVAL_MS {
            return;
        }
        self.last_prune_dead_ms = now;
        let removed = self.prune_dead_watches();
        if removed > 0 {
            log::info!(
                "daemon monitor pruned dead watches removed={} remaining={}",
                removed,
                self.watch_nodes.len()
            );
        }
    }

    /// 对账内核 live wd 集合，剪除已死亡的监视条目，返回删除的条目数。
    ///
    /// inotify 队列溢出会丢弃 IN_IGNORED/IN_DELETE 事件，`watch_nodes` 里对应
    /// 条目从此无人删除；内核复用 wd 后新目录的节点又推进同一个 Vec，条目逐
    /// 代堆积。以 `/proc/self/fdinfo/<fd>` 的内核侧 wd 清单为准删除不在册的
    /// 条目——该清单就是内核的真实 watch 表，任何丢失事件造成的漂移都会在
    /// 这里现形。fdinfo 读取失败时跳过本轮（不能因对账失败误删全部条目）。
    fn prune_dead_watches(&mut self) -> usize {
        if self.fd < 0 || self.watch_nodes.is_empty() {
            return 0;
        }
        let Some(live) = read_live_watch_fds(self.fd) else {
            return 0;
        };
        // 防呆：对账清单为空说明 fdinfo 读取或解析异常（此时内核明明还有
        // watch），绝不能把整张表清掉；宁可推迟到下一轮。
        if live.is_empty() {
            return 0;
        }
        let before = self.watch_nodes.len();
        // 清单规模与账本严重不符同样视为读取异常：正常情况下两者只差本次
        // 剪除目标（丢失事件造成的漂移），不会差一个数量级。
        if live.len() * 4 < before {
            return 0;
        }
        self.watch_nodes.retain(|wd, _| live.contains(wd));
        before - self.watch_nodes.len()
    }

    fn add_watch_root(&mut self, root: &WatchRoot) -> Option<WatchNode> {
        let start = select_watch_start(root)?;

        if self.watch_nodes.len() >= self.max_watches {
            self.mark_capacity_limited();
            return None;
        }

        let watcher = WatchWatcher {
            package_name: intern_shared(&root.package_name),
            record_display_root: intern_shared(&root.record_display_root),
            record_from_root: intern_shared(&root.record_from_root),
            excluded_roots: root.excluded_roots.clone().into(),
            source: root.source,
        };
        let node = WatchNode {
            backend_dir: start.backend_dir,
            display_dir: start.display_dir,
            watchers: intern_watchers(vec![watcher]),
        };

        repair_monitored_backend_owner_for_watchers_dir(
            &node.watchers,
            &node.display_dir,
            &node.backend_dir,
        );
        if self.add_watch_node(&node) {
            Some(node)
        } else {
            None
        }
    }

    fn expand_watch_tree_from(
        &mut self,
        root: WatchNode,
        repair_existing_files: bool,
        recurse_existing_tree: bool,
    ) {
        let mut stack = vec![(root, 0usize)];
        // 遍历预算：max_watches 只约束目录 watch 数量，而 repair_existing_files 打开时
        // 每个文件都会做一次 owner 修复，没有上限。溢出补偿会对所有来源打开该开关，
        // 大目录下这一步可能长时间占住监视线程，因此与 public_owner 路径一样设预算。
        let mut visited_dirs = 0usize;
        while let Some((node, depth)) = stack.pop() {
            if self.watch_nodes.len() >= self.max_watches {
                self.mark_capacity_limited();
                break;
            }
            visited_dirs = visited_dirs.saturating_add(1);
            if visited_dirs > MAX_EXISTING_TREE_REPAIR_DIRS {
                log::warn!(
                    "daemon monitor existing tree repair budget reached dirs={} root={}",
                    visited_dirs,
                    node.backend_dir
                );
                break;
            }

            let entries = match std::fs::read_dir(&node.backend_dir) {
                Ok(entries) => entries,
                Err(error) => {
                    let _ = error;
                    continue;
                }
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !inotify::is_safe_event_name(&name) {
                    continue;
                }
                let child_display_dir = paths::join(&node.display_dir, &name);
                let child_backend_dir = paths::join(&node.backend_dir, &name);
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    if repair_existing_files {
                        repair_monitored_backend_owner_for_watchers_dir(
                            &node.watchers,
                            &child_display_dir,
                            &child_backend_dir,
                        );
                    }
                    continue;
                }
                if !node.watchers.iter().any(|watcher| {
                    should_descend_into_child(
                        watcher.source,
                        watcher.record_display_root.as_ref(),
                        &child_display_dir,
                    )
                }) {
                    continue;
                }
                if self.watch_nodes.len() >= self.max_watches {
                    self.mark_capacity_limited();
                    break;
                }
                let child = WatchNode {
                    backend_dir: child_backend_dir,
                    display_dir: child_display_dir,
                    watchers: node.watchers.clone(),
                };
                repair_monitored_backend_owner_for_watchers_dir(
                    &child.watchers,
                    &child.display_dir,
                    &child.backend_dir,
                );
                if self.add_watch_node(&child)
                    && (recurse_existing_tree
                        || (node
                            .watchers
                            .iter()
                            .any(|watcher| watcher.source == "public_owner")
                            && depth < PUBLIC_OWNER_EXISTING_WATCH_DEPTH))
                {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
        }
    }

    fn repair_existing_public_tree(&mut self, root: &WatchNode) {
        let mut stack = vec![root.clone()];
        let mut repaired = 0usize;
        let mut scanned_entries = 0usize;
        self.drain_events();
        while let Some(node) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&node.backend_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                // 大量文件同样会延迟目录事件；按所有条目分批处理，沿用单轮事件预算。
                scanned_entries = scanned_entries.saturating_add(1);
                if scanned_entries.is_multiple_of(64) {
                    self.drain_events();
                }
                if repaired >= MAX_PUBLIC_OWNER_REPAIR_DIRS {
                    log::warn!(
                        "daemon public owner repair limit reached root={} limit={}",
                        root.backend_dir,
                        MAX_PUBLIC_OWNER_REPAIR_DIRS
                    );
                    return;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if !inotify::is_safe_event_name(&name) {
                    continue;
                }
                let child = WatchNode {
                    backend_dir: paths::join(&node.backend_dir, &name),
                    display_dir: paths::join(&node.display_dir, &name),
                    watchers: node.watchers.clone(),
                };
                if !node.watchers.iter().any(|watcher| {
                    should_descend_into_child(
                        watcher.source,
                        watcher.record_display_root.as_ref(),
                        &child.display_dir,
                    )
                }) {
                    continue;
                }
                repair_monitored_backend_owner_for_watchers_dir(
                    &child.watchers,
                    &child.display_dir,
                    &child.backend_dir,
                );
                repaired = repaired.saturating_add(1);
                stack.push(child);
            }
        }
        // 扫描每 1 秒一轮，但覆盖范围通常长期不变：只在目录数变化时记录摘要，
        // 长期不变时按心跳补充，避免摘要把挂载与监视日志挤出 tail 窗口。
        let previous_dirs = self
            .public_owner_repair_log_dirs
            .insert(root.backend_dir.clone(), repaired);
        self.public_owner_repair_log_count = self.public_owner_repair_log_count.saturating_add(1);
        if previous_dirs != Some(repaired)
            || self
                .public_owner_repair_log_count
                .is_multiple_of(PUBLIC_OWNER_REPAIR_LOG_HEARTBEAT)
        {
            log::info!(
                "daemon public owner repair scan root={} dirs={}",
                root.backend_dir,
                repaired
            );
        }
    }

    fn add_watch_node(&mut self, node: &WatchNode) -> bool {
        let wd = match inotify::add_watch(self.fd, &node.backend_dir) {
            Ok(wd) => wd,
            Err(error) => {
                self.note_add_watch_error(node, error);
                return false;
            }
        };

        let entries = self.watch_nodes.entry(wd).or_default();
        // 同一物理目录（同一 wd）被多个包根展开时合并进已有节点的 watchers，
        // 目录路径串每目录只存一份；不同 backend_dir 指向同一 inode 的别名
        // 场景仍是不同节点，事件路径推导不串视图。
        if let Some(existing) = entries
            .iter_mut()
            .find(|existing| existing.backend_dir == node.backend_dir)
        {
            if existing.watchers.len() == node.watchers.len()
                && existing
                    .watchers
                    .iter()
                    .zip(node.watchers.iter())
                    .all(|(left, right)| left == right)
            {
                // 集合完全一致（子树继承的共享集合），无需重建。
                return true;
            }
            let mut merged: Vec<WatchWatcher> = existing.watchers.as_ref().clone();
            for watcher in node.watchers.iter() {
                if !merged.iter().any(|candidate| candidate == watcher) {
                    merged.push(watcher.clone());
                }
            }
            existing.watchers = intern_watchers(merged);
            return true;
        }
        entries.push(node.clone());
        true
    }

    /// 记录 `inotify_add_watch` 失败原因。
    ///
    /// 内核 watch 配额耗尽必须置位 `capacity_limited`：否则深目录场景下每个子目录都
    /// 失败却被当作"无需递归"静默跳过，日志里只看到一个远小于当前预算的计数，
    /// 无法与"目录不存在"区分。目录不存在属于正常竞态，由 missing 重试路径处理。
    fn note_add_watch_error(&mut self, node: &WatchNode, error: inotify::AddWatchError) {
        match error {
            inotify::AddWatchError::Capacity(errno) => {
                if !self.capacity_limited {
                    log::warn!(
                        "daemon monitor kernel watch quota exhausted errno={} {} dir={}; \
                         检查 /proc/sys/fs/inotify/max_user_watches",
                        errno,
                        inotify::errno_text(errno),
                        node.backend_dir
                    );
                }
                self.capacity_limited = true;
            }
            inotify::AddWatchError::Missing => {}
            inotify::AddWatchError::InvalidPath => {
                log::warn!("daemon monitor watch path invalid dir={}", node.backend_dir);
            }
            inotify::AddWatchError::Other(errno) => {
                // 限频：深目录树下同一 errno 可能连续出现上千次。
                self.add_watch_error_count = self.add_watch_error_count.saturating_add(1);
                if self.add_watch_error_count.is_power_of_two() {
                    log::warn!(
                        "daemon monitor add_watch failed errno={} {} dir={} count={}",
                        errno,
                        inotify::errno_text(errno),
                        node.backend_dir,
                        self.add_watch_error_count
                    );
                }
            }
        }
    }

    fn mark_capacity_limited(&mut self) {
        if !self.capacity_limited {
            log::warn!(
                "daemon monitor watch limit reached n={} kernel_limit=/proc/sys/fs/inotify/max_user_watches",
                self.max_watches
            );
        }
        self.capacity_limited = true;
    }

    fn refresh_max_watches(&mut self) {
        let current = runtime_max_watches();
        if current == self.max_watches {
            return;
        }
        log::info!(
            "daemon monitor watch budget changed previous={} current={}",
            self.max_watches,
            current
        );
        self.max_watches = current;
        if self.watch_nodes.len() < self.max_watches {
            self.capacity_limited = false;
        }
    }

    fn handle_event(&mut self, event: &Event<'_>) {
        let mask = event.mask;
        if inotify::is_queue_overflow(mask) {
            // 溢出说明内核已经丢弃了数量未知的事件，只重建监视集无法补回这批事件对应的
            // owner 修复与路径记录，因此需要一次全量补偿扫描。
            self.needs_rebuild = true;
            // 但补偿扫描本身会遍历全树并做 owner 修复，而溢出通常正是写入风暴引起的：
            // 风暴期间反复执行全树扫描会拉长处理时间、导致继续溢出，形成
            // 「溢出→全扫→再溢出」的放大循环。因此对补偿扫描限流。
            let now_ms = paths::monotonic_ms();
            if now_ms.saturating_sub(self.last_overflow_resync_ms)
                >= OVERFLOW_RESYNC_MIN_INTERVAL_MS
            {
                self.overflow_resync = true;
                self.last_overflow_resync_ms = now_ms;
                log::warn!("daemon monitor queue overflow, full resync scheduled");
            } else {
                log::warn!("daemon monitor queue overflow, resync throttled");
            }
            return;
        }
        if inotify::is_watch_ignored(mask) {
            // 只有仍登记在案的 watch 意外消失才要求重建；包级增量重建已放弃的旧根、
            // 以及被删除目录自身的 watch（父目录仍在监视，删除后无覆盖缺口）都只做
            // 节点清理。此前无条件全量重建，应用批量清理缓存目录时每删一个目录就
            // 重走整棵监视树，形成秒级 100% 单核阵发。
            if self.watch_nodes.remove(&event.wd).is_some() {
                self.needs_rebuild = true;
            }
            return;
        }
        if inotify::is_self_removed(mask) {
            self.needs_rebuild = true;
            return;
        }
        if !inotify::is_relevant_event(mask) {
            return;
        }

        let name = inotify::event_name(event);
        if !inotify::is_safe_event_name(&name) {
            return;
        }

        let Some(nodes) = self.watch_nodes.get(&event.wd).cloned() else {
            return;
        };
        let is_dir = inotify::is_dir(mask);
        for node in nodes {
            // 节点级事件路径：display/backend 与 watcher 无关，每事件只算一次。
            let display_path = paths::normalize(&paths::join(&node.display_dir, &name));
            let backend_path = paths::join(&node.backend_dir, &name);

            // owner 修复按 (source, 作用域包) 去重：修复决策只取决于 source 与
            // redirect_root 的包名，同一目录上多个同源 watcher 的修复目标完全
            // 一致——高频事件下逐 watcher 重复 lstat 是监视线程 CPU 阵发的主要
            // 来源之一（真机 perf 采样确认）。
            let mut repaired: Vec<(&'static str, &str)> = Vec::new();
            for watcher in node.watchers.iter() {
                let scope_package = if watcher.source == "redirect_root" {
                    watcher.package_name.as_ref()
                } else {
                    ""
                };
                if repaired
                    .iter()
                    .any(|(source, package)| *source == watcher.source && *package == scope_package)
                {
                    continue;
                }
                repaired.push((watcher.source, scope_package));
                repair_monitored_backend_owner(
                    watcher.source,
                    &watcher.package_name,
                    &node.display_dir,
                    &node.backend_dir,
                );
                repair_monitored_backend_owner(
                    watcher.source,
                    &watcher.package_name,
                    &display_path,
                    &backend_path,
                );
            }

            // 目录创建/移入：合并登记新子树并一次性展开。
            // 此前按 watcher 逐个走 add_watch_tree 且 repair_existing_files=true，
            // 应用启动的建目录风暴下同一子树被重走 N 次、每个新文件都 lstat 修复，
            // 监视线程持续 ~112% 单核数十秒。运行期新建子树的文件由应用自身写入、
            // owner 天然正确；历史遗留 owner 交给全量重配置与溢出补偿扫描兜底。
            if is_dir
                && inotify::is_created_or_moved_to(mask)
                && node.watchers.iter().any(|watcher| {
                    should_descend_into_child(
                        watcher.source,
                        watcher.record_display_root.as_ref(),
                        &display_path,
                    )
                })
            {
                let child = WatchNode {
                    backend_dir: backend_path.clone(),
                    display_dir: display_path.clone(),
                    watchers: std::sync::Arc::clone(&node.watchers),
                };
                if self.add_watch_node(&child) {
                    self.expand_watch_tree_from(child, false, true);
                }
            }

            let operation_name = monitor_operation_from_mask(mask);
            for watcher in node.watchers.iter() {
                if watcher.source == "public_owner" || watcher.source == "private_owner" {
                    continue;
                }

                if !should_record_display_path(&display_path, &watcher.record_display_root)
                    || should_filter_display_path(&display_path, operation_name)
                    || is_under_any_root(&display_path, &watcher.excluded_roots)
                {
                    continue;
                }
                let identity = resolve_monitor_identity(
                    &watcher.package_name,
                    &display_path,
                    &backend_path,
                    watcher.source,
                );
                if should_skip_ambiguous_allowed_real_path_event(
                    &identity,
                    watcher.source,
                    &display_path,
                    &watcher.package_name,
                ) || should_skip_ambiguous_read_only_path_event(
                    &identity,
                    watcher.source,
                    &watcher.package_name,
                ) || should_skip_public_root_event_identity(
                    &identity,
                    watcher.source,
                    &watcher.package_name,
                ) {
                    continue;
                }
                let from_path = map_record_from_path(
                    &display_path,
                    &watcher.record_display_root,
                    &watcher.record_from_root,
                );
                if self.should_skip_duplicate(
                    &identity.package_name,
                    &display_path,
                    &from_path,
                    operation_name,
                    mask,
                ) {
                    continue;
                }
                let event_paths = MonitorEventPaths {
                    backend_path: backend_path.clone(),
                    display_path: display_path.clone(),
                    from_path,
                };
                emit_monitor_event(
                    &identity,
                    &event_paths,
                    &watcher.package_name,
                    watcher.source,
                    mask,
                    operation_name,
                );
            }
        }
    }

    fn should_skip_duplicate(
        &mut self,
        package_name: &str,
        path: &str,
        from_path: &str,
        operation_name: &str,
        mask: u32,
    ) -> bool {
        let now_ms = paths::monotonic_ms();
        if operation_name == "open:write" && !inotify::is_modify(mask) {
            let create_key = format!("{}|create|{}|{}", package_name, path, from_path);
            if self
                .recent_event_ms
                .get(&create_key)
                .is_some_and(|last_ms| now_ms.saturating_sub(*last_ms) < DUPLICATE_EVENT_WINDOW_MS)
            {
                return true;
            }
        }

        let event_key = format!("{}|{}|{}|{}", package_name, operation_name, path, from_path);
        if inotify::is_modify(mask) {
            if self
                .recent_event_ms
                .insert(event_key.clone(), now_ms)
                .is_none()
            {
                self.recent_event_order.push_back(event_key);
            }
            self.trim_recent_events();
            return false;
        }
        if let Some(last_ms) = self.recent_event_ms.get_mut(&event_key) {
            if now_ms.saturating_sub(*last_ms) < DUPLICATE_EVENT_WINDOW_MS {
                *last_ms = now_ms;
                return true;
            }
            *last_ms = now_ms;
            return false;
        }

        self.recent_event_ms.insert(event_key.clone(), now_ms);
        self.recent_event_order.push_back(event_key);
        if inotify::is_created_or_moved_to(mask) {
            let create_key = format!("{}|create|{}|{}", package_name, path, from_path);
            if self
                .recent_event_ms
                .insert(create_key.clone(), now_ms)
                .is_none()
            {
                self.recent_event_order.push_back(create_key);
            }
        }
        self.trim_recent_events();
        false
    }

    fn trim_recent_events(&mut self) {
        while self.recent_event_order.len() > MAX_RECENT_EVENTS {
            if let Some(oldest) = self.recent_event_order.pop_front() {
                self.recent_event_ms.remove(&oldest);
            }
        }
    }
}

impl Drop for RegularAppMonitor {
    fn drop(&mut self) {
        self.reset();
    }
}
