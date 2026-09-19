// 应用 specialize 后流程：等待挂载状态并安装 PLT Hook
use super::RuntimeFlow;
use super::mount_timing;
use crate::hook::{InterceptHub, install_fuse_fix_if_enabled};
use crate::java_hook;
use crate::module_mount_source::app_redirect_mounts_in;
use crate::platform::paths::monotonic_ms;
use crate::platform::{self, anti_detect};
use crate::redirect::policy;
use crate::zygisk::abi;
use std::sync::atomic::{AtomicBool, Ordering};

static PLT_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 认定挂载落定所需的连续稳定轮数。
///
/// 一个应用可能有多个挂载根，逐条建立。只看"出现了挂载"就返回，应用会在挂载只完成一半时
/// 开始访问被重定向的路径。要求连续多轮看到完全相同的挂载 ID 集合再放行，用很小的延迟
/// 换取"挂载集合已不再变化"这一判据。
const MOUNT_SETTLE_POLLS: u32 = 4;

impl RuntimeFlow {
    pub fn post_app_specialize(&mut self, _args: *const abi::AppSpecializeArgs) {
        let perf_started_ms = monotonic_ms();
        // 历史挂载状态标记的清理放在这里：post 对每个被注入的进程都会被调用，而 pre 存在
        // 多条会直接返回的分支（无有效配置的 fast bypass 等），放在 pre 里对那类应用永远清不到，
        // 正是它们的遗留标记会一直累积。这里早于本函数的所有提前返回，也早于 dlclose。
        if self.app_data_dir.is_empty() {
            log::warn!(
                "legacy marker sweep skipped pkg={} reason=app_data_dir_absent",
                self.package_name
            );
        } else {
            crate::legacy_mount_marker::sweep_legacy_markers(&self.app_data_dir);
        }
        if policy::is_media_provider_package(&self.package_name) {
            java_hook::start_hot_reload_after_specialize();
        }
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
                wait_for_module_mount(
                    &self.package_name,
                    self.app_pid,
                    self.is_mount_request_sent,
                    &mut self.is_mount_applied,
                );
                mount_wait_ms = monotonic_ms().saturating_sub(mount_started_ms);
            }
        } else if self.should_redirect && self.is_system_writer_hook_redirect {
            self.is_mount_applied = false;
            log::info!("writer per-caller hook map (skip mount wait)");
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

// 等待本模块的重定向挂载出现在当前进程的命名空间里。
fn wait_for_module_mount(
    package_name: &str,
    app_pid: i32,
    is_mount_request_sent: bool,
    is_mount_applied_out: &mut bool,
) {
    *is_mount_applied_out = false;
    let mount_started_ms = monotonic_ms();

    if app_pid <= 0 {
        log::warn!("mount ctx invalid, skip wait");
        return;
    }

    if !is_mount_request_sent {
        log::warn!("mount req not sent, wait daemon mount fallback");
    }

    log::info!(
        "wait module mount pid={} budget_ms={} polls={} delay_us={}",
        app_pid,
        mount_timing::post_mount_status_wait_budget_ms(),
        mount_timing::POST_MOUNT_STATUS_POLL_COUNT,
        mount_timing::POST_MOUNT_STATUS_POLL_DELAY_US
    );

    let mut settled: Vec<(u64, String)> = Vec::new();
    let mut settle_polls = 0u32;
    let mut poll_count = 0;
    let mut last_mount_count = 0usize;
    for _ in 0..mount_timing::POST_MOUNT_STATUS_POLL_COUNT {
        poll_count += 1;
        let signature = app_redirect_mounts_in(0, package_name)
            .into_iter()
            .map(|mount| (mount.mount_id, mount.mount_point))
            .collect::<Vec<_>>();
        last_mount_count = signature.len();
        if signature.is_empty() {
            // 挂载被摘除或尚未建立，重新开始计数，避免把一次瞬时观测当作落定。
            settled.clear();
            settle_polls = 0;
        } else if signature == settled {
            settle_polls += 1;
            if settle_polls >= MOUNT_SETTLE_POLLS {
                *is_mount_applied_out = true;
                break;
            }
        } else {
            settled = signature;
            settle_polls = 1;
        }
        // SAFETY: usleep 只接收整型参数，不涉及借用指针。
        unsafe { libc::usleep(mount_timing::POST_MOUNT_STATUS_POLL_DELAY_US) };
    }

    let elapsed_ms = monotonic_ms().saturating_sub(mount_started_ms);
    if *is_mount_applied_out {
        // 这一行的措辞被测试流的挂载确认流程匹配，改动前必须同步 .github/tests 下的检测式。
        log::info!(
            "app mount confirmed pid={} mounts={} polls={} elapsed_ms={}",
            app_pid,
            last_mount_count,
            poll_count,
            elapsed_ms
        );
    } else {
        log::warn!(
            "mount unknown/failed pid={} mounts={} polls={} elapsed_ms={}",
            app_pid,
            last_mount_count,
            poll_count,
            elapsed_ms
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
        if is_redirect_via_hook {
            crate::runtime_stats::record_runtime_activation();
        }
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
