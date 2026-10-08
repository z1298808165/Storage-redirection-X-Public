use crate::platform::inotify::Event;
use crate::platform::paths;
use libc::{
    IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO, c_int, inotify_add_watch,
    inotify_init1,
};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const EVENT_MASK: u32 = IN_CREATE | IN_DELETE | IN_CLOSE_WRITE | IN_MOVED_FROM | IN_MOVED_TO;

static INOTIFY_FD: AtomicI32 = AtomicI32::new(-1);
static LAST_CHANGE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
static APPS_WATCH_FD: AtomicI32 = AtomicI32::new(-1);
static CHANGED_PACKAGES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

fn changed_packages() -> &'static Mutex<BTreeSet<String>> {
    CHANGED_PACKAGES.get_or_init(|| Mutex::new(BTreeSet::new()))
}
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

    let apps_dir = paths::join(config_dir, super::source::APPS_CONFIG_DIR);
    let apps_wd = add_watch_with_id(fd, &apps_dir).unwrap_or(-1);
    if apps_wd < 0 {
        log::debug!("apps dir missing or unwatchable");
    }

    APPS_WATCH_FD.store(apps_wd, Ordering::Release);
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    None,
    Full,
    Apps(Vec<String>),
}

pub fn poll_changed() -> bool {
    !matches!(poll_changed_with_packages(), ChangeKind::None)
}

/// 读取一次配置目录事件并返回变化范围。
pub fn poll_changed_with_packages() -> ChangeKind {
    let fd = INOTIFY_FD.load(Ordering::Acquire);
    if fd < 0 {
        return ChangeKind::None;
    }

    let now_ms = paths::monotonic_ms() as u64;
    let last_poll_ms = LAST_POLL_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_poll_ms) < POLL_INTERVAL_MS
        || LAST_POLL_MS
            .compare_exchange(last_poll_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return ChangeKind::None;
    }

    let last_change_ms = LAST_CHANGE_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_change_ms) < CHANGE_DEBOUNCE_MS {
        return ChangeKind::None;
    }

    let mut buf = InotifyBuf::new();
    // SAFETY: 读入 buf 自身的完整区间，长度取自同一数组，不会越界。
    let len = unsafe { libc::read(fd, buf.0.as_mut_ptr() as *mut _, buf.0.len()) };
    if len <= 0 {
        return ChangeKind::None;
    }

    let mut changed = false;
    let mut full_change = false;
    crate::platform::inotify::for_each_event(&buf.0[..len as usize], |event| {
        if !is_config_event(event) {
            return;
        }
        changed = true;
        let name = std::str::from_utf8(event.name)
            .unwrap_or_default()
            .trim_end_matches('\0');
        if event.wd == APPS_WATCH_FD.load(Ordering::Acquire)
            && name.ends_with(".json")
            && !name.is_empty()
        {
            let package = name.trim_end_matches(".json").to_string();
            if !package.is_empty() {
                if let Ok(mut packages) = changed_packages().lock() {
                    packages.insert(package);
                }
                return;
            }
        }
        full_change = true;
    });

    if !changed {
        return ChangeKind::None;
    }
    LAST_CHANGE_MS.store(now_ms, Ordering::Relaxed);
    if full_change {
        if let Ok(mut packages) = changed_packages().lock() {
            packages.clear();
        }
        return ChangeKind::Full;
    }
    let packages = changed_packages()
        .lock()
        .map(|mut values| std::mem::take(&mut *values).into_iter().collect())
        .unwrap_or_default();
    ChangeKind::Apps(packages)

}

fn add_watch(fd: c_int, path: &str) -> bool {
    add_watch_with_id(fd, path).is_some()
}

fn add_watch_with_id(fd: c_int, path: &str) -> Option<i32> {
    let Ok(c_path) = CString::new(path) else {
        return None;
    };
    let wd = unsafe { inotify_add_watch(fd, c_path.as_ptr(), EVENT_MASK) };
    (wd >= 0).then_some(wd)
}

// 仅处理非目录的 .json 文件事件
fn is_config_event(event: &Event<'_>) -> bool {
    if (event.mask & libc::IN_ISDIR) != 0 {
        return false;
    }
    if !event.name.is_empty()
        && let Ok(name) = std::str::from_utf8(event.name)
    {
        return name.trim_end_matches('\0').ends_with(".json");
    }
    true
}
