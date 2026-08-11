#[path = "platform/anti_detect.rs"]
pub mod anti_detect;
#[path = "platform/elf_img.rs"]
pub mod elf_img;
#[path = "platform/errno.rs"]
pub mod errno;
#[path = "platform/fs.rs"]
pub mod fs;
#[path = "platform/gnu_debugdata.rs"]
pub mod gnu_debugdata;
#[path = "platform/linker.rs"]
pub mod linker;
#[path = "platform/module_paths.rs"]
pub mod module_paths;
#[path = "platform/paths.rs"]
pub mod paths;
#[path = "platform/unique_fd.rs"]
pub mod unique_fd;

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ANDROID_USER_ID_OFFSET: i32 = 100000;
pub const MIN_SUPPORTED_API_LEVEL: i32 = 31;
const ISOLATED_APP_ID_START: i32 = 99000;
const ISOLATED_APP_ID_END: i32 = 99999;
const PROP_VALUE_MAX: usize = 92;

pub fn android_api_level() -> i32 {
    unsafe { android_get_device_api_level() }
}

pub fn system_property_get(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }

    let Ok(c_name) = CString::new(name) else {
        return None;
    };
    let mut value = [0 as libc::c_char; PROP_VALUE_MAX];
    let len = unsafe { __system_property_get(c_name.as_ptr(), value.as_mut_ptr()) };
    if len <= 0 {
        return None;
    }

    let text = unsafe { CStr::from_ptr(value.as_ptr()) };
    Some(text.to_string_lossy().trim().to_string())
}

/// `sys.boot_completed` 是否已置位。
///
/// 该属性单调：一旦变为 `1` 就不会再变回去，因此结果为真后永久缓存。
/// 这条判断位于 open/openat 热路径上（MediaProvider 每次 open 都会经过），
/// 未缓存时每次都要构造 CString、调用 `__system_property_get` 并再分配一次字符串。
pub fn is_boot_completed() -> bool {
    static BOOT_COMPLETED: AtomicBool = AtomicBool::new(false);
    if BOOT_COMPLETED.load(Ordering::Relaxed) {
        return true;
    }
    if system_property_get("sys.boot_completed").as_deref() == Some("1") {
        BOOT_COMPLETED.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

/// 读取当前开机周期的 boot id。
///
/// 供跨重启保留的状态文件判断归属：进程号会跨 boot 复用，仅凭进程号可能把
/// 上一次开机的残留记录误判为本次开机的结果。读取失败时返回空串，由调用方
/// 决定回退行为。
pub fn read_boot_id() -> String {
    const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
    let Ok(c_path) = CString::new(BOOT_ID_PATH) else {
        return String::new();
    };
    // SAFETY: c_path 在本作用域内存活，是以 NUL 结尾的合法 C 字符串。
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return String::new();
    }

    let file = unique_fd::UniqueFd::new(fd);
    let mut buffer = [0u8; 128];
    // SAFETY: 只向本地缓冲写入，长度上限留出末位空间。
    let n = unsafe { libc::read(file.get(), buffer.as_mut_ptr() as *mut _, buffer.len() - 1) };
    if n <= 0 {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer[..n as usize]);
    text.trim_matches(|c| c == ' ' || c == '\n' || c == '\r' || c == '\t')
        .to_string()
}

/// 读取 Linux `/proc/<pid>/stat` 的进程启动时钟值（字段 22）。
///
/// PID 会被复用；需要跨异步清理周期识别同一个进程实例时，必须同时比较该值。
pub fn process_start_time_ticks(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let after_name = stat.rsplit_once(')')?.1.trim_start();
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// 判断 PID 当前是否仍指向指定的进程实例。
pub fn is_process_instance_alive(pid: i32, start_time_ticks: u64) -> bool {
    process_start_time_ticks(pid) == Some(start_time_ticks)
}

pub fn user_id_from_uid(uid: i32) -> i32 {
    if uid >= 0 {
        uid / ANDROID_USER_ID_OFFSET
    } else {
        0
    }
}

// 隔离进程 UID（app_id 99000-99999）无存储访问权限
pub fn is_isolated_uid(uid: i32) -> bool {
    if uid < 0 {
        return false;
    }
    let app_id = uid % ANDROID_USER_ID_OFFSET;
    (ISOLATED_APP_ID_START..=ISOLATED_APP_ID_END).contains(&app_id)
}

unsafe extern "C" {
    fn android_get_device_api_level() -> i32;
    fn __system_property_get(name: *const libc::c_char, value: *mut libc::c_char) -> i32;
}
