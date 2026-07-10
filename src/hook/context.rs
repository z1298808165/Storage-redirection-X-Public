use crate::platform::paths::monotonic_ms;
use std::cell::{Cell, RefCell};

thread_local! {
    static CALLER_PACKAGE: RefCell<String> = const { RefCell::new(String::new()) };
    static CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static CALLER_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static CALLER_FROM_EXTERNAL_SIGNAL: Cell<bool> = const { Cell::new(false) };
    static CALLER_SCOPE_STACK: RefCell<Vec<(String, i32)>> = const { RefCell::new(Vec::new()) };
    static FUSE_CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static FUSE_CALLER_UID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static FUSE_CALLER_PID: Cell<i32> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_UID: Cell<i32> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_UID_TS_MS: Cell<i64> = const { Cell::new(-1) };
    static BINDER_SAVED_CALLER_PACKAGE: RefCell<String> = const { RefCell::new(String::new()) };
    static BINDER_IDENTITY_CLEARED: Cell<bool> = const { Cell::new(false) };
    static REENTRY_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PROVIDER_PASSTHROUGH_DEPTH: Cell<u32> = const { Cell::new(0) };
    static EXPLICIT_CALLER_DECISION_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PATH_OWNER_INFERENCE_DISABLED_DEPTH: Cell<u32> = const { Cell::new(0) };
}

pub struct ReentryGuard;

impl ReentryGuard {
    pub fn enter() -> Self {
        REENTRY_DEPTH.with(|depth| depth.set(depth.get() + 1));
        ReentryGuard
    }

    pub fn is_reentrant() -> bool {
        REENTRY_DEPTH.with(|depth| depth.get() > 0)
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        REENTRY_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub struct ExplicitCallerDecisionGuard;

pub fn enter_explicit_caller_decision() -> ExplicitCallerDecisionGuard {
    EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    ExplicitCallerDecisionGuard
}

impl Drop for ExplicitCallerDecisionGuard {
    fn drop(&mut self) {
        EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub fn is_explicit_caller_decision_active() -> bool {
    EXPLICIT_CALLER_DECISION_DEPTH.with(|depth| depth.get() > 0)
}

pub struct PathOwnerInferenceGuard;

pub fn enter_path_owner_inference_disabled() -> PathOwnerInferenceGuard {
    PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    PathOwnerInferenceGuard
}

impl Drop for PathOwnerInferenceGuard {
    fn drop(&mut self) {
        PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

pub fn is_path_owner_inference_disabled() -> bool {
    PATH_OWNER_INFERENCE_DISABLED_DEPTH.with(|depth| depth.get() > 0)
}

pub fn set_current_caller_package(name: &str) {
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = name.to_string());
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
}

pub fn get_current_caller_package() -> String {
    CALLER_PACKAGE.with(|pkg| pkg.borrow().clone())
}

pub fn set_current_caller_uid(uid: i32) {
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
}

pub fn get_current_caller_uid() -> i32 {
    CALLER_UID.with(|cell| cell.get())
}

pub fn get_current_caller_age_ms() -> i64 {
    let now = monotonic_ms();
    CALLER_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

pub fn clear_current_caller() {
    CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
    CALLER_UID.with(|cell| cell.set(-1));
    CALLER_TS_MS.with(|cell| cell.set(-1));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
}

pub fn set_current_caller_from_external_signal(package_name: &str, uid: i32) {
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(true));
}

pub fn is_current_caller_from_external_signal() -> bool {
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.get())
}

pub fn push_current_caller_scope(package_name: &str, uid: i32) {
    let previous_package = get_current_caller_package();
    let previous_uid = get_current_caller_uid();
    CALLER_SCOPE_STACK.with(|stack| {
        stack.borrow_mut().push((previous_package, previous_uid));
    });
    CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    CALLER_UID.with(|cell| cell.set(uid));
    CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
    CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
}

pub fn pop_current_caller_scope() {
    let previous = CALLER_SCOPE_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some((package_name, uid)) = previous {
        CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name);
        CALLER_UID.with(|cell| cell.set(uid));
        if uid >= 0 {
            CALLER_TS_MS.with(|cell| cell.set(monotonic_ms()));
        } else {
            CALLER_TS_MS.with(|cell| cell.set(-1));
        }
        CALLER_FROM_EXTERNAL_SIGNAL.with(|cell| cell.set(false));
    } else {
        clear_current_caller();
    }
}

pub fn is_current_caller_scope_active() -> bool {
    CALLER_SCOPE_STACK.with(|stack| !stack.borrow().is_empty())
}

pub fn set_fuse_caller_uid(uid: i32) {
    FUSE_CALLER_UID.with(|cell| cell.set(uid));
    FUSE_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
}

pub fn set_fuse_caller_pid(pid: i32) {
    FUSE_CALLER_PID.with(|cell| cell.set(pid));
}

pub fn get_fuse_caller_uid() -> i32 {
    FUSE_CALLER_UID.with(|cell| cell.get())
}

pub fn get_fuse_caller_pid() -> i32 {
    FUSE_CALLER_PID.with(|cell| cell.get())
}

// 未缓存返回 -1
pub fn get_fuse_caller_uid_age_ms() -> i64 {
    let now = monotonic_ms();
    FUSE_CALLER_UID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

// 线程复用前必清，避免残留 UID/PID 串到后续请求
pub fn clear_fuse_caller_uid() {
    FUSE_CALLER_UID.with(|cell| cell.set(-1));
    FUSE_CALLER_UID_TS_MS.with(|cell| cell.set(-1));
    FUSE_CALLER_PID.with(|cell| cell.set(-1));
}

// 在 clearCallingIdentity 之前调用，保存真实调用方 UID
pub fn set_binder_saved_caller_uid(uid: i32) {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.set(uid));
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
}

pub fn set_binder_saved_caller_package(package_name: &str) {
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| *pkg.borrow_mut() = package_name.to_string());
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(monotonic_ms()));
}

pub fn get_binder_saved_caller_uid() -> i32 {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.get())
}

pub fn get_binder_saved_caller_package() -> String {
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow().clone())
}

// 未缓存返回 -1
pub fn get_binder_saved_caller_uid_age_ms() -> i64 {
    let now = monotonic_ms();
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| {
        let ts = cell.get();
        if ts < 0 || now < ts {
            return -1;
        }
        now - ts
    })
}

// 跨请求前必清，避免误用历史 UID
pub fn clear_binder_saved_caller_uid() {
    BINDER_SAVED_CALLER_UID.with(|cell| cell.set(-1));
    BINDER_SAVED_CALLER_UID_TS_MS.with(|cell| cell.set(-1));
    BINDER_SAVED_CALLER_PACKAGE.with(|pkg| pkg.borrow_mut().clear());
}

pub fn enter_provider_passthrough() {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
}

pub fn exit_provider_passthrough() {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| {
        let current = depth.get();
        if current <= 1 {
            depth.set(0);
            if !is_current_caller_scope_active() {
                clear_current_caller();
                clear_binder_saved_caller_uid();
                set_binder_identity_cleared(false);
            }
        } else {
            depth.set(current - 1);
        }
    });
}

pub fn is_provider_passthrough_active() -> bool {
    PROVIDER_PASSTHROUGH_DEPTH.with(|depth| depth.get() > 0)
}

pub fn set_binder_identity_cleared(cleared: bool) {
    BINDER_IDENTITY_CLEARED.with(|cell| cell.set(cleared));
}

// 处于 clearCallingIdentity 后、restoreCallingIdentity 前的区间
pub fn is_binder_identity_cleared() -> bool {
    BINDER_IDENTITY_CLEARED.with(|cell| cell.get())
}
