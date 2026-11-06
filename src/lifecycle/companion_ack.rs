//! 挂载请求的回执通道：应用侧等待"挂载执行方"的结论。
//!
//! 为什么需要它：应用侧必须知道自己被重定向到哪个视图，否则会在挂载尚未落定时访问被
//! 重定向的路径。过去的做法是应用自己读 `/proc/self/mountinfo`，再按**本模块特有的标识**
//! （挂载源前缀、模块私有锚点路径）认领自己的挂载。这条判据有两个致命问题：
//!
//! 1. 它要求挂载表里留着模块标识，而"不留痕迹"正是本项目的目标——两个方向直接冲突；
//! 2. 挂载源一旦原生化（`/dev/fuse`），判据会**静默**失效，应用只能空等满预算，最后被
//!    AMS 判启动超时。
//!
//! 回执是同一件事的另一种证据来源：真正执行挂载的进程（伴生进程）在挂载完成后，把结论
//! 写回发起请求时建立的那条连接。它天然与"挂载长什么样"无关，因此挂载源、锚点、命名空间
//! 结构怎么改都不影响判定。协议本身没有新增字节：`'M'` 请求的应答一直是 4 字节状态
//! （`send_mount_result`），此前只是被发起方立刻关闭连接丢掉了。
//!
//! 纪律：回执连接由发送方保留、由 specialize 收尾时**恰好消费一次**（等待或关闭）。
//! 泄漏到应用进程里会多出一条 `/proc/self/fd` 记录，那本身也是可观测痕迹。

use crate::platform::fs;
use crate::zygisk::abi;
use libc::c_int;

/// 伴生进程（挂载执行方）对一次挂载请求的结论。
///
/// 账本登记已经提前到回执之前（`daemon_mount` / `companion_mount` 的收尾顺序），因此
/// `Applied` 等价于"本次请求的挂载已经在目标命名空间落定"，不需要再读挂载表确认。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CompanionMountAck {
    /// 执行方回报成功。
    Applied,
    /// 执行方回报失败。
    Failed,
    /// 没有可用的回执连接：通道未建立或请求未发出。
    Unavailable,
    /// 预算内没有等到结论（对端过慢或提前关闭）。调用方只能退回一次性观测。
    Timeout,
}

impl CompanionMountAck {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
        }
    }
}

/// 发送挂载请求并**保留回执连接**，供 specialize 之后等待执行方的结论。
///
/// 返回的 fd 必须交给 [`await_mount_ack`] 或 [`discard_mount_reply`] 消费（两者都会关闭它）。
/// 失败时自行关闭并返回 `None`。
pub(crate) fn send_mount_request_retaining(api: Option<&abi::Api>, payload: &str) -> Option<c_int> {
    let api = api?;
    if payload.is_empty() {
        return None;
    }

    let fd = api.connect_companion();
    if fd < 0 {
        log::warn!("companion connect failed");
        return None;
    }

    // 协议 v2：首字节 'M' 标识挂载请求，companion 按类型分发。
    let payload_len = payload.len() as u32;
    let sent = fs::write_all(fd, b"M")
        && fs::write_all(fd, &payload_len.to_ne_bytes())
        && fs::write_all(fd, payload.as_bytes());
    if !sent {
        log::warn!("companion send failed");
        // SAFETY: fd 是本次 connect_companion 得到的合法描述符，仅关闭一次。
        unsafe { libc::close(fd) };
        return None;
    }

    log::info!("companion req sent (await ack)");
    Some(fd)
}

/// 等待挂载执行方回执，并负责关闭连接。
///
/// 用 `poll` + 一次读满 4 字节：这是"等一件确定会到来（或确定不会到来）的事"，不是轮询
/// ——没有周期检查，也不会在等待期间反复判断挂载表。预算由调用方给出，必须远低于 AMS 的
/// 进程启动超时（该等待跑在应用主线程上）。
///
/// **不用 `platform::fs::read_all_timeout`**：它把 `EAGAIN` 与 `EINTR` 之外的一切 `read`
/// 失败都当成"没有数据"直接返回，而且走的是被本模块 `read` hook 覆盖的 libc 路径。
/// 真机实测（duchamp / A16，2026-11-02）：这样会在对端还没写回执时提前 ~500ms 返回
/// `Timeout`，日志里表现为 `ack=timeout` 却 `observed_ready=true`——判据退化成兜底观测，
/// 等于白改。这里自己管循环：`EAGAIN` 继续等、`EOF` 才认"对端不给结论"，只有预算用尽
/// 才算 `Timeout`。
pub(crate) fn await_mount_ack(fd: c_int, budget_ms: i64) -> CompanionMountAck {
    if fd < 0 {
        return CompanionMountAck::Unavailable;
    }
    let result = read_ack_status(fd, budget_ms);
    // SAFETY: fd 由本函数负责关闭，只关闭一次。
    unsafe { libc::close(fd) };
    match result {
        Ok(status) if status != 0 => CompanionMountAck::Applied,
        Ok(_) => CompanionMountAck::Failed,
        Err(AckReadError::Closed) => CompanionMountAck::Unavailable,
        Err(AckReadError::Timeout) => CompanionMountAck::Timeout,
    }
}

enum AckReadError {
    /// 对端关闭连接却没有写回执。
    Closed,
    /// 预算内没有收到完整回执。
    Timeout,
}

/// 在预算内读满 4 字节回执。
///
/// 用裸 `SYS_read`：本模块的 PLT hook 覆盖了 `read`，走 libc 会先经过自己的路径判定，
/// 在应用进程里既多余又可能被第三方模块改写。
fn read_ack_status(fd: c_int, budget_ms: i64) -> Result<i32, AckReadError> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(budget_ms.max(0) as u64);
    let mut status = [0u8; 4];
    let mut filled = 0usize;
    while filled < status.len() {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(AckReadError::Timeout);
        }
        let remaining_ms = (deadline - now).as_millis().min(c_int::MAX as u128) as c_int;
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll_fd 是栈上有效的单元素 pollfd，数量参数与之匹配。
        let ready = unsafe { libc::poll(&mut poll_fd, 1, remaining_ms) };
        if ready < 0 {
            let errno = crate::platform::errno::last();
            if errno == libc::EINTR {
                continue;
            }
            log::debug!("mount ack poll failed errno={}", errno);
            return Err(AckReadError::Closed);
        }
        if ready == 0 {
            // 到点后由上面的 deadline 判定收口，这里继续下一轮。
            continue;
        }
        // SAFETY: 向 status 的未填充区间写入，长度不超过其剩余容量；裸系统调用不经 PLT。
        let read_n = unsafe {
            libc::syscall(
                libc::SYS_read,
                fd as libc::c_long,
                status[filled..].as_mut_ptr(),
                (status.len() - filled) as libc::c_long,
            )
        };
        if read_n < 0 {
            let errno = crate::platform::errno::last();
            if errno == libc::EINTR || errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                // 非阻塞 socket 的假唤醒：继续等，不算失败。
                continue;
            }
            log::debug!("mount ack read failed errno={} filled={}", errno, filled);
            return Err(AckReadError::Closed);
        }
        if read_n == 0 {
            // 对端在写回执前就关闭了连接。
            log::debug!("mount ack eof filled={}", filled);
            return Err(AckReadError::Closed);
        }
        filled += read_n as usize;
    }
    Ok(i32::from_ne_bytes(status))
}

/// 关闭不再需要的回执连接（不等待结论）。
pub(crate) fn discard_mount_reply(fd: c_int) {
    if fd >= 0 {
        // SAFETY: fd 由调用方转交所有权，只关闭一次。
        unsafe { libc::close(fd) };
    }
}

/// 从两个候选里选出要消费的那条回执连接，其余一律关闭。
///
/// 正常路径只有 `pre` 阶段的那一条；`post` 阶段补发"延迟负载"时会多出一条，取新的一条
/// 才能覆盖最终结果，旧连接必须关掉。
pub(crate) fn take_latest_reply(previous: c_int, latest: c_int) -> c_int {
    if latest >= 0 {
        discard_mount_reply(previous);
        latest
    } else {
        previous
    }
}
