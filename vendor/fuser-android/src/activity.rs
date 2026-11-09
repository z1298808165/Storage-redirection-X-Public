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
/// 宿主服务循环使用的「活动边沿」eventfd；-1 表示未注入。
///
/// 与完成事件分开是因为两者用途不同：完成事件供停摆看门狗判断「有没有进展」，
/// 活动边沿供服务循环把周期性任务（锚点校验、空闲页归还）**挂到真实活动上**，
/// 从而在没有请求时不产生任何定时唤醒。
static WAKE_EVENTFD: AtomicI32 = AtomicI32::new(-1);
/// 心跳写线程的活动通知：只在在途请求的 0↔1 边沿写入。
static HEARTBEAT_EVENTFD: AtomicI32 = AtomicI32::new(-1);

/// 最近一次请求分发完成的时刻（进程内单调毫秒）。
static LAST_COMPLETION_MS: AtomicI64 = AtomicI64::new(0);

/// 当前在途（已取出、尚未完成）的请求数。
pub static IN_FLIGHT: AtomicI64 = AtomicI64::new(0);

/// 设置完成事件 eventfd（宿主进程在挂载成功后注入；传 -1 表示取消注入）。
pub fn set_completion_eventfd(fd: i32) {
    COMPLETION_EVENTFD.store(fd, Ordering::Release);
}

/// 设置活动边沿 eventfd（宿主服务循环注入；传 -1 表示取消注入）。
///
/// 只在「在途 0→1」与「在途清零」两条边沿上写，与完成事件共享同一条边沿语义。
pub fn set_wake_eventfd(fd: i32) {
    WAKE_EVENTFD.store(fd, Ordering::Release);
}

/// 设置心跳写线程的活动通知 eventfd；fd 为 -1 时关闭通知。
pub fn set_heartbeat_eventfd(fd: i32) {
    HEARTBEAT_EVENTFD.store(fd, Ordering::Release);
}

/// 最近一次请求分发完成的时刻（`now_ms` 口径）；从未完成过时为 0。
pub fn last_completion_ms() -> i64 {
    LAST_COMPLETION_MS.load(Ordering::Acquire)
}

/// 当前在途（已取出、尚未完成）的请求数快照。
pub fn in_flight() -> i64 {
    IN_FLIGHT.load(Ordering::Acquire)
}

/// 当前墙钟毫秒（完成时刻的统一口径）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 请求分发开始：登记在途计数。
///
/// 只在「从空闲进入有在途」这一条边沿上通知看门狗。看门狗需要的是「这轮活动开始
/// 了」这个事实，而不是每个请求的到达：每请求写一次 eventfd 会让每个 FUSE 请求
/// 多一次系统调用并唤醒一次看门狗线程（媒体扫描这类上万请求/秒的场景下可占单核
/// 数个百分点），而它自己已经被 1 秒节拍的兜底检查覆盖。
pub(crate) fn begin_request() {
    if IN_FLIGHT.fetch_add(1, Ordering::AcqRel) == 0 {
        notify_watchdog();
    }
}

/// 请求分发结束：更新完成时刻，并在「在途清零」的边沿上唤醒看门狗。
///
/// 完成时刻是普通原子量，看门狗按自己的节拍读取即可；只有「排空」这一个边沿值得
/// 一次唤醒（它意味着这轮活动结束，看门狗可以立刻回到空闲判断）。
pub(crate) fn end_request() {
    LAST_COMPLETION_MS.store(now_ms(), Ordering::Release);
    if IN_FLIGHT.fetch_sub(1, Ordering::AcqRel) == 1 {
        notify_watchdog();
    }
}

/// 边沿唤醒：写一次 eventfd 计数（非阻塞语义，失败仅丢弃）。
///
/// 信号丢失只影响看门狗的唤醒时机，它的 1 秒兜底检查仍会读到同样的在途计数与
/// 完成时刻，判定结论不变。
fn notify_watchdog() {
    notify_fd(COMPLETION_EVENTFD.load(Ordering::Acquire));
    notify_fd(WAKE_EVENTFD.load(Ordering::Acquire));
    notify_fd(HEARTBEAT_EVENTFD.load(Ordering::Acquire));
}

/// 向一个 eventfd 写一次计数；fd 为负时什么都不做。
fn notify_fd(fd: i32) {
    if fd < 0 {
        return;
    }
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
