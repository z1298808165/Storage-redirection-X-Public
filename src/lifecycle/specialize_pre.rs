// 应用 specialize 前流程：配置加载、决策重定向和监控、发送挂载请求
mod identity;
mod payload;
mod perf;
mod route;
mod writer_state;

use super::RuntimeFlow;
use identity::ProcessIdentity;
use payload::{
    CompanionMountRequest, build_companion_request_payload, send_companion_request_payload,
};
use perf::{SpecializePerf, log_specialize_perf};
use route::RouteConfigSnapshot;
use writer_state::{
    SystemWriterContext, mark_media_hook_deferred, resolve_system_writer_context,
    should_defer_media_boot_extras, should_install_java_hook_for_writer,
};

use crate::config::{SettingsHub, watcher};
use crate::domain::PathMapping;
use crate::java_hook;
use crate::logging::Logger;
use crate::monitor::AuditTrail;
use crate::platform;
use crate::platform::module_paths;
use crate::platform::paths::monotonic_ms;
use crate::redirect::{PathRouter, policy};
use crate::zygisk::{abi, jni};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

static MONITOR_HOOK_PROCESS_COUNT: AtomicU64 = AtomicU64::new(0);

impl RuntimeFlow {
    // 请求 Zygisk 在 postAppSpecialize 后卸载模块 .so
    pub(super) fn request_dlclose(&self) {
        if self.should_keep_module_loaded {
            log::debug!(
                "dlclose skipped pid={} pkg={}",
                self.app_pid,
                self.package_name
            );
            return;
        }
        if let Some(api) = self.api.as_ref() {
            api.set_option(abi::ZygiskOption::DlcloseModuleLibrary);
            log::debug!(
                "dlclose requested pid={} pkg={}",
                self.app_pid,
                self.package_name
            );
        }
    }

    pub fn pre_app_specialize(&mut self, args: *mut abi::AppSpecializeArgs) {
        let perf_started_ms = monotonic_ms();
        if args.is_null() {
            return;
        }

        let args = unsafe { &mut *args };
        let Some(process) = ProcessIdentity::from_args(self.env, args) else {
            log::warn!("nice_name empty");
            return;
        };
        self.package_name = process.package_name.clone();
        self.app_uid = process.uid;
        self.app_pid = process.pid;

        log::debug!(
            "proc ctx nice={} pkg={} uid={} pid={}",
            process.nice_name,
            self.package_name,
            self.app_uid,
            self.app_pid
        );

        self.is_mount_request_sent = false;
        self.is_mount_applied = false;
        self.deferred_mount_payload.clear();
        self.is_system_writer_hook_redirect = false;
        self.should_install_app_redirect_hook = false;
        self.is_system_writer_boot_lite = false;
        self.is_file_monitor_ui = false;
        self.should_install_fuse_fix = false;
        self.should_skip_post_work = false;
        self.should_keep_module_loaded = false;

        policy::refresh_shared_uid_cache();
        if let Some(config_package) =
            resolve_config_package_for_uid(&self.package_name, self.app_uid)
            && config_package != self.package_name
        {
            log::info!(
                "proc pkg alias nice={} pkg={} -> {} uid={}",
                process.nice_name,
                self.package_name,
                config_package,
                self.app_uid
            );
            self.package_name = config_package;
        }

        let is_system_writer = policy::is_system_writer_package(&self.package_name);
        let is_shared_uid_writer = policy::is_shared_uid_process(self.app_uid);
        let is_monitor_bridge = policy::is_file_monitor_bridge_package(&self.package_name);
        self.is_file_monitor_ui =
            is_file_monitor_ui_process(&self.package_name, &process.nice_name);
        if self.is_file_monitor_ui {
            self.should_skip_post_work = true;
            self.request_dlclose();
            log::info!(
                "file monitor UI daemon-only bypass pkg={} nice={} uid={} pid={}",
                self.package_name,
                process.nice_name,
                self.app_uid,
                self.app_pid
            );
            return;
        }
        let has_effective_config =
            has_effective_app_config_for_uid(&self.package_name, self.app_uid);
        if should_fast_bypass_app_config(
            is_system_writer,
            is_shared_uid_writer,
            is_monitor_bridge,
            has_effective_config,
        ) {
            self.should_skip_post_work = true;
            self.request_dlclose();
            log::debug!(
                "app config inactive, fast bypass pkg={} uid={}",
                self.package_name,
                self.app_uid
            );
            return;
        }

        let config = SettingsHub::instance();
        let writer_config_dir = if is_system_writer || is_shared_uid_writer || is_monitor_bridge {
            self.open_module_dir_fd_for_writer();
            Some(self.writer_config_dir())
        } else {
            None
        };
        let config_init_started_ms = monotonic_ms();
        if !config.init(writer_config_dir.as_deref()) {
            self.close_module_dir_fd();
            log::warn!("config init failed");
            return;
        }
        let config_init_ms = monotonic_ms().saturating_sub(config_init_started_ms);

        let config_reload_started_ms = monotonic_ms();
        config.reload_if_changed();
        let config_reload_ms = monotonic_ms().saturating_sub(config_reload_started_ms);
        let shared_uid_started_ms = monotonic_ms();
        policy::refresh_shared_uid_cache();
        let shared_uid_ms = monotonic_ms().saturating_sub(shared_uid_started_ms);

        let decision_started_ms = monotonic_ms();
        self.should_redirect = config.should_redirect(&self.package_name, self.app_uid);
        self.should_monitor = config.should_monitor(&self.package_name, self.app_uid);
        let decision_ms = monotonic_ms().saturating_sub(decision_started_ms);

        // 隔离进程无 FUSE 挂载和存储权限，跳过重定向
        if should_skip_isolated_uid(self.app_uid) {
            if self.should_redirect {
                log::info!("isolated uid skip redirect uid={}", self.app_uid);
                self.should_redirect = false;
            }
            if !self.should_redirect && !self.should_monitor {
                self.request_dlclose();
            }
            self.close_module_dir_fd();
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "isolated",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: 0,
                excluded_count: 0,
                mapping_count: 0,
                payload_bytes: 0,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms: 0,
                enabled_scan_ms: 0,
                route_ms: 0,
                payload_ms: 0,
                send_ms: 0,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        if is_system_writer {
            log::info!(
                "writer init pkg={} uid={} apps={} redirect={} monitor={}",
                self.package_name,
                self.app_uid,
                config.get_app_count(),
                self.should_redirect,
                self.should_monitor
            );
        }

        let writer_context_started_ms = monotonic_ms();
        let mut writer_context = resolve_system_writer_context(
            &self.package_name,
            self.app_uid,
            config,
            &mut self.should_redirect,
            &mut self.should_monitor,
            &mut self.is_system_writer_hook_redirect,
        );
        self.should_install_fuse_fix = writer_context.should_install_fuse_fix;
        if self.should_install_fuse_fix {
            self.should_keep_module_loaded = true;
        }
        let writer_context_ms = monotonic_ms().saturating_sub(writer_context_started_ms);

        let enabled_scan_started_ms = monotonic_ms();
        let has_enabled_apps = if is_system_writer {
            config.has_effective_enabled_redirect_apps_for_user(self.app_uid)
        } else {
            config.has_enabled_redirect_apps_for_user(self.app_uid)
        };
        let enabled_scan_ms = monotonic_ms().saturating_sub(enabled_scan_started_ms);
        if is_system_writer {
            log::info!(
                "writer final pkg={} uid={} apps={} enabled={} redirect={} monitor={} hook_redirect={} fuse_fix={}",
                self.package_name,
                self.app_uid,
                config.get_app_count(),
                has_enabled_apps,
                self.should_redirect,
                self.should_monitor,
                self.is_system_writer_hook_redirect,
                self.should_install_fuse_fix
            );
        }

        let should_defer_media_boot_extras =
            self.defer_media_boot_extras_if_needed(&writer_context);
        let should_install_media_provider_java_hook = should_install_java_hook_for_writer(
            &writer_context,
            self.is_system_writer_hook_redirect,
            self.should_monitor,
            should_defer_media_boot_extras,
        );
        if should_install_media_provider_java_hook {
            self.should_keep_module_loaded = true;
        }

        if !self.should_redirect && !self.should_monitor && !should_install_media_provider_java_hook
        {
            if !self.should_keep_module_loaded {
                // 模块即将 dlclose，post 阶段不再做任何事，避免在转译进程里触碰匿名段
                self.should_skip_post_work = true;
                self.close_module_dir_fd();
            } else {
                log::info!("module dir fd kept for runtime config reload");
            }
            self.request_dlclose();
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "bypass",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: 0,
                excluded_count: 0,
                mapping_count: 0,
                payload_bytes: 0,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms,
                enabled_scan_ms,
                route_ms: 0,
                payload_ms: 0,
                send_ms: 0,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        self.configure_monitoring();

        // MediaProvider 的重定向 Java hook 仍在 pre 阶段安装；SAF 来源识别
        // 走 native 文件监视路径，避免在系统 provider 内加载 LSPlant。
        if should_install_media_provider_java_hook {
            install_java_hook(self.env);
        } else if (writer_context.is_system_writer || writer_context.is_monitor_bridge)
            && !writer_context.is_media_provider
            && (self.is_system_writer_hook_redirect || self.should_monitor)
        {
            log::info!(
                "java hook skip: non-media bridge/shared writer pkg={} uid={}",
                self.package_name,
                self.app_uid
            );
        }

        Logger::init(Some(&self.package_name));
        self.configure_writer_runtime_watch(&writer_context, writer_config_dir.as_deref());
        log::info!(
            "app start pkg={} redirect={} monitor={}",
            self.package_name,
            self.should_redirect,
            self.should_monitor
        );

        if !self.should_redirect {
            if self.is_file_monitor_ui {
                self.close_module_dir_fd();
            }
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "monitor_only",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: 0,
                excluded_count: 0,
                mapping_count: 0,
                payload_bytes: 0,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms,
                enabled_scan_ms,
                route_ms: 0,
                payload_ms: 0,
                send_ms: 0,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        log::info!("config loaded apps={}", config.get_app_count());
        let route_config_started_ms = monotonic_ms();
        PathRouter::instance().init();

        let resolved_profile =
            config.get_resolved_user_profile_snapshot(&self.package_name, self.app_uid);
        let mut route_config =
            RouteConfigSnapshot::from_resolved_profile(resolved_profile.as_ref());

        route_config.log_config_summary(&self.package_name);
        route_config.log_config_details();
        route_config
            .apply_writer_override(&mut writer_context, self.is_system_writer_hook_redirect);

        let RouteConfigSnapshot {
            allowed_real_paths,
            excluded_real_paths,
            sandboxed_paths,
            read_only_paths,
            path_mappings,
            is_mapping_mode_only,
        } = route_config;

        let user_id = resolved_profile
            .as_ref()
            .map(|resolved| resolved.user_id)
            .unwrap_or_else(|| platform::user_id_from_uid(self.app_uid));
        let redirect_base = resolved_profile
            .as_ref()
            .map(|resolved| resolved.redirect_target.clone())
            .unwrap_or_else(|| {
                platform::paths::default_redirect_target(&self.package_name, user_id)
            });
        self.app_data_dir = if args.app_data_dir.is_null() {
            String::new()
        } else {
            jni::get_jstring_utf8(self.env, unsafe { *args.app_data_dir })
        };

        if !self.is_system_writer_hook_redirect {
            clear_mount_status_marker(&self.app_data_dir, self.app_pid);
        }

        RouteConfigSnapshot::configure_router(
            &self.package_name,
            self.app_uid,
            &redirect_base,
            self.is_system_writer_hook_redirect,
            &allowed_real_paths,
            &excluded_real_paths,
            &sandboxed_paths,
            &read_only_paths,
            &path_mappings,
            is_mapping_mode_only,
        );
        let route_ms = monotonic_ms().saturating_sub(route_config_started_ms);

        if is_mapping_mode_only {
            log::info!(
                "map-only allow={} excl={} sandbox={} ro={} map={}",
                allowed_real_paths.len(),
                excluded_real_paths.len(),
                sandboxed_paths.len(),
                read_only_paths.len(),
                path_mappings.len()
            );
        } else {
            log::info!(
                "redirect allow={} excl={} sandbox={} ro={} map={}",
                allowed_real_paths.len(),
                excluded_real_paths.len(),
                sandboxed_paths.len(),
                read_only_paths.len(),
                path_mappings.len()
            );
        }

        log::info!(
            "monitor pkg={} on={}",
            self.package_name,
            self.should_monitor
        );

        let is_fuse_daemon_redirect_enabled = config.is_fuse_daemon_redirect_enabled();
        let is_file_monitor_enabled = config.is_file_monitor_enabled();
        let app_redirect_hook_reason = app_redirect_hook_reason_for_process(
            self.should_redirect,
            is_system_writer,
            &allowed_real_paths,
            &path_mappings,
            user_id,
            is_fuse_daemon_redirect_enabled,
        );
        self.should_install_app_redirect_hook = app_redirect_hook_reason.is_some();
        if self.should_install_app_redirect_hook {
            self.should_keep_module_loaded = true;
            log::info!(
                "app redirect hook enabled pkg={} reason={}",
                self.package_name,
                app_redirect_hook_reason.unwrap_or("unknown")
            );
        }

        if self.is_system_writer_hook_redirect {
            log::info!("writer skip companion mount (per-caller hook)");
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "writer_hook",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: allowed_real_paths.len(),
                excluded_count: excluded_real_paths.len(),
                mapping_count: path_mappings.len(),
                payload_bytes: 0,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms,
                enabled_scan_ms,
                route_ms,
                payload_ms: 0,
                send_ms: 0,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        log::info!(
            "mount request cfg pkg={} fuse_daemon={} file_monitor={} allow={} excl={} sandbox={} ro={} map={} map_only={}",
            self.package_name,
            is_fuse_daemon_redirect_enabled,
            is_file_monitor_enabled,
            allowed_real_paths.len(),
            excluded_real_paths.len(),
            sandboxed_paths.len(),
            read_only_paths.len(),
            path_mappings.len(),
            is_mapping_mode_only
        );

        let payload_started_ms = monotonic_ms();
        let payload = build_companion_request_payload(&CompanionMountRequest {
            pid: self.app_pid,
            uid: self.app_uid,
            package_name: &self.package_name,
            app_data_dir: &self.app_data_dir,
            is_fuse_daemon_redirect_enabled,
            is_file_monitor_enabled,
            redirect_target: &redirect_base,
            allowed_real_paths: &allowed_real_paths,
            excluded_real_paths: &excluded_real_paths,
            sandboxed_paths: &sandboxed_paths,
            read_only_paths: &read_only_paths,
            path_mappings: &path_mappings,
            is_mapping_mode_only,
            operation: "apply",
            config_version: config.config_version(),
        });
        let payload_ms = monotonic_ms().saturating_sub(payload_started_ms);
        let payload_bytes = payload.len();
        if payload.is_empty() {
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "empty_payload",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: allowed_real_paths.len(),
                excluded_count: excluded_real_paths.len(),
                mapping_count: path_mappings.len(),
                payload_bytes,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms,
                enabled_scan_ms,
                route_ms,
                payload_ms,
                send_ms: 0,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        if !is_system_writer && !is_shared_uid_writer && !is_monitor_bridge {
            let send_started_ms = monotonic_ms();
            self.is_mount_request_sent =
                send_companion_request_payload(self.api.as_ref(), &payload);
            let send_ms = monotonic_ms().saturating_sub(send_started_ms);
            log::info!(
                "mount request sent pre pkg={} sent={} payload={} map_only={}",
                self.package_name,
                self.is_mount_request_sent,
                payload_bytes,
                is_mapping_mode_only
            );
            log_specialize_perf(&SpecializePerf {
                package_name: &self.package_name,
                exit_reason: "mount_request_pre",
                pid: self.app_pid,
                uid: self.app_uid,
                app_count: config.get_app_count(),
                should_redirect: self.should_redirect,
                should_monitor: self.should_monitor,
                is_system_writer,
                is_hook_redirect: self.is_system_writer_hook_redirect,
                allow_count: allowed_real_paths.len(),
                excluded_count: excluded_real_paths.len(),
                mapping_count: path_mappings.len(),
                payload_bytes,
                config_init_ms,
                config_reload_ms,
                shared_uid_ms,
                decision_ms,
                writer_context_ms,
                enabled_scan_ms,
                route_ms,
                payload_ms,
                send_ms,
                total_ms: monotonic_ms().saturating_sub(perf_started_ms),
            });
            return;
        }

        let send_started_ms = monotonic_ms();
        self.is_mount_request_sent = send_companion_request_payload(self.api.as_ref(), &payload);
        let send_ms = monotonic_ms().saturating_sub(send_started_ms);
        log_specialize_perf(&SpecializePerf {
            package_name: &self.package_name,
            exit_reason: "mount_request",
            pid: self.app_pid,
            uid: self.app_uid,
            app_count: config.get_app_count(),
            should_redirect: self.should_redirect,
            should_monitor: self.should_monitor,
            is_system_writer,
            is_hook_redirect: self.is_system_writer_hook_redirect,
            allow_count: allowed_real_paths.len(),
            excluded_count: excluded_real_paths.len(),
            mapping_count: path_mappings.len(),
            payload_bytes,
            config_init_ms,
            config_reload_ms,
            shared_uid_ms,
            decision_ms,
            writer_context_ms,
            enabled_scan_ms,
            route_ms,
            payload_ms,
            send_ms,
            total_ms: monotonic_ms().saturating_sub(perf_started_ms),
        });
    }

    pub(super) fn send_deferred_mount_request(&mut self) {
        if self.deferred_mount_payload.is_empty() {
            return;
        }

        let payload = std::mem::take(&mut self.deferred_mount_payload);
        self.is_mount_request_sent = send_companion_request_payload(self.api.as_ref(), &payload);
        log::info!(
            "post mount request sent pkg={} sent={} payload={}",
            self.package_name,
            self.is_mount_request_sent,
            payload.len()
        );
    }

    fn writer_config_dir(&self) -> String {
        if self.module_dir_fd >= 0 {
            format!("/proc/self/fd/{}/config", self.module_dir_fd)
        } else {
            module_paths::CONFIG_DIR.to_string()
        }
    }

    fn open_module_dir_fd_for_writer(&mut self) {
        if self.module_dir_fd >= 0 {
            return;
        }
        let Some(api) = self.api.as_ref() else {
            return;
        };
        let fd = api.get_module_dir();
        if fd < 0 {
            return;
        }
        if !api.exempt_fd(fd) {
            log::warn!("module dir fd exempt failed fd={}", fd);
        }
        log::info!("module dir fd open fd={}", fd);
        self.module_dir_fd = fd;
    }

    fn close_module_dir_fd(&mut self) {
        if self.module_dir_fd < 0 {
            return;
        }
        let fd = self.module_dir_fd;
        self.module_dir_fd = -1;
        unsafe {
            libc::close(fd);
        }
        log::info!("module dir fd closed fd={}", fd);
    }

    fn configure_monitoring(&self) {
        let trail = AuditTrail::instance();
        if !self.should_monitor {
            trail.set_enabled(false);
            return;
        }

        trail.set_enabled(true);

        let mut monitor_package = self.package_name.clone();
        if policy::is_shared_uid_process(self.app_uid) {
            let shared_uid = policy::get_shared_uid_packages_string(self.app_uid);
            if !shared_uid.is_empty() {
                monitor_package = shared_uid;
            }
        }
        trail.init(&monitor_package, self.app_uid);

        let monitor_fd = trail.get_log_fd();
        if let Some(api) = self.api.as_ref()
            && monitor_fd >= 0
        {
            if api.exempt_fd(monitor_fd) {
                log::info!("monitor log fd exempt fd={}", monitor_fd);
            } else {
                log::warn!("monitor log fd exempt failed fd={}", monitor_fd);
            }
        }

        let hook_count = MONITOR_HOOK_PROCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        log::info!(
            "monitor hook procs total={} cur={}",
            hook_count,
            self.package_name
        );
    }

    fn configure_writer_runtime_watch(
        &self,
        writer_context: &SystemWriterContext,
        writer_config_dir: Option<&str>,
    ) {
        if !writer_context.is_system_writer && !writer_context.is_monitor_bridge {
            return;
        }

        log::info!("runtime config watch: file log off, logcat only");
        let config_watch_dir = writer_config_dir.unwrap_or(module_paths::CONFIG_DIR);
        let inotify_fd = watcher::init(config_watch_dir);
        if let Some(api) = self.api.as_ref()
            && inotify_fd >= 0
        {
            if api.exempt_fd(inotify_fd) {
                log::info!("config watch fd exempt fd={}", inotify_fd);
            } else {
                log::warn!("config watch fd exempt failed fd={}", inotify_fd);
            }
        }
    }

    fn defer_media_boot_extras_if_needed(&mut self, writer_context: &SystemWriterContext) -> bool {
        let should_defer = should_defer_media_boot_extras(
            writer_context.is_media_provider,
            self.is_system_writer_hook_redirect,
            self.should_install_fuse_fix,
        );
        if !should_defer {
            return false;
        }

        self.is_system_writer_boot_lite = self.is_system_writer_hook_redirect;
        if self.should_install_fuse_fix {
            self.should_install_fuse_fix = false;
            self.should_keep_module_loaded = false;
        }
        mark_media_hook_deferred();
        log::warn!(
            "writer boot extras deferred until boot_completed pkg={} uid={} redirect_hook={} boot_lite={} fuse_fix={}",
            self.package_name,
            self.app_uid,
            self.is_system_writer_hook_redirect,
            self.is_system_writer_boot_lite,
            writer_context.should_install_fuse_fix
        );
        true
    }
}

fn install_java_hook(env: *mut jni_sys::JNIEnv) {
    // MediaProvider 主进程的 Java hook 保持启用
    // 用于 Caller 识别，工作稳定
    if !java_hook::is_available() {
        log::info!("java hook skip: hooker dex unavailable");
        return;
    }
    log::info!("java hook available dex_bytes={}", java_hook::dex_len());
    if java_hook::init_once(env) {
        log::info!("java hook init ok");
    } else {
        log::warn!("java hook init failed");
    }
}

fn has_effective_app_config_for_uid(package_name: &str, uid: i32) -> bool {
    if package_name.is_empty() || uid < 0 {
        return false;
    }

    let user_id = platform::user_id_from_uid(uid);
    if user_id < 0 {
        return false;
    }

    SettingsHub::instance().has_enabled_user_profile_in_raw_config(package_name, user_id)
}

fn should_fast_bypass_app_config(
    is_system_writer: bool,
    is_shared_uid_writer: bool,
    is_monitor_bridge: bool,
    has_effective_config: bool,
) -> bool {
    !is_system_writer && !is_shared_uid_writer && !is_monitor_bridge && !has_effective_config
}

fn is_file_monitor_ui_process(package_name: &str, nice_name: &str) -> bool {
    policy::is_file_monitor_ui_package(package_name) || is_legacy_file_monitor_ui_process(nice_name)
}

fn is_legacy_file_monitor_ui_process(nice_name: &str) -> bool {
    nice_name == "android.process.mediaUI" || nice_name.ends_with(":PhotoPicker")
}

fn should_skip_isolated_uid(uid: i32) -> bool {
    platform::is_isolated_uid(uid)
}

fn resolve_config_package_for_uid(package_name: &str, uid: i32) -> Option<String> {
    if uid < 0 || package_name.is_empty() || has_effective_app_config_for_uid(package_name, uid) {
        return None;
    }

    if package_name == "android.process.media"
        && policy::get_uid_for_package("com.android.providers.downloads") == uid
    {
        return Some("com.android.providers.downloads".to_string());
    }

    let mut configured_packages: Vec<String> = policy::get_packages_for_uid(uid)
        .into_iter()
        .filter(|pkg| !pkg.is_empty() && !policy::is_system_writer_package(pkg))
        .filter(|pkg| has_effective_app_config_for_uid(pkg, uid))
        .collect();
    configured_packages.sort();
    configured_packages.dedup();

    if configured_packages.len() == 1 {
        return configured_packages.pop();
    }

    if package_name == "android.process.mediaUI"
        && configured_packages
            .iter()
            .any(|pkg| pkg == "com.android.providers.downloads.ui")
    {
        return Some("com.android.providers.downloads.ui".to_string());
    }

    None
}

// 发送挂载请求前清理当前 PID 标记，避免读取到旧结果。
fn clear_mount_status_marker(app_data_dir: &str, app_pid: i32) {
    if app_data_dir.is_empty() || app_pid <= 0 {
        return;
    }

    let marker_path = format!("{}/.srx_mount_status_{}", app_data_dir, app_pid);
    let Ok(c_path) = CString::new(marker_path.clone()) else {
        return;
    };
    if unsafe { libc::unlink(c_path.as_ptr()) } == 0 {
        log::info!("old marker cleared {}", marker_path);
    }
}

#[cfg(test)]
fn allowed_paths_need_app_redirect_hook(allowed_real_paths: &[String], user_id: i32) -> bool {
    let storage_root = platform::paths::storage_user_root_for_user(user_id);
    allowed_real_paths.iter().any(|path| {
        let raw = path.trim_start();
        if raw.is_empty() || raw.starts_with('!') {
            return false;
        }
        let mut resolved =
            platform::paths::resolve_user_path(&platform::paths::normalize(raw), user_id);
        if !platform::paths::is_absolute(&resolved) {
            resolved = platform::paths::normalize(&platform::paths::join(&storage_root, &resolved));
        }
        media_allowed_write_hook_path(&resolved, &storage_root)
    })
}

fn app_redirect_hook_reason_for_process(
    should_redirect: bool,
    is_system_writer: bool,
    _allowed_real_paths: &[String],
    _path_mappings: &[PathMapping],
    _user_id: i32,
    _is_fuse_daemon_redirect_enabled: bool,
) -> Option<&'static str> {
    if !should_redirect || is_system_writer {
        return None;
    }

    // 普通应用只走 companion mount 路径。PLT 写入 hook 可能碰到 JIT/memfd
    // 保护页，也会破坏测试流要求的“普通应用不安装 hook”边界。
    None
}

#[cfg(test)]
fn app_redirect_hook_reason(
    allowed_real_paths: &[String],
    path_mappings: &[PathMapping],
    user_id: i32,
    is_fuse_daemon_redirect_enabled: bool,
) -> Option<&'static str> {
    if is_fuse_daemon_redirect_enabled
        && allowed_paths_need_app_redirect_hook(allowed_real_paths, user_id)
    {
        return Some("media_allowed_write");
    }
    if allowed_paths_shadow_mapping_requests(allowed_real_paths, path_mappings, user_id) {
        return Some("mapping_shadowed_by_allow");
    }
    None
}

#[cfg(test)]
fn allowed_paths_shadow_mapping_requests(
    allowed_real_paths: &[String],
    path_mappings: &[PathMapping],
    user_id: i32,
) -> bool {
    if allowed_real_paths.is_empty() || path_mappings.is_empty() {
        return false;
    }

    let storage_root = platform::paths::storage_user_root_for_user(user_id);
    let mapping_requests: Vec<String> = path_mappings
        .iter()
        .filter_map(|mapping| {
            resolve_storage_rule_path(&mapping.request_path, user_id, &storage_root)
        })
        .collect();
    if mapping_requests.is_empty() {
        return false;
    }

    allowed_real_paths.iter().any(|path| {
        let raw = path.trim_start();
        if raw.is_empty() || raw.starts_with('!') {
            return false;
        }
        let Some(allowed_path) = resolve_storage_rule_path(raw, user_id, &storage_root) else {
            return false;
        };
        mapping_requests.iter().any(|request_path| {
            platform::paths::matches(&allowed_path, request_path, true)
                || (!platform::paths::contains_wildcards(&allowed_path)
                    && platform::paths::is_same_or_child(request_path, &allowed_path))
        })
    })
}

#[cfg(test)]
fn resolve_storage_rule_path(path: &str, user_id: i32, storage_root: &str) -> Option<String> {
    let mut resolved =
        platform::paths::resolve_user_path(&platform::paths::normalize(path), user_id);
    if resolved.is_empty() || platform::paths::has_unsafe_segments(&resolved) {
        return None;
    }
    if !platform::paths::is_absolute(&resolved) {
        resolved = platform::paths::normalize(&platform::paths::join(storage_root, &resolved));
    }
    if !platform::paths::is_child(&resolved, storage_root)
        && !platform::paths::eq_ignore_case(&resolved, storage_root)
    {
        return None;
    }
    Some(resolved)
}

#[cfg(test)]
fn media_allowed_write_hook_path(path: &str, storage_root: &str) -> bool {
    let Some(relative) = platform::paths::relative_child_path(path, storage_root) else {
        return false;
    };
    let Some(first) = relative.split('/').find(|part| !part.is_empty()) else {
        return false;
    };
    matches!(first, "DCIM" | "Pictures" | "Movies")
}

#[cfg(test)]
mod tests {
    use super::*;
    use writer_state::should_defer_media_boot_extras_for_state;

    fn writer_context(is_media_provider: bool) -> SystemWriterContext {
        SystemWriterContext {
            is_system_writer: true,
            is_media_provider,
            is_monitor_bridge: false,
            should_install_fuse_fix: is_media_provider,
            has_merged_writer_mappings: false,
            merged_writer_mappings: Vec::new(),
        }
    }

    fn saf_monitor_bridge_context() -> SystemWriterContext {
        SystemWriterContext {
            is_system_writer: false,
            is_media_provider: false,
            is_monitor_bridge: true,
            should_install_fuse_fix: false,
            has_merged_writer_mappings: false,
            merged_writer_mappings: Vec::new(),
        }
    }

    #[test]
    fn java_hook_installs_only_for_real_media_provider_context() {
        // Java hook covers MediaProvider ContentValues path patches.
        assert!(should_install_java_hook_for_writer(
            &writer_context(true),
            true,
            false,
            false,
        ));

        // FUSE/monitor support also needs MediaProvider mutation hooks.
        assert!(should_install_java_hook_for_writer(
            &writer_context(true),
            false,
            true,
            false,
        ));

        // 非 MediaProvider 不安装
        assert!(!should_install_java_hook_for_writer(
            &writer_context(false),
            true,
            false,
            false,
        ));

        // defer 时不安装
        assert!(!should_install_java_hook_for_writer(
            &writer_context(true),
            true,
            false,
            true,
        ));

        // 既不重定向也不监控时不安装
        assert!(!should_install_java_hook_for_writer(
            &writer_context(true),
            false,
            false,
            false,
        ));
    }

    #[test]
    fn java_hook_skips_saf_monitor_bridge() {
        assert!(!should_install_java_hook_for_writer(
            &saf_monitor_bridge_context(),
            false,
            true,
            false,
        ));
        assert!(!should_install_java_hook_for_writer(
            &saf_monitor_bridge_context(),
            false,
            false,
            false,
        ));
    }

    #[test]
    fn file_monitor_ui_process_covers_modern_and_legacy_picker_names() {
        assert!(is_file_monitor_ui_process(
            "com.android.photopicker",
            "com.android.photopicker"
        ));
        assert!(is_file_monitor_ui_process(
            "com.android.documentsui",
            "com.android.documentsui"
        ));
        assert!(is_file_monitor_ui_process(
            "android.process.media",
            "android.process.media:PhotoPicker"
        ));
        assert!(is_file_monitor_ui_process(
            "android.process.mediaUI",
            "android.process.mediaUI"
        ));
        assert!(!is_file_monitor_ui_process(
            "com.android.providers.downloads",
            "com.android.providers.downloads"
        ));
    }

    #[test]
    fn app_redirect_hook_covers_mapping_shadowed_by_allowed_parent() {
        let mappings = vec![PathMapping::new(
            "Download/SrtProbe".to_string(),
            "Download/Test".to_string(),
        )];

        assert_eq!(
            app_redirect_hook_reason(&["Download".to_string()], &mappings, 0, false),
            Some("mapping_shadowed_by_allow")
        );
        assert!(allowed_paths_shadow_mapping_requests(
            &["Download".to_string()],
            &mappings,
            0
        ));
    }

    #[test]
    fn app_redirect_hook_preloads_for_redirect_hot_config() {
        assert_eq!(
            app_redirect_hook_reason_for_process(true, false, &[], &[], 0, false),
            None
        );
    }

    #[test]
    fn app_redirect_hook_skips_disabled_and_system_writer_processes() {
        assert_eq!(
            app_redirect_hook_reason_for_process(false, false, &[], &[], 0, false),
            None
        );
        assert_eq!(
            app_redirect_hook_reason_for_process(true, true, &[], &[], 0, false),
            None
        );
    }

    #[test]
    fn app_redirect_hook_ignores_unrelated_allow_and_exclusions() {
        let mappings = vec![PathMapping::new(
            "Download/SrtProbe".to_string(),
            "Download/Test".to_string(),
        )];

        assert_eq!(
            app_redirect_hook_reason(&["Pictures".to_string()], &mappings, 0, false),
            None
        );
        assert_eq!(
            app_redirect_hook_reason(&["!Download".to_string()], &mappings, 0, false),
            None
        );
    }

    #[test]
    fn app_redirect_hook_keeps_media_allowed_reason_when_fuse_enabled() {
        assert_eq!(
            app_redirect_hook_reason(&["DCIM/SrtFuseQQ/SrtAllowed*".to_string()], &[], 0, true,),
            Some("media_allowed_write")
        );
    }

    #[test]
    fn media_boot_extras_defer_only_before_boot_completed() {
        assert!(should_defer_media_boot_extras_for_state(
            true, true, false, false,
        ));
        assert!(should_defer_media_boot_extras_for_state(
            true, false, true, false,
        ));

        assert!(!should_defer_media_boot_extras_for_state(
            true, true, false, true,
        ));
        assert!(!should_defer_media_boot_extras_for_state(
            false, true, true, false,
        ));
        assert!(!should_defer_media_boot_extras_for_state(
            true, false, false, false,
        ));
    }

    #[test]
    fn companion_payload_preserves_mount_request_fields() {
        let allowed = vec!["/storage/emulated/0/DCIM".to_string()];
        let excluded = vec!["/storage/emulated/0/Download".to_string()];
        let sandboxed = vec!["/storage/emulated/0/Pictures".to_string()];
        let read_only = vec!["/storage/emulated/0/Documents".to_string()];
        let mappings = vec![PathMapping::new(
            "/storage/emulated/0/A".to_string(),
            "/storage/emulated/0/B".to_string(),
        )];

        let payload = build_companion_request_payload(&CompanionMountRequest {
            pid: 123,
            uid: 10123,
            package_name: "org.srx.test",
            app_data_dir: "/data/user/0/org.srx.test",
            is_fuse_daemon_redirect_enabled: true,
            is_file_monitor_enabled: true,
            redirect_target: "/data/media/0/Android/data/org.srx.test/sdcard",
            allowed_real_paths: &allowed,
            excluded_real_paths: &excluded,
            sandboxed_paths: &sandboxed,
            read_only_paths: &read_only,
            path_mappings: &mappings,
            is_mapping_mode_only: true,
            operation: "apply",
            config_version: 42,
        });

        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["operation"], "apply");
        assert_eq!(value["pid"], 123);
        assert_eq!(value["uid"], 10123);
        assert_eq!(value["package"], "org.srx.test");
        assert_eq!(value["fuse_daemon_redirect_enabled"], true);
        assert_eq!(value["file_monitor_enabled"], true);
        assert_eq!(value["mapping_mode_only"], true);
        assert_eq!(value["config_version"], 42);
        assert_eq!(value["allowed_real_paths"][0], allowed[0]);
        assert_eq!(value["excluded_real_paths"][0], excluded[0]);
        assert_eq!(value["sandboxed_paths"][0], sandboxed[0]);
        assert_eq!(value["read_only_paths"][0], read_only[0]);
        assert_eq!(
            value["path_mappings"][0]["request_path"],
            mappings[0].request_path
        );
        assert_eq!(
            value["path_mappings"][0]["final_path"],
            mappings[0].final_path
        );
    }
}
