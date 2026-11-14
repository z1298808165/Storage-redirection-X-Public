// 接入结果协议的外置桩：注入生产函数，逐一触发失败出口，核对每次只回包一次。
#![allow(dead_code, unused_variables, non_camel_case_types)]
extern crate self as log;
#[macro_export]
macro_rules! warn {
    ($($token:tt)*) => {{}};
}
#[macro_export]
macro_rules! info {
    ($($token:tt)*) => {{}};
}
#[macro_export]
macro_rules! debug {
    ($($token:tt)*) => {{}};
}
use std::ffi::{CStr, CString};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
static CASE: AtomicUsize = AtomicUsize::new(0);
static SENT: Mutex<Vec<i32>> = Mutex::new(Vec::new());
// 目标路径上的挂载层数。接入按「替换而非叠加」实现：逐个摘到空再挂，所以桩必须能
// 表达「摘掉就少一层」，否则 `detach_all_layers_at_target` 的循环永远摘不完。
static LAYERS: AtomicUsize = AtomicUsize::new(1);
fn layers() -> usize {
    LAYERS.load(Ordering::Relaxed)
}
fn reset_layers() {
    LAYERS.store(1, Ordering::Relaxed);
}
fn drop_layer() -> bool {
    LAYERS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_ok()
}
fn attach_layer() {
    LAYERS.store(1, Ordering::Relaxed);
}
fn case() -> usize {
    CASE.load(Ordering::Relaxed)
}
mod libc {
    use super::*;
    pub type c_int = i32;
    pub type c_ulong = u64;
    pub const MS_BIND: i32 = 4096;
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
    pub type c_uint = u32;
    pub const EACCES: i32 = 13;
    pub unsafe fn close(_: i32) -> i32 {
        0
    }
    pub type gid_t = u32;
    pub const MS_SLAVE: i32 = 1 << 18;
    pub const MNT_DETACH: i32 = 2;
    pub const SYS_setfsuid: i64 = 0;
    pub unsafe fn umount2(_: *const i8, _: i32) -> i32 {
        if super::drop_layer() { 0 } else { -1 }
    }
    pub unsafe fn getgroups(_: i32, _: *mut gid_t) -> i32 {
        0
    }
    pub unsafe fn setgroups(_: usize, _: *const gid_t) -> i32 {
        0
    }
    pub unsafe fn syscall(_: i64, _: u32) -> i64 {
        0
    }
    pub unsafe fn mount(
        _: *const i8,
        _: *const i8,
        _: *const i8,
        _: u64,
        _: *const c_void,
    ) -> i32 {
        0
    }
    pub unsafe fn send(fd: i32, data: *const c_void, size: usize, _: i32) -> isize {
        assert_eq!(fd, 99);
        assert_eq!(size, std::mem::size_of::<i32>());
        SENT.lock().unwrap().push(unsafe { *(data as *const i32) });
        if case() == 14 { -1 } else { size as isize }
    }
}
mod paths {
    // 协议用例的目标根都不是存储视图根，别名展开在 production 代码里会提前返回；
    // 这里只提供编译所需的签名桩。
    pub fn storage_alias_roots_for_user(_: i32) -> Vec<String> {
        Vec::new()
    }
    pub fn is_same_or_child(_: &str, _: &str) -> bool {
        false
    }
    pub fn data_media_user_root_for_user(_: i32) -> String {
        String::new()
    }
    // 符号别名去重用的词法归一：只是去掉尾部斜杠，足够本用例（别名列表为空、
    // 运行期不会走到绑定分支）。
    pub fn normalize(path: &str) -> String {
        path.trim_end_matches('/').to_string()
    }
}
// 别名展开用它排除 f2fs 直连后端别名；本桩的别名列表为空，运行期不会命中，
// 只要提供同签名实现即可满足编译。
fn is_f2fs_direct_backend_alias(_: &str, _: i32) -> bool {
    false
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
        pub mount_id: u64,
        pub dev: String,
        pub source: String,
        pub fs_type: &'static str,
    }
    // 接入前的目标私有化只对「目标本身是挂载点」才需要。协议用例的目标根都按挂载点
    // 处理，保持与引入该判据之前一致的私有化路径，用例才继续覆盖 mount 失败出口。
    pub fn live_mounts_at(_: i32, _: &str) -> Vec<Entry> {
        (0..super::layers())
            .map(|index| Entry {
                mount_id: index as u64 + 1,
                dev: "0:1".to_string(),
                source: "本次会话".to_string(),
                fs_type: "fuse",
            })
            .collect()
    }
    pub fn topmost_live_mount(_: i32, _: &str) -> Option<Entry> {
        if super::case() == 10 {
            None
        } else {
            // 归属判定已改按超级块身份：case 11 模拟"上一次会话留下的层"（不同 dev），
            // 其余 case 与本次会话同一超级块。挂载源不再参与判定，保持同形即可。
            Some(Entry {
                mount_id: 1,
                dev: if super::case() == 11 {
                    "0:2".to_string()
                } else {
                    "0:1".to_string()
                },
                source: "本次会话".to_string(),
                fs_type: "fuse",
            })
        }
    }
}
struct HostSessionView {
    child_pid: i32,
    mount_point: String,
    mount_source: String,
    mount_id: u64,
    dev: String,
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
        _ => {
            attach_layer();
            Ok(())
        }
    }
}
// P5-d：视图根的传播属性设置。桩只用于验证回包契约，传播属性本身不在断言范围。
// quality-allow(lint-suppression): 该桩由门禁按需注入，生产侧调用点不在本文件内，故需抑制 dead_code。
#[allow(dead_code)]
// 替换策略的传播降级桩：本组用例的目标根都不是视图根，运行期不会走到该分支。
fn make_mount_slave_only(_: &CStr) -> bool {
    true
}

fn make_mount_slave(_: &CStr) -> bool {
    true
}

fn make_mount_private(_: &CStr) -> bool {
    case() != 9
}
// __PRODUCTION_CONSTS__
// __PRODUCTION_FUNCTIONS__
fn run(view: &HostSessionView, target_root: &str) -> bool {
    let ready_sockets = [0, 99];
    let user_id: i32 = 0;
    // 视图根替换的可用性探针需要应用 uid；本组用例不进入该分支，取值不影响断言。
    let app_uid: i32 = 10000;
    // __PRODUCTION_DISPATCH__
    ok
}
fn main() {
    for id in 0..15 {
        CASE.store(id, Ordering::Relaxed);
        SENT.lock().unwrap().clear();
        reset_layers();
        let view = HostSessionView {
            child_pid: 42,
            mount_point: if id == 12 { "\0" } else { "/宿主" }.to_string(),
            mount_source: "本次会话".to_string(),
            mount_id: 1,
            dev: "0:1".to_string(),
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
