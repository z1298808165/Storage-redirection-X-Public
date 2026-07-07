use super::super::context;
use super::super::runtime;
use super::super::stats::InterceptHub;
use libc::{c_char, c_int, c_void};

unsafe fn call_dlopen(filename: *const c_char, flags: c_int) -> *mut c_void {
    let self_ptr = hooked_dlopen as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || libc::dlopen(filename, flags),
        |prev| {
            let f: unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void =
                std::mem::transmute(prev);
            f(filename, flags)
        },
    )
}

unsafe fn call_android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const c_void,
) -> *mut c_void {
    let self_ptr = hooked_android_dlopen_ext as *mut c_void;
    runtime::call_prev(
        self_ptr,
        || libc::dlopen(filename, flags),
        |prev| {
            let f: unsafe extern "C" fn(*const c_char, c_int, *const c_void) -> *mut c_void =
                std::mem::transmute(prev);
            f(filename, flags, extinfo)
        },
    )
}

pub unsafe extern "C" fn hooked_dlopen(filename: *const c_char, flags: c_int) -> *mut c_void {
    if context::ReentryGuard::is_reentrant() {
        return call_dlopen(filename, flags);
    }

    let result = {
        let _guard = context::ReentryGuard::enter();
        call_dlopen(filename, flags)
    };
    if !result.is_null() {
        InterceptHub::instance().refresh_hooks_after_late_load("dlopen");
    }
    result
}

pub unsafe extern "C" fn hooked_android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const c_void,
) -> *mut c_void {
    if context::ReentryGuard::is_reentrant() {
        return call_android_dlopen_ext(filename, flags, extinfo);
    }

    let result = {
        let _guard = context::ReentryGuard::enter();
        call_android_dlopen_ext(filename, flags, extinfo)
    };
    if !result.is_null() {
        InterceptHub::instance().refresh_hooks_after_late_load("android_dlopen_ext");
    }
    result
}
