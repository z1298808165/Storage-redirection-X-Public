mod caller;
mod context;
mod diagnostic;
mod entries;
mod fuse_fix;
mod jni_query;
mod media_fuse;
mod monitor;
mod ops;
mod path;
mod reload;
mod runtime;
pub mod stats;
mod util;

use crate::platform;
use std::ffi::CStr;

pub(crate) use caller::{
    capture_java_binder_caller, enter_java_caller_scope, exit_java_caller_scope,
};
pub(crate) use context::{
    enter_explicit_caller_decision, enter_path_owner_inference_disabled,
    enter_provider_passthrough, exit_provider_passthrough, get_binder_saved_caller_package,
    is_explicit_caller_decision_active, is_path_owner_inference_disabled,
    is_provider_passthrough_active,
};
pub(crate) use jni_query::{
    is_redirect_enabled_for_caller_uid, resolve_download_media_placeholder_path_for_caller,
    resolve_open_storage_path_for_caller, rewrite_cursor_storage_path_for_caller,
    rewrite_media_store_bucket_id_for_caller, rewrite_media_store_storage_path_for_caller,
    should_hide_cursor_storage_path_for_caller, storage_path_exists_by_syscall,
};
pub(crate) use media_fuse::{
    record_read_only_fuse_operation, should_allow_public_mapping_target_access,
};
pub use stats::InterceptHub;

pub(crate) fn refresh_runtime_config_from_settings() {
    let hub = InterceptHub::instance();
    hub.refresh_monitor_runtime_config();
    fuse_fix::refresh_runtime_config();
    let package_name = hub.get_package_name();
    if hub.is_runtime_hook_initialized()
        && !package_name.is_empty()
        && platform::is_boot_completed()
    {
        fuse_fix::install_if_enabled(&package_name);
    }
}

pub(crate) fn refresh_runtime_config_after_disk_change() {
    reload::force_after_disk_change();
    refresh_runtime_config_from_settings();
}

pub(crate) fn refresh_runtime_config_throttled() {
    if context::ReentryGuard::is_reentrant() {
        return;
    }

    let _guard = context::ReentryGuard::enter();
    if matches!(
        reload::poll_or_check_throttled(),
        reload::RuntimeReloadCheck::Skipped
    ) {
        return;
    }
    refresh_runtime_config_from_settings();
}

fn refresh_runtime_config_from_native_reentrant() {
    let did_reload = reload::poll_or_check_throttled().did_reload();

    if did_reload {
        log::debug!("runtime config refreshed from native reentrant path");
    }
    InterceptHub::instance().refresh_monitor_runtime_config();
    fuse_fix::sync_runtime_enabled_from_settings();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn srx_refresh_runtime_config_from_native() {
    if context::ReentryGuard::is_reentrant() {
        refresh_runtime_config_from_native_reentrant();
        return;
    }
    refresh_runtime_config_throttled();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn srx_should_allow_fuse_private_owner_sqlite_access(
    path: *const libc::c_char,
    caller_uid: u32,
) -> bool {
    if path.is_null() {
        return false;
    }

    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    media_fuse::should_allow_private_owner_sqlite_access(&path, caller_uid as i32)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn srx_should_allow_fuse_public_mapping_access(
    path: *const libc::c_char,
    caller_uid: u32,
) -> bool {
    if path.is_null() {
        return false;
    }

    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    media_fuse::should_allow_public_mapping_target_access(&path, caller_uid as i32)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn srx_should_force_fuse_userspace_private_owner_sqlite(
    path: *const libc::c_char,
) -> bool {
    if path.is_null() {
        return false;
    }

    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    media_fuse::should_force_userspace_for_private_owner_sqlite_path(&path)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn srx_prepare_fuse_private_owner_sqlite_sidecar(
    path: *const libc::c_char,
) -> bool {
    if path.is_null() {
        return false;
    }

    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    media_fuse::prepare_private_owner_sqlite_sidecar(&path)
}

pub fn install_fuse_fix_if_enabled(package_name: &str) {
    fuse_fix::install_if_enabled(package_name);
}
