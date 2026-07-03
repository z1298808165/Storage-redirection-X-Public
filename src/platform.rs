#[path = "platform/anti_detect.rs"]
pub mod anti_detect;
#[path = "platform/elf_img.rs"]
pub mod elf_img;
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

pub fn is_boot_completed() -> bool {
    system_property_get("sys.boot_completed").as_deref() == Some("1")
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
