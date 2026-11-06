use crate::platform::{self, paths};
use std::cell::RefCell;
#[path = "mount/alias.rs"]
mod alias;
#[path = "mount/apply.rs"]
mod apply;
#[path = "mount/apply_targets.rs"]
mod apply_targets;
#[path = "mount/map.rs"]
mod map;
/// 自有私有目录恢复的重试队列（跨进程用标记文件承载，daemon 主循环负责排期）。
///
/// quality-allow(lint-suppression): 该模块的读侧（`take_due_packages`）只由 daemon
/// 可执行档调用，库目标编译时天然无引用，需显式豁免 dead_code。
#[allow(dead_code)]
#[path = "mount/own_private_pending.rs"]
pub mod own_private_pending;
#[path = "mount/planner.rs"]
mod planner;
#[path = "mount/planner_paths.rs"]
mod planner_paths;
#[path = "mount/propagation.rs"]
pub(crate) mod propagation;

/// `open_tree(2)` 的 `OPEN_TREE_CLONE`：克隆出一个游离（未附着到任何命名空间）的挂载。
const OPEN_TREE_CLONE: libc::c_uint = 0x0000_0001;
/// `move_mount(2)` 的 `MOVE_MOUNT_F_EMPTY_PATH`：源由 fd 指定（游离挂载没有路径）。
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;

/// 系统存储视图的**游离克隆**（`open_tree(OPEN_TREE_CLONE)`）。
///
/// 用途：自有私有目录（`Android/{data,media,obb}/<pkg>`）的恢复 bind 需要一个**纯系统视图**
/// 作为源。这个源必须在本模块任何挂载落地**之前**取——一旦别名视图 `.../emulated/<user>`
/// 被本模块接管，经任何路径解析 `<...>/emulated/<user>/...` 都会走到我们自己的宿主会话上，
/// 而 root 请求没有对应会话策略（`fuse policy namespace miss uid=0` → `fallback_deny`），
/// lookup 直接 ENOENT，恢复 bind 随之失败（真机 2026-11-02 实测
/// `own private system view restored count=0` + `bind … errno=2`）。
///
/// 克隆与源挂载共享同一个 superblock，因此经它读到的是真正的 MediaProvider FUSE 视图；
/// 且它不附着于任何命名空间，用完 `close` 即可，不留下任何挂载痕迹。
pub struct SystemViewClone {
    fd: libc::c_int,
}

impl SystemViewClone {
    /// 克隆 `/mnt/user/<user_id>/emulated`（emulated 根：本模块从不覆盖它）。
    pub fn capture(user_id: i32) -> Option<Self> {
        // 别名族把 `/mnt/user/<u>/emulated` 整族换成了本模块视图，它不再是干净的系统视图；
        // 改克隆**直通后端**（本模块刻意不覆盖它），结构与 emulated 根相同、内容是数据真身。
        let source = format!("/mnt/pass_through/{user_id}/emulated");
        let c_source = std::ffi::CString::new(source.clone()).ok()?;
        // SAFETY: open_tree 的参数与内核一致（dfd=AT_FDCWD、NUL 结尾路径、只克隆不附着），
        // 因此不会改动任何命名空间。
        let fd = unsafe {
            libc::syscall(
                libc::SYS_open_tree,
                libc::AT_FDCWD,
                c_source.as_ptr(),
                OPEN_TREE_CLONE,
            )
        } as libc::c_int;
        if fd < 0 {
            let errno = crate::platform::errno::last();
            log::warn!(
                "system view clone failed src={} errno={} {}",
                source,
                errno,
                crate::platform::errno::text(errno)
            );
            return None;
        }
        log::debug!("system view cloned src={} fd={}", source, fd);
        Some(Self { fd })
    }

    /// 把游离克隆附着到 `target`（一次性消费该挂载）。
    ///
    /// **不能**拿 `/proc/self/fd/<n>/...` 当 bind 源：`mount(MS_BIND)` 要求源挂载属于
    /// 调用方当前命名空间（内核 `do_loopback` 的 `check_mnt`），而游离克隆不在任何命名
    /// 空间里——真机实测 `bind … errno=22 EINVAL`。搬运游离/跨命名空间挂载只能走
    /// `move_mount`：它把克隆直接附着到目标路径，附着之后它就是本命名空间的挂载点。
    ///
    /// 附着成功即消费 fd（本结构体的 Drop 会再 close 一次，返回 EBADF 属正常）。
    pub fn attach_at(&self, target: &str) -> bool {
        let Ok(c_target) = std::ffi::CString::new(target) else {
            return false;
        };
        let Ok(c_empty) = std::ffi::CString::new("") else {
            return false;
        };
        // SAFETY: 源由 fd 指定（MOVE_MOUNT_F_EMPTY_PATH），target 是 NUL 结尾的合法路径；
        // 成功后该挂载归属目标命名空间。
        let ret = unsafe {
            libc::syscall(
                libc::SYS_move_mount,
                self.fd,
                c_empty.as_ptr(),
                libc::AT_FDCWD,
                c_target.as_ptr(),
                MOVE_MOUNT_F_EMPTY_PATH,
            )
        };
        if ret != 0 {
            let errno = crate::platform::errno::last();
            log::warn!(
                "system view clone attach failed target={} errno={} {}",
                target,
                errno,
                crate::platform::errno::text(errno)
            );
            return false;
        }
        log::debug!("system view clone attached target={}", target);
        true
    }
}

impl Drop for SystemViewClone {
    fn drop(&mut self) {
        // SAFETY: fd 由 capture 创建且只在此处关闭一次。
        unsafe { libc::close(self.fd) };
    }
}

pub struct MountPlanner {
    should_unshare: bool,
    is_namespace_ready: bool,
    is_storage_root_redirected: bool,
    package_name: String,
    app_uid: i32,
    user_id: i32,
    app_data_dir: String,
    redirect_target: String,
    mounted_targets: RefCell<Vec<String>>,
    // 每次绑定前记录的「目标原始 inode」，供 `verify_fuse_backed_bind_landing` 判断这次
    // stat 读到的是绑定后的视图还是绑定前那层。只在单次 `bind_mount_inner` 调用期间有效。
    pre_bind_target_inode: RefCell<Option<(u64, u64)>>,
    is_file_monitor_enabled: bool,
    real_storage_anchor: Option<String>,
    /// 由调用方在**挂载之前**捕获的系统视图克隆（见 [`SystemViewClone`]）。
    ///
    /// 存在时自有私有目录的恢复锚点改由它附着而来：源必须是"本模块挂载落地之前"的
    /// 系统视图快照，否则 `<anchor>/<user>` 会解析到我们自己的宿主会话上（root 请求
    /// 无会话策略 → `fallback_deny` → ENOENT），恢复 bind 稳定失败
    /// （真机 2026-11-02：`own private system view restored count=0`）。
    system_view_clone: std::cell::RefCell<Option<SystemViewClone>>,
}

#[derive(Copy, Clone)]
enum PrimaryMountFailure {
    AbortAll,
    StopCurrentTarget,
    ContinueAliases,
}

const MAX_WILDCARD_MOUNT_MATCHES: usize = 128;
const MAX_WILDCARD_SCAN_DIRS: usize = 512;

pub(super) fn concrete_mount_fallback_parent(
    resolved_path: &str,
    storage_path: &str,
) -> Option<String> {
    paths::wildcard_mount_fallback_parent(resolved_path, storage_path)
}

fn concrete_wildcard_mount_matches_for_roots(
    resolved_path: &str,
    storage_path: &str,
    scan_roots: &[String],
) -> Vec<String> {
    let normalized = paths::normalize(resolved_path);
    if normalized.is_empty() || !paths::contains_wildcards(&normalized) {
        return Vec::new();
    }

    let Some(relative_rule) = paths::relative_child_path(&normalized, storage_path) else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for scan_root in scan_roots {
        let source_rule = paths::join(scan_root, relative_rule);
        append_concrete_wildcard_mount_matches_from_root(
            &source_rule,
            storage_path,
            scan_root,
            &mut matches,
        );
    }

    paths::sort_dedup_paths_case_insensitive(&mut matches);
    matches
}

fn append_concrete_wildcard_mount_matches_from_root(
    source_rule: &str,
    storage_path: &str,
    scan_storage_root: &str,
    matches: &mut Vec<String>,
) {
    let scan_root = concrete_prefix_before_wildcard_preserving_alias(source_rule);
    if scan_root.is_empty()
        || (!paths::eq_ignore_case(&scan_root, scan_storage_root)
            && !paths::is_child(&scan_root, scan_storage_root))
    {
        log::debug!(
            "wildcard scan skip root outside storage source={} scan_root={} storage={}",
            source_rule,
            scan_root,
            scan_storage_root
        );
        return;
    }
    if !std::path::Path::new(&scan_root).is_dir() {
        log::debug!(
            "wildcard scan skip missing root source={} scan_root={} storage={}",
            source_rule,
            scan_root,
            scan_storage_root
        );
        return;
    }

    let root_depth = path_segment_count(&scan_root);
    let rule_depth = path_segment_count(source_rule);
    if rule_depth <= root_depth {
        return;
    }

    let mut source_matches = Vec::new();
    let mut scanned_dirs = 0usize;
    scan_wildcard_mount_matches(
        &scan_root,
        rule_depth - root_depth,
        source_rule,
        scan_storage_root,
        &mut source_matches,
        &mut scanned_dirs,
    );
    log::debug!(
        "wildcard scan root done source={} scan_root={} storage={} matches={} scanned={}",
        source_rule,
        scan_root,
        scan_storage_root,
        source_matches.len(),
        scanned_dirs
    );
    for source_match in source_matches {
        let Some(relative) = paths::relative_child_path(&source_match, scan_storage_root) else {
            continue;
        };
        matches.push(paths::join(storage_path, relative));
    }
}

fn scan_wildcard_mount_matches(
    current_dir: &str,
    remaining_depth: usize,
    rule_path: &str,
    storage_path: &str,
    matches: &mut Vec<String>,
    scanned_dirs: &mut usize,
) {
    if remaining_depth == 0
        || matches.len() >= MAX_WILDCARD_MOUNT_MATCHES
        || *scanned_dirs >= MAX_WILDCARD_SCAN_DIRS
    {
        return;
    }

    let Ok(entries) = std::fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if matches.len() >= MAX_WILDCARD_MOUNT_MATCHES || *scanned_dirs >= MAX_WILDCARD_SCAN_DIRS {
            break;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        *scanned_dirs += 1;
        let candidate = normalize_host_path(entry.path());
        if !paths::is_child(&candidate, storage_path)
            || !wildcard_prefix_matches(rule_path, &candidate)
        {
            continue;
        }

        if remaining_depth == 1 {
            if paths::matches(rule_path, &candidate, true) {
                matches.push(candidate);
            }
            continue;
        }

        scan_wildcard_mount_matches(
            &candidate,
            remaining_depth - 1,
            rule_path,
            storage_path,
            matches,
            scanned_dirs,
        );
    }
}

fn wildcard_prefix_matches(rule_path: &str, candidate_path: &str) -> bool {
    let rule_segments: Vec<&str> = rule_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let candidate_depth = path_segment_count(candidate_path);
    if candidate_depth == 0 || candidate_depth > rule_segments.len() {
        return false;
    }

    let prefix = rule_segments[..candidate_depth].join("/");
    let prefix_rule = if rule_path.starts_with('/') {
        format!("/{prefix}")
    } else {
        prefix
    };
    paths::matches(&prefix_rule, candidate_path, false)
}

fn normalize_host_path(path: impl AsRef<std::path::Path>) -> String {
    normalize_mount_scan_path(&path.as_ref().to_string_lossy().replace('\\', "/"))
}

fn concrete_prefix_before_wildcard_preserving_alias(path: &str) -> String {
    let normalized = normalize_mount_scan_path(path);
    if normalized.is_empty() || !paths::contains_wildcards(&normalized) {
        return normalized;
    }

    let mut kept = Vec::new();
    for segment in normalized.split('/').filter(|segment| !segment.is_empty()) {
        if paths::contains_wildcards(segment) {
            break;
        }
        kept.push(segment);
    }
    if kept.is_empty() {
        return String::new();
    }

    let prefix = kept.join("/");
    if normalized.starts_with('/') {
        format!("/{prefix}")
    } else {
        prefix
    }
}

fn normalize_mount_scan_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(path.len());
    let mut is_last_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if !is_last_slash {
                result.push('/');
                is_last_slash = true;
            }
        } else {
            result.push(ch);
            is_last_slash = false;
        }
    }

    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }
    result
}

fn path_segment_count(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

impl MountPlanner {
    pub fn new(
        package_name: &str,
        app_uid: i32,
        app_data_dir: &str,
        redirect_target: &str,
        should_unshare: bool,
    ) -> Self {
        let user_id = platform::user_id_from_uid(app_uid);
        log::debug!(
            "mount redirect pkg={} uid={} user={}",
            package_name,
            app_uid,
            user_id
        );
        Self {
            should_unshare,
            is_namespace_ready: false,
            is_storage_root_redirected: false,
            package_name: package_name.to_string(),
            app_uid,
            user_id,
            app_data_dir: app_data_dir.to_string(),
            redirect_target: paths::normalize(redirect_target),
            mounted_targets: RefCell::new(Vec::new()),
            pre_bind_target_inode: RefCell::new(None),
            is_file_monitor_enabled: false,
            real_storage_anchor: None,
            system_view_clone: std::cell::RefCell::new(None),
        }
    }

    /// 设置调用方在挂载之前捕获的系统视图克隆（见 [`SystemViewClone`]）。
    pub fn set_system_view_clone(&mut self, clone: SystemViewClone) {
        *self.system_view_clone.borrow_mut() = Some(clone);
    }

    pub fn set_file_monitor_enabled(&mut self, enabled: bool) {
        self.is_file_monitor_enabled = enabled;
    }

    pub fn take_mounted_targets(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.mounted_targets.borrow_mut())
    }

    pub(super) fn concrete_wildcard_mount_matches(
        &self,
        resolved_path: &str,
        storage_path: &str,
    ) -> Vec<String> {
        let mut scan_roots = Vec::with_capacity(4);
        append_unique_scan_root(&mut scan_roots, storage_path.to_string());
        append_unique_scan_root(
            &mut scan_roots,
            paths::data_media_user_root_for_user(self.user_id),
        );
        append_unique_scan_root(
            &mut scan_roots,
            self.to_data_media_backend_path(&self.redirect_target),
        );
        if let Some(anchor) = &self.real_storage_anchor {
            append_unique_scan_root(&mut scan_roots, anchor.clone());
        }
        concrete_wildcard_mount_matches_for_roots(resolved_path, storage_path, &scan_roots)
    }
}

fn append_unique_scan_root(scan_roots: &mut Vec<String>, root: String) {
    if root.is_empty()
        || scan_roots
            .iter()
            .any(|existing| paths::eq_ignore_case(existing, &root))
    {
        return;
    }
    scan_roots.push(root);
}
