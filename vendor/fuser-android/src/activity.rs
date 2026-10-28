// 宿主会话活动信号：供停摆看门狗区分「忙而慢」与「已死」。
//
// 背景：内核 FUSE 请求没有超时机制，worker 池任一线程永久阻塞即永久占坑，
// 积压的请求只能靠外部观测发现。本模块以两个原子量 + 一个 eventfd 提供零锁
// 的活动信号：每次请求分发完成即更新最后完成时刻并写 eventfd，看门狗线程
// 阻塞在 eventfd 上（空闲零 CPU），停止唤醒即停止服务的直接证据。
// eventfd 由宿主进程在挂载成功后注入；未注入（默认 -1）时只维护原子量，无开销。

use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

/// 宿主进程注入的完成事件 eventfd；-1 表示未注入。
static COMPLETION_EVENTFD: AtomicI32 = AtomicI32::new(-1);

/// 最近一次请求分发完成的时刻（进程内单调毫秒）。
static LAST_COMPLETION_MS: AtomicI64 = AtomicI64::new(0);

/// 当前在途（已取出、尚未完成）的请求数。
pub static IN_FLIGHT: AtomicI64 = AtomicI64::new(0);

pub fn set_completion_eventfd(fd: i32) {
    COMPLETION_EVENTFD.store(fd, Ordering::Release);
}

pub fn last_completion_ms() -> i64 {
    LAST_COMPLETION_MS.load(Ordering::Acquire)
}

pub fn in_flight() -> i64 {
    IN_FLIGHT.load(Ordering::Acquire)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 请求分发开始：登记在途计数。
pub(crate) fn begin_request() {
    IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
}

/// 请求分发结束：更新完成时刻并唤醒看门狗。
///
/// eventfd 写入用非阻塞语义（EFD_NONFLAG 下写 1 只累积计数），失败仅丢弃——
/// 信号丢失只影响看门狗的唤醒时机，下一秒的超时路径仍会兜底。
pub(crate) fn end_request() {
    LAST_COMPLETION_MS.store(now_ms(), Ordering::Release);
    IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    let fd = COMPLETION_EVENTFD.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: fd 是宿主进程创建的合法 eventfd；写入 8 字节计数是 eventfd 契约。
        unsafe {
            let value: u64 = 1;
            libc::write(
                fd,
                &value as *const u64 as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}
