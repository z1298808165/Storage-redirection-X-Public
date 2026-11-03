//! 经 MediaProvider 视图删除一个后端文件（**用到时临时克隆、用完即弃**）。
//!
//! 背景：自有 `Android/media/<pkg>` 下 sqlite `-shm` 毒化后，直接删 f2fs 真身会让
//! MediaProvider 缓存里的 dentry 指向已消失的文件（幽灵节点），应用随后的 O_CREAT
//! 全部失败；因此删除必须先经 MP 自己的 FUSE 走一遍，让它在节点簿记里显式失效该名称。
//!
//! 历史实现把这个视图做成**长生命周期绑定**（`tmp/fuse_host_media` 在宿主命名空间里
//! 长期存在），由此引出两个在长生命周期模型里无法解决的根问题：
//!
//! 1. **陈旧**：克隆诞生时绑定的是当时那条 MP FUSE 连接。MP 重启、`vold volume
//!    abort_fuse`、或 MP 自身崩溃后，宿主命名空间（建立会话时已 `MS_REC|MS_PRIVATE`）
//!    收不到新的 MP 挂载，这份克隆永远停在旧连接上；
//! 2. **无界等待**：陈旧的连接是「挂载记录还在、无人应答」形态，任何经它的 syscall
//!    都会无限期挂在 FUSE 队列里。而这个 syscall 位于宿主的 FUSE LOOKUP 回调中，
//!    挂住一次就是一个 worker 永久失去服务能力；16 个 worker 全挂即宿主整体停摆，
//!    看门狗只能靠 abort 换代自救，换代又给所有已接入应用留下死挂载层。
//!
//! 现在的做法是把这两件事从结构上消掉，而不是给长生命周期模型打补丁：
//!
//! - **每次用到时现克隆**：子进程 `setns` 进 init 命名空间（vold 与 MediaProvider 的
//!   挂载在那里，永远是最新一代）→ `unshare(CLONE_NEWNS)` 拿私有副本 →
//!   `open_tree(OPEN_TREE_CLONE)` 克隆 MP 视图 → `move_mount` 挂到模块目录下的临时
//!   挂载点 → `unlinkat`。子进程退出即销毁私有命名空间，挂载随之消失，磁盘上不留
//!   长期状态。**没有长生命周期对象，就没有陈旧**：MP 换代后下一次调用自然克隆到
//!   新一代连接。
//! - **调用方永不等**：删除跑在一次性子进程里，父进程用 `pidfd` + `poll` 等内核的
//!   进程退出事件做有界裁决（不做定时轮询）；超时即 `SIGKILL` 并有界回收。最长阻塞
//!   的是子进程而不是宿主 worker，连接无人应答也伤不到宿主。
//!
//! 失败按「本次删不掉」处理：调用方的既有兜底是 f2fs 直删（保证毒化数据消失），
//! 幽灵节点风险退回到加视图之前的形态。连续失败按冷却窗节流，避免毒化反复命中时
//! 变成 fork 风暴。

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicI64, Ordering};

/// 单次视图操作的有界裁决窗口。
///
/// 正常路径是几次本地系统调用加一次 FUSE 往返（毫秒级）；1.5 秒是「慢而活」与
/// 「无人应答」的分界，且明显小于宿主停摆判据（`HOST_STALL_NO_COMPLETION_MS`
/// 为 6 秒），不会与看门狗互相误伤。
const VIEW_OP_TIMEOUT_MS: i32 = 1_500;
/// `SIGKILL` 之后的有界回收窗口：超时即放弃，绝不阻塞调用线程。
const REAP_TIMEOUT_MS: i32 = 1_000;
/// 视图不可用后的重试冷却：期间同一进程不再为自愈起子进程。
const VIEW_FAIL_COOLDOWN_MS: i64 = 30_000;

/// 上次失败的时刻（monotonic 毫秒），0 表示没有失败记录。
static LAST_FAIL_MS: AtomicI64 = AtomicI64::new(0);

/// 一次视图删除的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewUnlink {
    /// 已通过 MediaProvider 视图删除，MP 的节点簿记已失效。
    Removed,
    /// 视图不可用（连接类错误或超时）：本次放弃，由调用方的 f2fs 直删兜底。
    Unavailable,
    /// 视图在线，但删除没成功（`ENOENT` 等）。同样交给兜底。
    Failed,
    /// 处于失败冷却窗或无法 fork：本次不尝试。
    Skipped,
}

impl ViewUnlink {
    /// 诊断用短名。
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// 该 errno 是否说明「连接不可用」而不是「这次操作的对象有问题」。
fn is_connection_errno(error_no: i32) -> bool {
    matches!(
        error_no,
        libc::ENOTCONN | libc::EIO | libc::ENODEV | libc::ESTALE
    )
}

/// 记录失败时刻，让后续调用进入冷却窗。
///
/// 这是节流而不是轮询：没有任何循环在等状态，只是把「刚失败过」记成时间戳，
/// 避免毒化反复命中时每次都 fork 子进程。
fn enter_fail_cooldown() {
    let now = crate::platform::paths::monotonic_ms();
    LAST_FAIL_MS.store(now, Ordering::Relaxed);
    log::debug!("fuse media view fail cooldown set at_ms={}", now);
}

/// 冷却窗是否仍然有效。
fn fail_cooldown_active() -> bool {
    let last = LAST_FAIL_MS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    crate::platform::paths::monotonic_ms().saturating_sub(last) < VIEW_FAIL_COOLDOWN_MS
}

/// 经 MediaProvider 视图删除 `rel`（相对用户存储根的路径）。
///
/// `user_id` 用于定位 `/mnt/user/<user>/emulated` 这条 MP 视图挂载。
pub(crate) fn unlink_relative(user_id: i32, rel: &str) -> ViewUnlink {
    if user_id < 0 || rel.is_empty() {
        return ViewUnlink::Skipped;
    }
    if fail_cooldown_active() {
        return ViewUnlink::Skipped;
    }

    let view_root = crate::platform::module_paths::FUSE_HOST_MEDIA_VIEW_DIR;
    let Ok(c_view_root) = CString::new(view_root) else {
        return ViewUnlink::Skipped;
    };
    let Ok(c_target) = CString::new(format!("{view_root}/{user_id}/{rel}")) else {
        return ViewUnlink::Skipped;
    };
    let Ok(c_source) = CString::new(format!("/mnt/user/{user_id}/emulated")) else {
        return ViewUnlink::Skipped;
    };

    let outcome = run_view_unlink(&ViewUnlinkJob {
        init_ns: c"/proc/1/ns/mnt",
        source: &c_source,
        view_root: &c_view_root,
        target: &c_target,
    });
    match outcome {
        ChildOutcome::Completed(0) => ViewUnlink::Removed,
        ChildOutcome::Completed(code) => {
            if is_connection_errno(code) {
                enter_fail_cooldown();
                log::warn!(
                    "fuse media view unlink unavailable errno={} {} rel={} fallback=f2fs-direct",
                    code,
                    crate::platform::errno::text(code),
                    rel
                );
                ViewUnlink::Unavailable
            } else {
                // 非连接类失败（`EPERM`/`EINVAL` 表示本环境不允许这套 `setns`+`unshare`
                // 流程、`ENOENT` 表示 MP 侧本来就没有该节点）同样进冷却：否则毒化反复
                // 命中时会按 LOOKUP 频率起一次性子进程，代价与收益完全不成比例。
                enter_fail_cooldown();
                log::info!(
                    "fuse media view unlink not applied errno={} {} rel={} fallback=f2fs-direct",
                    code,
                    crate::platform::errno::text(code),
                    rel
                );
                ViewUnlink::Failed
            }
        }
        ChildOutcome::TimedOut => {
            enter_fail_cooldown();
            log::warn!(
                "fuse media view unlink timeout timeout_ms={} rel={} fallback=f2fs-direct",
                VIEW_OP_TIMEOUT_MS,
                rel
            );
            ViewUnlink::Unavailable
        }
        ChildOutcome::Unavailable => ViewUnlink::Skipped,
    }
}

/// 子进程要用的全部路径（fork 前构造完毕，子进程内不做堆分配）。
struct ViewUnlinkJob<'a> {
    init_ns: &'a CStr,
    source: &'a CStr,
    view_root: &'a CStr,
    target: &'a CStr,
}

/// 子进程的执行结果。
enum ChildOutcome {
    /// 正常退出，携带退出码（0 成功，其余为 errno 的最低字节）。
    Completed(i32),
    /// 有界窗口内没有退出：视图连接无人应答。
    TimedOut,
    /// 无法执行（fork 失败等）。
    Unavailable,
}

/// 起一次性子进程完成「私有一份 init 命名空间 → 克隆 MP 视图 → 删除目标」。
fn run_view_unlink(job: &ViewUnlinkJob<'_>) -> ChildOutcome {
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志准备；子进程只做系统调用后立即 _exit。
    let child = unsafe { libc::fork() };
    if child < 0 {
        return ChildOutcome::Unavailable;
    }
    if child == 0 {
        let code = child_view_unlink(job);
        // SAFETY: 退出码只承载 errno 最低字节（本模块关心的 ENOTCONN=107 等均小于
        // 255）；_exit 终止子进程，不跑 atexit。
        unsafe { libc::_exit(code & 0xff) };
    }
    match wait_exit(child, VIEW_OP_TIMEOUT_MS) {
        ExitWait::Code(code) => ChildOutcome::Completed(code),
        // 被信号终止：不是本模块的预期路径，按无法判定处理。
        ExitWait::Signaled => ChildOutcome::Unavailable,
        ExitWait::Timeout => {
            // SAFETY: child 已确认仍在运行；SIGKILL 只作用于它。
            unsafe { libc::kill(child, libc::SIGKILL) };
            reap_child_bounded(child);
            ChildOutcome::TimedOut
        }
    }
}

/// 子进程主体：返回 0 或 errno。
///
/// 顺序不能变：私有命名空间必须在克隆之前建立，否则附着进去的挂载会留在原命名空间
/// 里（挂载生命周期长于进程，那正是「长生命周期陈旧视图」的成因）。
fn child_view_unlink(job: &ViewUnlinkJob<'_>) -> i32 {
    // SAFETY: 下方全部是 NUL 结尾的路径与本地 fd 的系统调用，不触碰 Rust 运行时状态。
    unsafe {
        // 1. 切到 init 命名空间：只有那里保证是当前一代的 MP 挂载。
        let ns_fd = libc::open(job.init_ns.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if ns_fd >= 0 {
            let switched = libc::setns(ns_fd, libc::CLONE_NEWNS);
            libc::close(ns_fd);
            if switched != 0 {
                return crate::platform::errno::last();
            }
        }
        // 2. 私有副本：附着进来的挂载随本进程退出一起消失，磁盘上不留长期状态。
        if libc::unshare(libc::CLONE_NEWNS) != 0 {
            return crate::platform::errno::last();
        }
        // 3. 整棵树转私有，防止附着沿共享传播组外播到其它命名空间。
        if libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
            std::ptr::null(),
        ) != 0
        {
            return crate::platform::errno::last();
        }
        // 4. 视图挂载点（普通目录，已存在时 EEXIST 无视）。`<user>` 子树由挂上来的
        //    MP 视图自带，不要再往挂载点里 mkdir 一层用户目录。
        libc::mkdir(job.view_root.as_ptr(), 0o755);

        // 5. 克隆 MP 视图：源必须属于当前（私有）命名空间，这正是先 setns 再 unshare 的用意。
        let tree_fd = libc::syscall(
            libc::SYS_open_tree,
            libc::AT_FDCWD,
            job.source.as_ptr(),
            OPEN_TREE_CLONE,
        ) as libc::c_int;
        if tree_fd < 0 {
            return crate::platform::errno::last();
        }
        // 6. 附着到视图根：MP 视图自带 `<user>` 子树，因此目标是根而不是 `<user>`，
        //    否则整棵 fuse 树会多套一层用户目录，路径拼不出来。
        let attached = libc::syscall(
            libc::SYS_move_mount,
            tree_fd,
            c"".as_ptr(),
            libc::AT_FDCWD,
            job.view_root.as_ptr(),
            MOVE_MOUNT_F_EMPTY_PATH,
        );
        let attach_errno = crate::platform::errno::last();
        libc::close(tree_fd);
        if attached != 0 {
            return attach_errno;
        }
        // 7. 经视图删除：MP 在自己的节点簿记里失效该名称，并删掉后端文件。
        if libc::syscall(libc::SYS_unlinkat, libc::AT_FDCWD, job.target.as_ptr(), 0) == 0 {
            0
        } else {
            crate::platform::errno::last()
        }
    }
}

/// `open_tree` 的克隆标志（`linux/mount.h` 的 `OPEN_TREE_CLONE`，libc 未导出）。
const OPEN_TREE_CLONE: libc::c_uint = 1;
/// `move_mount` 的「源由 fd 给出」标志（`MOVE_MOUNT_F_EMPTY_PATH`）。
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;

/// 子进程退出等待结果。
enum ExitWait {
    /// 正常退出，携带退出码。
    Code(i32),
    /// 被信号终止（已回收）。
    Signaled,
    /// 窗口内没有退出，子进程仍在运行。
    Timeout,
}

/// 等子进程退出并回收，最长 `timeout_ms`。
///
/// 用 `pidfd_open` + `poll` 等内核的进程退出事件，不做定时轮询；`pidfd_open`
/// 不可用（内核 < 5.3）时退回有界的 `WNOHANG` 兼容路径。
fn wait_exit(child: i32, timeout_ms: i32) -> ExitWait {
    // SAFETY: SYS_pidfd_open 只按 pid 创建内核 fd，第三个实参 0u32 是无标志常量。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, child, 0u32) as libc::c_int };
    if pidfd >= 0 {
        let mut pfd = libc::pollfd {
            fd: pidfd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd 是本地初始化的单个 pollfd，poll 只写入其 revents。
        let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        // SAFETY: pidfd 由上面的 pidfd_open 创建，此处是唯一关闭点。
        unsafe { libc::close(pidfd) };
        if ready > 0 {
            return reap_exited(child);
        }
        if ready == 0 {
            return ExitWait::Timeout;
        }
        // poll 失败（EINTR 等）：退回下面的有界循环，不据此判定超时。
    }
    let deadline = crate::platform::paths::monotonic_ms() + i64::from(timeout_ms);
    loop {
        if let Some(outcome) = try_reap(child) {
            return outcome;
        }
        if crate::platform::paths::monotonic_ms() >= deadline {
            return ExitWait::Timeout;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// `WNOHANG` 试回收一次：`None` 表示子进程还在运行。
fn try_reap(child: i32) -> Option<ExitWait> {
    let mut status: libc::c_int = 0;
    // SAFETY: child 是本进程 fork 出的子进程；WNOHANG 不阻塞。
    let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
    if waited == child {
        return Some(exit_wait_from_status(status));
    }
    if waited < 0 {
        // 已被回收或不是本进程子进程：不能再等。
        return Some(ExitWait::Signaled);
    }
    None
}

/// 内核报告子进程已退出后做阻塞回收（此时是僵尸，立即返回）。
fn reap_exited(child: i32) -> ExitWait {
    let mut status: libc::c_int = 0;
    // SAFETY: child 已由 pidfd 事件确认退出，waitpid 不会阻塞。
    let waited = unsafe { libc::waitpid(child, &mut status, 0) };
    if waited != child {
        return ExitWait::Signaled;
    }
    exit_wait_from_status(status)
}

fn exit_wait_from_status(status: libc::c_int) -> ExitWait {
    if libc::WIFEXITED(status) {
        ExitWait::Code(libc::WEXITSTATUS(status))
    } else {
        ExitWait::Signaled
    }
}

/// SIGKILL 之后的有界回收：最多等 [`REAP_TIMEOUT_MS`]，绝不阻塞调用线程。
///
/// 回收失败（子进程陷在 D 态）不是错误路径：它正是「视图连接无人应答」的证据，
/// 只在日志里留一条现场，僵尸由会话退出统一兜底。
fn reap_child_bounded(child: i32) {
    if matches!(wait_exit(child, REAP_TIMEOUT_MS), ExitWait::Timeout) {
        log::warn!(
            "fuse media view child unkillable pid={} effect=leaked-zombie released-by-session-exit",
            child
        );
    }
}
