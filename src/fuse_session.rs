//! FUSE 会话状态与挂载子进程 IPC 的共享定义。
//!
//! daemon 侧（`daemon_mount`）与 companion 侧（`lifecycle::companion_mount`）共用：
//! 两条挂载路径写同一份会话语义、走同一个 socket 协议收发挂载结果。历史上两处
//! 各写一份已发生过行为分叉，因此这里收敛为单一实现，两侧只保留各自的请求建模
//! 与宿主会话发现方式（daemon 进程内持有、companion 经快照发现）。

use crate::fuse_redirect::MountRequestFields;
use crate::platform::errno::{last as last_errno, text as errno_text};
use libc::{SO_RCVTIMEO, SOL_SOCKET, c_int, c_void, recv, send, setsockopt};

/// 一次 scoped FUSE 挂载的会话记录。
#[derive(Clone)]
pub(crate) struct FuseMountState {
    pub(crate) target: String,
    /// 服务该挂载的 scoped FUSE 子进程；接入共享宿主会话时为 0（见 `host_session`）。
    pub(crate) child: i32,
    pub(crate) child_start_time_ticks: u64,
    /// 该挂载由共享宿主会话承载时记录 `(pid, start_time_ticks)`。
    ///
    /// 宿主会话是**跨应用共享**的：终止它会打掉所有接入该会话的应用挂载，因此
    /// 状态文件、回滚与清理都必须按这个字段把宿主挂载与 scoped 会话分流。
    pub(crate) host_session: Option<(i32, u64)>,
}

/// 回滚已建立的 scoped FUSE 会话：先卸载挂载点，再按台账字段分流终止子进程。
///
/// 顺序契约：卸载在前、终止在后——先终止子进程会让挂载点进入死挂载状态，卸载
/// 仍可进行但取证信息变少；宿主会话只卸载不终止（见 `host_session` 文档）。
pub(crate) fn rollback_scoped_fuse_services(states: &[FuseMountState]) {
    for state in states.iter().rev() {
        if let Ok(c_target) = std::ffi::CString::new(state.target.as_str()) {
            // SAFETY: c_target 是以 NUL 结尾的合法路径，且在本次调用期间保持存活。
            if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } != 0 {
                let errno = last_errno();
                if errno != libc::EINVAL && errno != libc::ENOENT {
                    log::warn!(
                        "fuse rollback umount failed target={} errno={} {}",
                        state.target,
                        errno,
                        errno_text(errno)
                    );
                }
            }
        }
        if let Some((host_pid, _)) = state.host_session {
            log::info!(
                "fuse rollback keeps shared host session target={} host={}",
                state.target,
                host_pid
            );
            continue;
        }
        crate::fuse_terminate::terminate_fuse_process(
            state.child,
            (state.child_start_time_ticks != 0).then_some(state.child_start_time_ticks),
        );
    }
}

/// 批量启动 scoped FUSE 会话；单根失败只丢弃该根，全部根失败前先回滚再收敛成存储根。
///
/// 部分失败的恢复契约：先收回已启动的会话，再以单个存储根会话保留原始规则的动态
/// 匹配，避免失败根变成无规则覆盖；存储根重试同样失败才交给调用方走 namespace 回退
/// 与能力失败记账。`start_one` 由两侧传入各自的按根启动函数（宿主会话发现方式不同）。
pub(crate) fn start_scoped_fuse_services(
    request: &(impl MountRequestFields + ?Sized),
    roots: &[String],
    real_root_override: Option<String>,
    start_one: impl Fn(&str, Option<String>) -> Option<FuseMountState>,
) -> Option<Vec<FuseMountState>> {
    if roots.is_empty() {
        return Some(Vec::new());
    }

    let mut states = Vec::with_capacity(roots.len());
    let mut failed_roots: Vec<&str> = Vec::new();
    for root in roots {
        match start_one(root, real_root_override.clone()) {
            Some(state) => states.push(state),
            None => failed_roots.push(root.as_str()),
        }
    }

    if !failed_roots.is_empty() {
        log::warn!(
            "fuse partial scoped mount pkg={} pid={} mounted={} failed={} failed_roots={}",
            request.package_name(),
            request.pid(),
            states.len(),
            failed_roots.len(),
            failed_roots.join(",")
        );
        // 规划阶段已跳过这些预期 FUSE 根对应的 bind；部分失败时先收回已启动会话，
        // 再以单个存储根会话保留原始规则的动态匹配，避免失败根变成无规则覆盖。
        let user_id = crate::platform::user_id_from_uid(request.uid());
        let storage_root = crate::platform::paths::storage_user_root_for_user(user_id);
        rollback_scoped_fuse_services(&states);
        if let Some(state) = start_one(&storage_root, real_root_override) {
            log::warn!(
                "fuse partial roots collapsed to storage root pkg={} pid={} failed={}",
                request.package_name(),
                request.pid(),
                failed_roots.len()
            );
            return Some(vec![state]);
        }
        log::warn!(
            "fuse partial roots and storage-root retry failed pkg={} pid={}",
            request.package_name(),
            request.pid()
        );
        return None;
    }

    if states.is_empty() {
        return None;
    }
    Some(states)
}

/// 把 waitpid 的状态位解码为可读文本（exit=/stop sig=/sig= core=）。
pub(crate) fn decode_wait_status(status: c_int) -> String {
    let signal = status & 0x7f;
    if signal == 0 {
        let exit_code = (status >> 8) & 0xff;
        return format!("exit={}", exit_code);
    }
    if signal == 0x7f {
        let stop_signal = (status >> 8) & 0xff;
        return format!("stop sig={}", stop_signal);
    }
    let is_core_dump = (status & 0x80) != 0;
    format!("sig={} core={}", signal, is_core_dump)
}

/// 给挂载子进程的结果 socket 设置接收超时；失败记告警（超时不生效会让父进程永久阻塞）。
pub(crate) fn set_recv_timeout(sock: c_int, child: i32, seconds: i64) {
    let tv = libc::timeval {
        tv_sec: seconds,
        tv_usec: 0,
    };
    // SAFETY: tv 是栈上有效的 timeval，选项长度与其类型完全一致。
    let opt_ret = unsafe {
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &tv as *const _ as *const c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        )
    };
    if opt_ret != 0 {
        let errno = last_errno();
        log::warn!(
            "setsockopt failed child={} sec={} errno={} {}",
            child,
            seconds,
            errno,
            errno_text(errno)
        );
    }
}

/// 从结果 socket 读取一个 i32 结果。
pub(crate) fn recv_result(sock: c_int, result: &mut i32) -> isize {
    // SAFETY: result 指向有效的 i32 存储，recv 只写入不借用。
    unsafe {
        recv(
            sock,
            result as *mut _ as *mut c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    }
}

/// 把挂载结果写回父进程；失败（errno 或短写）记告警并返回 false，成功记 debug。
pub(crate) fn send_mount_result(sock: c_int, result: i32) -> bool {
    let expected_size = std::mem::size_of::<i32>() as isize;
    // SAFETY: send 只接收整型参数与栈上缓冲指针，不涉及借用指针。
    let sent = unsafe {
        send(
            sock,
            &result as *const _ as *const c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    if sent != expected_size {
        if sent < 0 {
            let errno = last_errno();
            log::warn!(
                "send result failed sock={} errno={} {}",
                sock,
                errno,
                errno_text(errno)
            );
        } else {
            log::warn!(
                "send result short sock={} sent={} want={}",
                sock,
                sent,
                expected_size
            );
        }
        return false;
    }
    log::debug!("send result sock={} ret={}", sock, result);
    true
}
