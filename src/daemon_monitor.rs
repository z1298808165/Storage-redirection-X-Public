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
    build_private_owner_repair_roots, build_watch_roots, dedup_roots, is_under_any_root,
    select_watch_start, should_descend_into_child, should_record_display_path,
    sort_roots_by_monitor_priority,
};
use std::collections::{HashMap, VecDeque};

const DUPLICATE_EVENT_WINDOW_MS: i64 = 1500;
const MISSING_ROOT_RETRY_MS: i64 = 1000;
const MAX_RECENT_EVENTS: usize = 512;
const MAX_WATCHES: usize = 8192;

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
    last_rebuild_ms: i64,
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
            last_rebuild_ms: 0,
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

    pub fn reconfigure(&mut self, config: &SettingsHub) {
        let version = config.config_version();
        if !self.needs_rebuild && self.config_version == version {
            if self.should_retry_missing_roots() {
                self.retry_missing_watch_roots();
            }
            return;
        }

        // Missing roots can force periodic rebuilds. Drain queued events before
        // closing the old inotify fd so creations observed during the last
        // loop are not dropped by the rebuild.
        self.drain_events();
        self.reset();
        self.config_version = version;
        self.last_rebuild_ms = paths::monotonic_ms();
        self.needs_rebuild = false;

        let specs = config.get_monitor_app_specs();
        if specs.is_empty() {
            return;
        }
        if !self.ensure_fd() {
            self.needs_rebuild = true;
            return;
        }

        let mut roots = Vec::new();
        for spec in &specs {
            roots.extend(build_private_owner_repair_roots(spec));
            roots.extend(build_watch_roots(spec));
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

        if !self.capacity_limited {
            for node in expansion_roots {
                let repair_existing_files = node.source == "private_owner";
                self.expand_watch_tree_from(node, repair_existing_files);
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
                self.expand_watch_tree_from(node, repair_existing_files);
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

        let mut buffer = [0u8; 16 * 1024];
        loop {
            let n = inotify::read_into(self.fd, &mut buffer);
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
                let event = unsafe { &*(buffer.as_ptr().add(offset) as *const inotify_event) };
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
        self.expand_watch_tree_from(node, true);
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

    fn expand_watch_tree_from(&mut self, root: WatchNode, repair_existing_files: bool) {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
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
                if self.add_watch_node(&child) {
                    stack.push(child);
                }
            }
        }
    }

    fn add_watch_node(&mut self, node: &WatchNode) -> bool {
        let Some(wd) = inotify::add_watch(self.fd, &node.backend_dir) else {
            return false;
        };

        let nodes = self.watch_nodes.entry(wd).or_default();
        if !nodes.iter().any(|existing| existing == node) {
            nodes.push(node.clone());
        }
        true
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
            self.needs_rebuild = true;
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
        let create_key = format!("{}|create|{}|{}", package_name, path, from_path);
        if operation_name == "open:write"
            && !inotify::is_modify(mask)
            && self
                .recent_event_ms
                .get(&create_key)
                .is_some_and(|last_ms| now_ms.saturating_sub(*last_ms) < DUPLICATE_EVENT_WINDOW_MS)
        {
            return true;
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
            self.recent_event_ms.insert(create_key.clone(), now_ms);
            self.recent_event_order.push_back(create_key);
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
