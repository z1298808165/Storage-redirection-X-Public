#[path = "daemon_monitor/events.rs"]
mod events;
#[path = "daemon_monitor/inotify.rs"]
mod inotify;
#[path = "daemon_monitor/roots.rs"]
mod roots;

use crate::config::SettingsHub;
use crate::platform::paths;
use events::{
    MonitorEventPaths, emit_monitor_event, monitor_operation_from_mask,
    repair_monitored_backend_owner, resolve_monitor_identity, should_filter_display_path,
    should_skip_ambiguous_allowed_real_path_event, should_skip_ambiguous_read_only_path_event,
    should_skip_public_root_event_identity,
};
use libc::inotify_event;
use roots::{
    build_private_owner_repair_roots, build_public_owner_repair_root, build_watch_roots,
    dedup_roots, is_under_any_root, select_watch_start, should_descend_into_child,
    should_record_display_path, sort_roots_by_monitor_priority,
};
use std::collections::{HashMap, VecDeque};

const DUPLICATE_EVENT_WINDOW_MS: i64 = 1500;
const MISSING_ROOT_RETRY_MS: i64 = 1000;
const MAX_RECENT_EVENTS: usize = 512;
const MAX_WATCHES: usize = 8192;
const MAX_PUBLIC_OWNER_REPAIR_DIRS: usize = 32768;
const PUBLIC_OWNER_EXISTING_WATCH_DEPTH: usize = 2;

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

#[derive(Clone, PartialEq, Eq)]
struct WatchNode {
    package_name: String,
    backend_dir: String,
    display_dir: String,
    record_display_root: String,
    record_from_root: String,
    excluded_roots: Vec<String>,
    source: &'static str,
}

struct WatchStart {
    backend_dir: String,
    display_dir: String,
}

pub struct RegularAppMonitor {
    fd: i32,
    config_version: u64,
    watch_nodes: HashMap<i32, Vec<WatchNode>>,
    recent_event_ms: HashMap<String, i64>,
    recent_event_order: VecDeque<String>,
    missing_watch_roots: Vec<WatchRoot>,
    missing_roots: usize,
    capacity_limited: bool,
    needs_rebuild: bool,
    /// inotify 队列溢出后待执行的一次全量补偿扫描。
    overflow_resync: bool,
    last_rebuild_ms: i64,
    /// `inotify_add_watch` 非预期 errno 的累计次数，用于按 2 的幂限频告警。
    add_watch_error_count: u32,
}

impl RegularAppMonitor {
    pub fn new() -> Self {
        Self {
            fd: -1,
            config_version: 0,
            watch_nodes: HashMap::new(),
            recent_event_ms: HashMap::new(),
            recent_event_order: VecDeque::new(),
            missing_watch_roots: Vec::new(),
            missing_roots: 0,
            capacity_limited: false,
            needs_rebuild: true,
            overflow_resync: false,
            last_rebuild_ms: 0,
            add_watch_error_count: 0,
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

    pub fn reconfigure(&mut self, config: &SettingsHub, force: bool) {
        let version = config.config_version();
        if !force && !self.needs_rebuild && self.config_version == version {
            if self.should_retry_missing_roots() {
                self.retry_missing_watch_roots();
            }
            return;
        }

        // 缺失的根目录可能触发周期性重建。关闭旧 inotify fd 前先排空队列事件，
        // 避免重建时丢失上一轮循环中观测到的创建事件。
        self.drain_events();
        self.reset();
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
            if self.watch_nodes.len() >= MAX_WATCHES {
                self.capacity_limited = true;
                break;
            }
        }

        // 溢出补偿扫描需要对全部来源重新执行 owner 修复，不能只覆盖 private_owner。
        let overflow_resync = std::mem::take(&mut self.overflow_resync);
        if !self.capacity_limited {
            for node in expansion_roots {
                let repair_existing_files = overflow_resync || node.source == "private_owner";
                let recurse_existing_tree = overflow_resync || node.source != "public_owner";
                if node.source == "public_owner" {
                    self.repair_existing_public_tree(&node);
                }
                self.expand_watch_tree_from(node, repair_existing_files, recurse_existing_tree);
                if self.capacity_limited {
                    break;
                }
            }
        }
        self.missing_watch_roots = missing_watch_roots;

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
        let mut expansion_roots = Vec::new();
        let mut roots = std::mem::take(&mut self.missing_watch_roots).into_iter();
        while let Some(root) = roots.next() {
            if self.watch_nodes.len() >= MAX_WATCHES {
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
                let repair_existing_files = node.source == "private_owner";
                let recurse_existing_tree = node.source != "public_owner";
                if node.source == "public_owner" {
                    self.repair_existing_public_tree(&node);
                }
                self.expand_watch_tree_from(node, repair_existing_files, recurse_existing_tree);
                if self.capacity_limited {
                    break;
                }
            }
        }
        self.missing_roots = still_missing.len();
        self.missing_watch_roots = still_missing;
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

    pub fn drain_events(&mut self) {
        if self.fd < 0 {
            return;
        }

        // inotify_event 需要 4 字节对齐；内核保证每个事件总长度是 sizeof(int) 的倍数，
        // 因此缓冲区起始 4 字节对齐后，后续每个事件也满足对齐要求。
        let mut buffer = inotify::InotifyBuf::<{ 16 * 1024 }>::new();
        loop {
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

            let mut offset = 0usize;
            let total = n as usize;
            while offset + std::mem::size_of::<inotify_event>() <= total {
                // SAFETY: 循环条件已保证 offset 起至少还有一个完整 inotify_event，
                // 且 InotifyBuf 按 4 字节对齐，事件起始地址满足对齐要求。
                let event = unsafe { &*(buffer.0.as_ptr().add(offset) as *const inotify_event) };
                let event_len = inotify::event_len(event);
                if event_len == 0 || offset + event_len > total {
                    break;
                }
                self.handle_event(event);
                offset += event_len;
            }
        }
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

    fn reset(&mut self) {
        if self.fd >= 0 {
            inotify::close_fd(self.fd);
        }
        self.fd = -1;
        self.watch_nodes.clear();
        self.missing_watch_roots.clear();
        self.missing_roots = 0;
        self.capacity_limited = false;
    }

    fn add_watch_tree(&mut self, root: &WatchRoot) -> bool {
        let Some(node) = self.add_watch_root(root) else {
            return false;
        };
        self.expand_watch_tree_from(node, true, true);
        true
    }

    fn add_watch_root(&mut self, root: &WatchRoot) -> Option<WatchNode> {
        let start = select_watch_start(root)?;

        if self.watch_nodes.len() >= MAX_WATCHES {
            self.mark_capacity_limited();
            return None;
        }

        let node = WatchNode {
            package_name: root.package_name.clone(),
            backend_dir: start.backend_dir,
            display_dir: start.display_dir,
            record_display_root: root.record_display_root.clone(),
            record_from_root: root.record_from_root.clone(),
            excluded_roots: root.excluded_roots.clone(),
            source: root.source,
        };

        repair_monitored_backend_owner(
            node.source,
            &node.package_name,
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
        while let Some((node, depth)) = stack.pop() {
            if self.watch_nodes.len() >= MAX_WATCHES {
                self.mark_capacity_limited();
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
                        repair_monitored_backend_owner(
                            node.source,
                            &node.package_name,
                            &child_display_dir,
                            &child_backend_dir,
                        );
                    }
                    continue;
                }
                if !should_descend_into_child(&node, &child_display_dir) {
                    continue;
                }
                if self.watch_nodes.len() >= MAX_WATCHES {
                    self.mark_capacity_limited();
                    break;
                }
                let child = WatchNode {
                    package_name: node.package_name.clone(),
                    backend_dir: child_backend_dir,
                    display_dir: child_display_dir,
                    record_display_root: node.record_display_root.clone(),
                    record_from_root: node.record_from_root.clone(),
                    excluded_roots: node.excluded_roots.clone(),
                    source: node.source,
                };
                repair_monitored_backend_owner(
                    child.source,
                    &child.package_name,
                    &child.display_dir,
                    &child.backend_dir,
                );
                if self.add_watch_node(&child)
                    && (recurse_existing_tree
                        || (node.source == "public_owner"
                            && depth < PUBLIC_OWNER_EXISTING_WATCH_DEPTH))
                {
                    stack.push((child, depth.saturating_add(1)));
                }
            }
        }
    }

    fn repair_existing_public_tree(&self, root: &WatchNode) {
        let mut stack = vec![root.clone()];
        let mut repaired = 0usize;
        while let Some(node) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&node.backend_dir) else {
                continue;
            };
            for entry in entries.flatten() {
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
                    package_name: node.package_name.clone(),
                    backend_dir: paths::join(&node.backend_dir, &name),
                    display_dir: paths::join(&node.display_dir, &name),
                    record_display_root: node.record_display_root.clone(),
                    record_from_root: node.record_from_root.clone(),
                    excluded_roots: node.excluded_roots.clone(),
                    source: node.source,
                };
                if !should_descend_into_child(&node, &child.display_dir) {
                    continue;
                }
                repair_monitored_backend_owner(
                    child.source,
                    &child.package_name,
                    &child.display_dir,
                    &child.backend_dir,
                );
                repaired = repaired.saturating_add(1);
                stack.push(child);
            }
        }
        log::info!(
            "daemon public owner repair scan root={} dirs={}",
            root.backend_dir,
            repaired
        );
    }

    fn add_watch_node(&mut self, node: &WatchNode) -> bool {
        let wd = match inotify::add_watch(self.fd, &node.backend_dir) {
            Ok(wd) => wd,
            Err(error) => {
                self.note_add_watch_error(node, error);
                return false;
            }
        };

        let nodes = self.watch_nodes.entry(wd).or_default();
        if !nodes.iter().any(|existing| existing == node) {
            nodes.push(node.clone());
        }
        true
    }

    /// 记录 `inotify_add_watch` 失败原因。
    ///
    /// 内核 watch 配额耗尽必须置位 `capacity_limited`：否则深目录场景下每个子目录都
    /// 失败却被当作"无需递归"静默跳过，日志里只看到一个远小于 MAX_WATCHES 的计数，
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
            log::warn!("daemon monitor watch limit reached n={}", MAX_WATCHES);
        }
        self.capacity_limited = true;
    }

    fn handle_event(&mut self, event: &inotify_event) {
        let mask = event.mask;
        if inotify::is_queue_overflow(mask) {
            // 溢出说明内核已经丢弃了数量未知的事件，只重建监视集无法补回这批事件对应的
            // owner 修复与路径记录，这里额外登记一次全量补偿扫描。
            self.needs_rebuild = true;
            self.overflow_resync = true;
            log::warn!("daemon monitor queue overflow");
            return;
        }
        if inotify::is_watch_ignored(mask) {
            self.watch_nodes.remove(&event.wd);
            self.needs_rebuild = true;
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
            let event_paths = MonitorEventPaths::from_node(&node, &name);

            repair_monitored_backend_owner(
                node.source,
                &node.package_name,
                &node.display_dir,
                &node.backend_dir,
            );
            repair_monitored_backend_owner(
                node.source,
                &node.package_name,
                &event_paths.display_path,
                &event_paths.backend_path,
            );

            if is_dir
                && inotify::is_created_or_moved_to(mask)
                && should_descend_into_child(&node, &event_paths.display_path)
            {
                let child = WatchRoot {
                    package_name: node.package_name.clone(),
                    backend_root: event_paths.backend_path.clone(),
                    display_root: event_paths.display_path.clone(),
                    record_display_root: node.record_display_root.clone(),
                    record_from_root: node.record_from_root.clone(),
                    excluded_roots: node.excluded_roots.clone(),
                    source: node.source,
                };
                let _ = self.add_watch_tree(&child);
            }

            if node.source == "public_owner" || node.source == "private_owner" {
                continue;
            }

            let operation_name = monitor_operation_from_mask(mask);
            if !should_record_display_path(&event_paths.display_path, &node.record_display_root)
                || should_filter_display_path(&event_paths.display_path, operation_name)
                || is_under_any_root(&event_paths.display_path, &node.excluded_roots)
            {
                continue;
            }
            let identity = resolve_monitor_identity(
                &node.package_name,
                &event_paths.display_path,
                &event_paths.backend_path,
                node.source,
            );
            if should_skip_ambiguous_allowed_real_path_event(
                &identity,
                node.source,
                &event_paths.display_path,
                &node.package_name,
            ) || should_skip_ambiguous_read_only_path_event(
                &identity,
                node.source,
                &node.package_name,
            ) || should_skip_public_root_event_identity(
                &identity,
                node.source,
                &node.package_name,
            ) {
                continue;
            }
            if self.should_skip_duplicate(
                &identity.package_name,
                &event_paths.display_path,
                &event_paths.from_path,
                operation_name,
                mask,
            ) {
                continue;
            }
            emit_monitor_event(
                &identity,
                &event_paths,
                &node.package_name,
                node.source,
                mask,
                operation_name,
            );
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
