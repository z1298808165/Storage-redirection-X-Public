// 应用 specialize 后流程：确认挂载结论并安装 PLT Hook
use super::RuntimeFlow;
use super::mount_timing;
use crate::hook::{InterceptHub, install_fuse_fix_if_enabled};
use crate::java_hook;
use crate::module_mount_source::{app_redirect_mounts_in, storage_view_root_is_module_session};
use crate::platform::paths::monotonic_ms;
use crate::platform::{self, anti_detect};
use crate::redirect::policy;
use crate::zygisk::abi;
use std::sync::atomic::{AtomicBool, Ordering};

static PLT_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

impl RuntimeFlow {
    pub fn post_app_specialize(&mut self, _args: *const abi::AppSpecializeArgs) {
        let perf_started_ms = monotonic_ms();
        // 此时进程域已切换，私有日志通道可用：把 pre（zygote 阶段）只能写进 logcat
        // 的关键结论补投到 running.log，避免开机几十秒后 logcat 缓冲滚掉即永久丢失。
        crate::logging::flush_deferred_logs();
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

        let mut mount_check_ms = 0;
        if self.should_redirect && !self.is_system_writer_hook_redirect {
            if policy::is_file_monitor_bridge_package(&self.package_name) {
                // 桥接进程保留自身重定向时不挂模块 FUSE 会话
                // （scoped_fuse_mount_roots_for_request 对这类请求返回空根），
                // 原生写入的沙盒重定向由进程内 hook 兜底（is_app_redirect_owner），
                // 无需发送/等待挂载，应用立即恢复运行。同时启动配置监视线程，
                // 保证用户在管理界面修改规则后秒级生效（无会话可重挂，只能靠
                // 进程内路由重建）。
                self.is_mount_applied = false;
                crate::hook::config_watcher::start_for(self.package_name.clone());
                log::info!("monitor bridge hook redirect (skip mount wait)");
            } else if platform::is_isolated_uid(self.app_uid) {
                self.is_mount_applied = false;
                log::info!(
                    "isolated uid skip mount wait uid={} pid={}",
                    self.app_uid,
                    self.app_pid
                );
            } else {
                let mount_started_ms = monotonic_ms();
                // pre 阶段通常已经发出请求；这里只补发"延迟负载"这一类。
                self.send_deferred_mount_request();
                conclude_mount_readiness(
                    &self.package_name,
                    self.app_pid,
                    self.app_uid,
                    self.is_mount_request_sent,
                    self.is_mapping_mode_only,
                    &mut self.is_mount_applied,
                );
                mount_check_ms = monotonic_ms().saturating_sub(mount_started_ms);
            }
        } else if self.should_redirect && self.is_system_writer_hook_redirect {
            self.is_mount_applied = false;
            log::info!("writer per-caller hook map (skip mount wait)");
        }

        let hook_started_ms = monotonic_ms();
        let mp_hooks_disabled = super::specialize_pre::mp_hooks_disabled()
            && policy::is_media_provider_package(&self.package_name);
        let plt_disabled = super::specialize_pre::plt_hook_disabled()
            && policy::is_media_provider_package(&self.package_name);
        let fuse_fix_disabled = super::specialize_pre::fuse_fix_disabled()
            && policy::is_media_provider_package(&self.package_name);
        let is_redirect_via_hook = self.should_redirect
            && (self.is_system_writer_hook_redirect || self.should_install_app_redirect_hook);
        let is_plt_hook_active = if mp_hooks_disabled || plt_disabled {
            log::info!(
                "plt hook skip: disabled by diagnostic marker mp={} plt={}",
                mp_hooks_disabled,
                plt_disabled
            );
            false
        } else if self.should_install_fuse_fix && !is_redirect_via_hook {
            install_media_runtime_hook(&self.package_name, self.should_monitor)
        } else if should_install_process_plt_hook(self, is_redirect_via_hook) {
            // 自身配置了重定向规则的进程（如启用了 sandboxed_paths 的下载提供器）即使
            // 走监视 profile 也要保留进程内重定向兜底：挂载层（宿主/屏障）全部失效时，
            // 没有这一层 sandboxed_paths 的写操作会穿透到系统 FUSE 真实落盘。
            let is_app_redirect_owner = self.should_redirect && !is_redirect_via_hook;
            install_plt_hook(
                &self.package_name,
                self.should_monitor,
                is_redirect_via_hook,
                self.is_system_writer_boot_lite,
                self.should_install_app_redirect_hook,
                is_app_redirect_owner,
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
        if self.should_install_fuse_fix && !mp_hooks_disabled && !fuse_fix_disabled {
            install_fuse_fix_if_enabled(&self.package_name);
        }
        crate::hook::refresh_runtime_config_from_settings();
        let hook_ms = monotonic_ms().saturating_sub(hook_started_ms);

        let zn_available = crate::zn_api::available();
        log::info!("zygisk next api available={zn_available}");
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
            mount_check_ms,
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

// 确认本次挂载请求的结论。
//
// 判据只有一条：**应用此刻能不能在自己的命名空间里看见重定向结果**。曾经的做法是等
// 挂载执行方写回执，但那条通道在本项目里不可能成立（SELinux 域迁移：socket 由 zygote
// 域创建，应用 specialize 后属于 untrusted_app 域，`read` 直接被拒——`mount_request`
// 模块头注释里有真机 AVC 证据）。每次启动白等 50~60 毫秒，换来一条
// `avc: denied { read } ... unix_stream_socket` 审计痕迹，而判据本身没有任何功能
// 消费者（只进 `perf post` 与结论日志）。执行方的结论由执行方自己记
// （`perf companion mount ... ok=` / `companion fuse view unavailable reason=`）。
//
// 因此这里是一次**观测**，不是等待：不轮询、不睡眠、不占预算。
//
// 日志里的 `app mount confirmed pid=` 前缀被测试流匹配，改动前必须同步 .github/tests 下
// 的检测式；前缀本身与 `pid=` 之后紧邻的取值不能改。结论分两级，措辞不得混淆：
// `app mount confirmed`（观测到重定向生效）／`app mount unconcluded`（这一刻观测不到
// 证据——**没有结论**，不是失败，info）。
fn conclude_mount_readiness(
    package_name: &str,
    app_pid: i32,
    app_uid: i32,
    is_mount_request_sent: bool,
    is_mapping_mode_only: bool,
    is_mount_applied_out: &mut bool,
) {
    *is_mount_applied_out = false;

    if app_pid <= 0 {
        log::warn!("mount ctx invalid, skip wait");
        return;
    }

    if !is_mount_request_sent {
        log::warn!("mount req not sent, wait daemon mount fallback");
    }

    let observed_started_ms = monotonic_ms();
    let mounts = app_redirect_mounts_in(0, package_name);
    let mount_count = mounts.len();
    // 就绪证据分三条，全部与模块标识无关：
    // 1. 仅映射模式：本就不建立存储根重定向；
    // 2. 沙箱根（`<包名>/sdcard`）出现在 `root` 字段——命名空间 bind 后端的判据；
    // 3. collapsed 形态：视图根的设备号已换成模块会话的（见 `storage_view_root_is_module_session`）。
    let observed_ready = is_mapping_mode_only
        || mounts.iter().any(|mount| mount.is_sandbox_root)
        || storage_view_root_is_module_session(crate::platform::user_id_from_uid(app_uid));
    let observed_ms = monotonic_ms().saturating_sub(observed_started_ms);
    *is_mount_applied_out = observed_ready;

    if observed_ready {
        log::info!(
            "app mount confirmed pid={} mounts={} observed_ms={} pkg={}",
            app_pid,
            mount_count,
            observed_ms,
            package_name
        );
        return;
    }
    // 「这一刻看不见」不等于失败：应用可能在挂载落定之前就跑到了这里（真机实测：同一次
    // 启动里该 pid 稍后 `/storage/emulated` 的设备号就是本会话的）。按 info 记，并带上
    // 是否发出过请求，便于区分「没发」与「发了但还没落定」。
    log::info!(
        "app mount unconcluded pid={} mounts={} observed_ms={} map_only={} sent={} pkg={}",
        app_pid,
        mount_count,
        observed_ms,
        is_mapping_mode_only,
        is_mount_request_sent,
        package_name
    );
}

fn log_post_perf(
    flow: &RuntimeFlow,
    exit_reason: &str,
    mount_check_ms: i64,
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
        "perf post pkg={} pid={} exit={} redirect={} monitor={} hook_redirect={} boot_lite={} fuse_fix={} mount_sent={} mount_applied={} mount_check_ms={} hook_ms={} anti_ms={} total_ms={}",
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
        mount_check_ms,
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
    is_app_redirect_owner: bool,
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
    hub.set_redirect_via_hook(is_redirect_via_hook);
    if is_app_write_redirect {
        hub.init_app_write_redirect(package_name, should_monitor);
    } else if is_boot_lite {
        hub.init_boot_lite(package_name, should_monitor);
    } else {
        hub.init(package_name, is_monitor_only, should_monitor);
    }
    if is_monitor_only && is_app_redirect_owner {
        // 自身配置了重定向规则的桥接进程：hook 符号集保持 Monitor profile，但写操作
        // 决策链放行到 PathRouter（见 effective_monitor_only）。这是挂载层全部失效时
        // 对 sandboxed_paths 的进程内 fail-closed 兕底。
        hub.set_app_redirect_owner(true);
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
