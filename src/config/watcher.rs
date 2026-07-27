use crate::platform::paths;
use libc::{
    IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO, c_int, inotify_add_watch,
    inotify_event, inotify_init1,
};
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

const EVENT_MASK: u32 = IN_CREATE | IN_DELETE | IN_CLOSE_WRITE | IN_MOVED_FROM | IN_MOVED_TO;

static INOTIFY_FD: AtomicI32 = AtomicI32::new(-1);
static LAST_CHANGE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
const CHANGE_DEBOUNCE_MS: u64 = 100;
const POLL_INTERVAL_MS: u64 = 25;

// 初始化 inotify 并添加监听，返回 fd（用于 exempt）
// 必须在 pre_app_specialize 阶段调用（此时有 root 权限）
pub fn init(config_dir: &str) -> i32 {
    let fd = unsafe { inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        log::warn!("inotify init failed");
        return -1;
    }

    if !add_watch(fd, config_dir) {
        log::warn!("watch config dir failed {}", config_dir);
    }

    let apps_dir = paths::join(config_dir, "apps");
    if !add_watch(fd, &apps_dir) {
        log::debug!("apps dir missing or unwatchable");
    }

    INOTIFY_FD.store(fd, Ordering::Release);
    log::info!("config watcher ready fd={}", fd);
    fd
}

// inotify_event 需要 4 字节对齐；内核保证每个事件总长度是 sizeof(int) 的倍数，
// 因此缓冲区起始 4 字节对齐后，后续每个事件也满足对齐要求。
// 使用 4096 字节容纳多个事件，避免 1024 字节时单次 read 截断。
#[repr(align(4))]
struct InotifyBuf([u8; 4096]);

impl InotifyBuf {
    fn new() -> Self {
        Self([0u8; 4096])
    }
}

// 非阻塞检查是否有配置变更事件
// 在 hook 热路径调用，无事件时开销极小（一次非阻塞 read 系统调用）
pub fn poll_changed() -> bool {
    let fd = INOTIFY_FD.load(Ordering::Acquire);
    if fd < 0 {
        return false;
    }

    let now_ms = paths::monotonic_ms() as u64;
    let last_poll_ms = LAST_POLL_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_poll_ms) < POLL_INTERVAL_MS
        || LAST_POLL_MS
            .compare_exchange(last_poll_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return false;
    }

    let last_change_ms = LAST_CHANGE_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_change_ms) < CHANGE_DEBOUNCE_MS {
        return false;
    }

    let mut buf = InotifyBuf::new();
    // SAFETY: 读入 buf 自身的完整区间，长度取自同一数组，不会越界。
    let len = unsafe { libc::read(fd, buf.0.as_mut_ptr() as *mut _, buf.0.len()) };
    if len <= 0 {
        return false;
    }

    let mut changed = false;
    let mut offset = 0usize;
    let total = len as usize;
    while offset + std::mem::size_of::<inotify_event>() <= total {
        // SAFETY: 循环条件已保证 offset 起至少还有一个完整 inotify_event，
        // 且 InotifyBuf 按 4 字节对齐，事件起始地址满足对齐要求。
        let event = unsafe { &*(buf.0.as_ptr().add(offset) as *const inotify_event) };
        let event_len = std::mem::size_of::<inotify_event>() + event.len as usize;
        if event_len == 0 || offset + event_len > total {
            break;
        }

        if is_config_event(event) {
            changed = true;
        }

        offset += event_len;
    }

    if changed {
        LAST_CHANGE_MS.store(now_ms, Ordering::Relaxed);
    }
    changed
}

fn add_watch(fd: c_int, path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    let wd = unsafe { inotify_add_watch(fd, c_path.as_ptr(), EVENT_MASK) };
    wd >= 0
}

// 仅处理非目录的 .json 文件事件
fn is_config_event(event: &inotify_event) -> bool {
    if (event.mask & libc::IN_ISDIR) != 0 {
        return false;
    }
    if event.len > 0 {
        let name_ptr = unsafe { (event as *const inotify_event).add(1) as *const u8 };
        let name_slice = unsafe { std::slice::from_raw_parts(name_ptr, event.len as usize) };
        if let Ok(name) = std::str::from_utf8(name_slice) {
            return name.trim_end_matches('\0').ends_with(".json");
        }
    }
    true
}
