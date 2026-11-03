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
//! - **调用方永不等**：删除跑在子进程里，调用方等的是**内核的进程退出事件**
//!   （`pidfd` + `poll`，没有 `pidfd` 时用 `SIGCHLD` + `sigtimedwait`），两条路都
//!   不轮询；超时即 `SIGKILL`。最长阻塞的是子进程而不是宿主 worker，连接无人应答也
//!   伤不到宿主。
//!
//! # 为什么是「监督进程 + worker」两层，而不是直接 fork 一个子进程
//!
//! 一次性子进程模型最容易在**回收**这一环泄漏，本项目历史上已经吃过两次：
//! worker 被 init 收养后 `waitpid` 返回 `ECHILD` 被误判成回收成功，SIGKILL 永不发出，
//! `srx_fuse` 残留空转；以及父进程放弃回收后僵尸长期挂在 daemon 名下。
//!
//! FUSE 上做 syscall 的进程有可能落进不可中断睡眠（真机抓到过
//! `wchan=__fuse_simple_request fuse_dentry_revalidate` 的 D 态），此时 `SIGKILL`
//! 只是被记账、进程不会立刻离开进程表。如果这个进程是宿主的直接子进程，宿主又不
//! 再 `wait` 它，它就会在宿主名下永久留一个僵尸/活体，**每失败一次多一个**。
//!
//! 所以结构改成：
//!
//! ```text
//! 宿主 ──fork──▶ mid（监督进程，只做 fork/poll/kill/write，不接触任何 FUSE）
//!                  └──fork──▶ worker（真正 setns + 克隆 + unlink 的进程）
//! ```
//!
//! - mid 身上没有任何会阻塞在别人的文件系统上的操作，因此它**一定**在有限时间内退出，
//!   宿主回收它永远不会卡住 —— 宿主进程表里只会短暂出现这一个必定回收的条目；
//! - worker 由 mid 收尸；mid 放弃回收后 worker 变成孤儿被 init 收养，由 init 负责
//!   `wait`，**僵尸不会跨父进程存活**，也就不会在宿主名下累积；
//! - 万一 worker 陷在 D 态迟迟不退，它就是「视图连接无人应答」的现场。这类残留通过
//!   [`PENDING_CHILD_PID`] 记账：在它离开进程表之前不再起新的 worker，泄漏上界因此
//!   被钉成**最多一个**，而不是按冷却周期无限增长。
//!
//! 视图侧本身没有泄漏面：`move_mount` 发生在 worker 的私有命名空间内，worker 一退出
//! 挂载即随之销毁；磁盘上只会留下 `tmp/fuse_host_media` 这一个幂等的空目录。
//!
//! 失败按「本次删不掉」处理：调用方的既有兜底是 f2fs 直删（保证毒化数据消失），
//! 幽灵节点风险退回到加视图之前的形态。

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

/// 单次视图操作的有界裁决窗口。
///
/// 正常路径是几次本地系统调用加一次 FUSE 往返（毫秒级）；1.5 秒是「慢而活」与
/// 「无人应答」的分界，且明显小于宿主停摆判据（`HOST_STALL_NO_COMPLETION_MS`
/// 为 6 秒），不会与看门狗互相误伤。
const VIEW_OP_TIMEOUT_MS: i32 = 1_500;
/// `SIGKILL` 之后的有界回收窗口：超时即放弃，绝不阻塞调用线程。
const REAP_TIMEOUT_MS: i32 = 1_000;
/// 等监督进程交回结论的窗口：它内部的等待预算（[`VIEW_OP_TIMEOUT_MS`] +
/// [`REAP_TIMEOUT_MS`]）之上留出余量，正常情况下这个上限根本用不到。
const SUPERVISOR_TIMEOUT_MS: i32 = 4_000;
/// 视图不可用后的重试冷却：期间同一进程不再为自愈起子进程。
const VIEW_FAIL_COOLDOWN_MS: i64 = 30_000;

/// 上次失败的时刻（monotonic 毫秒），0 表示没有失败记录。
static LAST_FAIL_MS: AtomicI64 = AtomicI64::new(0);
/// 同一时刻是否已经有一个视图子进程在飞：并发命中只放一个进去，其余让位给直删兜底。
static INFLIGHT: AtomicBool = AtomicBool::new(false);
/// 还没离开进程表的视图子进程 pid（0 表示没有）。
///
/// 只有监督进程确认「SIGKILL 之后仍有界窗口内退不掉」时才会写入。此时那个 worker 已
/// 被 init 收养，本进程再也 `wait` 不到它，只能观察 `/proc/<pid>` 消失来判断它终于
/// 退出。存在期间不再起新的 worker —— 这是把「连接停摆」的代价钉死成一个进程的关键。
static PENDING_CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// 一次视图删除的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewUnlink {
    /// 已通过 MediaProvider 视图删除，MP 的节点簿记已失效。
    Removed,
    /// 视图不可用（连接类错误或超时）：本次放弃，由调用方的 f2fs 直删兜底。
    Unavailable,
    /// 视图在线，但删除没成功（`ENOENT` 等）。同样交给兜底。
    Failed,
    /// 处于失败冷却窗、已有子进程在飞、或无法 fork：本次不尝试。
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

/// 上一个视图子进程是否还赖在进程表里。
///
/// 顺手做两件事：是我们自己的直接子进程时用 `WNOHANG` 立刻收尸（监督进程异常残留的
/// 兜底）；不是（worker 已被 init 收养）时只看它是否还存在。只有确认它彻底离开进程表
/// 才允许起新的一次，因此不管失败多少次，残留进程数上界都是一个。
fn pending_child_alive() -> bool {
    let pid = PENDING_CHILD_PID.load(Ordering::Relaxed);
    if pid <= 0 {
        return false;
    }
    let mut status: libc::c_int = 0;
    // SAFETY: pid 是此前记录的本进程子进程；WNOHANG 不阻塞，ECHILD 表示已不是我们的
    // 子进程（被 init 收养），两种情况都不影响下面的存在性判断。
    let _ = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return true;
    }
    PENDING_CHILD_PID.store(0, Ordering::Relaxed);
    false
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
            // 失败一律进冷却：连接类错误说明视图停摆，非连接类（`EPERM` 表示本环境不
            // 允许 `setns`+`unshare` 这套流程、`ENOENT` 表示 MP 侧本来就没有该节点）
            // 说明这条路在本机走不通。两种情况都不值得按 LOOKUP 频率反复起子进程。
            enter_fail_cooldown();
            if is_connection_errno(code) {
                log::warn!(
                    "fuse media view unlink unavailable errno={} {} rel={} fallback=f2fs-direct",
                    code,
                    crate::platform::errno::text(code),
                    rel
                );
                ViewUnlink::Unavailable
            } else {
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
        // 并发让位：不算失败，不进冷却——另一个调用正在替这次尝试，让它先跑完。
        ChildOutcome::Busy => ViewUnlink::Skipped,
        // 起不了子进程（fork/pipe 失败）：属本地资源问题，同样要冷却，否则会打成
        // fork 风暴。
        ChildOutcome::Failed => {
            enter_fail_cooldown();
            log::warn!(
                "fuse media view spawn failed rel={} fallback=f2fs-direct",
                rel
            );
            ViewUnlink::Skipped
        }
    }
}

/// 子进程要用的全部路径（fork 前构造完毕，子进程内不做堆分配）。
struct ViewUnlinkJob<'a> {
    init_ns: &'a CStr,
    source: &'a CStr,
    view_root: &'a CStr,
    target: &'a CStr,
}

/// 一次视图删除的裁决结果。
enum ChildOutcome {
    /// 正常退出，携带退出码（0 成功，其余为 errno）。
    Completed(i32),
    /// 有界窗口内没有退出：视图连接无人应答。
    TimedOut,
    /// 无法起子进程（fork/pipe 失败）。
    Failed,
    /// 已有子进程在飞或上次的残留还没退：本次让位。
    Busy,
}

/// 管道载荷：结论 + 需要记账的残留进程 pid。
const PAYLOAD_LEN: usize = 8;
/// 监督进程写回的哨兵：worker 在窗口内没有退出。
const SENTINEL_TIMEOUT: i32 = -1;
/// 监督进程写回的哨兵：worker 起不来或异常终止。
const SENTINEL_UNAVAILABLE: i32 = -2;

/// 单飞行闸门的释放器：离开作用域即放行，任何提前返回都不会把闸门粘住。
struct InflightRelease;

impl Drop for InflightRelease {
    fn drop(&mut self) {
        INFLIGHT.store(false, Ordering::Release);
    }
}

/// 起「监督进程 + worker」两层子进程完成「私有一份 init 命名空间 → 克隆 MP 视图 → 删除目标」。
fn run_view_unlink(job: &ViewUnlinkJob<'_>) -> ChildOutcome {
    // 单飞行：同一条自愈路径上只允许一个视图子进程在飞。并发命中直接让位给直删兜底，
    // 避免 16 个 worker 同时为毒化各起一套子进程。
    if INFLIGHT.swap(true, Ordering::AcqRel) {
        return ChildOutcome::Busy;
    }
    let _release = InflightRelease;

    // 上次那个没能回收的 worker 还在进程表里，说明视图连接仍然无人应答：这时再起新的
    // 只会把残留进程数往上堆，直接让位。
    if pending_child_alive() {
        log::debug!("fuse media view pending child still alive, skip this round");
        return ChildOutcome::Busy;
    }

    crate::logging::prepare_for_fork();

    let mut fds: [libc::c_int; 2] = [0; 2];
    // SAFETY: fds 是本地两元素数组，pipe2 只写入它。
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return ChildOutcome::Failed;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // SAFETY: fork 前已完成日志准备；子进程只做系统调用，且以 _exit 结束。
    let supervisor = unsafe { libc::fork() };
    if supervisor < 0 {
        // SAFETY: 两个 fd 尚未被任何子进程继承，回收后没有副作用。
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return ChildOutcome::Failed;
    }
    if supervisor == 0 {
        // SAFETY: 监督进程接管写端，读端在本进程无用；本函数不返回。
        unsafe {
            libc::close(read_fd);
            supervise_worker(job, write_fd);
        }
    }

    // SAFETY: 父进程不写管道，只读结论。
    unsafe { libc::close(write_fd) };
    let outcome = collect_outcome(read_fd, supervisor);
    // SAFETY: read_fd 由本进程创建，这里是唯一关闭点。
    unsafe { libc::close(read_fd) };
    outcome
}

/// 监督进程主体：起 worker、有界裁决、把结论写回管道，然后退出。**永不返回**。
///
/// 这里刻意只做 `fork` / `wait` / `kill` / `write`：没有任何一步会阻塞在别人的文件
/// 系统上，所以本进程一定在有限时间内退出，宿主回收它永远不会卡。
fn supervise_worker(job: &ViewUnlinkJob<'_>, write_fd: libc::c_int) -> ! {
    // SAFETY: 以下全部是对本地 fd / 子进程的系统调用，不触碰 Rust 运行时状态。
    unsafe {
        let worker = libc::fork();
        if worker < 0 {
            write_payload(write_fd, SENTINEL_UNAVAILABLE, 0);
            libc::_exit(0);
        }
        if worker == 0 {
            // worker 只把结果压进退出码（errno 全部小于 256），由监督进程统一写管道：
            // 管道里**只有监督进程一条记录**，父进程不会读到两份互相覆盖的结论。
            let code = child_view_unlink(job);
            libc::_exit(code & 0xff);
        }
        match wait_exit(worker, VIEW_OP_TIMEOUT_MS) {
            ExitWait::Code(code) => write_payload(write_fd, code, 0),
            // 被信号终止（SIGBUS 等）：worker 已回收，本次按「没做成」处理。
            ExitWait::Signaled => write_payload(write_fd, SENTINEL_UNAVAILABLE, 0),
            // 没有 pidfd 就无法有界裁决：杀掉 worker 让 init 兜底收尸，本次直接判失败。
            ExitWait::Unsupported => {
                libc::kill(worker, libc::SIGKILL);
                write_payload(write_fd, SENTINEL_UNAVAILABLE, 0);
            }
            ExitWait::Timeout => {
                libc::kill(worker, libc::SIGKILL);
                // SIGKILL 对有界窗口内仍不退的 worker 不生效（不可中断睡眠）；此时
                // 放弃回收，把它交给调用方记账。本进程退出后它被 init 收养，僵尸不会
                // 跨父进程存活。
                let unreaped = match wait_exit(worker, REAP_TIMEOUT_MS) {
                    ExitWait::Timeout => worker,
                    _ => 0,
                };
                write_payload(write_fd, SENTINEL_TIMEOUT, unreaped);
            }
        }
        libc::_exit(0);
    }
}

/// 读回监督进程的结论，并保证把它回收掉。
fn collect_outcome(read_fd: libc::c_int, supervisor: i32) -> ChildOutcome {
    let mut payload = [0u8; PAYLOAD_LEN];
    let read_result = read_payload_bounded(read_fd, SUPERVISOR_TIMEOUT_MS, &mut payload);
    if matches!(read_result, PayloadRead::Timeout) {
        // 监督进程不接触 FUSE，理论上不会卡；走到这里说明它被外部信号或 OOM 拦住了。
        // SAFETY: supervisor 是本进程刚 fork 出的子进程。
        unsafe { libc::kill(supervisor, libc::SIGKILL) };
    }
    // 监督进程是宿主唯一的直接子进程，必须回收，否则僵尸挂在宿主名下。
    if matches!(wait_exit(supervisor, REAP_TIMEOUT_MS), ExitWait::Timeout) {
        log::warn!(
            "fuse media view supervisor unkillable pid={} effect=leaked-child released-by-session-exit",
            supervisor
        );
    }
    if !matches!(read_result, PayloadRead::Full) {
        return ChildOutcome::Failed;
    }
    let conclusion = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let pending = i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
    if pending > 0 {
        PENDING_CHILD_PID.store(pending, Ordering::Relaxed);
        log::warn!(
            "fuse media view child unreaped pid={} effect=bounded-single-pending-relaxed-by-init",
            pending
        );
    }
    match conclusion {
        SENTINEL_TIMEOUT => ChildOutcome::TimedOut,
        SENTINEL_UNAVAILABLE => ChildOutcome::Failed,
        code => ChildOutcome::Completed(code),
    }
}

/// 管道读取结果。
enum PayloadRead {
    Full,
    Eof,
    Timeout,
}

/// 有界读满 [`PAYLOAD_LEN`] 字节。
fn read_payload_bounded(
    fd: libc::c_int,
    timeout_ms: i32,
    buffer: &mut [u8; PAYLOAD_LEN],
) -> PayloadRead {
    let deadline = crate::platform::paths::monotonic_ms() + i64::from(timeout_ms);
    let mut filled = 0;
    while filled < PAYLOAD_LEN {
        let remaining = (deadline - crate::platform::paths::monotonic_ms()).max(0);
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd 是本地初始化的单个 pollfd，poll 只写入其 revents。
        let ready = unsafe { libc::poll(&mut pfd, 1, remaining as libc::c_int) };
        if ready < 0 {
            if crate::platform::errno::last() == libc::EINTR {
                continue;
            }
            return PayloadRead::Timeout;
        }
        if ready == 0 {
            return PayloadRead::Timeout;
        }
        // SAFETY: buffer[filled..] 是有效的可写切片，长度按剩余容量传入。
        let read = unsafe {
            libc::read(
                fd,
                buffer[filled..].as_mut_ptr().cast(),
                PAYLOAD_LEN - filled,
            )
        };
        if read == 0 {
            return PayloadRead::Eof;
        }
        if read < 0 {
            if crate::platform::errno::last() == libc::EINTR {
                continue;
            }
            return PayloadRead::Timeout;
        }
        filled += read as usize;
    }
    PayloadRead::Full
}

/// 往管道写结论。只在子进程里用：失败无需上报，读端关了就是父进程已经放弃。
///
/// # Safety
///
/// `write_fd` 必须是本进程持有的可写管道端。
unsafe fn write_payload(write_fd: libc::c_int, conclusion: i32, pending_pid: i32) {
    let mut buffer = [0u8; PAYLOAD_LEN];
    buffer[..4].copy_from_slice(&conclusion.to_ne_bytes());
    buffer[4..].copy_from_slice(&pending_pid.to_ne_bytes());
    let mut written = 0;
    while written < PAYLOAD_LEN {
        // SAFETY: buffer[written..] 是有效可读切片，长度按剩余容量传入。
        let n = unsafe {
            libc::write(
                write_fd,
                buffer[written..].as_ptr().cast(),
                PAYLOAD_LEN - written,
            )
        };
        if n <= 0 {
            if n < 0 && crate::platform::errno::last() == libc::EINTR {
                continue;
            }
            return;
        }
        written += n as usize;
    }
}

/// worker 主体：返回 0 或 errno。
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
    /// 两种事件等待机制都不可用（`pidfd` 缺失且信号屏蔽失败），无法做有界裁决。
    Unsupported,
}

/// 本进程所在内核是否支持 `pidfd`（Linux 5.3+/Android 11+）。
///
/// 只是选择**等待机制**，不决定功能可用性：没有 `pidfd` 就走 `SIGCHLD` +
/// `sigtimedwait`，同样是有界的事件等待。因此这里不做任何功能降级。
fn pidfd_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        // SAFETY: pidfd_open(pid, 0) 只按 pid 申请一个内核 fd，成功后立刻关闭。
        let fd = unsafe {
            libc::syscall(libc::SYS_pidfd_open, libc::getpid() as libc::pid_t, 0u32) as libc::c_int
        };
        if fd < 0 {
            log::info!("fuse media view wait_path=sigchld reason=pidfd_unavailable");
            return false;
        }
        // SAFETY: fd 由上面的 pidfd_open 创建，此处是唯一关闭点。
        unsafe { libc::close(fd) };
        true
    })
}

/// 等子进程退出并回收，最长 `timeout_ms`。
///
/// 两条路都是**事件驱动**、都不轮询：
///
/// 1. `pidfd_open` + `poll`：等内核把「这个进程退出了」这件事投递到 fd 上；
/// 2. `SIGCHLD` + `sigtimedwait`：没有 `pidfd` 时等内核投递子进程状态变更信号。标准
///    信号在「有子进程退出」这件事上有合并语义但**不会凭空丢失判定能力**——循环每轮
///    开头先用 `WNOHANG` 复查一次目标 pid，屏蔽前就退出的竞态因此也被覆盖。
///
/// 之所以两条路都保留而不是「没有 pidfd 就不要视图」：视图删除是毒化 `-shm` 自愈的
/// 唯一无副作用路径，停用它等于把幽灵节点的风险重新交还给应用（真机上表现为数据库
/// 打不开）。等待机制是可替换的实现细节，不该变成功能开关。
fn wait_exit(child: i32, timeout_ms: i32) -> ExitWait {
    if pidfd_available() {
        return wait_exit_by_pidfd(child, timeout_ms);
    }
    wait_exit_by_sigchld(child, timeout_ms)
}

/// `pidfd` 路径：`poll` 等进程退出事件。
fn wait_exit_by_pidfd(child: i32, timeout_ms: i32) -> ExitWait {
    // SAFETY: SYS_pidfd_open 只按 pid 创建内核 fd，第三个实参 0u32 是无标志常量。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, child, 0u32) as libc::c_int };
    if pidfd < 0 {
        // 探测成功但具体 pid 打不开（例如它是线程组组长之外的形态）：换信号路径。
        return wait_exit_by_sigchld(child, timeout_ms);
    }
    let deadline = crate::platform::paths::monotonic_ms() + i64::from(timeout_ms);
    let outcome = loop {
        let remaining = (deadline - crate::platform::paths::monotonic_ms()).max(0);
        let mut pfd = libc::pollfd {
            fd: pidfd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd 是本地初始化的单个 pollfd，poll 只写入其 revents；剩余预算已
        // 收敛到非负，作为毫秒超时传入是合法的。
        let ready = unsafe { libc::poll(&mut pfd, 1, remaining as libc::c_int) };
        if ready > 0 {
            break reap_exited(child);
        }
        if ready == 0 {
            break ExitWait::Timeout;
        }
        // poll 被信号打断：带着剩余预算重试，不据此判定超时。
        if crate::platform::errno::last() != libc::EINTR {
            break wait_exit_by_sigchld(child, timeout_ms);
        }
    };
    // SAFETY: pidfd 由上面的 pidfd_open 创建，此处是唯一关闭点。
    unsafe { libc::close(pidfd) };
    outcome
}

/// `SIGCHLD` 路径：`sigtimedwait` 等子进程状态变更，同样是事件驱动。
fn wait_exit_by_sigchld(child: i32, timeout_ms: i32) -> ExitWait {
    // SAFETY: sigset_t 是 POD 全零即「空集」的合法初始值，随后由 sigemptyset 重新初始化。
    let mut block: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: 同上，previous 只用于接收内核写回的旧屏蔽字，无其它前置约束。
    let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: block / previous 都是本地栈上的 sigset_t，仅由内核读写。
    unsafe {
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGCHLD);
    }
    // 先屏蔽再复查：屏蔽之后到达的 SIGCHLD 会排队，不会像默认动作那样被丢弃。
    // SAFETY: block 已初始化，previous 是本地可写 sigset_t；pthread_sigmask 只作用于
    // 当前线程的屏蔽字，不影响宿主其它线程。
    if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut previous) } != 0 {
        return ExitWait::Unsupported;
    }
    let deadline = crate::platform::paths::monotonic_ms() + i64::from(timeout_ms);
    let outcome = loop {
        // 每轮先看一次内核状态：覆盖「屏蔽前就已退出」的窗口，也覆盖「信号已被本轮消费」。
        let mut status: libc::c_int = 0;
        // SAFETY: child 是本进程子进程；WNOHANG 不阻塞。
        let waited = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
        if waited == child {
            break exit_wait_from_status(status);
        }
        if waited < 0 {
            break ExitWait::Signaled;
        }
        let remaining = (deadline - crate::platform::paths::monotonic_ms()).max(0);
        if remaining == 0 {
            break ExitWait::Timeout;
        }
        let timeout = libc::timespec {
            tv_sec: remaining / 1000,
            tv_nsec: (remaining % 1000) * 1_000_000,
        };
        // SAFETY: siginfo_t 是 POD，全零只是占位初值，sigtimedwait 会整体写入。
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: info 是本地 siginfo_t 供内核写入，timeout 是本地 timespec 只读。
        let received = unsafe { libc::sigtimedwait(&block, &mut info, &timeout) };
        if received == libc::SIGCHLD {
            continue;
        }
        if received < 0 {
            let error_no = crate::platform::errno::last();
            if error_no == libc::EINTR {
                continue;
            }
            // EAGAIN 是 sigtimedwait 的超时语义。
            if error_no == libc::EAGAIN {
                break ExitWait::Timeout;
            }
            break ExitWait::Unsupported;
        }
    };
    // SAFETY: previous 由上面的 pthread_sigmask 填好，此处恢复原屏蔽字。
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
    outcome
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
