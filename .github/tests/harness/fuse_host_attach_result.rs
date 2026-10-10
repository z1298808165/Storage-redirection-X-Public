// 接入结果协议的外置桩：注入生产函数，逐一触发失败出口，核对每次只回包一次。
#![allow(dead_code, unused_variables, non_camel_case_types)]
extern crate self as log;
#[macro_export]
macro_rules! warn {
    ($($token:tt)*) => {{}};
}
use std::ffi::{CStr, CString};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
static CASE: AtomicUsize = AtomicUsize::new(0);
static SENT: Mutex<Vec<i32>> = Mutex::new(Vec::new());
fn case() -> usize {
    CASE.load(Ordering::Relaxed)
}
mod libc {
    use super::*;
    pub type c_int = i32;
    pub type c_void = std::ffi::c_void;
    pub const O_RDONLY: i32 = 0;
    pub const O_CLOEXEC: i32 = 1;
    pub const CLONE_NEWNS: i32 = 2;
    pub const ENOTCONN: i32 = 107;
    pub unsafe fn open(path: *const i8, _: i32) -> i32 {
        let app_ns = unsafe { CStr::from_ptr(path) }.to_bytes() == b"/proc/self/ns/mnt";
        match (case(), app_ns) {
            (1, true) | (2, false) => -1,
            (_, true) => 11,
            _ => 12,
        }
    }
    pub unsafe fn setns(fd: i32, _: i32) -> i32 {
        if (case() == 3 && fd == 12) || (case() == 4 && fd == 11) {
            -1
        } else {
            0
        }
    }
    pub unsafe fn mkdir(_: *const i8, _: u32) -> i32 {
        0
    }
    pub unsafe fn send(fd: i32, data: *const c_void, size: usize, _: i32) -> isize {
        assert_eq!(fd, 99);
        assert_eq!(size, std::mem::size_of::<i32>());
        SENT.lock().unwrap().push(unsafe { *(data as *const i32) });
        if case() == 14 { -1 } else { size as isize }
    }
}
mod platform {
    pub mod errno {
        pub fn last() -> i32 {
            107
        }
        pub fn text(_: i32) -> &'static str {
            ""
        }
    }
}
mod mount_ledger {
    pub struct Entry {
        pub source: String,
        pub fs_type: &'static str,
    }
    pub fn topmost_live_mount(_: i32, _: &str) -> Option<Entry> {
        if super::case() == 10 {
            None
        } else {
            Some(Entry {
                source: if super::case() == 11 {
                    "旧会话"
                } else {
                    "本次会话"
                }
                .to_string(),
                fs_type: "fuse",
            })
        }
    }
}
struct HostSessionView {
    child_pid: i32,
    mount_point: String,
    mount_source: String,
}
struct UniqueFd(i32);
impl UniqueFd {
    fn new(fd: i32) -> Self {
        Self(fd)
    }
    fn get(&self) -> i32 {
        self.0
    }
}
fn log_errno(_: &str) {}
fn open_detached_mount(_: &CStr) -> i32 {
    if case() == 5 { -1 } else { 13 }
}
fn clear_dead_srx_layers_at_target(_: &str) -> bool {
    case() == 6
}
fn move_detached_mount(_: i32, _: &CStr) -> Result<(), i32> {
    match case() {
        6 | 7 => Err(107),
        8 => Err(5),
        _ => Ok(()),
    }
}
fn make_mount_private(_: &CStr) -> bool {
    case() != 9
}
// __PRODUCTION_FUNCTIONS__
fn run(view: &HostSessionView, target_root: &str) -> bool {
    let ready_sockets = [0, 99];
    // __PRODUCTION_DISPATCH__
    ok
}
fn main() {
    for id in 0..15 {
        CASE.store(id, Ordering::Relaxed);
        SENT.lock().unwrap().clear();
        let view = HostSessionView {
            child_pid: 42,
            mount_point: if id == 12 { "\0" } else { "/宿主" }.to_string(),
            mount_source: "本次会话".to_string(),
        };
        let ok = run(&view, if id == 13 { "\0" } else { "/应用" });
        assert_eq!(ok, id == 0, "错误路径不应报告成功：{id}");
        let sent = SENT.lock().unwrap();
        assert_eq!(sent.len(), 1, "所有正常出口必须且只能回包一次：{id}");
        let expected = match id {
            0 | 14 => 0,
            7 => -107,
            _ => -1,
        };
        assert_eq!(sent[0], expected, "错误码分类应保持不变：{id}");
    }
}
