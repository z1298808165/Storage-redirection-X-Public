// 应用 specialize 后流程：确认挂载结论并安装 PLT Hook
use super::RuntimeFlow;
use super::companion_ack::{
    CompanionMountAck, await_mount_ack, discard_mount_reply, take_latest_reply,
};
use super::mount_timing;
use crate::hook::{InterceptHub, install_fuse_fix_if_enabled};
use crate::java_hook;
use crate::module_mount_source::{app_redirect_mounts_in, storage_view_root_is_fuse_mount};
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
        // 回执连接在任何分支之前取出：本函数有多条提前返回，漏掉一条就会把伴生进程的
        // socket 留在应用进程里——既是资源泄漏，也会在 `/proc/self/fd` 下多出一条可观测痕迹。
        let pending_reply_fd = self.take_mount_reply_fd();
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
            discard_mount_reply(pending_reply_fd);
            log_post_perf(self, "skip", 0, 0, 0, perf_started_ms);
            return;
        }

        if !self.should_redirect && !self.should_monitor && !self.should_install_fuse_fix {
            discard_mount_reply(pending_reply_fd);
            log_post_perf(self, "bypass", 0, 0, 0, perf_started_ms);
            return;
        }

        let mut mount_wait_ms = 0;
        if self.should_redirect && !self.is_system_writer_hook_redirect {
            if policy::is_file_monitor_bridge_package(&self.package_name) {
                // 桥接进程保留自身重定向时不挂模块 FUSE 会话
                // （scoped_fuse_mount_roots_for_request 对这类请求返回空根），
                // 原生写入的沙盒重定向由进程内 hook 兜底（is_app_redirect_owner），
                // 无需发送/等待挂载，应用立即恢复运行。同时启动配置监视线程，
                // 保证用户在管理界面修改规则后秒级生效（无会话可重挂，只能靠
                // 进程内路由重建）。
                self.is_mount_applied = false;
                discard_mount_reply(pending_reply_fd);
                crate::hook::config_watcher::start_for(self.package_name.clone());
                log::info!("monitor bridge hook redirect (skip mount wait)");
            } else if platform::is_isolated_uid(self.app_uid) {
                self.is_mount_applied = false;
                discard_mount_reply(pending_reply_fd);
                log::info!(
                    "isolated uid skip mount wait uid={} pid={}",
                    self.app_uid,
                    self.app_pid
                );
            } else {
                let mount_started_ms = monotonic_ms();
                // pre 阶段通常已经发出请求并保留了回执连接；这里只补发"延迟负载"这一类。
                self.send_deferred_mount_request();
                let reply_fd = take_latest_reply(pending_reply_fd, self.take_mount_reply_fd());
                conclude_mount_readiness(
                    &self.package_name,
                    self.app_pid,
                    self.app_uid,
                    reply_fd,
                    self.is_mount_request_sent,
                    self.is_mapping_mode_only,
                    &mut self.is_mount_applied,
                );
                mount_wait_ms = monotonic_ms().saturating_sub(mount_started_ms);
            }
        } else if self.should_redirect && self.is_system_writer_hook_redirect {
            self.is_mount_applied = false;
            discard_mount_reply(pending_reply_fd);
            log::info!("writer per-caller hook map (skip mount wait)");
        } else {
            // 只监视、不重定向：没有挂载请求，回执连接理论上不存在，仍按纪律释放。
            discard_mount_reply(pending_reply_fd);
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

// 确认本次挂载请求的结论。
//
// 判据是**挂载执行方的回执**，不是应用自己读挂载表：回执由真正执行挂载的进程在挂载落定
// 后发出（账本登记已提前到回执之前），与挂载源、锚点路径、命名空间结构都无关。这样应用侧
// 的判据不再要求"挂载表里留着模块标识"，与"不留痕迹"的目标不再冲突；也正因为不依赖标识，
// 挂载源原生化之后这里不需要任何改动。
//
// 拿不到回执（通道未建立、对端过慢）时才做**一次**观测兜底——只观测一次，不轮询：轮询在
// 这里既耗电又掩盖真实结论，而"等待"本身已经由阻塞读变成了事件驱动。
//
// 日志里的 `app mount confirmed pid=` 前缀被测试流匹配，改动前必须同步 .github/tests 下的
// 检测式；前缀本身与 `pid=` 之后紧邻的取值不能改。
fn conclude_mount_readiness(
    package_name: &str,
    app_pid: i32,
    app_uid: i32,
    reply_fd: libc::c_int,
    is_mount_request_sent: bool,
    is_mapping_mode_only: bool,
    is_mount_applied_out: &mut bool,
) {
    *is_mount_applied_out = false;
    let wait_started_ms = monotonic_ms();

    if app_pid <= 0 {
        log::warn!("mount ctx invalid, skip wait");
        discard_mount_reply(reply_fd);
        return;
    }

    if !is_mount_request_sent {
        log::warn!("mount req not sent, wait daemon mount fallback");
    }

    let budget_ms = mount_timing::post_mount_ack_wait_budget_ms();
    log::info!(
        "wait mount ack pid={} budget_ms={} reply={} map_only={}",
        app_pid,
        budget_ms,
        reply_fd >= 0,
        is_mapping_mode_only
    );

    let ack = await_mount_ack(reply_fd, budget_ms);
    let elapsed_ms = monotonic_ms().saturating_sub(wait_started_ms);

    // 一次性观测：回执可用时只用于诊断（挂载表里能不能看见对应层），拿不到回执时才是兜底
    // 判据。这里沿用既有的归属判据，因为兜底路径本来就走的是"能认出自己挂载"的情形。
    let mounts = app_redirect_mounts_in(0, package_name);
    let mount_count = mounts.len();
    // 就绪证据分三条，全部与模块标识无关：
    // 1. 仅映射模式：本就不建立存储根重定向；
    // 2. 沙箱根（`<包名>/sdcard`）出现在 `root` 字段——命名空间 bind 后端的判据；
    // 3. collapsed 形态：存储视图根本身就是 fuse 挂载（见 `storage_view_root_is_fuse_mount`）。
    let observed_ready = is_mapping_mode_only
        || mounts.iter().any(|mount| mount.is_sandbox_root)
        || storage_view_root_is_fuse_mount(crate::platform::user_id_from_uid(app_uid));

    let is_applied = match ack {
        // 回执就是结论：执行方已经把挂载落定，且账本已先行登记。
        CompanionMountAck::Applied => true,
        CompanionMountAck::Failed => false,
        // 通道不可用或超时：退回一次性观测，与旧判据等价（但不再反复轮询）。
        CompanionMountAck::Unavailable | CompanionMountAck::Timeout => observed_ready,
    };
    *is_mount_applied_out = is_applied;

    if is_applied {
        log::info!(
            "app mount confirmed pid={} mounts={} elapsed_ms={} ack={} observed_ready={}",
            app_pid,
            mount_count,
            elapsed_ms,
            ack.as_str(),
            observed_ready
        );
    } else {
        // 对重定向应用这通常意味着"存储根重定向没建立起来"，应用读到的是未重定向的真实视图；
        // `ack=failed` 与 `ack=timeout` 要分开看：前者是执行方明确失败，后者是没等到结论。
        log::warn!(
            "mount unknown/failed pid={} mounts={} elapsed_ms={} ack={} observed_ready={} map_only={}",
            app_pid,
            mount_count,
            elapsed_ms,
            ack.as_str(),
            observed_ready,
            is_mapping_mode_only
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
