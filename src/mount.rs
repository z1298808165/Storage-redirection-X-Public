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
/// 等待克隆子进程结论的有界窗口（毫秒）。
///
/// 克隆本身只碰挂载表、不读文件内容，正常是微秒级；给 2s 只是为了在极端卡顿时
/// 仍能有序放弃，而不是把挂载主链路挂住。
const CLONE_WAIT_TIMEOUT_MS: libc::c_long = 2000;

/// `move_mount(2)` 的 `MOVE_MOUNT_F_EMPTY_PATH`：源由 fd 指定（游离挂载没有路径）。
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;

/// 系统存储视图的**游离克隆**（`open_tree(OPEN_TREE_CLONE)`）。
///
/// 用途：自有私有目录（`Android/{data,media,obb}/<pkg>`）的恢复 bind 需要一个**纯系统视图**
/// 作为源，且该源必须是 MediaProvider 服务的那一层。
///
/// # 为什么源取自 init 命名空间
///
/// 各种取源的「捷径」都被真机否决过，记录在此避免重走：
///
/// - `/mnt/pass_through/<user>/emulated`（f2fs 直连后端）：结构与 emulated 根相同，但**不是**
///   MP 视图。绑过去的自有私有目录里应用写的文件按调用方 umask 落 mode（SQLite 边车即
///   `0600`），绕过 MP 的 `(mode & ~0777) | 0664` 口径，MP 用自身身份（附加组
///   media_rw/ext_data_rw、`CapEff: 0`）打不开，跨进程读同一份库的一方拿到
///   `SQLITE_CANTOPEN`（真机 2026-11-12 duchamp：`Android/media/<pkg>` 是 f2fs 直连 bind，
///   两只 `-wal`/`-shm` 为 0600，管理器一次冷启 16 条）。
/// - 应用命名空间里的 `/mnt/user/<user>/emulated`：**本模块自己就覆盖它**（视图根上提之后
///   别名族与存储根都换成模块 FUSE）。克隆到的是自己的会话，root 请求无对应策略 → ENOENT；
///   即便绑上也只是「模块层对模块层」，两条路径仍各持一套页缓存。
///
/// 而 **init 命名空间**（`/proc/1/ns/mnt`）里模块从不挂载，那里的 `/mnt/user/<user>/emulated`
/// 恒为 MediaProvider 的 FUSE 视图。真机 2026-11-12 duchamp 实证（同一时刻、同一目录）：
///
/// ```text
/// 应用命名空间 /mnt/user/0/emulated/...  -> dev 0:142（本模块 FUSE）
/// init 命名空间 /mnt/user/0/emulated/... -> dev 0:131（MediaProvider FUSE）
/// init 命名空间 /storage/emulated/...    -> dev 0:131（同一份）
/// ```
///
/// 取法沿用 `fuse_redirect::media_view::unlink_relative` 的成熟范式：**子进程** `setns` 进 init
/// 命名空间后克隆，再用 `SCM_RIGHTS` 把克隆 fd 交回宿主（细节见 [`SystemViewClone::capture`]）。
/// 克隆与源挂载共享超级块；宿主 `move_mount` 附着后即持有自己的引用，子进程退出不留下挂载
/// 痕迹。
pub struct SystemViewClone {
    fd: libc::c_int,
}

impl SystemViewClone {
    /// 克隆自有私有目录恢复所需的系统存储视图。
    ///
    /// # 为什么必须经 init 命名空间
    ///
    /// 两条「捷径」都被真机否决，记录在此避免重走：
    ///
    /// - `/mnt/pass_through/<user>/emulated`（f2fs 直连后端）：结构与 emulated 根一致，但
    ///   **不是** MP 视图。绑过去的自有私有目录里，应用写的文件按调用方 umask 落 mode
    ///   （SQLite 边车即 `0600`），绕过 MP 的 `(mode & ~0777) | 0664` 口径，MP 以自身身份
    ///   （附加组 media_rw/ext_data_rw、`CapEff: 0`）打不开 → 跨进程读同一份库的一方拿到
    ///   `SQLITE_CANTOPEN`（真机 2026-11-12 duchamp：管理器一次冷启 16 条）。
    /// - 应用命名空间里的 `/mnt/user/<user>/emulated`：**本模块自己覆盖它**（视图根上提后
    ///   别名族与存储根都换成模块 FUSE），克隆到的是自己的会话。
    ///
    /// init 命名空间（`/proc/1/ns/mnt`）里模块从不挂载，`/mnt/user/<user>/emulated` 恒为
    /// MediaProvider 的 FUSE 视图。真机 2026-11-12 duchamp 同一目录实测：应用命名空间
    /// `dev 0:142`（本模块），init 命名空间 `dev 0:131`（MediaProvider）。
    ///
    /// # 为什么要子进程 + `SCM_RIGHTS`
    ///
    /// `setns` 会整段换掉本进程的挂载命名空间，而调用方随后还要在应用命名空间里完成恢复
    /// bind，不能在宿主里切。`open_tree`/`mount(MS_BIND)` 也不接受跨命名空间的 magic link
    /// 路径（内核 `check_mnt` → `EINVAL`，真机实测 `/proc/1/root/...` 报 errno=22）。
    /// 因此沿用 `fuse_redirect::media_view` 的范式：**子进程** setns 进 init 命名空间后克隆，
    /// 再用 `SCM_RIGHTS` 把克隆 fd 传回宿主。克隆出的是游离挂载（不属于任何命名空间），
    /// 跨进程传递后仍可 `move_mount` 附着到宿主命名空间。
    pub fn capture(user_id: i32) -> Option<Self> {
        let source = std::ffi::CString::new(format!("/mnt/user/{user_id}/emulated")).ok()?;
        let mut pair = [0 as libc::c_int; 2];
        // SAFETY: pair 是本栈帧内的两元素数组，socketpair 成功即写入两个有效 fd。
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) } != 0
        {
            log::warn!(
                "system view clone socketpair failed errno={}",
                crate::platform::errno::last()
            );
            return None;
        }
        let (host_fd, child_fd) = (pair[0], pair[1]);
        // 接收有界：子进程万一在 setns/克隆上停住，宿主不能在挂载关键路径上无限等。
        // 超时按「本轮放弃恢复、下一轮幂等重试」处理，不会让应用拿不到存储。
        let timeout = libc::timeval {
            tv_sec: CLONE_WAIT_TIMEOUT_MS / 1000,
            tv_usec: (CLONE_WAIT_TIMEOUT_MS % 1000) * 1000,
        };
        // SAFETY: setsockopt 只读本栈帧内的 timeval。
        unsafe {
            libc::setsockopt(
                host_fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &timeout as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
        // SAFETY: 子进程内只做系统调用与 `_exit`；`source` 在 fork 前构造好，子进程不做堆分配。
        let child = unsafe { libc::fork() };
        if child < 0 {
            log::warn!(
                "system view clone fork failed errno={}",
                crate::platform::errno::last()
            );
            // SAFETY: 两个 fd 均由本函数创建且尚未移交。
            unsafe {
                libc::close(host_fd);
                libc::close(child_fd);
            }
            return None;
        }
        if child == 0 {
            // 子进程：克隆并把 fd 送回宿主；无论成败都在这里结束，返回值由宿主从通道读到。
            clone_in_init_ns_and_send(child_fd, source.as_ptr());
            // SAFETY: 只关闭本进程内的 socket fd，随后立即 `_exit`（不运行析构）。
            unsafe {
                libc::close(child_fd);
                libc::_exit(0);
            }
        }
        // `receive_fd` 是安全包装；这里只需在拿到结果后关闭本端 socket。
        let received = receive_fd(host_fd);
        // SAFETY: host_fd 由本函数创建且至此不再需要。
        unsafe { libc::close(host_fd) };
        let mut status = 0 as libc::c_int;
        // 抗 EINTR：父进程可能在等子进程期间被信号打断，未回收就会留下僵尸。
        loop {
            // SAFETY: child 是本进程刚 fork 的子进程，status 指向栈上有效空间。
            let reaped = unsafe { libc::waitpid(child, &mut status, 0) };
            if reaped == child || reaped < 0 {
                break;
            }
        }
        let Some(fd) = received else {
            log::warn!(
                "system view clone failed user={} (child setns/clone unavailable)",
                user_id
            );
            return None;
        };
        log::debug!("system view cloned from init ns user={} fd={}", user_id, fd);
        Some(Self { fd })
    }

    /// 把游离克隆附着到 `target`（一次性消费该挂载）。
    ///
    /// **不能**拿 `/proc/self/fd/<n>/...` 当 bind 源：`mount(MS_BIND)` 要求源挂载属于调用方
    /// 当前命名空间（内核 `do_loopback` 的 `check_mnt`），而游离克隆不在任何命名空间里——
    /// 真机实测 `bind … errno=22 EINVAL`。搬运游离/跨命名空间挂载只能走 `move_mount`。
    ///
    /// 附着成功即消费 fd（`Drop` 会再 `close` 一次，返回 EBADF 属正常）。
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

/// 子进程侧：setns 进 init 挂载命名空间，克隆 `source`，把 fd 经 `sock` 送回宿主。
///
/// **只做系统调用**：不分配堆内存、不打日志——fork 后的子进程内分配可能与父进程持有同一把
/// 锁而自锁（因此 `source` 在 `fork` 之前就构造好，以裸指针传入）。失败则一端不写，宿主读到
/// EOF 后按「本轮放弃恢复」处理。
fn clone_in_init_ns_and_send(sock: libc::c_int, source: *const libc::c_char) {
    // SAFETY: 本节全部是 NUL 结尾路径与本地 fd 的系统调用，运行在刚 fork 的子进程里。
    unsafe {
        let ns_fd = libc::open(c"/proc/1/ns/mnt".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if ns_fd < 0 {
            return;
        }
        let switched = libc::setns(ns_fd, libc::CLONE_NEWNS);
        libc::close(ns_fd);
        if switched != 0 {
            return;
        }
        let tree_fd = libc::syscall(libc::SYS_open_tree, libc::AT_FDCWD, source, OPEN_TREE_CLONE)
            as libc::c_int;
        if tree_fd < 0 {
            return;
        }
        send_fd(sock, tree_fd);
        libc::close(tree_fd);
    }
}

/// 经 `SCM_RIGHTS` 送出一个 fd（单字节载荷，仅作握手信号）。
fn send_fd(sock: libc::c_int, fd: libc::c_int) -> bool {
    // SAFETY: iov/control 均为本栈帧内缓冲，sendmsg 只读取它们。
    unsafe {
        let byte: u8 = b'V';
        let iov = libc::iovec {
            iov_base: &byte as *const u8 as *mut libc::c_void,
            iov_len: 1,
        };
        // SAFETY: CMSG_SPACE 为纯常量计算，无运行时副作用。
        let mut control =
            [0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) } as usize];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &iov as *const libc::iovec as *mut libc::iovec;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = control.len();
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return false;
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as usize;
        std::ptr::copy_nonoverlapping(
            &fd as *const libc::c_int as *const u8,
            libc::CMSG_DATA(cmsg),
            std::mem::size_of::<libc::c_int>(),
        );
        libc::sendmsg(sock, &msg, 0) == 1
    }
}

/// 宿主侧：从 `sock` 收一个 `SCM_RIGHTS` 送来的 fd。
fn receive_fd(sock: libc::c_int) -> Option<libc::c_int> {
    // SAFETY: iov/control 均为本栈帧内缓冲，recvmsg 只写入它们。
    unsafe {
        let mut byte: u8 = 0;
        let iov = libc::iovec {
            iov_base: &mut byte as *mut u8 as *mut libc::c_void,
            iov_len: 1,
        };
        // SAFETY: CMSG_SPACE 为纯常量计算，无运行时副作用。
        let mut control =
            [0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) } as usize];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &iov as *const libc::iovec as *mut libc::iovec;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = control.len();
        if libc::recvmsg(sock, &mut msg, 0) != 1 {
            return None;
        }
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null()
            || (*cmsg).cmsg_level != libc::SOL_SOCKET
            || (*cmsg).cmsg_type != libc::SCM_RIGHTS
        {
            return None;
        }
        let mut fd: libc::c_int = -1;
        std::ptr::copy_nonoverlapping(
            libc::CMSG_DATA(cmsg),
            &mut fd as *mut libc::c_int as *mut u8,
            std::mem::size_of::<libc::c_int>(),
        );
        if fd < 0 { None } else { Some(fd) }
    }
}

impl Drop for SystemViewClone {
    fn drop(&mut self) {
        // SAFETY: fd 由 capture 创建且只在此处关闭一次；若已随 `move_mount` 消费则返回 EBADF。
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
