use super::runtime;
use crate::config::SettingsHub;
use crate::platform::elf_img::ElfImg;
use crate::redirect::policy;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const LIB_FUSE_JNI: &str = "libfuse_jni.so";
const EQUALS_IGNORE_CASE_NDK_SYMBOL: &str =
    "_ZN7android4base16EqualsIgnoreCaseENSt6__ndk117basic_string_viewIcNS1_11char_traitsIcEEEES5_";
const EQUALS_IGNORE_CASE_STD_SYMBOL: &str =
    "_ZN7android4base16EqualsIgnoreCaseENSt3__117basic_string_viewIcNS1_11char_traitsIcEEEES5_";

const IS_APP_ACCESSIBLE_PATH_SYMBOLS: &[&str] = &[
    "_ZN13mediaprovider4fuseL22is_app_accessible_pathEP4fuseRKNSt6__ndk112basic_stringIcNS3_11char_traitsIcEENS3_9allocatorIcEEEEj",
    "_ZN13mediaprovider4fuseL22is_app_accessible_pathEP4fuseRKNSt3__112basic_stringIcNS3_11char_traitsIcEENS3_9allocatorIcEEEEj",
];
const IS_PACKAGE_OWNED_PATH_SYMBOLS: &[&str] = &[
    "_ZL21is_package_owned_pathRKNSt6__ndk112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEES7_",
    "_ZL21is_package_owned_pathRKNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEES7_",
];
const IS_BPF_BACKING_PATH_SYMBOLS: &[&str] = &[
    "_ZL19is_bpf_backing_pathRKNSt6__ndk112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEE",
    "_ZL19is_bpf_backing_pathRKNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEE",
];
const SHOULD_OPEN_WITH_FUSE_SYMBOLS: &[&str] = &[
    "_ZN13mediaprovider4fuse10FuseDaemon18ShouldOpenWithFuseEibRKNSt6__ndk112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE",
    "_ZN13mediaprovider4fuse10FuseDaemon18ShouldOpenWithFuseEibRKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE",
];
static INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static COMPARE_HOOKS_REGISTERED: AtomicBool = AtomicBool::new(false);
static COMPARE_HOOKS_REFRESHED: AtomicBool = AtomicBool::new(false);
static COMPARE_HOOKS_PATCHED_DIRECT: AtomicBool = AtomicBool::new(false);
static DISABLED_LOGGED: AtomicBool = AtomicBool::new(false);
static LAST_RUNTIME_ENABLED: AtomicBool = AtomicBool::new(true);
static REENABLE_RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static TARGET_ENABLED: AtomicBool = AtomicBool::new(false);
static TARGET_PACKAGE: RwLock<String> = RwLock::new(String::new());
static COMPARE_PATCH_LOCK: Mutex<()> = Mutex::new(());
static RETRY_COUNT: AtomicU32 = AtomicU32::new(0);
const MAX_RETRY_COUNT: u32 = 16;

unsafe extern "C" {
    fn srx_fuse_fix_install(
        is_app_accessible_path: *mut c_void,
        is_package_owned_path: *mut c_void,
        is_bpf_backing_path: *mut c_void,
        should_open_with_fuse: *mut c_void,
        reply_open_slot: *mut c_void,
        reply_create_slot: *mut c_void,
        passthrough_enable_slot: *mut c_void,
        passthrough_open_slot: *mut c_void,
    ) -> i32;
    fn srx_fuse_fix_is_installed() -> bool;
    fn srx_fuse_fix_set_enabled(enabled: bool);
    fn srx_fuse_fix_strcasecmp(lhs: *const libc::c_char, rhs: *const libc::c_char) -> libc::c_int;
    fn srx_fuse_fix_equals_ignore_case(lhs: usize, rhs: usize) -> bool;
}

pub fn install_if_enabled(package_name: &str) {
    let uid = unsafe { libc::getuid() as i32 };
    if !policy::is_media_provider_package(package_name) && !policy::is_shared_uid_process(uid) {
        return;
    }
    mark_target_package(package_name);
    install_target_if_enabled();
}

pub fn retry_if_target_enabled() {
    if !TARGET_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    install_target_if_enabled();
}

pub fn refresh_runtime_config() {
    let enabled = sync_runtime_enabled_from_settings();
    if TARGET_ENABLED.load(Ordering::Relaxed) && !unsafe { srx_fuse_fix_is_installed() } {
        log::info!(
            "media fuse compatibility runtime refresh enabled={}, retry install for active target",
            enabled
        );
        install_target_if_enabled();
    }
}

pub(super) fn sync_runtime_enabled_from_settings() -> bool {
    let enabled = SettingsHub::instance().is_fuse_fix_enabled();
    unsafe {
        srx_fuse_fix_set_enabled(enabled);
    }
    let installed = unsafe { srx_fuse_fix_is_installed() };
    let previous_enabled = LAST_RUNTIME_ENABLED.swap(enabled, Ordering::Relaxed);
    if previous_enabled != enabled {
        let package_name = TARGET_PACKAGE
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        log::info!(
            "fuse fix runtime enabled={} installed={} target={} pkg={}",
            enabled,
            installed,
            TARGET_ENABLED.load(Ordering::Relaxed),
            package_name
        );
        if enabled && !previous_enabled {
            restart_media_provider_after_reenable(&package_name, installed);
        }
    }
    if enabled {
        DISABLED_LOGGED.store(false, Ordering::Relaxed);
    }
    enabled
}

fn mark_target_package(package_name: &str) {
    TARGET_ENABLED.store(true, Ordering::Relaxed);
    let mut target = TARGET_PACKAGE
        .write()
        .unwrap_or_else(|err| err.into_inner());
    if target.is_empty() {
        *target = package_name.to_string();
    }
}

fn restart_media_provider_after_reenable(package_name: &str, installed: bool) {
    if !policy::is_media_provider_package(package_name) {
        return;
    }
    if REENABLE_RESTART_REQUESTED.swap(true, Ordering::AcqRel) {
        return;
    }
    log::warn!(
        "fuse fix re-enabled after disabled, restart media provider to drop fuse dentry cache installed={}",
        installed
    );
    unsafe {
        libc::kill(libc::getpid(), libc::SIGKILL);
    }
}

fn install_target_if_enabled() {
    let enabled = sync_runtime_enabled_from_settings();
    if unsafe { srx_fuse_fix_is_installed() } {
        INSTALL_ATTEMPTED.store(true, Ordering::Relaxed);
        return;
    }
    if should_skip_native_fuse_fix_for_platform(crate::platform::android_api_level()) {
        if !INSTALL_ATTEMPTED.swap(true, Ordering::AcqRel) {
            log::warn!(
                "fuse fix native hooks skipped on Android 14 x86_64; Java media mutation remains active"
            );
        }
        return;
    }
    if !enabled {
        if !DISABLED_LOGGED.swap(true, Ordering::Relaxed) {
            log::info!(
                "zero-width fuse fix disabled; media fuse compatibility hooks remain installable installed={}",
                unsafe { srx_fuse_fix_is_installed() }
            );
        }
    }
    register_compare_hooks_once();
    if RETRY_COUNT.load(Ordering::Relaxed) >= MAX_RETRY_COUNT {
        return;
    }
    if INSTALL_ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }

    let Some(elf) = ElfImg::load(LIB_FUSE_JNI) else {
        log::warn!("fuse fix skip: {} not loaded", LIB_FUSE_JNI);
        INSTALL_ATTEMPTED.store(false, Ordering::Release);
        RETRY_COUNT.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let is_app_accessible_path = find_any(&elf, IS_APP_ACCESSIBLE_PATH_SYMBOLS);
    let is_package_owned_path = find_any(&elf, IS_PACKAGE_OWNED_PATH_SYMBOLS);
    let is_bpf_backing_path = find_any(&elf, IS_BPF_BACKING_PATH_SYMBOLS);
    let should_open_with_fuse = find_any(&elf, SHOULD_OPEN_WITH_FUSE_SYMBOLS);
    let reply_open_slot = find_first_plt_slot(&elf, "fuse_reply_open");
    let reply_create_slot = find_first_plt_slot(&elf, "fuse_reply_create");
    let passthrough_enable_slot = find_first_plt_slot(&elf, "fuse_passthrough_enable");
    let passthrough_open_slot = find_first_plt_slot(&elf, "fuse_passthrough_open");

    let installed = unsafe {
        srx_fuse_fix_install(
            is_app_accessible_path,
            is_package_owned_path,
            is_bpf_backing_path,
            should_open_with_fuse,
            reply_open_slot,
            reply_create_slot,
            passthrough_enable_slot,
            passthrough_open_slot,
        )
    };
    if installed > 0 {
        let package_name = TARGET_PACKAGE
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        log::info!(
            "fuse fix installed pkg={} hooks={} app_accessible={} package_owned={} bpf_backing={} should_open_with_fuse={} reply_open_slot={} reply_create_slot={} passthrough_enable_slot={} passthrough_open_slot={}",
            package_name,
            installed,
            !is_app_accessible_path.is_null(),
            !is_package_owned_path.is_null(),
            !is_bpf_backing_path.is_null(),
            !should_open_with_fuse.is_null(),
            !reply_open_slot.is_null(),
            !reply_create_slot.is_null(),
            !passthrough_enable_slot.is_null(),
            !passthrough_open_slot.is_null()
        );
    } else {
        INSTALL_ATTEMPTED.store(false, Ordering::Release);
        RETRY_COUNT.fetch_add(1, Ordering::Relaxed);
        log::warn!(
            "fuse fix install failed app_accessible={} package_owned={} bpf_backing={} should_open_with_fuse={} reply_open_slot={} reply_create_slot={} passthrough_enable_slot={} passthrough_open_slot={} rc={}",
            !is_app_accessible_path.is_null(),
            !is_package_owned_path.is_null(),
            !is_bpf_backing_path.is_null(),
            !should_open_with_fuse.is_null(),
            !reply_open_slot.is_null(),
            !reply_create_slot.is_null(),
            !passthrough_enable_slot.is_null(),
            !passthrough_open_slot.is_null(),
            installed
        );
    }
}

fn find_any(elf: &ElfImg, symbols: &[&str]) -> *mut c_void {
    for symbol in symbols {
        let addr = elf.find(symbol);
        if !addr.is_null() {
            return addr;
        }
    }
    std::ptr::null_mut()
}

fn find_first_plt_slot(elf: &ElfImg, symbol: &str) -> *mut c_void {
    elf.find_plt_slots(symbol)
        .into_iter()
        .next()
        .map(|slot| slot as *mut c_void)
        .unwrap_or(std::ptr::null_mut())
}

fn should_skip_native_fuse_fix_for_platform(api_level: i32) -> bool {
    cfg!(target_arch = "x86_64") && api_level == 34
}

fn register_compare_hooks_once() {
    if COMPARE_HOOKS_REGISTERED.load(Ordering::Acquire) {
        refresh_compare_hooks();
        return;
    }

    if COMPARE_HOOKS_REGISTERED.swap(true, Ordering::AcqRel) {
        refresh_compare_hooks();
        return;
    }

    let errno = srx_hook::init(srx_hook::HookMode::Automatic, false);
    if !errno.is_ok() {
        COMPARE_HOOKS_REGISTERED.store(false, Ordering::Release);
        log::warn!("fuse fix compare hook init failed err={:?}", errno);
        return;
    }

    let strcasecmp_stub = srx_hook::hook_single(
        LIB_FUSE_JNI,
        None,
        "strcasecmp",
        srx_fuse_fix_strcasecmp as *mut c_void,
        None,
        std::ptr::null_mut(),
    );
    let equals_ndk_stub = srx_hook::hook_single(
        LIB_FUSE_JNI,
        None,
        EQUALS_IGNORE_CASE_NDK_SYMBOL,
        srx_fuse_fix_equals_ignore_case as *mut c_void,
        None,
        std::ptr::null_mut(),
    );
    let equals_std_stub = srx_hook::hook_single(
        LIB_FUSE_JNI,
        None,
        EQUALS_IGNORE_CASE_STD_SYMBOL,
        srx_fuse_fix_equals_ignore_case as *mut c_void,
        None,
        std::ptr::null_mut(),
    );

    if strcasecmp_stub.is_none() && equals_ndk_stub.is_none() && equals_std_stub.is_none() {
        COMPARE_HOOKS_REGISTERED.store(false, Ordering::Release);
        log::warn!("fuse fix compare hook register failed");
        return;
    }

    refresh_compare_hooks();

    log::info!(
        "fuse fix compare hooks registered strcasecmp={} equals_ndk={} equals_std={}",
        strcasecmp_stub.is_some(),
        equals_ndk_stub.is_some(),
        equals_std_stub.is_some()
    );
}

fn refresh_compare_hooks() {
    if COMPARE_HOOKS_REFRESHED.load(Ordering::Acquire) {
        return;
    }

    let (refresh_errno, refresh_errors) = srx_hook::refresh();
    for err in &refresh_errors {
        log::warn!(
            "fuse fix compare hook resolve failed path={} err={:?}",
            err.module_path,
            err.errno
        );
    }
    if !refresh_errno.is_ok() {
        log::warn!(
            "fuse fix compare hook refresh failed err={:?}",
            refresh_errno
        );
        return;
    }

    COMPARE_HOOKS_REFRESHED.store(true, Ordering::Release);
    log::info!("fuse fix compare hooks refreshed");
    patch_compare_hooks_direct_once();
}

fn patch_compare_hooks_direct_once() {
    if COMPARE_HOOKS_PATCHED_DIRECT.load(Ordering::Acquire) {
        return;
    }
    let _guard = COMPARE_PATCH_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if COMPARE_HOOKS_PATCHED_DIRECT.load(Ordering::Acquire) {
        return;
    }
    let Some(elf) = ElfImg::load(LIB_FUSE_JNI) else {
        log::warn!("fuse fix compare direct skip: {} not loaded", LIB_FUSE_JNI);
        return;
    };
    let strcasecmp_count =
        patch_plt_slots(&elf, "strcasecmp", srx_fuse_fix_strcasecmp as *mut c_void);
    let equals_ndk_count = patch_plt_slots(
        &elf,
        EQUALS_IGNORE_CASE_NDK_SYMBOL,
        srx_fuse_fix_equals_ignore_case as *mut c_void,
    );
    let equals_std_count = patch_plt_slots(
        &elf,
        EQUALS_IGNORE_CASE_STD_SYMBOL,
        srx_fuse_fix_equals_ignore_case as *mut c_void,
    );
    if strcasecmp_count == 0 && equals_ndk_count == 0 && equals_std_count == 0 {
        log::warn!("fuse fix compare direct patch found no slots");
        return;
    }
    COMPARE_HOOKS_PATCHED_DIRECT.store(true, Ordering::Release);
    log::info!(
        "fuse fix compare direct patched strcasecmp={} equals_ndk={} equals_std={}",
        strcasecmp_count,
        equals_ndk_count,
        equals_std_count
    );
}

fn patch_plt_slots(elf: &ElfImg, symbol: &str, replacement: *mut c_void) -> usize {
    let mut patched = 0usize;
    for slot in elf.find_plt_slots(symbol) {
        if patch_plt_slot(slot, replacement as usize) {
            patched += 1;
        }
    }
    patched
}

fn patch_plt_slot(slot: usize, replacement: usize) -> bool {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        log::warn!("fuse fix compare direct invalid page size");
        return false;
    }
    let page_size = page_size as usize;
    let page_start = slot & !(page_size - 1);
    if unsafe {
        libc::mprotect(
            page_start as *mut libc::c_void,
            page_size,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    } != 0
    {
        log::warn!(
            "fuse fix compare direct mprotect rw failed slot={:#x} errno={}",
            slot,
            runtime::current_errno()
        );
        return false;
    }

    unsafe {
        std::ptr::write_volatile(slot as *mut usize, replacement);
    }

    if unsafe { libc::mprotect(page_start as *mut libc::c_void, page_size, libc::PROT_READ) } != 0 {
        log::warn!(
            "fuse fix compare direct mprotect ro failed slot={:#x} errno={}",
            slot,
            runtime::current_errno()
        );
    }
    unsafe {
        crate::platform::linker::__clear_cache(
            slot as *mut c_void,
            (slot + size_of::<usize>()) as *mut c_void,
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_14_x86_64_skips_native_fuse_fix() {
        assert_eq!(
            should_skip_native_fuse_fix_for_platform(34),
            cfg!(target_arch = "x86_64")
        );
    }

    #[test]
    fn other_android_versions_keep_native_fuse_fix_available() {
        assert!(!should_skip_native_fuse_fix_for_platform(33));
        assert!(!should_skip_native_fuse_fix_for_platform(35));
    }
}
