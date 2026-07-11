use libc::{
    IN_ATTRIB, IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_DELETE_SELF, IN_IGNORED, IN_ISDIR,
    IN_MODIFY, IN_MOVE_SELF, IN_MOVED_FROM, IN_MOVED_TO, IN_Q_OVERFLOW, c_void, inotify_add_watch,
    inotify_event, inotify_init1, read,
};
use std::ffi::CString;

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

pub(super) fn add_watch(fd: i32, path: &str) -> Option<i32> {
    let c_path = cstring_path(path)?;
    let wd = unsafe { inotify_add_watch(fd, c_path.as_ptr(), EVENT_MASK) };
    if wd < 0 { None } else { Some(wd) }
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

pub(super) fn is_self_removed(mask: u32) -> bool {
    (mask & (IN_DELETE_SELF | IN_MOVE_SELF)) != 0
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

pub(super) fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}
