use super::{context, stats::InterceptHub};
use crate::platform::proc_identity::RequestIdentity;
use crate::redirect::policy;
use libc::{RTLD_LOCAL, RTLD_NOW, c_char, c_void, dlopen, dlsym, getpid, getuid};
use std::ffi::CString;
use std::sync::OnceLock;

const BINDER_SAVED_CALLER_HARD_LIMIT_MS: i64 = 30_000;
const FUSE_CALLER_WINDOW_MS: i64 = 1_500;

// 请求作用域内保留框架捕获的身份，底层文件操作不能用进程名或 UID 候选覆盖它。
pub fn update_caller_package_for_current_thread(hub: &InterceptHub) {
    if context::is_current_caller_scope_active() {
        return;
    }
    let saved_identity = context::current_request_identity();
    context::set_request_identity(None);
    context::set_monitor_caller_pid(-1);
    context::set_monitor_caller_uid(-1);
    if hub.with_package_name(policy::is_media_provider_package) {
        super::ops::read::ensure_fuse_engine_read_patch();
    }
    let binder_uid = resolve_caller_uid_by_binder();
    let (uid, pid) = if binder_uid >= 0 {
        (binder_uid, resolve_caller_pid_by_binder())
    } else if context::is_binder_identity_cleared() {
        let uid = context::get_binder_saved_caller_uid();
        let age = context::get_binder_saved_caller_uid_age_ms();
        if uid >= 0 && (0..=BINDER_SAVED_CALLER_HARD_LIMIT_MS).contains(&age) {
            (uid, -1)
        } else {
            context::clear_binder_saved_caller_uid();
            context::set_binder_identity_cleared(false);
            (-1, -1)
        }
    } else if (0..=1500).contains(&context::get_fuse_caller_uid_age_ms()) {
        (
            context::get_fuse_caller_uid(),
            context::get_fuse_caller_pid(),
        )
    } else {
        (-1, -1)
    };
    if uid < 0 {
        hub.clear_current_caller();
        return;
    }
    context::set_monitor_caller_uid(uid);
    context::set_monitor_caller_pid(pid);
    let identity =
        if context::is_binder_identity_cleared() && context::get_binder_saved_caller_uid() == uid {
            saved_identity.filter(|identity| identity.uid == uid)
        } else {
            crate::daemon_identity_query::query_process_identity(pid, uid)
        }
        .unwrap_or_else(|| RequestIdentity::unresolved(uid, pid));
    let package = identity.package.clone();
    context::set_request_identity(Some(identity));
    context::set_current_caller_from_external_signal(&package, uid);
}

pub fn update_caller_for_mutation(hub: &InterceptHub, _pathname: *const c_char) {
    update_caller_package_for_current_thread(hub);
}

pub fn capture_binder_caller_uid_if_available() {
    if context::is_binder_identity_cleared() {
        return;
    }
    let uid = if context::is_current_caller_scope_active() {
        context::get_current_caller_uid()
    } else {
        resolve_caller_uid_by_binder()
    };
    if uid < 0 {
        context::clear_binder_saved_caller_uid();
        return;
    }
    let package = if context::is_current_caller_scope_active() {
        context::get_current_caller_package()
    } else {
        resolve_java_caller_package(uid, resolve_caller_pid_by_binder()).unwrap_or_default()
    };
    context::set_binder_saved_caller_uid(uid);
    context::set_binder_saved_caller_package(&package);
}

// 共享用户 ID 只用于监视回退，不能作为包名参与重定向决策。
pub fn enter_java_caller_scope(
    uid: i32,
    pid: i32,
    package: &str,
    captured: Option<&str>,
) -> RequestIdentity {
    let pid = if pid > 0 {
        pid
    } else if uid >= 0
        && context::get_fuse_caller_uid() == uid
        && (0..=FUSE_CALLER_WINDOW_MS).contains(&context::get_fuse_caller_uid_age_ms())
    {
        context::get_fuse_caller_pid()
    } else {
        -1
    };
    let current = crate::daemon_identity_query::query_process_identity(pid, uid);
    let mut identity = if let Some(captured) = captured {
        let mut captured = serde_json::from_str::<RequestIdentity>(captured)
            .ok()
            .filter(|identity| identity.uid == uid && identity.pid == pid)
            .unwrap_or_else(|| RequestIdentity::unresolved(uid, pid));
        captured.revalidate(current.as_ref());
        captured
    } else {
        current.unwrap_or_else(|| RequestIdentity::unresolved(uid, pid))
    };
    if captured.is_none() && !identity.is_terminal() && !package.is_empty() {
        identity.package = package.to_owned();
    }
    let package = identity.package.clone();
    if !identity.is_terminal() {
        context::remember_cross_thread_caller(&identity);
    }
    context::push_current_caller_scope_with_shared_user(&package, uid, &identity.shared_user_id);
    context::set_request_identity(Some(identity.clone()));
    context::set_monitor_caller_uid(uid);
    context::set_monitor_caller_pid(pid);
    context::set_binder_saved_caller_uid(uid);
    context::set_binder_saved_caller_package(&package);
    identity
}

pub fn exit_java_caller_scope() {
    context::pop_current_caller_scope();
}

fn resolve_java_caller_package(caller_uid: i32, caller_pid: i32) -> Option<String> {
    let identity = crate::daemon_identity_query::query_process_identity(caller_pid, caller_uid)
        .unwrap_or_else(|| RequestIdentity::unresolved(caller_uid, caller_pid));
    let package = (!identity.package.is_empty()).then(|| identity.package.clone());
    context::set_request_identity(Some(identity));
    package
}

fn resolve_caller_uid_by_binder() -> i32 {
    let caller_uid = get_caller_binder_symbols().calling_uid().unwrap_or(-1);
    let self_uid = unsafe { getuid() } as i32;
    if caller_uid == self_uid {
        return -1;
    }

    caller_uid
}

fn resolve_caller_pid_by_binder() -> i32 {
    let caller_pid = get_caller_binder_symbols().calling_pid().unwrap_or(-1);
    if caller_pid <= 0 || caller_pid == unsafe { getpid() } as i32 {
        return -1;
    }

    caller_pid
}

#[derive(Clone, Copy, Default)]
struct CallerBinderSymbols {
    binder_self_or_null: Option<unsafe extern "C" fn() -> *mut c_void>,
    binder_get_calling_uid: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    binder_get_calling_pid: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

impl CallerBinderSymbols {
    fn current_state(&self) -> Option<*mut c_void> {
        // 只借用已有 TLS；创建 Binder 状态会在驱动 open 的 hook 中重入初始化锁。
        // SAFETY: selfOrNull 来自进程内 libbinder，返回本线程现存状态或空指针。
        let state = unsafe { (self.binder_self_or_null?)() };
        (!state.is_null()).then_some(state)
    }

    fn calling_uid(&self) -> Option<i32> {
        let state = self.current_state()?;
        // SAFETY: state 由本线程的 selfOrNull 返回，getter 来自同一 libbinder。
        i32::try_from(unsafe { (self.binder_get_calling_uid?)(state) }).ok()
    }

    fn calling_pid(&self) -> Option<i32> {
        let state = self.current_state()?;
        // SAFETY: state 由本线程的 selfOrNull 返回，getter 来自同一 libbinder。
        Some(unsafe { (self.binder_get_calling_pid?)(state) })
    }
}

fn get_caller_binder_symbols() -> &'static CallerBinderSymbols {
    static SYMBOLS: OnceLock<CallerBinderSymbols> = OnceLock::new();
    SYMBOLS.get_or_init(|| {
        let mut symbols = CallerBinderSymbols::default();

        unsafe {
            if let Some(libbinder_name) = c_str("libbinder.so") {
                let libbinder = dlopen(libbinder_name.as_ptr(), RTLD_NOW | RTLD_LOCAL);
                if !libbinder.is_null() {
                    symbols.binder_self_or_null =
                        load_symbol(libbinder, "_ZN7android14IPCThreadState10selfOrNullEv");
                    symbols.binder_get_calling_uid =
                        load_symbol(libbinder, "_ZNK7android14IPCThreadState13getCallingUidEv");
                    if symbols.binder_get_calling_uid.is_none() {
                        symbols.binder_get_calling_uid =
                            load_symbol(libbinder, "_ZN7android14IPCThreadState13getCallingUidEv");
                    }
                    symbols.binder_get_calling_pid =
                        load_symbol(libbinder, "_ZNK7android14IPCThreadState13getCallingPidEv");
                    if symbols.binder_get_calling_pid.is_none() {
                        symbols.binder_get_calling_pid =
                            load_symbol(libbinder, "_ZN7android14IPCThreadState13getCallingPidEv");
                    }
                }
            }
        }

        symbols
    })
}

unsafe fn load_symbol<T>(handle: *mut c_void, name: &str) -> Option<T> {
    let c_name = c_str(name)?;
    let symbol = dlsym(handle, c_name.as_ptr());
    if symbol.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&symbol))
    }
}

fn c_str(value: &str) -> Option<CString> {
    CString::new(value).ok()
}

#[cfg(test)]
#[path = "../../tests/native-unit/binder_caller.rs"]
mod tests;
