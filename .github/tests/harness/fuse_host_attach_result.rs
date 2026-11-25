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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static CASE: AtomicUsize = AtomicUsize::new(0);
static SENT: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static OPENED: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static CLOSED: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static CLONES: AtomicUsize = AtomicUsize::new(0);
static REAPED: AtomicBool = AtomicBool::new(false);
static CAPABILITY_OK: AtomicBool = AtomicBool::new(false);
static ALIAS_LAYERS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static GROUP_CALLS: Mutex<Vec<Vec<u32>>> = Mutex::new(Vec::new());
static FSUID_CALLS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
// 目标路径上的挂载层数。接入按「替换而非叠加」实现：逐个摘到空再挂，所以桩必须能
// 表达「摘掉就少一层」，否则 `detach_all_layers_at_target` 的循环永远摘不完。
static LAYERS: AtomicUsize = AtomicUsize::new(1);
fn layers() -> usize {
    LAYERS.load(Ordering::Relaxed)
}
fn reset_layers() {
    LAYERS.store(if matches!(case(), 16 | 18) { 2 } else { 1 }, Ordering::Relaxed);
}
fn drop_layer() -> bool {
    LAYERS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_ok()
}
fn attach_layer() {
    LAYERS.fetch_add(1, Ordering::Relaxed);
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
        let fd = match (case(), app_ns) {
            (1, true) | (2, false) => -1,
            (_, true) => 11,
            _ => 12,
        };
        if fd >= 0 { OPENED.lock().unwrap().push(fd); }
        fd
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
    pub unsafe fn close(fd: i32) -> i32 {
        CLOSED.lock().unwrap().push(fd);
        0
    }
    pub type gid_t = u32;
    pub const MS_SLAVE: i32 = 1 << 18;
    pub const MNT_DETACH: i32 = 2;
    pub const SYS_setfsuid: i64 = 0;
    pub unsafe fn umount2(_: *const i8, _: i32) -> i32 {
        if case() == 16 && super::layers() == 1 { return -1; }
        if super::drop_layer() { 0 } else { -1 }
    }
    pub unsafe fn getgroups(_: i32, groups: *mut gid_t) -> i32 {
        if case() == 27 { return -1; }
        if case() == 31 {
            unsafe { *groups = 1000; *groups.add(1) = 2000; }
            return 2;
        }
        0
    }
    pub unsafe fn setgroups(count: usize, groups: *const gid_t) -> i32 {
        let mut calls = GROUP_CALLS.lock().unwrap();
        calls.push(unsafe { std::slice::from_raw_parts(groups, count) }.to_vec());
        if case() == 28 && calls.len() == 1 || case() == 29 && calls.len() == 2 { -1 } else { 0 }
    }
    pub unsafe fn syscall(_: i64, uid: u32) -> i64 {
        FSUID_CALLS.lock().unwrap().push(uid);
        0
    }
    pub unsafe fn mount(
        _: *const i8,
        target: *const i8,
        _: *const i8,
        _: u64,
        _: *const c_void,
    ) -> i32 {
        if case() == 23 {
            ALIAS_LAYERS.lock().unwrap().push(unsafe { CStr::from_ptr(target) }.to_str().unwrap().into());
        }
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
    // 别名用例使用上提的 /emulated 根，不查询带用户号的别名清单。
    pub fn storage_alias_roots_for_user(_: i32) -> Vec<String> {
        Vec::new()
    }
    pub fn is_same_or_child(_: &str, _: &str) -> bool {
        false
    }
    pub fn data_media_user_root_for_user(_: i32) -> String {
        String::new()
    }
    // 本夹具不构造符号链接，归一桩只处理尾部斜杠。
    pub fn normalize(path: &str) -> String {
        path.trim_end_matches('/').to_string()
    }
}
// 别名用例保留全部存储入口，不模拟 f2fs 直连后端。
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
    pub fn live_mounts_at(_: i32, target: &str) -> Vec<Entry> {
        let count = if super::case() == 23 {
            usize::from(super::ALIAS_LAYERS.lock().unwrap().iter().any(|path| path == target))
        } else { super::layers() };
        (0..count)
            .map(|index| Entry {
                mount_id: index as u64 + 1,
                dev: "0:1".to_string(),
                source: "本次会话".to_string(),
                fs_type: "fuse",
            })
            .collect()
    }
    pub fn try_live_mounts_at(pid: i32, target: &str) -> std::io::Result<Vec<Entry>> {
        if super::case() == 24 || super::case() == 25 && super::layers() == 0 {
            return Err(std::io::Error::from_raw_os_error(13));
        }
        Ok(live_mounts_at(pid, target))
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
impl Drop for UniqueFd {
    fn drop(&mut self) { if self.0 >= 0 { unsafe { libc::close(self.0) }; } }
}
fn log_errno(_: &str) {}
fn open_detached_mount(_: &CStr, recursive: bool) -> i32 {
    let count = CLONES.fetch_add(1, Ordering::Relaxed);
    if case() == 5 || case() == 15 && recursive { return -1; }
    let fd = 13 + count as i32;
    OPENED.lock().unwrap().push(fd);
    fd
}
fn clear_dead_srx_layers_at_target(_: &str, _: i32) -> bool {
    case() == 6
}
fn storage_view_layers_replaceable(_: &str, _: i32, _: &str) -> bool {
    case() != 17 && !(case() == 18 && layers() == 1)
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
// 主目标替换前的传播降级成功；传播失败由附着后的路径单独注入。
fn make_mount_slave_only(_: &CStr) -> bool {
    true
}

fn make_mount_slave(_: &CStr) -> bool {
    case() != 20
}

fn make_mount_private(target: &CStr) -> bool {
    if case() == 23 {
        assert!(ALIAS_LAYERS.lock().unwrap().iter().any(|path| path.as_bytes() == target.to_bytes()), "普通目录不能设置挂载传播属性");
    }
    !matches!(case(), 9 | 20)
}
fn alias_metadata(path: &str) -> std::io::Result<std::fs::Metadata> {
    if case() == 23 { std::fs::symlink_metadata(".") } else { std::fs::symlink_metadata(path) }
}
fn probe_metadata(_: &str) -> std::io::Result<std::fs::Metadata> {
    match case() {
        32 => Err(std::io::Error::from_raw_os_error(13)),
        33 => Err(std::io::Error::from_raw_os_error(5)),
        _ => std::fs::metadata("."),
    }
}
fn log_attach_child_blocked_point(_: i32, _: &str, _: i32, _: i64) {
    assert!(!REAPED.load(Ordering::Relaxed));
}
fn reap_host_child(_: i32) -> (bool, &'static str) {
    REAPED.store(true, Ordering::Relaxed);
    (case() != 26, "stopped")
}
fn host_storage_aliases_current(_: &str, _: i32, _: &str) -> bool {
    assert!(REAPED.load(Ordering::Relaxed), "停止子进程后才能接受最终视图");
    !matches!(case(), 19 | 21)
}
mod fuse_redirect {
    pub mod config {
        pub fn record_fuse_capability_result(ok: bool, _: &str, _: &str) {
            super::super::CAPABILITY_OK.store(ok, super::super::Ordering::Relaxed);
        }
    }
}
fn timeout_result(view: &HostSessionView, target_root: &str) -> bool {
    let (child, user_id, app_uid, timeout_sec) = (42, 0, 10000, 3);
    // __PRODUCTION_TIMEOUT__
}
// __PRODUCTION_CONSTS__
// __PRODUCTION_FUNCTIONS__
fn run(view: &HostSessionView, target_root: &str) -> bool {
    let ready_sockets = [0, 99];
    let user_id: i32 = 0;
    // 主协议用例使用非存储根，应用凭据探针由设备测试验证。
    let app_uid: i32 = 10000;
    // __PRODUCTION_DISPATCH__
    ok
}
fn main() {
    for id in (0..19).chain([24, 25]) {
        CASE.store(id, Ordering::Relaxed);
        SENT.lock().unwrap().clear();
        OPENED.lock().unwrap().clear();
        CLOSED.lock().unwrap().clear();
        CLONES.store(0, Ordering::Relaxed);
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
        let mut opened = OPENED.lock().unwrap().clone();
        let mut closed = CLOSED.lock().unwrap().clone();
        opened.sort();
        closed.sort();
        assert_eq!(opened, closed, "接入出口不得泄漏或重复关闭 fd：{id}");
        if id == 15 { assert_eq!(layers(), 1, "备份失败不得摘旧层"); }
        if id == 16 { assert_eq!(layers(), 2, "部分摘除失败必须恢复原栈顶"); }
        if id == 9 { assert_eq!(layers(), 1, "传播设置失败必须恢复备份"); }
        if id == 17 { assert_eq!(layers(), 1, "未知层不得被摘除"); }
        if id == 18 { assert_eq!(layers(), 1, "逐层校验必须保留中途出现的未知层"); }
        if matches!(id, 24 | 25) { assert_eq!(layers(), 1, "读表失败不能当成没有挂载"); }
    }
    for id in (19..23).chain([26]) {
        CASE.store(id, Ordering::Relaxed);
        REAPED.store(false, Ordering::Relaxed);
        CAPABILITY_OK.store(false, Ordering::Relaxed);
        let view = HostSessionView { child_pid: 42, mount_point: String::new(), mount_source: String::new(), mount_id: 1, dev: "0:1".into() };
        let ok = timeout_result(&view, "/storage/emulated");
        assert_eq!(ok, id == 22, "超时只能接受全部就位且传播修复成功的视图：{id}");
        assert_eq!(CAPABILITY_OK.load(Ordering::Relaxed), ok);
    }
    CASE.store(23, Ordering::Relaxed);
    let view = HostSessionView { child_pid: 42, mount_point: String::new(), mount_source: String::new(), mount_id: 1, dev: "0:1".into() };
    assert!(attach_host_view_to_storage_aliases("/storage/emulated", 0, &view));
    assert_eq!(ALIAS_LAYERS.lock().unwrap().len(), 3, "已有目录但没有挂载的别名也必须接入");
    for id in 27..34 {
        CASE.store(id, Ordering::Relaxed);
        GROUP_CALLS.lock().unwrap().clear();
        FSUID_CALLS.lock().unwrap().clear();
        reset_layers();
        let result = host_attach_child_main(&view, "/storage/emulated", 0, 10000);
        assert_eq!(result == 0, matches!(id, 30..=32), "凭据探针错误不能冒充接入成功：{id}");
        if id == 27 {
            assert!(FSUID_CALLS.lock().unwrap().is_empty(), "读组失败不能改变凭据");
        } else {
            let expected = if id == 31 { vec![1000, 2000] } else { vec![] };
            assert_eq!(*GROUP_CALLS.lock().unwrap(), vec![vec![9997], expected], "原补充组包括空集合都必须恢复：{id}");
            assert_eq!(*FSUID_CALLS.lock().unwrap(), vec![10000, 0], "fsuid 必须恢复：{id}");
        }
    }
}
