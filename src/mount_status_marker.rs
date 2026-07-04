use crate::platform::fs;
use crate::platform::unique_fd::UniqueFd;
use libc::{O_CLOEXEC, O_CREAT, O_TRUNC, O_WRONLY, c_char, chmod, chown, open};
use std::ffi::{CStr, CString};
use std::sync::OnceLock;

type GetFileCon = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> libc::c_int;
type SetFileCon = unsafe extern "C" fn(*const c_char, *const c_char) -> libc::c_int;
type FreeCon = unsafe extern "C" fn(*mut c_char);

#[derive(Clone, Copy)]
struct SelinuxApi {
    getfilecon: GetFileCon,
    setfilecon: SetFileCon,
    freecon: FreeCon,
}

static SELINUX_API: OnceLock<Option<SelinuxApi>> = OnceLock::new();

pub(crate) fn write_mount_status_marker(
    app_data_dir: &str,
    pid: i32,
    uid: i32,
    is_ok: bool,
) -> bool {
    if app_data_dir.is_empty() || pid <= 0 || uid < 0 {
        log::warn!(
            "marker args invalid dir={} pid={} uid={}",
            app_data_dir,
            pid,
            uid
        );
        return false;
    }

    let marker_path = format!("{}/.srx_mount_status_{}", app_data_dir, pid);
    let Ok(c_path) = CString::new(marker_path.clone()) else {
        log::warn!("marker path invalid {}", marker_path);
        return false;
    };
    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "marker open failed path={} pid={} errno={} {}",
            marker_path,
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }
    let file = UniqueFd::new(fd);

    let mut marker_ok = true;
    if !copy_parent_selinux_context(&c_path, app_data_dir, &marker_path, pid) {
        marker_ok = false;
    }

    unsafe {
        if chown(c_path.as_ptr(), uid as u32, uid as u32) != 0 {
            marker_ok = false;
            let errno = last_errno();
            log::warn!(
                "marker chown failed path={} pid={} uid={} errno={} {}",
                marker_path,
                pid,
                uid,
                errno,
                errno_text(errno)
            );
        }
        if chmod(c_path.as_ptr(), 0o600) != 0 {
            marker_ok = false;
            let errno = last_errno();
            log::warn!(
                "marker chmod failed path={} pid={} errno={} {}",
                marker_path,
                pid,
                errno,
                errno_text(errno)
            );
        }
    }

    let value = if is_ok { b'1' } else { b'0' };
    if !fs::write_all(file.get(), &[value]) {
        marker_ok = false;
        log::warn!(
            "marker write failed path={} pid={} val={}",
            marker_path,
            pid,
            value as char
        );
    }
    if unsafe { libc::fsync(file.get()) } != 0 {
        marker_ok = false;
        let errno = last_errno();
        log::warn!(
            "marker fsync failed path={} pid={} errno={} {}",
            marker_path,
            pid,
            errno,
            errno_text(errno)
        );
    }
    if marker_ok {
        log::debug!(
            "marker ok path={} pid={} val={}",
            marker_path,
            pid,
            value as char
        );
    }
    marker_ok
}

fn copy_parent_selinux_context(
    marker_path: &CString,
    app_data_dir: &str,
    marker_path_display: &str,
    pid: i32,
) -> bool {
    let Some(api) = selinux_api() else {
        return false;
    };
    let Ok(parent_path) = CString::new(app_data_dir) else {
        return false;
    };

    let mut context: *mut c_char = std::ptr::null_mut();
    let get_ret = unsafe { (api.getfilecon)(parent_path.as_ptr(), &mut context as *mut _) };
    if get_ret < 0 || context.is_null() {
        let errno = last_errno();
        log::warn!(
            "marker getfilecon failed dir={} path={} pid={} errno={} {}",
            app_data_dir,
            marker_path_display,
            pid,
            errno,
            errno_text(errno)
        );
        return false;
    }

    let set_ret = unsafe { (api.setfilecon)(marker_path.as_ptr(), context as *const c_char) };
    let errno = if set_ret != 0 { last_errno() } else { 0 };
    let context_text = unsafe { CStr::from_ptr(context) }
        .to_string_lossy()
        .to_string();
    unsafe { (api.freecon)(context) };

    if set_ret != 0 {
        log::warn!(
            "marker setfilecon failed path={} pid={} ctx={} errno={} {}",
            marker_path_display,
            pid,
            context_text,
            errno,
            errno_text(errno)
        );
        return false;
    }

    log::debug!(
        "marker context ok path={} pid={} ctx={}",
        marker_path_display,
        pid,
        context_text
    );
    true
}

fn selinux_api() -> Option<&'static SelinuxApi> {
    SELINUX_API.get_or_init(load_selinux_api).as_ref()
}

fn load_selinux_api() -> Option<SelinuxApi> {
    let handle =
        unsafe { libc::dlopen(c"libselinux.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        log::warn!("marker selinux api unavailable: dlopen libselinux.so failed");
        return None;
    }

    let getfilecon = unsafe { libc::dlsym(handle, c"getfilecon".as_ptr()) };
    let setfilecon = unsafe { libc::dlsym(handle, c"setfilecon".as_ptr()) };
    let freecon = unsafe { libc::dlsym(handle, c"freecon".as_ptr()) };
    if getfilecon.is_null() || setfilecon.is_null() || freecon.is_null() {
        log::warn!("marker selinux api unavailable: missing symbol");
        return None;
    }

    Some(SelinuxApi {
        getfilecon: unsafe { std::mem::transmute::<*mut libc::c_void, GetFileCon>(getfilecon) },
        setfilecon: unsafe { std::mem::transmute::<*mut libc::c_void, SetFileCon>(setfilecon) },
        freecon: unsafe { std::mem::transmute::<*mut libc::c_void, FreeCon>(freecon) },
    })
}

fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}

fn errno_text(code: i32) -> String {
    unsafe {
        CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .to_string()
    }
}
