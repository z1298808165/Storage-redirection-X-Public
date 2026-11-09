//! 挂载请求的发送，以及"应用侧为什么不再等回执"。
//!
//! 应用侧曾经保留这条连接的读端，用来接收挂载执行方（伴生进程）写回的 4 字节结论。
//! 这个设计**在本项目里不可能成立**，原因不是实现细节，而是 SELinux 的域迁移：
//!
//! - 请求在 `pre_app_specialize` 发出，那时进程还在 `u:r:zygote:s0` 域，socket 由该域
//!   创建、SID 也随之固定为 zygote；
//! - 应用 specialize 时整个进程切到 `u:r:untrusted_app:s0`，此后对这条继承来的 socket
//!   做 `read` 会被 SELinux 直接拒绝。
//!
//! 真机实证（2026-11-09，2311DRK48C / A16）dmesg 审计原文，逐行摘录：
//!
//! - 请求行：`avc: denied { read } for comm="one:gpu_process" path="socket:[207528]" dev="sockfs"`
//! - 上下文：`scontext=u:r:untrusted_app:s0:c18,c257,c512,c768 tcontext=u:r:zygote:s0`
//! - 对象类：`tclass=unix_stream_socket permissive=0`
//!
//! 应用侧看到的是 `read` 返回 `-EACCES`（`mount ack eof without status fd=6 filled=0
//! revents=0x1 errno=13`），而执行方在同一毫秒里 `companion ack sent fd=4 status=1`
//! ——数据确实写出来了，只是读不到。留着重试、加预算、换读法都没有用：`recv`/`readv`/
//! `splice` 走的是同一个 `file_permission` 钩子。
//!
//! 两个后果决定了取舍：
//!
//! 1. 这条通道**永远返回"没有结论"**，只能退回观测兜底，等于白等 50~60 毫秒；
//! 2. 更糟的是，每次应用启动都会在 dmesg 里留下一条 `avc: denied { read } ...
//!    unix_stream_socket` 审计行——那是模块**可被观测的痕迹**，与"不留痕"的目标直接
//!    冲突。而它换来的判据（`is_mount_applied`）在功能上没有任何消费者，只进
//!    `perf post` 与结论日志。
//!
//! 因此现在的做法是：**发完即关**。连接不在应用进程里多留一个 fd，不再把 zygote 域的
//! socket 带过域边界，也不再把"读不到"包装成"没有结论"。应用侧的就位判定回到一次性
//! 观测（见 `specialize_post::conclude_mount_readiness` 的判据说明），执行方的结论由
//! 执行方自己记（`perf companion mount ... ok=` / `companion fuse view unavailable
//! reason=`），两边各自诚实。

use crate::platform::fs;
use crate::zygisk::abi;

/// 发送挂载请求（协议 v2：首字节 `'M'` + 4 字节长度 + JSON 负载），发送后立即关闭连接。
///
/// 返回是否**完整写出**。调用方只用它区分"请求发出去了"与"根本没发"（perf 里的
/// `mount_sent`），不要把它当成挂载结论。
pub(crate) fn send_mount_request(api: Option<&abi::Api>, payload: &str) -> bool {
    let Some(api) = api else {
        return false;
    };
    if payload.is_empty() {
        return false;
    }

    let fd = api.connect_companion();
    if fd < 0 {
        log::warn!("companion connect failed");
        return false;
    }

    let payload_len = payload.len() as u32;
    let sent = fs::write_all(fd, b"M")
        && fs::write_all(fd, &payload_len.to_ne_bytes())
        && fs::write_all(fd, payload.as_bytes());
    // 立刻关闭：AF_UNIX 流式 socket 上，已写出的字节留在对端接收队列里，关闭本端不影响
    // 对端读取（对端读完才会看到 EOF）。保留读端毫无意义且有害，理由见模块头注释。
    // SAFETY: fd 是本次 connect_companion 得到的合法描述符，只在此处关闭一次。
    unsafe { libc::close(fd) };

    if !sent {
        log::warn!("companion send failed");
        return false;
    }
    log::info!(
        "companion req sent payload={} (fire-and-forget)",
        payload.len()
    );
    true
}
