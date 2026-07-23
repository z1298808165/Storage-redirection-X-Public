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

    let mut buf = [0u8; 1024];
    let len = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
    if len <= 0 {
        return false;
    }

    let mut changed = false;
    let mut offset = 0;
    while offset < len as usize {
        let event = unsafe { &*(buf.as_ptr().add(offset) as *const inotify_event) };
        let event_len = std::mem::size_of::<inotify_event>() + event.len as usize;

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
