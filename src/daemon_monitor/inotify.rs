use libc::{
    IN_ATTRIB, IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_DELETE_SELF, IN_IGNORED, IN_ISDIR,
    IN_MODIFY, IN_MOVE_SELF, IN_MOVED_FROM, IN_MOVED_TO, IN_Q_OVERFLOW, c_void, inotify_add_watch,
    inotify_event, inotify_init1, read,
};
use std::ffi::CString;

/// inotify 读取缓冲区。`inotify_event` 需要至少 4 字节对齐（wd/mask/cookie/len 均为 i32/u32），
/// 内核保证每个事件总长度是 sizeof(int) 的倍数，故缓冲区起始 4 字节对齐后首个及后续事件均满足对齐要求。
#[repr(align(4))]
pub(super) struct InotifyBuf<const N: usize>(pub(super) [u8; N]);

impl<const N: usize> InotifyBuf<N> {
    pub(super) const fn new() -> Self {
        Self([0u8; N])
    }
}

const EVENT_MASK: u32 = IN_CREATE
    | IN_MODIFY
    | IN_CLOSE_WRITE
    | IN_MOVED_TO
    | IN_MOVED_FROM
    | IN_DELETE
    | IN_ATTRIB
    | IN_DELETE_SELF
    | IN_MOVE_SELF;

pub(super) fn init_nonblocking() -> i32 {
    unsafe { inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) }
}

pub(super) fn close_fd(fd: i32) {
    unsafe {
        libc::close(fd);
    }
}

pub(super) fn read_into(fd: i32, buffer: &mut [u8]) -> isize {
    unsafe { read(fd, buffer.as_mut_ptr() as *mut c_void, buffer.len()) }
}

/// `inotify_add_watch` 失败原因。
///
/// 必须区分这几类：内核 watch 配额耗尽（`max_user_watches`，与模块内部的
/// `MAX_WATCHES` 无关）需要标记容量受限并告警；目录不存在属于正常竞态，交由
/// missing 重试处理；其余 errno 需要限频告警而不是静默跳过。
pub(super) enum AddWatchError {
    /// 内核 watch 配额耗尽或内存不足。
    Capacity(i32),
    /// 目标目录不存在或已被删除。
    Missing,
    /// 路径含 NUL 等无法构造 C 字符串的情况。
    InvalidPath,
    /// 其它 errno。
    Other(i32),
}

pub(super) fn add_watch(fd: i32, path: &str) -> Result<i32, AddWatchError> {
    let Some(c_path) = cstring_path(path) else {
        return Err(AddWatchError::InvalidPath);
    };
    let wd = unsafe { inotify_add_watch(fd, c_path.as_ptr(), EVENT_MASK) };
    if wd >= 0 {
        return Ok(wd);
    }
    let errno = crate::platform::errno::last();
    Err(match errno {
        libc::ENOSPC | libc::ENOMEM => AddWatchError::Capacity(errno),
        libc::ENOENT => AddWatchError::Missing,
        other => AddWatchError::Other(other),
    })
}

pub(super) fn event_len(event: &inotify_event) -> usize {
    std::mem::size_of::<inotify_event>() + event.len as usize
}

pub(super) fn event_name(event: &inotify_event) -> String {
    if event.len == 0 {
        return String::new();
    }
    let name_ptr = unsafe { (event as *const inotify_event).add(1) as *const u8 };
    let name_bytes = unsafe { std::slice::from_raw_parts(name_ptr, event.len as usize) };
    let end = name_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name_bytes.len());
    if end == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&name_bytes[..end]).to_string()
}

pub(super) fn is_safe_event_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

pub(super) fn is_queue_overflow(mask: u32) -> bool {
    (mask & IN_Q_OVERFLOW) != 0
}

pub(super) fn is_watch_ignored(mask: u32) -> bool {
    (mask & IN_IGNORED) != 0
}

pub(super) fn is_self_deleted(mask: u32) -> bool {
    (mask & IN_DELETE_SELF) != 0
}

/// 被监视目录自身被重命名或移动。
///
/// 与删除不同，移动之后 watch 仍然有效，内核不会补发 `IN_IGNORED`，
/// 因此必须由调用方主动清理，否则记录的路径会一直是旧路径。
pub(super) fn is_self_moved(mask: u32) -> bool {
    (mask & IN_MOVE_SELF) != 0
}

/// 向内核注销一个 watch。
///
/// 忽略 EINVAL：watch 可能已经被内核自动移除，此时重复注销不是错误。
pub(super) fn remove_watch(fd: i32, wd: i32) {
    if fd < 0 {
        return;
    }
    // Android 的 inotify_rm_watch 第二参数是 u32；wd 来自内核返回的非负值，
    // 直接用 as 转换，不做可能 panic 的受检转换，以免异常输入直接终止 daemon。
    // SAFETY: fd 已在上方判非负，wd 是本模块 add_watch 从内核取得并记录的描述符；
    // inotify_rm_watch 不接触本进程内存，非法 fd/wd 只会返回 EINVAL 而非未定义行为。
    if unsafe { libc::inotify_rm_watch(fd, wd as u32) } != 0 {
        let errno = last_errno();
        if errno != libc::EINVAL {
            log::warn!(
                "daemon monitor inotify_rm_watch failed wd={} errno={} {}",
                wd,
                errno,
                errno_text(errno)
            );
        }
    }
}

pub(super) fn is_relevant_event(mask: u32) -> bool {
    (mask
        & (IN_CREATE
            | IN_MODIFY
            | IN_CLOSE_WRITE
            | IN_MOVED_TO
            | IN_MOVED_FROM
            | IN_DELETE
            | IN_ATTRIB))
        != 0
}

pub(super) fn is_dir(mask: u32) -> bool {
    (mask & IN_ISDIR) != 0
}

pub(super) fn is_created_or_moved_to(mask: u32) -> bool {
    (mask & (IN_CREATE | IN_MOVED_TO)) != 0
}

pub(super) fn is_modify(mask: u32) -> bool {
    (mask & IN_MODIFY) != 0
}

pub(super) fn cstring_path(path: &str) -> Option<CString> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    CString::new(path).ok()
}

pub(super) use crate::platform::errno::last as last_errno;
pub(super) use crate::platform::errno::text as errno_text;
