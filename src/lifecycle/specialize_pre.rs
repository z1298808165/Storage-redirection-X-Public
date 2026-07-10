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

        // 系统代写进程在 specialize 后可能无法通过绝对路径访问模块目录。先保留
        // Zygisk 模块目录 FD，让 UID 归因与其余配置共用同一可访问来源。
        if should_open_writer_config_fd_before_uid_resolution(&self.package_name) {
            self.open_module_dir_fd_for_writer();
            policy::set_shared_uid_config_dir(&self.writer_config_dir());
        } else {
            policy::refresh_shared_uid_cache();
        }
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
            self.close_module_dir_fd();
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
            self.close_module_dir_fd();
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
            let config_dir = self.writer_config_dir();
            policy::set_shared_uid_config_dir(&config_dir);
            Some(config_dir)
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
        Logger::init(Some(&self.package_name));

        // MediaProvider 的重定向 Java hook 仍在 pre 阶段安装；SAF 来源识别
        // 走 native 文件监视路径，避免在系统 provider 内加载 LSPlant。
        if should_install_media_provider_java_hook {
            // Java hook 会在零配置启动时预装并跨配置热加载继续工作。仅登记
            // 系统写入进程身份，使后续回调能刷新监控状态，不在这里安装 PLT hook。
            crate::hook::InterceptHub::instance().register_runtime_identity(&self.package_name);
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

fn should_open_writer_config_fd_before_uid_resolution(package_name: &str) -> bool {
    policy::is_system_writer_package(package_name)
        || policy::is_file_monitor_bridge_package(package_name)
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
mod tests {
    use super::*;

    #[test]
    fn file_monitor_ui_bypass_covers_system_oem_and_legacy_names() {
        assert!(is_file_monitor_ui_process(
            "com.android.documentsui",
            "com.android.documentsui"
        ));
        assert!(is_file_monitor_ui_process(
            "com.coloros.filemanager",
            "com.coloros.filemanager"
        ));
        assert!(is_file_monitor_ui_process(
            "com.android.providers.media",
            "com.android.providers.media:PhotoPicker"
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
}
