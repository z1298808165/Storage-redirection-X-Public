//! 有界等待自有子进程退出并回收。
//!
//! daemon、宿主与 companion 里有十余处"fork 子进程 → 有界等待退出 → 超时 SIGKILL"的
//! 流程，此前各自用 `waitpid(WNOHANG)` + 10~50ms sleep 轮询。这里统一为事件驱动：
//! `pidfd_open` + `poll(剩余预算)`，子进程退出由内核立即唤醒，等待期间零唤醒。
//!
//! 只有 pidfd 不可用的内核（Linux < 5.3）才回退到原来的短间隔复查——该内核上没有
//! 其它按 pid 等待且不改线程信号屏蔽的机制（`SIGCHLD` + `sigtimedwait` 需要在多线程
//! daemon 里抢占进程级信号，会和其它等待方互相吞信号），回退间隔与改造前一致。
//!
//! 不阻塞：子进程可能卡在不可中断的 D 态（FUSE 挂死时探针里的 stat 就会这样），
//! 调用方必须给出超时，本函数到点返回 [`ChildExit::TimedOut`]，不代为 SIGKILL。

/// 无 pidfd 时的复查间隔（与改造前各调用点的最小间隔一致）。
const FALLBACK_POLL_MS: i64 = 10;

/// 子进程等待结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildExit {
    /// 正常退出并已回收，携带退出码。
    Exited(i32),
    /// 被信号终止并已回收，携带信号值。
    Signaled(i32),
    /// 预算内未退出，子进程仍在运行（未回收）。
    TimedOut,
    /// `waitpid` 失败（如 `ECHILD`：已被别处回收或 `SIGCHLD` 被忽略），携带 errno。
    Failed(i32),
}

impl ChildExit {
    /// 是否已经不需要再等待（已回收或无从回收）。
    pub fn is_settled(self) -> bool {
        !matches!(self, ChildExit::TimedOut)
    }

    /// 正常退出且退出码为 `code`。
    pub fn exited_with(self, code: i32) -> bool {
        self == ChildExit::Exited(code)
    }
}

/// 最多等待 `timeout_ms` 毫秒，期间子进程退出即回收并返回。
///
/// `timeout_ms <= 0` 时只做一次非阻塞复查。
pub fn wait_child_exit(pid: libc::pid_t, timeout_ms: i64) -> ChildExit {
    if let Some(outcome) = try_reap(pid) {
        return outcome;
    }
    if timeout_ms <= 0 {
        return ChildExit::TimedOut;
    }
    let deadline = super::paths::monotonic_ms() + timeout_ms;
    // SAFETY: SYS_pidfd_open 只按 pid 创建内核 fd，第三个实参 0u32 是无标志常量。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) as libc::c_int };
    if pidfd < 0 {
        return wait_by_recheck(pid, deadline);
    }
    let fd = super::unique_fd::UniqueFd::new(pidfd);
    loop {
        let remaining = deadline - super::paths::monotonic_ms();
        if remaining <= 0 {
            return try_reap(pid).unwrap_or(ChildExit::TimedOut);
        }
        let mut pfd = libc::pollfd {
            fd: fd.get(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd 是本地初始化的单个 pollfd，poll 只写入其 revents；剩余预算为正且
        // 不超过调用方给出的 i64 超时，截断到 c_int 上限是合法的毫秒超时。
        let ready = unsafe {
            libc::poll(
                &mut pfd,
                1,
                remaining.min(i64::from(libc::c_int::MAX)) as libc::c_int,
            )
        };
        if ready > 0 {
            // pidfd 可读即线程组组长已退出，通常已是僵尸可立即回收；其余线程尚未全部
            // 结束时 waitpid 仍返回 0，此时转短间隔复查直到预算耗尽。
            if let Some(outcome) = try_reap(pid) {
                return outcome;
            }
            return wait_by_recheck(pid, deadline);
        }
        if ready < 0 && super::errno::last() != libc::EINTR {
            return wait_by_recheck(pid, deadline);
        }
    }
}

/// 非阻塞回收一次；子进程仍在运行时返回 `None`。
fn try_reap(pid: libc::pid_t) -> Option<ChildExit> {
    let mut status: libc::c_int = 0;
    // SAFETY: status 是栈上有效整型；WNOHANG 不阻塞，pid 由调用方保证是自有子进程。
    let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if waited == pid {
        return Some(if libc::WIFSIGNALED(status) {
            ChildExit::Signaled(libc::WTERMSIG(status))
        } else {
            ChildExit::Exited(libc::WEXITSTATUS(status))
        });
    }
    if waited < 0 {
        let errno = super::errno::last();
        if errno == libc::EINTR {
            return None;
        }
        return Some(ChildExit::Failed(errno));
    }
    None
}

/// pidfd 不可用时的回退：按改造前的短间隔复查到截止时刻。
fn wait_by_recheck(pid: libc::pid_t, deadline: i64) -> ChildExit {
    loop {
        if let Some(outcome) = try_reap(pid) {
            return outcome;
        }
        let remaining = deadline - super::paths::monotonic_ms();
        if remaining <= 0 {
            return ChildExit::TimedOut;
        }
        std::thread::sleep(std::time::Duration::from_millis(
            remaining.min(FALLBACK_POLL_MS) as u64,
        ));
    }
}

/// 等待**任意**进程（不要求是自己的子进程）退出，最多 `timeout_ms` 毫秒；返回是否已退出。
///
/// 用于已被 init 收养、`waitpid` 只会得到 `ECHILD` 的目标：pidfd 对非子进程同样在
/// 退出时可读。`still_alive` 由调用方给出存活判据（通常带启动时刻校验，防 pid 复用），
/// 进入等待前与 pidfd 打开后各复查一次，覆盖「打开 pidfd 前已退出、pid 被复用」的窗口。
/// 本函数不回收僵尸；目标若恰好是自己的子进程，调用方仍需自行 `waitpid`。
pub fn wait_process_gone(
    pid: libc::pid_t,
    timeout_ms: i64,
    still_alive: impl Fn() -> bool,
) -> bool {
    if !still_alive() {
        return true;
    }
    let deadline = super::paths::monotonic_ms() + timeout_ms.max(0);
    // SAFETY: SYS_pidfd_open 只按 pid 创建内核 fd，第三个实参 0u32 是无标志常量。
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) as libc::c_int };
    if pidfd >= 0 {
        let fd = super::unique_fd::UniqueFd::new(pidfd);
        if !still_alive() {
            return true;
        }
        loop {
            let remaining = deadline - super::paths::monotonic_ms();
            if remaining <= 0 {
                return !still_alive();
            }
            let mut pfd = libc::pollfd {
                fd: fd.get(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pfd 是本地初始化的单个 pollfd；剩余预算为正并截断到 c_int 上限。
            let ready = unsafe {
                libc::poll(
                    &mut pfd,
                    1,
                    remaining.min(i64::from(libc::c_int::MAX)) as libc::c_int,
                )
            };
            if ready > 0 {
                return true;
            }
            if ready < 0 && super::errno::last() != libc::EINTR {
                break;
            }
        }
    }
    // pidfd 不可用（或 poll 异常）：回退为短间隔存活复查。
    loop {
        if !still_alive() {
            return true;
        }
        let remaining = deadline - super::paths::monotonic_ms();
        if remaining <= 0 {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(
            remaining.min(FALLBACK_POLL_MS) as u64,
        ));
    }
}
