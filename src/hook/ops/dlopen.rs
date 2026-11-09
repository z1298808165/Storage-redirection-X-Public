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
        let hub = InterceptHub::instance();
        hub.refresh_hooks_after_late_load("dlopen");
        retry_fuse_engine_patch_after_late_load(hub);
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
        let hub = InterceptHub::instance();
        hub.refresh_hooks_after_late_load("android_dlopen_ext");
        retry_fuse_engine_patch_after_late_load(hub);
    }
    result
}

/// 刚有库被 `dlopen` 成功：媒体提供器进程立即重置并重试 FUSE 引擎 read 补丁。
///
/// 目标库（`libfuse.so` / `libfuse_jni.so` / `libappfuse.so`）都是晚于 hook 安装按需
/// 加载的。以前靠「每次 read 检查一次、固定周期重试」来抓这个时机，代价是库永远不
/// 出现的 ROM 上会永久重试。现在把「库出现」这个事件当触发条件：dlopen 成功即重置
/// 退避并立刻试一次，定时重试只作为极端情形的兜底（5 分钟一次）。
///
/// 只对媒体提供器进程生效：其它进程不会去读 `/dev/fuse` 请求，做 ELF 定位纯属浪费。
fn retry_fuse_engine_patch_after_late_load(hub: &InterceptHub) {
    if !hub.with_package_name(crate::redirect::policy::is_media_provider_package) {
        return;
    }
    super::read::retry_after_late_load();
}
