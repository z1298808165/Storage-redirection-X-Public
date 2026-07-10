// 应用 specialize 后流程：等待挂载状态并安装 PLT Hook
use super::RuntimeFlow;
use super::mount_timing;
use crate::hook::{InterceptHub, install_fuse_fix_if_enabled};
use crate::platform::paths::monotonic_ms;
use crate::platform::unique_fd::UniqueFd;
use crate::platform::{self, anti_detect};
use crate::redirect::policy;
use crate::zygisk::abi;
use libc::{O_CLOEXEC, O_RDONLY, open, read, unlink};
use std::sync::atomic::{AtomicBool, Ordering};

static PLT_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

impl RuntimeFlow {
    pub fn post_app_specialize(&mut self, _args: *const abi::AppSpecializeArgs) {
        let perf_started_ms = monotonic_ms();
        if self.should_skip_post_work {
            log_post_perf(self, "skip", 0, 0, 0, perf_started_ms);
            return;
        }

        if !self.should_redirect && !self.should_monitor && !self.should_install_fuse_fix {
            log_post_perf(self, "bypass", 0, 0, 0, perf_started_ms);
            return;
        }

        let mut mount_wait_ms = 0;
        if self.should_redirect && !self.is_system_writer_hook_redirect {
            if platform::is_isolated_uid(self.app_uid) {
                self.is_mount_applied = false;
                log::info!(
                    "isolated uid skip mount wait uid={} pid={}",
                    self.app_uid,
                    self.app_pid
                );
            } else {
                let mount_started_ms = monotonic_ms();
                self.send_deferred_mount_request();
                wait_for_mount_status(
                    &self.app_data_dir,
                    self.app_pid,
                    self.is_mount_request_sent,
                    &mut self.is_mount_applied,
                );
                mount_wait_ms = monotonic_ms().saturating_sub(mount_started_ms);
            }
        } else if self.should_redirect && self.is_system_writer_hook_redirect {
            self.is_mount_applied = false;
            log::info!("writer per-caller hook map (skip marker wait)");
        }

        let hook_started_ms = monotonic_ms();
        let is_redirect_via_hook = self.should_redirect
            && (self.is_system_writer_hook_redirect || self.should_install_app_redirect_hook);
        let is_plt_hook_active = if self.should_install_fuse_fix && !is_redirect_via_hook {
            install_media_runtime_hook(&self.package_name, self.should_monitor)
        } else if should_install_process_plt_hook(self, is_redirect_via_hook) {
            install_plt_hook(
                &self.package_name,
                self.should_monitor,
                is_redirect_via_hook,
                self.is_system_writer_boot_lite,
                self.should_install_app_redirect_hook,
            )
        } else {
            log::info!(
                "plt hook skip pkg={} redirect={} monitor={} file_ui={}",
                self.package_name,
                self.should_redirect,
                self.should_monitor,
                self.is_file_monitor_ui
            );
            false
        };
        if self.should_install_fuse_fix {
            install_fuse_fix_if_enabled(&self.package_name);
        }
        crate::hook::refresh_runtime_config_from_settings();
        let hook_ms = monotonic_ms().saturating_sub(hook_started_ms);

        let anti_started_ms = monotonic_ms();
        // Hook 安装后命名匿名可执行区域，覆盖模块代码和 hook trampoline
        let named_count = if is_plt_hook_active && !self.is_system_writer_boot_lite {
            anti_detect::name_anonymous_executable_regions()
        } else {
            log::debug!(
                "anon region rename skipped active={} boot_lite={}",
                is_plt_hook_active,
                self.is_system_writer_boot_lite
            );
            0
        };
        let anti_ms = monotonic_ms().saturating_sub(anti_started_ms);
        if named_count > 0 {
            log::info!("anon regions named n={}", named_count);
        }
        if !is_plt_hook_active && !self.should_keep_module_loaded {
            self.request_dlclose();
        }
        log_post_perf(
            self,
            "done",
            mount_wait_ms,
            hook_ms,
            anti_ms,
            perf_started_ms,
        );
    }
}

fn should_install_process_plt_hook(flow: &RuntimeFlow, is_redirect_via_hook: bool) -> bool {
    if flow.is_file_monitor_ui {
        return false;
    }

    if !flow.should_monitor && !is_redirect_via_hook {
        return false;
    }

    policy::is_system_writer_package(&flow.package_name)
        || (flow.should_monitor && policy::is_saf_native_monitor_bridge_package(&flow.package_name))
}

// 轮询读取挂载状态标记文件，确认挂载是否成功
fn wait_for_mount_status(
    app_data_dir: &str,
    app_pid: i32,
    is_mount_request_sent: bool,
    is_mount_applied_out: &mut bool,
) {
    *is_mount_applied_out = false;
    let mut last_errno_code = 0;
    let mount_started_ms = monotonic_ms();

    if app_data_dir.is_empty() || app_pid <= 0 {
        log::warn!("mount ctx invalid, skip wait");
        return;
    }

    if !is_mount_request_sent {
        log::warn!("mount req not sent, wait daemon marker fallback");
    }

    let marker_path = format!("{}/.srx_mount_status_{}", app_data_dir, app_pid);
    log::info!(
        "wait marker {} budget_ms={} polls={} delay_us={}",
        marker_path,
        mount_timing::post_mount_status_wait_budget_ms(),
        mount_timing::POST_MOUNT_STATUS_POLL_COUNT,
        mount_timing::POST_MOUNT_STATUS_POLL_DELAY_US
    );

    let Ok(c_path) = std::ffi::CString::new(marker_path.clone()) else {
        return;
    };

    let mut poll_count = 0;
    for _ in 0..mount_timing::POST_MOUNT_STATUS_POLL_COUNT {
        poll_count += 1;
        let fd = unsafe { open(c_path.as_ptr(), O_RDONLY | O_CLOEXEC) };
        if fd >= 0 {
            let file = UniqueFd::new(fd);
            let mut ch = [0u8; 1];
            let n = unsafe { read(file.get(), ch.as_mut_ptr() as *mut _, 1) };
            if n == 1 {
                unsafe { unlink(c_path.as_ptr()) };
                *is_mount_applied_out = ch[0] == b'1';
                let elapsed_ms = monotonic_ms().saturating_sub(mount_started_ms);
                log::info!(
                    "marker read n={} val={} applied={} polls={} elapsed_ms={}",
                    n,
                    ch[0] as char,
                    *is_mount_applied_out,
                    poll_count,
                    elapsed_ms
                );
                break;
            }
        } else {
            last_errno_code = unsafe { *libc::__errno() };
        }
        unsafe { libc::usleep(mount_timing::POST_MOUNT_STATUS_POLL_DELAY_US) };
    }

    if *is_mount_applied_out {
        log::info!("app mount confirmed pid={}", app_pid);
    } else {
        log::warn!(
            "mount unknown/failed pid={} marker={} polls={} errno={}",
            app_pid,
            marker_path,
            poll_count,
            last_errno_code
        );
    }
}

fn log_post_perf(
    flow: &RuntimeFlow,
    exit_reason: &str,
    mount_wait_ms: i64,
    hook_ms: i64,
    anti_ms: i64,
    started_ms: i64,
) {
    let total_ms = monotonic_ms().saturating_sub(started_ms);
    if total_ms < mount_timing::POST_SPECIALIZE_SLOW_MS
        && !flow.should_redirect
        && !flow.should_monitor
        && !flow.should_install_fuse_fix
    {
        return;
    }
    log::info!(
        "perf post pkg={} pid={} exit={} redirect={} monitor={} hook_redirect={} boot_lite={} fuse_fix={} mount_sent={} mount_applied={} mount_wait_ms={} hook_ms={} anti_ms={} total_ms={}",
        flow.package_name,
        flow.app_pid,
        exit_reason,
        flow.should_redirect,
        flow.should_monitor,
        flow.is_system_writer_hook_redirect,
        flow.is_system_writer_boot_lite,
        flow.should_install_fuse_fix,
        flow.is_mount_request_sent,
        flow.is_mount_applied,
        mount_wait_ms,
        hook_ms,
        anti_ms,
        total_ms
    );
}

fn install_plt_hook(
    package_name: &str,
    should_monitor: bool,
    is_redirect_via_hook: bool,
    is_boot_lite: bool,
    is_app_write_redirect: bool,
) -> bool {
    let is_monitor_only = !is_redirect_via_hook;
    let should_install = should_monitor || is_redirect_via_hook;
    if !should_install {
        log::info!("plt hook skip");
        return false;
    }

    if PLT_HOOK_INSTALLED.swap(true, Ordering::AcqRel) {
        log::info!("plt hook already installed");
        return true;
    }

    log::info!(
        "plt hook install redirect={} monitor={} boot_lite={}",
        !is_monitor_only,
        should_monitor,
        is_boot_lite
    );

    let hub = InterceptHub::instance();
    if is_app_write_redirect {
        hub.init_app_write_redirect(package_name, should_monitor);
    } else if is_boot_lite {
        hub.init_boot_lite(package_name, should_monitor);
    } else {
        hub.init(package_name, is_monitor_only, should_monitor);
    }
    if hub.install() {
        log::info!("plt hook ok");
        true
    } else {
        PLT_HOOK_INSTALLED.store(false, Ordering::Release);
        log::warn!("plt hook failed");
        false
    }
}

fn install_media_runtime_hook(package_name: &str, should_monitor: bool) -> bool {
    if PLT_HOOK_INSTALLED.swap(true, Ordering::AcqRel) {
        log::info!("plt hook already installed");
        return true;
    }

    log::info!("plt hook install media runtime monitor={}", should_monitor);
    let hub = InterceptHub::instance();
    hub.init_media_runtime(package_name, should_monitor);
    if hub.install() {
        log::info!("plt hook ok");
        true
    } else {
        PLT_HOOK_INSTALLED.store(false, Ordering::Release);
        log::warn!("plt hook failed");
        false
    }
}
