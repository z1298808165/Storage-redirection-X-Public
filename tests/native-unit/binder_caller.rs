use super::CallerBinderSymbols;
use std::ffi::c_void;

unsafe extern "C" fn no_state() -> *mut c_void {
    std::ptr::null_mut()
}

unsafe extern "C" fn existing_state() -> *mut c_void {
    std::ptr::dangling_mut::<c_void>()
}

unsafe extern "C" fn calling_uid(state: *mut c_void) -> u32 {
    assert_eq!(state, std::ptr::dangling_mut::<c_void>());
    10123
}

unsafe extern "C" fn calling_pid(state: *mut c_void) -> i32 {
    assert_eq!(state, std::ptr::dangling_mut::<c_void>());
    42
}

#[test]
fn missing_thread_state_never_calls_binder_getters() {
    for state in [
        None,
        Some(no_state as unsafe extern "C" fn() -> *mut c_void),
    ] {
        let symbols = CallerBinderSymbols {
            binder_self_or_null: state,
            binder_get_calling_uid: Some(calling_uid),
            binder_get_calling_pid: Some(calling_pid),
        };
        assert_eq!(symbols.calling_uid(), None);
        assert_eq!(symbols.calling_pid(), None);
    }
}

#[test]
fn existing_thread_state_keeps_callers_and_missing_getters_unknown() {
    let mut symbols = CallerBinderSymbols {
        binder_self_or_null: Some(existing_state),
        binder_get_calling_uid: Some(calling_uid),
        binder_get_calling_pid: Some(calling_pid),
    };
    assert_eq!(symbols.calling_uid(), Some(10123));
    assert_eq!(symbols.calling_pid(), Some(42));
    symbols.binder_get_calling_uid = None;
    symbols.binder_get_calling_pid = None;
    assert_eq!(symbols.calling_uid(), None);
    assert_eq!(symbols.calling_pid(), None);
}
