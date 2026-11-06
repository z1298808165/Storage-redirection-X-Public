use super::super::stats::InterceptHub;
use super::super::{diagnostic, path as path_utils, runtime, util::c_str_to_string};
use libc::{FILE, c_char, c_void};
use std::ffi::CString;

fn redirect_fopen_path<F>(hub: &InterceptHub, op: &str, path: *const c_char, call: F) -> *mut FILE
where
    F: FnOnce(*const c_char) -> *mut FILE,
{
    // SAFETY: path 由 fopen/freopen 调用方保证为 NUL 结尾或空指针。
    let path_text = unsafe { c_str_to_string(path) };
    if path_text.is_empty() {
        return call(path);
    }
    let decision_path = path_utils::normalize_decision_path(&path_text);
    if !path_utils::is_relevant_storage_path(hub, &decision_path) {
        return call(path);
    }
    diagnostic::log_diag_path_event(hub, op, "input", &decision_path, -1);
    let decision = crate::redirect::process_redirect_path(hub, &decision_path);
    diagnostic::log_diag_redirect_decision(hub, op, &decision_path, &decision);
    if decision.is_redirect()
        && let Ok(c_path) = CString::new(decision.new_path)
    {
        return call(c_path.as_ptr());
    }
    call(path)
}

fn call_fopen_prev(proxy: *mut c_void, path: *const c_char, mode: *const c_char) -> *mut FILE {
    // SAFETY: proxy/path/mode 由 hook 入口按 libc ABI 传入，call_prev 只在当前调用期间使用。
    unsafe {
        runtime::call_prev(
            proxy,
            || libc::fopen(path, mode),
            |prev| {
                let f: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut FILE =
                    std::mem::transmute(prev);
                f(path, mode)
            },
        )
    }
}

// quality-allow(lint-suppression): 入口本身已是 unsafe fn，显式 unsafe 块用于强调调用约定。
#[allow(unused_unsafe)]
pub unsafe extern "C" fn hooked_fopen(path: *const c_char, mode: *const c_char) -> *mut FILE {
    let self_ptr = hooked_fopen as *mut c_void;
    // SAFETY: path/mode 由 libc 按 ABI 传入，with_hook_guard 只在本调用内使用。
    unsafe {
        runtime::with_hook_guard(
            || call_fopen_prev(self_ptr, path, mode),
            |hub| {
                redirect_fopen_path(hub, "fopen", path, |call_path| {
                    call_fopen_prev(self_ptr, call_path, mode)
                })
            },
        )
    }
}

// quality-allow(lint-suppression): 入口本身已是 unsafe fn，显式 unsafe 块用于强调调用约定。
#[allow(unused_unsafe)]
pub unsafe extern "C" fn hooked_fopen64(path: *const c_char, mode: *const c_char) -> *mut FILE {
    let self_ptr = hooked_fopen64 as *mut c_void;
    // SAFETY: path/mode 由 libc 按 ABI 传入，with_hook_guard 只在本调用内使用。
    unsafe {
        runtime::with_hook_guard(
            || call_fopen_prev(self_ptr, path, mode),
            |hub| {
                redirect_fopen_path(hub, "fopen64", path, |call_path| {
                    call_fopen_prev(self_ptr, call_path, mode)
                })
            },
        )
    }
}

// quality-allow(lint-suppression): 入口本身已是 unsafe fn，显式 unsafe 块用于强调调用约定。
#[allow(unused_unsafe)]
pub unsafe extern "C" fn hooked_freopen(
    path: *const c_char,
    mode: *const c_char,
    stream: *mut FILE,
) -> *mut FILE {
    let self_ptr = hooked_freopen as *mut c_void;
    let call = |call_path: *const c_char| -> *mut FILE {
        // SAFETY: call_path 由重定向逻辑提供，stream 由 libc 调用方传入。
        unsafe {
            runtime::call_prev(
                self_ptr,
                || libc::freopen(call_path, mode, stream),
                |prev| {
                    let f: unsafe extern "C" fn(
                        *const c_char,
                        *const c_char,
                        *mut FILE,
                    ) -> *mut FILE = std::mem::transmute(prev);
                    f(call_path, mode, stream)
                },
            )
        }
    };
    // SAFETY: path/mode/stream 由 libc 按 ABI 传入，with_hook_guard 只在本调用内使用。
    unsafe {
        runtime::with_hook_guard(
            || call(path),
            |hub| redirect_fopen_path(hub, "freopen", path, call),
        )
    }
}
