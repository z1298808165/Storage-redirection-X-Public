//! FUSE 服务子进程的统一终止流程，daemon 侧（`daemon_mount`）与 companion 侧
//! （`lifecycle::companion_mount`）共用同一份实现。
//!
//! 语义以 companion 原实现为准：
//! - SIGTERM 失败（通常说明子进程已退出成僵尸或权限受限）不直接返回，仍继续回收；
//!   直接返回会把僵尸进程留在父进程下，长期运行会耗尽进程表。
//! - SIGTERM 与 SIGKILL 之后都以 `/proc` 存活探测判定目标是否退出，并循环等待完整
//!   宽限窗口，SIGKILL 之后仍存活才告警。
//!
//! 回收判定不能依赖 `waitpid`：FUSE 服务子进程由挂载 worker fork，worker 退出后
//! 会被 init 收养，此后 `waitpid` 固定返回负值（ECHILD）。把负返回值当作「已回收」
//! 会在第一次循环就直接返回，永远走不到 SIGKILL 升级，留下长期存活并空转的残留
//! 服务进程。

use crate::platform::errno::{last as last_errno, text as errno_text};
use libc::{SIGKILL, SIGTERM, WNOHANG, c_int, kill, waitpid};

/// SIGTERM 宽限与 SIGKILL 后各自的最长等待（毫秒），与改造前 30×10ms 同预算。
const GRACE_WINDOW_MS: i64 = 300;

/// 判定目标进程实例是否仍然存活。
fn process_identity_alive(pid: i32, start_time_ticks: Option<u64>) -> bool {
    match start_time_ticks {
        Some(start) => crate::platform::is_process_instance_alive(pid, start),
        None => crate::platform::process_exists(pid),
    }
}

/// 终止 FUSE 服务子进程并等待其退出：先 SIGTERM 等一个宽限窗口，仍存活再 SIGKILL。
pub(crate) fn terminate_fuse_process(pid: i32, start_time_ticks: Option<u64>) {
    if !process_identity_alive(pid, start_time_ticks) {
        return;
    }
    // SAFETY: kill 只接收整型参数，不涉及借用指针。
    if unsafe { kill(pid, SIGTERM) } != 0 {
        let errno = last_errno();
        log::debug!(
            "fuse child SIGTERM failed pid={} errno={} {}",
            pid,
            errno,
            errno_text(errno)
        );
    }
    wait_for_exit(pid, start_time_ticks);
    if !process_identity_alive(pid, start_time_ticks) {
        return;
    }
    // SAFETY: kill 只接收整型参数，不涉及借用指针。
    let _ = unsafe { kill(pid, SIGKILL) };
    wait_for_exit(pid, start_time_ticks);
    if process_identity_alive(pid, start_time_ticks) {
        log::warn!("fuse child still alive after SIGKILL pid={}", pid);
    }
}

/// 在宽限窗口内等待目标退出：由 pidfd 退出事件唤醒，退出后顺带尝试回收僵尸。
///
/// 存活判据只看 `/proc`（带启动时刻），不看 `waitpid`：目标可能已被 init 收养。
fn wait_for_exit(pid: i32, start_time_ticks: Option<u64>) {
    crate::platform::child_wait::wait_process_gone(pid, GRACE_WINDOW_MS, || {
        // 目标若仍是本进程子进程，退出后是僵尸，`/proc` 仍可见；先回收再判活。
        let mut status: c_int = 0;
        // SAFETY: status 是栈上有效的 c_int；WNOHANG 不阻塞，非子进程时返回 ECHILD。
        let _ = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
        process_identity_alive(pid, start_time_ticks)
    });
}
