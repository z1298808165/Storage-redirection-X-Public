//! 宿主心跳共享页：daemon 与 `srx_fuse_host` 之间的一页 `MAP_SHARED` 匿名内存。
//!
//! 背景：宿主心跳原本是「写临时文件 + rename」的周期落盘，它是模块**唯一一处空闲期
//! 磁盘写入**——每次心跳都会产生目录元数据变更与 f2fs 日志提交，而它承载的只是几个
//! 数字。宿主进程由 daemon `fork()` 得到（不 exec），fork 前建立的 `MAP_SHARED` 匿名
//! 映射在父子进程间天然共享同一物理页，因此心跳可以退化为**纯内存操作**：写方零系统
//! 调用、零磁盘 I/O，读方也只是一次内存读。
//!
//! 一致性用 seqlock 保证（写方串行、读方重试）。读方在重试失败时回退到上一次成功读
//! 到的快照，这样即使写方恰好在写中途冻结，判定逻辑拿到的仍是「一个会自然变旧的
//! 心跳」，而不是「读不到心跳」——后者会让停摆判定漏判，前者会正常触发嫌疑。
//!
//! 未安装共享页（`install()` 失败）时全部接口退化为 `None`，调用方回落到文件心跳，
//! 功能不降级。

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// 页内魔数："SRXB"。
const BEAT_MAGIC: u32 = 0x5352_5842;
/// 共享页一页足够（结构体只有几十字节）；按页对齐可避免与邻居共享 cache line。
const BEAT_PAGE_LEN: usize = 4096;
/// 读方重试上限：写方写一轮是纳秒级，重试超过这个次数只能说明写方卡在临界区里。
const READ_RETRY_LIMIT: usize = 4;

/// 共享页布局。字段全部用原子类型，保证跨进程访问不会引入未定义行为。
#[repr(C)]
struct BeatPage {
    /// seqlock：奇数表示写方正在写。
    seq: AtomicU64,
    magic: AtomicU32,
    host_pid: AtomicI32,
    conn: AtomicI32,
    waiting: AtomicI64,
    last_completion_ms: AtomicI64,
    wall_ms: AtomicI64,
    /// 写方本次的心跳间隔（毫秒）。读方据此推算陈旧阈值——写方在空闲期会把间隔拉长，
    /// 阈值必须跟着走，否则会把「只是心跳变慢」误判成冻结。
    interval_ms: AtomicI64,
}

/// 一次心跳快照（与文件格式语义一致）。
#[derive(Clone, Copy, Debug)]
pub struct Beat {
    pub host_pid: i32,
    pub conn: i32,
    pub waiting: i64,
    pub last_completion_ms: i64,
    pub wall_ms: i64,
    /// 写方下一次心跳的间隔；0 表示旧版本写方（读方按默认值处理）。
    pub interval_ms: i64,
}

static PAGE: AtomicUsize = AtomicUsize::new(0);
/// 心跳唤醒 eventfd：daemon 阻塞等它，宿主在两条边沿上各写一次。
///
/// 两条边沿：
/// 1. **共享页建立**（daemon 从「没有会话」进入「有会话」）；
/// 2. **空闲→活跃**（宿主开始服务请求）。
///
/// 第二条是关键：空闲期心跳间隔被拉长到 60 秒，若只靠时间，daemon 要等「这条空闲心跳
/// 变陈旧」（2 倍间隔 = 120 秒）才会再看一眼；而一旦有请求进来，需要立刻切回 20 秒级
/// 的判据。把「开始有活」做成一个事件，这个窗口就不存在了。
///
/// 必须由 **daemon 在 fork 宿主之前** 建立：fork 之后两边才持有同一个 eventfd。
static WAKE_EVENTFD: AtomicI32 = AtomicI32::new(-1);
/// 读方缓存：seqlock 重试失败时回退到最近一次成功读到的快照。
static LAST_GOOD: Mutex<Option<Beat>> = Mutex::new(None);
static PAGE_INSTALL_LOCK: Mutex<()> = Mutex::new(());

fn page() -> Option<&'static BeatPage> {
    let raw = PAGE.load(Ordering::Acquire);
    if raw == 0 {
        return None;
    }
    // SAFETY: 该地址只在 install() 中由 mmap 建立，指向一块至少 BEAT_PAGE_LEN 字节、
    // 生命周期与进程相同的共享映射；BeatPage 是 repr(C) 且只含原子类型。
    Some(unsafe { &*(raw as *const BeatPage) })
}

/// 共享页是否已建立。
///
/// daemon 侧用它决定「心跳是否应该来自共享页」：已建立时不再回退文件心跳——文件通道
/// 只是共享页不可用时的退路，残留的旧文件会被误当成当前心跳。
pub fn installed() -> bool {
    PAGE.load(Ordering::Acquire) != 0
}

/// 建立（或取回）心跳唤醒 eventfd。
///
/// 幂等；**必须在 fork 宿主子进程之前调用**，否则父子两边会持有不同的 eventfd。
/// 共享页若已建立则立刻写一次，避免等待方错过「已经发生」这件事。
pub fn ensure_wake_fd() -> i32 {
    let existing = WAKE_EVENTFD.load(Ordering::Acquire);
    if existing >= 0 {
        return existing;
    }
    // SAFETY: eventfd 只创建内核对象并返回 fd；标志均为合法常量。
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return -1;
    }
    match WAKE_EVENTFD.compare_exchange(-1, fd, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            if installed() {
                signal_fd(fd);
            }
            fd
        }
        Err(winner) => {
            // 竞争失败：关闭自己这份，用赢家那份。两边都有各自的对象，但只有一份会被
            // 父子进程同时持有，因此必须收敛到同一个 fd 号。
            // SAFETY: fd 由本次调用创建，尚未交给任何人。
            unsafe { libc::close(fd) };
            winner
        }
    }
}

/// 宿主侧：报告「从空闲进入活跃」，让 daemon 立刻改按活跃阈值重新评估。
///
/// 只在边沿上调用（不是每次心跳），代价是每次「开始有活」一次 eventfd 写。
pub fn signal_activity() {
    signal_fd(WAKE_EVENTFD.load(Ordering::Acquire));
}

fn signal_fd(fd: i32) {
    if fd < 0 {
        return;
    }
    // SAFETY: fd 是等待方创建的合法 eventfd；写 8 字节计数是其契约，失败只是丢一次唤醒。
    unsafe {
        let value: u64 = 1;
        libc::write(fd, &value as *const u64 as *const libc::c_void, 8);
    }
}

/// 建立共享页（幂等）。必须在 `fork()` 宿主子进程**之前**调用，子进程才能继承。
///
/// 返回是否可用。失败只意味着心跳继续走文件通道。
pub fn install() -> bool {
    if PAGE.load(Ordering::Acquire) != 0 {
        return true;
    }
    let _guard = PAGE_INSTALL_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if PAGE.load(Ordering::Acquire) != 0 {
        return true;
    }
    // SAFETY: 匿名共享映射，不涉及文件；参数均为合法常量组合。
    let raw = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            BEAT_PAGE_LEN,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if raw == libc::MAP_FAILED {
        return false;
    }
    // 清零后写入魔数与偶数序号，表示「尚无心跳」。
    // SAFETY: raw 是一整页可写映射，长度 BEAT_PAGE_LEN 未越界。
    unsafe { std::ptr::write_bytes(raw as *mut u8, 0, BEAT_PAGE_LEN) };
    let page = raw as *mut BeatPage;
    // SAFETY: 映射已建立且对齐满足 BeatPage 要求（页对齐）。
    unsafe {
        (*page).seq.store(0, Ordering::Release);
        (*page).magic.store(BEAT_MAGIC, Ordering::Release);
    }
    PAGE.store(raw as usize, Ordering::Release);
    // 唤醒 fd 必须在 fork 之前就位：装页发生在 fork 之前，这里一并建立。
    let wake_fd = ensure_wake_fd();
    // 通知等待方「从此心跳只会来自共享页」。
    signal_fd(wake_fd);
    true
}

/// 写一次心跳（宿主侧）。共享页不存在时返回 false，调用方走文件通道。
pub fn write_beat(beat: Beat) -> bool {
    let Some(page) = page() else {
        return false;
    };
    // 新宿主由 daemon 在确认旧宿主退出后 fork。唯一写者可先把上一代留下的奇数序号
    // 归一为偶数，再进入本轮写入，避免读方永久拒绝快照。
    let mut seq = page.seq.load(Ordering::Acquire);
    if seq & 1 == 1 {
        seq = seq.wrapping_add(1);
        page.seq.store(seq, Ordering::Release);
    }
    page.seq.store(seq.wrapping_add(1), Ordering::SeqCst);
    page.host_pid.store(beat.host_pid, Ordering::Relaxed);
    page.conn.store(beat.conn, Ordering::Relaxed);
    page.waiting.store(beat.waiting, Ordering::Relaxed);
    page.last_completion_ms
        .store(beat.last_completion_ms, Ordering::Relaxed);
    page.wall_ms.store(beat.wall_ms, Ordering::Relaxed);
    page.interval_ms.store(beat.interval_ms, Ordering::Relaxed);
    page.magic.store(BEAT_MAGIC, Ordering::Relaxed);
    page.seq.store(seq.wrapping_add(2), Ordering::SeqCst);
    true
}

/// 读一次心跳（daemon 侧）。
///
/// 从来没有心跳（序号为 0）时返回 `None`；写入过程中读到撕裂值时回退到上一次成功
/// 读到的快照（见模块注释里的理由）。
pub fn read_beat() -> Option<Beat> {
    let page = page()?;
    for _ in 0..READ_RETRY_LIMIT {
        let before = page.seq.load(Ordering::SeqCst);
        if before == 0 {
            return None;
        }
        if before & 1 == 1 {
            continue;
        }
        if page.magic.load(Ordering::Relaxed) != BEAT_MAGIC {
            return None;
        }
        let beat = Beat {
            host_pid: page.host_pid.load(Ordering::Relaxed),
            conn: page.conn.load(Ordering::Relaxed),
            waiting: page.waiting.load(Ordering::Relaxed),
            last_completion_ms: page.last_completion_ms.load(Ordering::Relaxed),
            wall_ms: page.wall_ms.load(Ordering::Relaxed),
            interval_ms: page.interval_ms.load(Ordering::Relaxed),
        };
        if page.seq.load(Ordering::SeqCst) == before {
            if let Ok(mut cached) = LAST_GOOD.lock() {
                *cached = Some(beat);
            }
            return Some(beat);
        }
    }
    LAST_GOOD.lock().ok().and_then(|cached| *cached)
}
