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

/// SIGTERM 宽限与 SIGKILL 后等待的探测轮数；每轮 10ms，共约 300ms。
const GRACE_POLLS: usize = 30;
/// 每轮探测之间的休眠时长（微秒）。
const POLL_SLEEP_USEC: u32 = 10 * 1000;

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

/// 在宽限窗口内轮询等待目标退出：`waitpid` 回收成功或 `/proc` 探测确认消失即返回。
fn wait_for_exit(pid: i32, start_time_ticks: Option<u64>) {
    for _ in 0..GRACE_POLLS {
        let mut status: c_int = 0;
        // SAFETY: status 是栈上有效的 c_int，指针在调用期间保持有效。
        let wait_ret = unsafe { waitpid(pid, &mut status as *mut _, WNOHANG) };
        if wait_ret == pid {
            return;
        }
        // SIGTERM/SIGKILL 之后同样不能依赖 `waitpid` 判断目标是否消失，否则会对
        // 已经退出但无法回收的目标误报残留。
        if !process_identity_alive(pid, start_time_ticks) {
            return;
        }
        // SAFETY: usleep 只接收整型参数，不涉及借用指针。
        unsafe { libc::usleep(POLL_SLEEP_USEC) };
    }
}
