use super::context;
use super::entries::{
    HookProfile, HookProfileSet, build_hook_entries, count_hooks_for_profile, is_hook_enabled,
};
use crate::config::SettingsHub;
use crate::monitor::AuditTrail;
use crate::redirect::policy;
use std::ffi::{CStr, c_char, c_void};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};

const STATS_TAG: &str = "Stats";
const SELF_MODULE_PATH: &str = "libsrx_core.so";
const SQLITE_MODULE_BASENAME: &str = "libsqlite.so";
const HOOK_SENSITIVE_RENDER_MODULES: &[&str] = &[
    "libhwui.so",
    "libvulkan.so",
    "vulkan.adreno.so",
    "vulkan.mali.so",
    "vulkan.powervr.so",
    "libllvm-glnext.so",
    "libllvm-qgl.so",
    "libEGL_adreno.so",
    "libGLESv1_CM_adreno.so",
    "libGLESv2_adreno.so",
    "libGLES_mali.so",
    "libEGL_POWERVR_ROGUE.so",
    "libGLESv1_CM_POWERVR_ROGUE.so",
    "libGLESv2_POWERVR_ROGUE.so",
    "libadreno_utils.so",
    "libgsl.so",
];
const FLUSH_THRESHOLD: i32 = 32;
const FLUSH_INTERVAL_MS: i64 = 2000;
const LATE_HOOK_REFRESH_INTERVAL_MS: i64 = 1000;
const JIT_CACHE_MEMFD_PREFIX: &str = "/memfd:jit-cache";

pub struct InterceptHub {
    package_name: RwLock<String>,
    is_initialized: AtomicBool,
    is_hooks_installed: AtomicBool,
    is_fuse_fix_only: AtomicBool,
    is_boot_lite: AtomicBool,
    is_app_write_only: AtomicBool,
    is_monitor_only: AtomicBool,
    is_monitor_enabled: AtomicBool,
    stats: AtomicStats,
    pending_redirect_count: AtomicI32,
    last_flush_ms: AtomicI64,
    last_late_hook_refresh_ms: AtomicI64,
}

#[derive(Default)]
struct AtomicStats {
    open_calls: AtomicU64,
    openat_calls: AtomicU64,
    stat_calls: AtomicU64,
    access_calls: AtomicU64,
    mkdir_calls: AtomicU64,
    unlink_calls: AtomicU64,
    rename_calls: AtomicU64,
    opendir_calls: AtomicU64,
    readlink_calls: AtomicU64,
    total_redirected: AtomicU64,
}

impl AtomicStats {
    const fn new() -> Self {
        Self {
            open_calls: AtomicU64::new(0),
            openat_calls: AtomicU64::new(0),
            stat_calls: AtomicU64::new(0),
            access_calls: AtomicU64::new(0),
            mkdir_calls: AtomicU64::new(0),
            unlink_calls: AtomicU64::new(0),
            rename_calls: AtomicU64::new(0),
            opendir_calls: AtomicU64::new(0),
            readlink_calls: AtomicU64::new(0),
            total_redirected: AtomicU64::new(0),
        }
    }
}

impl InterceptHub {
    pub fn instance() -> &'static InterceptHub {
        &INTERCEPT_HUB
    }

    pub fn init(
        &self,
        package_name: &str,
        is_monitor_only: bool,
        is_monitor_enabled: bool,
    ) -> bool {
        if self.is_initialized.load(Ordering::Relaxed) {
            return true;
        }

        if !package_name.is_empty() {
            let mut name = self
                .package_name
                .write()
                .unwrap_or_else(|err| err.into_inner());
            *name = package_name.to_string();
        }

        self.is_monitor_only
            .store(is_monitor_only, Ordering::Relaxed);
        self.is_monitor_enabled
            .store(is_monitor_enabled, Ordering::Relaxed);
        self.is_app_write_only.store(false, Ordering::Relaxed);

        log::info!(
            "hook manager init pkg={} redirect={} monitor={}",
            self.get_package_name(),
            !is_monitor_only,
            is_monitor_enabled
        );

        self.is_initialized.store(true, Ordering::Relaxed);
        true
    }

    pub fn init_media_runtime(&self, package_name: &str, is_monitor_enabled: bool) -> bool {
        if !self.init(package_name, true, is_monitor_enabled) {
            return false;
        }
        self.is_fuse_fix_only.store(true, Ordering::Relaxed);
        log::info!(
            "hook manager media-runtime pkg={} monitor={}",
            self.get_package_name(),
            is_monitor_enabled
        );
        true
    }

    pub fn init_app_write_redirect(&self, package_name: &str, is_monitor_enabled: bool) -> bool {
        if !self.init(package_name, false, is_monitor_enabled) {
            return false;
        }
        self.is_app_write_only.store(true, Ordering::Relaxed);
        log::info!(
            "hook manager app-write pkg={} monitor={}",
            self.get_package_name(),
            is_monitor_enabled
        );
        true
    }

    pub fn init_boot_lite(&self, package_name: &str, is_monitor_enabled: bool) -> bool {
        if !self.init(package_name, false, is_monitor_enabled) {
            return false;
        }
        self.is_boot_lite.store(true, Ordering::Relaxed);
        log::info!("hook manager boot-lite pkg={}", self.get_package_name());
        true
    }

    pub fn is_monitor_only(&self) -> bool {
        self.is_monitor_only.load(Ordering::Relaxed)
    }

    pub fn is_monitor_enabled(&self) -> bool {
        self.is_monitor_enabled.load(Ordering::Relaxed)
    }

    pub fn is_app_write_only(&self) -> bool {
        self.is_app_write_only.load(Ordering::Relaxed)
    }

    pub fn refresh_monitor_runtime_config(&self) {
        let package_name = self.get_package_name();
        if package_name.is_empty() {
            return;
        }

        let uid = unsafe { libc::getuid() as i32 };
        let should_monitor = SettingsHub::instance().should_monitor(&package_name, uid);
        let was_monitoring = self.is_monitor_enabled.load(Ordering::Relaxed);
        if should_monitor && !was_monitoring {
            self.is_monitor_enabled.store(true, Ordering::Relaxed);
        }
        AuditTrail::instance().set_enabled(should_monitor);

        if should_monitor && !was_monitoring {
            let mut monitor_package = package_name.clone();
            if policy::is_shared_uid_process(uid) {
                let shared_uid = policy::get_shared_uid_packages_string(uid);
                if !shared_uid.is_empty() {
                    monitor_package = shared_uid;
                }
            }
            AuditTrail::instance().init(&monitor_package, uid);
            log::info!("monitor runtime enabled pkg={} uid={}", package_name, uid);
        } else if !should_monitor && was_monitoring {
            self.is_monitor_enabled.store(false, Ordering::Relaxed);
            log::info!("monitor runtime disabled pkg={} uid={}", package_name, uid);
        }
    }

    pub fn is_redirect_enabled(&self) -> bool {
        !self.is_monitor_only()
    }

    pub fn get_package_name(&self) -> String {
        self.package_name
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn set_current_caller_package(&self, caller_package: &str) {
        context::set_current_caller_package(caller_package);
    }

    pub fn get_current_caller_package(&self) -> String {
        context::get_current_caller_package()
    }

    pub fn set_current_caller_uid(&self, caller_uid: i32) {
        context::set_current_caller_uid(caller_uid);
    }

    pub fn get_current_caller_uid(&self) -> i32 {
        context::get_current_caller_uid()
    }

    pub fn clear_current_caller(&self) {
        context::clear_current_caller();
    }

    // 由 FUSE 请求头填入，用于跨进程调用方识别
    pub fn get_fuse_caller_uid(&self) -> i32 {
        context::get_fuse_caller_uid()
    }

    pub fn is_hooks_installed(&self) -> bool {
        self.is_hooks_installed.load(Ordering::Relaxed)
    }

    pub fn install(&self) -> bool {
        if self.is_hooks_installed() {
            log::warn!("hook already installed");
            return true;
        }

        log::info!("hook install start");
        let errno = srx_hook::init(srx_hook::HookMode::Automatic, false);
        if !errno.is_ok() {
            log::warn!("srx_hook init failed err={:?}", errno);
            return false;
        }

        let ignore_errno = srx_hook::add_ignore(SELF_MODULE_PATH);
        if !ignore_errno.is_ok() {
            log::warn!("add_ignore failed err={:?}", ignore_errno);
        }
        let sqlite_ignore_errno = srx_hook::add_ignore(SQLITE_MODULE_BASENAME);
        if !sqlite_ignore_errno.is_ok() {
            log::warn!(
                "add_ignore failed path={} err={:?}",
                SQLITE_MODULE_BASENAME,
                sqlite_ignore_errno
            );
        }
        for module in HOOK_SENSITIVE_RENDER_MODULES {
            let ignore_errno = srx_hook::add_ignore(module);
            if !ignore_errno.is_ok() {
                log::warn!("add_ignore failed path={} err={:?}", module, ignore_errno);
            }
        }

        let is_system_writer = policy::is_system_writer_package(&self.get_package_name());
        let (active_profiles, profile_name) = select_hook_profile(
            is_system_writer,
            self.is_monitor_only(),
            self.is_fuse_fix_only.load(Ordering::Relaxed),
            self.is_boot_lite.load(Ordering::Relaxed),
            self.is_app_write_only.load(Ordering::Relaxed),
        );

        let selected_hook_count = count_hooks_for_profile(active_profiles);
        log::info!(
            "hook profile={} count={}",
            profile_name,
            selected_hook_count
        );

        let entries = build_hook_entries();
        let mut hook_list: Vec<&'static str> = Vec::new();
        let mut optional_missing: Vec<&'static str> = Vec::new();
        let mut required_failed: Vec<&'static str> = Vec::new();

        for entry in entries {
            if !is_hook_enabled(active_profiles, entry.profiles) {
                continue;
            }

            let stub = srx_hook::hook_partial(
                hook_caller_allow_filter,
                std::ptr::null_mut(),
                None,
                entry.symbol,
                entry.new_func,
                None,
                std::ptr::null_mut(),
            );

            if stub.is_none() {
                if entry.is_optional {
                    optional_missing.push(entry.symbol);
                    log::warn!(
                        "optional hook unavailable sym={} profile={} pkg={}",
                        entry.symbol,
                        profile_name,
                        self.get_package_name()
                    );
                    continue;
                }
                required_failed.push(entry.symbol);
                log::warn!(
                    "hook register failed sym={} required=true profile={} pkg={}",
                    entry.symbol,
                    profile_name,
                    self.get_package_name()
                );
                continue;
            }

            hook_list.push(entry.symbol);
        }

        if !required_failed.is_empty() {
            log::warn!(
                "required hooks failed profile={} pkg={} failed_count={} failed=[{}] registered_count={} selected_count={}",
                profile_name,
                self.get_package_name(),
                required_failed.len(),
                required_failed.join(", "),
                hook_list.len(),
                selected_hook_count
            );
            srx_hook::clear();
            return false;
        }

        let hook_list_text = hook_list.join(", ");
        log::info!(
            "hook registered profile={} count={} optional_missing_count={} list={}",
            profile_name,
            hook_list.len(),
            optional_missing.len(),
            hook_list_text
        );
        if !optional_missing.is_empty() {
            log::warn!(
                "optional hooks unavailable profile={} pkg={} missing=[{}]",
                profile_name,
                self.get_package_name(),
                optional_missing.join(", ")
            );
        }

        log::info!(
            "hook refresh start profile={} pkg={} registered_count={}",
            profile_name,
            self.get_package_name(),
            hook_list.len()
        );
        let (refresh_errno, refresh_errors) = srx_hook::refresh();
        for (index, err) in refresh_errors.iter().enumerate() {
            log::warn!(
                "module resolve failed index={} total={} path={} err={:?} profile={} pkg={}",
                index + 1,
                refresh_errors.len(),
                err.module_path,
                err.errno,
                profile_name,
                self.get_package_name()
            );
        }
        if !refresh_errno.is_ok() {
            if is_ignorable_refresh_failure(refresh_errno, &refresh_errors, profile_name) {
                log::warn!(
                    "hook refresh degraded err={:?} errors_count={} profile={} pkg={}, keep hooks with ignored module failures",
                    refresh_errno,
                    refresh_errors.len(),
                    profile_name,
                    self.get_package_name()
                );
                self.is_hooks_installed.store(true, Ordering::Relaxed);
                log::info!("hook install done (degraded)");
                return true;
            }
            log::warn!(
                "hook refresh failed err={:?} errors_count={} profile={} pkg={}",
                refresh_errno,
                refresh_errors.len(),
                profile_name,
                self.get_package_name()
            );
            srx_hook::clear();
            return false;
        }

        self.is_hooks_installed.store(true, Ordering::Relaxed);
        log::info!("hook install done");
        true
    }

    pub fn refresh_hooks_after_late_load(&self, reason: &str) {
        if !self.is_hooks_installed()
            || !self.is_app_write_only.load(Ordering::Relaxed)
            || context::ReentryGuard::is_reentrant()
        {
            return;
        }

        let now_ms = crate::platform::paths::monotonic_ms();
        let last_ms = self.last_late_hook_refresh_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last_ms) < LATE_HOOK_REFRESH_INTERVAL_MS {
            return;
        }
        if self
            .last_late_hook_refresh_ms
            .compare_exchange(last_ms, now_ms, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let _guard = context::ReentryGuard::enter();
        let (refresh_errno, refresh_errors) = srx_hook::refresh();
        if refresh_errno.is_ok() {
            log::info!("hook late refresh ok reason={}", reason);
            return;
        }
        if is_ignorable_refresh_failure(refresh_errno, &refresh_errors, "app-write") {
            log::warn!(
                "hook late refresh degraded reason={} err={:?} errors_count={}",
                reason,
                refresh_errno,
                refresh_errors.len()
            );
            return;
        }
        log::warn!(
            "hook late refresh failed reason={} err={:?} errors_count={}",
            reason,
            refresh_errno,
            refresh_errors.len()
        );
    }

    // 按阈值或时间双触发刷盘，避免频繁写入
    pub fn increment_global_redirect_count(&self) {
        if !crate::logging::is_debug_logging_enabled() {
            return;
        }
        let pending = self.pending_redirect_count.fetch_add(1, Ordering::Relaxed) + 1;
        if pending >= FLUSH_THRESHOLD {
            self.flush_to_global_stats();
            return;
        }

        let now_ms = crate::platform::paths::monotonic_ms();
        let last_flush = self.last_flush_ms.load(Ordering::Relaxed);
        if now_ms - last_flush >= FLUSH_INTERVAL_MS {
            self.flush_to_global_stats();
        }
    }

    pub fn increment_open_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.open_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_openat_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.openat_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_stat_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.stat_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_access_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.access_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_mkdir_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.mkdir_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_unlink_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.unlink_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_rename_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.rename_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_opendir_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.opendir_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_readlink_calls(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.readlink_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn increment_total_redirected(&self) {
        if crate::logging::is_debug_logging_enabled() {
            self.stats.total_redirected.fetch_add(1, Ordering::Relaxed);
        }
    }

    // 通过 logcat 广播，由外部收集器汇总
    fn flush_to_global_stats(&self) {
        if !crate::logging::is_debug_logging_enabled() {
            return;
        }
        let pending = self.pending_redirect_count.swap(0, Ordering::Relaxed);
        if pending <= 0 {
            return;
        }

        self.last_flush_ms
            .store(crate::platform::paths::monotonic_ms(), Ordering::Relaxed);

        // 收集器按 "+N" 解析
        log::info!(target: STATS_TAG, "+{}", pending);
    }
}

fn is_ignorable_refresh_failure(
    refresh_errno: srx_hook::SrxHookErrno,
    refresh_errors: &[srx_hook::RefreshError],
    profile_name: &str,
) -> bool {
    if refresh_errors.is_empty() {
        return false;
    }
    if profile_name == "app-write" {
        if matches!(
            refresh_errno,
            srx_hook::SrxHookErrno::ReadElf | srx_hook::SrxHookErrno::Format
        ) {
            return refresh_errors.iter().all(|err| {
                matches!(
                    err.errno,
                    srx_hook::SrxHookErrno::ReadElf | srx_hook::SrxHookErrno::Format
                )
            });
        }

        return refresh_errno == srx_hook::SrxHookErrno::SetProt
            && refresh_errors.iter().all(|err| {
                err.errno == srx_hook::SrxHookErrno::SetProt
                    && is_deleted_jit_memfd_module(&err.module_path)
            });
    }
    if refresh_errno != srx_hook::SrxHookErrno::Format {
        return false;
    }
    refresh_errors.iter().all(|err| {
        err.errno == srx_hook::SrxHookErrno::Format
            && err.module_path.ends_with(SQLITE_MODULE_BASENAME)
    })
}

fn is_deleted_jit_memfd_module(module_path: &str) -> bool {
    module_path
        .strip_prefix(JIT_CACHE_MEMFD_PREFIX)
        .is_some_and(|suffix| suffix.starts_with(' ') && suffix.contains("(deleted)"))
}

fn select_hook_profile(
    is_system_writer: bool,
    is_monitor_only: bool,
    is_fuse_fix_only: bool,
    is_boot_lite: bool,
    is_app_write_only: bool,
) -> (HookProfileSet, &'static str) {
    if is_app_write_only {
        return (
            HookProfileSet::from_profile(HookProfile::AppWrite),
            "app-write",
        );
    }
    if is_fuse_fix_only && is_monitor_only {
        return (
            HookProfileSet::from_profile(HookProfile::Monitor).with(HookProfile::FuseFix),
            "media-runtime",
        );
    }
    if is_fuse_fix_only {
        return (
            HookProfileSet::from_profile(HookProfile::FuseFix),
            "fuse-fix",
        );
    }
    if is_boot_lite {
        return (
            HookProfileSet::from_profile(HookProfile::SystemWriterBootLite),
            "system-writer-boot-lite",
        );
    }
    if is_system_writer {
        if is_monitor_only {
            return (
                HookProfileSet::from_profile(HookProfile::SystemWriter),
                "system-writer-monitor",
            );
        }
        return (
            HookProfileSet::from_profile(HookProfile::SystemWriter),
            "system-writer",
        );
    }
    if is_monitor_only {
        return (
            HookProfileSet::from_profile(HookProfile::Monitor),
            "monitor",
        );
    }
    (HookProfileSet::from_profile(HookProfile::Full), "full")
}

static INTERCEPT_HUB: InterceptHub = InterceptHub {
    package_name: RwLock::new(String::new()),
    is_initialized: AtomicBool::new(false),
    is_hooks_installed: AtomicBool::new(false),
    is_fuse_fix_only: AtomicBool::new(false),
    is_boot_lite: AtomicBool::new(false),
    is_app_write_only: AtomicBool::new(false),
    is_monitor_only: AtomicBool::new(false),
    is_monitor_enabled: AtomicBool::new(false),
    stats: AtomicStats::new(),
    pending_redirect_count: AtomicI32::new(0),
    last_flush_ms: AtomicI64::new(0),
    last_late_hook_refresh_ms: AtomicI64::new(0),
};

unsafe extern "C" fn hook_caller_allow_filter(
    caller_path_name: *const c_char,
    _arg: *mut c_void,
) -> bool {
    if caller_path_name.is_null() {
        return false;
    }
    let path = unsafe { CStr::from_ptr(caller_path_name) }.to_string_lossy();
    should_hook_caller_module(&path)
}

fn should_hook_caller_module(pathname: &str) -> bool {
    if pathname.is_empty() || pathname.starts_with('[') {
        return false;
    }

    let lower = pathname.to_ascii_lowercase();
    if lower.ends_with("libsrx_core.so") || lower.ends_with("libsrx_hook.so") {
        return false;
    }
    if lower.ends_with("libsqlite.so") {
        return false;
    }

    !is_hook_sensitive_render_module(&lower)
}

fn is_hook_sensitive_render_module(lower_pathname: &str) -> bool {
    let basename = lower_pathname.rsplit('/').next().unwrap_or(lower_pathname);
    if basename == "libhwui.so"
        || basename == "libvulkan.so"
        || (basename.starts_with("vulkan.") && basename.ends_with(".so"))
        || basename.starts_with("libegl_")
        || basename.starts_with("libgles")
        || basename.contains("adreno")
        || basename.contains("mali")
        || basename.contains("powervr")
        || basename.contains("libllvm-gl")
        || basename.contains("libllvm-qgl")
    {
        return true;
    }

    lower_pathname.contains("/vendor/")
        && (lower_pathname.contains("/egl/")
            || lower_pathname.contains("/hw/vulkan.")
            || lower_pathname.contains("/libllvm-"))
}

#[cfg(test)]
mod refresh_failure_tests {
    use super::*;

    fn refresh_error(module_path: &str, errno: srx_hook::SrxHookErrno) -> srx_hook::RefreshError {
        srx_hook::RefreshError {
            module_path: module_path.to_string(),
            errno,
        }
    }

    #[test]
    fn app_write_ignores_deleted_jit_memfd_setprot_failures() {
        let errors = vec![
            refresh_error(
                "/memfd:jit-cache (deleted)",
                srx_hook::SrxHookErrno::SetProt,
            ),
            refresh_error(
                "/memfd:jit-cache 123 (deleted)",
                srx_hook::SrxHookErrno::SetProt,
            ),
        ];

        assert!(is_ignorable_refresh_failure(
            srx_hook::SrxHookErrno::SetProt,
            &errors,
            "app-write"
        ));
    }

    #[test]
    fn app_write_keeps_setprot_failures_for_real_libraries_fatal() {
        let errors = vec![refresh_error(
            "/apex/com.android.runtime/lib64/bionic/libc.so",
            srx_hook::SrxHookErrno::SetProt,
        )];

        assert!(!is_ignorable_refresh_failure(
            srx_hook::SrxHookErrno::SetProt,
            &errors,
            "app-write"
        ));
    }

    #[test]
    fn app_write_keeps_existing_readelf_format_tolerance() {
        let errors = vec![
            refresh_error(
                "/memfd:jit-cache (deleted)",
                srx_hook::SrxHookErrno::ReadElf,
            ),
            refresh_error("/bad/module", srx_hook::SrxHookErrno::Format),
        ];

        assert!(is_ignorable_refresh_failure(
            srx_hook::SrxHookErrno::ReadElf,
            &errors,
            "app-write"
        ));
    }

    #[test]
    fn app_write_requires_deleted_jit_memfd_for_setprot_tolerance() {
        let errors = vec![refresh_error(
            "/memfd:jit-cache",
            srx_hook::SrxHookErrno::SetProt,
        )];

        assert!(!is_ignorable_refresh_failure(
            srx_hook::SrxHookErrno::SetProt,
            &errors,
            "app-write"
        ));

        let similarly_named_errors = vec![refresh_error(
            "/memfd:jit-cache-test (deleted)",
            srx_hook::SrxHookErrno::SetProt,
        )];

        assert!(!is_ignorable_refresh_failure(
            srx_hook::SrxHookErrno::SetProt,
            &similarly_named_errors,
            "app-write"
        ));
    }
}

#[cfg(test)]
mod caller_filter_tests {
    use super::*;

    #[test]
    fn caller_filter_skips_render_and_hook_modules() {
        assert!(!should_hook_caller_module(
            "/vendor/lib64/hw/vulkan.adreno.so"
        ));
        assert!(!should_hook_caller_module(
            "/vendor/lib64/egl/libGLES_mali.so"
        ));
        assert!(!should_hook_caller_module("/system/lib64/libvulkan.so"));
        assert!(!should_hook_caller_module("libsrx_core.so"));
        assert!(!should_hook_caller_module("libsqlite.so"));
    }

    #[test]
    fn caller_filter_keeps_provider_and_app_modules() {
        assert!(should_hook_caller_module(
            "/apex/com.android.runtime/lib64/bionic/libc.so"
        ));
        assert!(should_hook_caller_module(
            "/data/app/com.example/base.apk!/lib/arm64-v8a/libfoo.so"
        ));
        assert!(should_hook_caller_module(
            "/apex/com.android.mediaprovider/lib64/libfuse_jni.so"
        ));
    }
}
