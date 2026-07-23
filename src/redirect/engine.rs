// 重定向决策引擎：统一处理普通应用与系统代写进程的路径重定向
mod caller;
mod policy;
mod trace;

use self::caller::{
    SystemWriterCallerContext, SystemWriterCallerSignal,
    has_system_writer_mapping_request_owner_hint, has_system_writer_recent_public_caller_hint,
    is_media_provider_internal_without_caller, resolve_system_writer_caller_context,
};
use self::policy::{
    SystemWriterPolicyRequest, process_system_writer_policy, resolve_system_writer_enablement,
};
use self::trace::{
    SystemWriterPolicyTiming, SystemWriterTrace, WriterRedirectPerf, log_redirect_perf,
    log_writer_redirect_perf,
};
use super::policy as redirect_policy;
use super::router::{RedirectAction, RedirectDecision};
use super::writer;
use crate::config::{SettingsHub, watcher};
use crate::hook::stats::InterceptHub;
use crate::monitor::AuditTrail;
use crate::platform::{self, paths};
use std::sync::atomic::{AtomicU64, Ordering};

const WRITER_ALLOWED_LOG_STEP: u64 = 256;
const REDIRECT_SLOW_MS: i64 = 5;
const REDIRECT_SAMPLE_STEP: u64 = 2048;
const WRITER_CONFIG_RELOAD_INTERVAL_MS: i64 = 1000;
static WRITER_ALLOWED_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
static REDIRECT_DECISION_COUNT: AtomicU64 = AtomicU64::new(0);
static WRITER_LAST_CONFIG_RELOAD_CHECK_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn should_bypass_system_writer_provider_passthrough(is_system_writer_process: bool) -> bool {
    is_system_writer_process && crate::hook::is_provider_passthrough_active()
}

#[inline]
fn should_log_step(count: u64, step: u64) -> bool {
    count == 1 || count.is_multiple_of(step)
}

struct NormalizedRequestPath {
    path: String,
    is_data_media_input: bool,
}

struct SystemWriterRedirectRequest<'a> {
    hub: &'a InterceptHub,
    package_name: String,
    self_uid: i32,
    is_shared_uid: bool,
    is_explicit_caller_decision: bool,
    is_write_operation: bool,
    pathname: &'a str,
    perf_started_ms: i64,
}

fn normalize_request_path(pathname: &str) -> NormalizedRequestPath {
    let raw_normalized = paths::normalize(pathname);
    let is_data_media_input = paths::starts_with(&raw_normalized, "/data/media/");
    let path = if is_data_media_input {
        writer::data_media_to_storage_path(&raw_normalized)
    } else {
        raw_normalized
    };
    NormalizedRequestPath {
        path,
        is_data_media_input,
    }
}

fn process_provider_passthrough_redirect(
    package_name: &str,
    pathname: &str,
    perf_started_ms: i64,
) -> RedirectDecision {
    let normalized = normalize_request_path(pathname);
    let normalized_path = normalized.path;
    let decision = RedirectDecision {
        action: RedirectAction::Allow,
        new_path: String::new(),
        is_mapping: false,
    };
    log_writer_redirect_perf(&WriterRedirectPerf {
        package_name,
        exit_reason: "provider_passthrough",
        caller_package: "",
        path: pathname,
        normalized_path: &normalized_path,
        reload_ms: 0,
        caller_ms: 0,
        enable_ms: 0,
        mapping_ms: 0,
        allow_ms: 0,
        fallback_ms: 0,
        total_ms: paths::monotonic_ms().saturating_sub(perf_started_ms),
        decision: &decision,
    });
    decision
}

fn process_app_mount_namespace_redirect(
    package_name: &str,
    self_uid: i32,
    pathname: &str,
    is_write_operation: bool,
    perf_started_ms: i64,
) -> RedirectDecision {
    let normalized = normalize_request_path(pathname);
    let normalized_path = normalized.path;
    let is_data_media = normalized.is_data_media_input;
    let user_id = platform::user_id_from_uid(self_uid);
    let resolved_path = paths::resolve_user_path(&normalized_path, user_id);
    let router = super::router::PathRouter::instance();
    if writer::is_path_in_user_storage(&resolved_path, user_id) {
        let read_only_check_path = if is_write_operation {
            router.read_only_check_path(&resolved_path)
        } else {
            String::new()
        };
        if !read_only_check_path.is_empty() {
            let decision = RedirectDecision {
                action: RedirectAction::DenyReadOnly,
                is_mapping: read_only_check_path != resolved_path,
                new_path: read_only_check_path,
            };
            log_redirect_perf(
                "app",
                package_name,
                "read_only",
                pathname,
                &resolved_path,
                perf_started_ms,
                &decision,
            );
            return decision;
        }
        let mapped_path = router.map_path(&resolved_path);
        if !mapped_path.is_empty() && mapped_path != resolved_path {
            let decision = RedirectDecision {
                action: RedirectAction::Redirect,
                new_path: resolve_system_writer_output_path(
                    &normalized_path,
                    &mapped_path,
                    is_data_media,
                    package_name,
                ),
                is_mapping: true,
            };
            log_redirect_perf(
                "app",
                package_name,
                "mapping",
                pathname,
                &resolved_path,
                perf_started_ms,
                &decision,
            );
            return decision;
        }
        if !is_write_operation && router.is_path_readable_by_read_only_rule(&resolved_path) {
            let decision = RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            };
            log_redirect_perf(
                "app",
                package_name,
                "read_only_read",
                pathname,
                &resolved_path,
                perf_started_ms,
                &decision,
            );
            return decision;
        }
        if is_write_operation
            && router.is_path_allowed_real(&resolved_path)
            && allowed_media_write_needs_backend_redirect(&resolved_path, user_id)
        {
            let backend_path = writer::storage_to_data_media_path(&resolved_path);
            if !backend_path.is_empty() && backend_path != resolved_path {
                let decision = RedirectDecision {
                    action: RedirectAction::Redirect,
                    new_path: backend_path,
                    is_mapping: false,
                };
                log_redirect_perf(
                    "app",
                    package_name,
                    "allowed_media_backend",
                    pathname,
                    &resolved_path,
                    perf_started_ms,
                    &decision,
                );
                return decision;
            }
        }
        if !is_write_operation
            && router.is_path_mapping_target(&resolved_path)
            && !router.is_path_allowed_real(&resolved_path)
        {
            let redirect_target = router.redirect_target();
            let fallback_path =
                writer::map_path_by_caller_fallback(&resolved_path, &redirect_target, user_id);
            let backend_path = writer::storage_to_data_media_path(&fallback_path);
            if !backend_path.is_empty() && backend_path != resolved_path {
                let decision = RedirectDecision {
                    action: RedirectAction::Redirect,
                    new_path: backend_path,
                    is_mapping: false,
                };
                log_redirect_perf(
                    "app",
                    package_name,
                    "mapping_target_read",
                    pathname,
                    &resolved_path,
                    perf_started_ms,
                    &decision,
                );
                return decision;
            }
        }
        if !(router.is_path_excluded(&resolved_path) || router.is_path_sandboxed(&resolved_path)) {
            let decision = RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            };
            log_redirect_perf(
                "app",
                package_name,
                "mount_ns",
                pathname,
                "",
                perf_started_ms,
                &decision,
            );
            return decision;
        }
        let redirect_target = router.redirect_target();
        let fallback_path =
            writer::map_path_by_caller_fallback(&resolved_path, &redirect_target, user_id);
        if !fallback_path.is_empty() && fallback_path != resolved_path {
            let decision = RedirectDecision {
                action: RedirectAction::Redirect,
                new_path: resolve_system_writer_output_path(
                    &normalized_path,
                    &fallback_path,
                    is_data_media,
                    package_name,
                ),
                is_mapping: false,
            };
            log_redirect_perf(
                "app",
                package_name,
                "excluded_hook",
                pathname,
                &resolved_path,
                perf_started_ms,
                &decision,
            );
            return decision;
        }
    }

    let decision = RedirectDecision {
        action: RedirectAction::Allow,
        new_path: String::new(),
        is_mapping: false,
    };
    log_redirect_perf(
        "app",
        package_name,
        "mount_ns",
        pathname,
        "",
        perf_started_ms,
        &decision,
    );
    decision
}

fn allowed_media_write_needs_backend_redirect(resolved_path: &str, user_id: i32) -> bool {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(resolved_path, &storage_root) else {
        return false;
    };
    let Some(first) = relative.split('/').find(|part| !part.is_empty()) else {
        return false;
    };
    matches!(first, "DCIM" | "Pictures" | "Movies")
}

// 根据进程身份决策路径重定向，系统代写进程使用按调用方映射
pub fn process_redirect_path(hub: &InterceptHub, pathname: &str) -> RedirectDecision {
    process_redirect_path_inner(hub, pathname, false)
}

pub fn process_write_redirect_path(hub: &InterceptHub, pathname: &str) -> RedirectDecision {
    process_redirect_path_inner(hub, pathname, true)
}

fn process_redirect_path_inner(
    hub: &InterceptHub,
    pathname: &str,
    is_write_operation: bool,
) -> RedirectDecision {
    let perf_started_ms = paths::monotonic_ms();
    let self_uid = unsafe { libc::getuid() as i32 };
    let package_name = hub.get_package_name();
    let is_shared_uid = redirect_policy::is_shared_uid_process(self_uid);
    let is_system_writer_process =
        redirect_policy::is_system_writer_package(&package_name) || is_shared_uid;
    let is_explicit_caller_decision = crate::hook::is_explicit_caller_decision_active();
    if should_bypass_system_writer_provider_passthrough(is_system_writer_process) {
        return process_provider_passthrough_redirect(&package_name, pathname, perf_started_ms);
    }
    if !is_system_writer_process {
        return process_app_mount_namespace_redirect(
            &package_name,
            self_uid,
            pathname,
            is_write_operation,
            perf_started_ms,
        );
    }

    process_system_writer_redirect(SystemWriterRedirectRequest {
        hub,
        package_name,
        self_uid,
        is_shared_uid,
        is_explicit_caller_decision,
        is_write_operation,
        pathname,
        perf_started_ms,
    })
}

fn process_system_writer_redirect(request: SystemWriterRedirectRequest<'_>) -> RedirectDecision {
    let SystemWriterRedirectRequest {
        hub,
        package_name,
        self_uid,
        is_shared_uid,
        is_explicit_caller_decision,
        is_write_operation,
        pathname,
        perf_started_ms,
    } = request;

    let reload_started_ms = paths::monotonic_ms();
    let mut reload_ms = 0;
    // 非阻塞检查 inotify 事件，有配置变更时触发重载
    if watcher::poll_changed() {
        crate::hook::refresh_runtime_config_after_disk_change();
        reload_ms = paths::monotonic_ms().saturating_sub(reload_started_ms);
    }
    if reload_ms == 0 {
        reload_ms = reload_writer_config_by_fingerprint(reload_started_ms);
    }

    if pathname.is_empty() {
        return SystemWriterTrace {
            package_name: &package_name,
            normalized_path: "",
            pathname,
            reload_ms,
            perf_started_ms,
        }
        .allow("empty_path", "", 0, 0);
    }

    let normalized = normalize_request_path(pathname);
    let normalized_path = normalized.path;
    let is_data_media = normalized.is_data_media_input;
    let writer_trace = SystemWriterTrace {
        package_name: &package_name,
        pathname,
        normalized_path: &normalized_path,
        reload_ms,
        perf_started_ms,
    };

    let caller_started_ms = paths::monotonic_ms();
    let mut caller_signal = SystemWriterCallerSignal::from_hub(
        hub,
        &package_name,
        self_uid,
        is_explicit_caller_decision,
    );

    let user_id = writer::resolve_system_writer_user_id(
        &normalized_path,
        &mut caller_signal.effective_caller_uid,
    );
    let has_anonymous_caller = !caller_signal.has_external_caller_signal;
    let has_anonymous_private_owner_hint = is_write_operation
        && has_anonymous_caller
        && resolve_android_private_path_owner(&normalized_path).is_some();
    let has_anonymous_mapping_request_owner_hint = has_anonymous_caller
        && has_system_writer_mapping_request_owner_hint(user_id, &normalized_path);
    let has_anonymous_redirect_owner_hint = has_anonymous_caller
        && (has_anonymous_private_owner_hint
            || has_anonymous_mapping_request_owner_hint
            || (is_write_operation
                && redirect_policy::is_media_provider_package(&package_name)
                && has_system_writer_recent_public_caller_hint(user_id, &normalized_path)));
    if !caller_signal.has_external_caller_signal
        && !has_anonymous_mapping_request_owner_hint
        && let Some(self_rule) = resolve_system_writer_self_explicit_rule(
            &package_name,
            self_uid,
            user_id,
            &normalized_path,
            is_data_media,
            is_write_operation,
            true,
        )
    {
        writer_trace.log(
            self_rule.exit_reason,
            &package_name,
            paths::monotonic_ms().saturating_sub(caller_started_ms),
            self_rule.enable_ms,
            SystemWriterPolicyTiming {
                mapping_ms: self_rule.mapping_ms,
                fallback_ms: self_rule.fallback_ms,
                ..Default::default()
            },
            &self_rule.decision,
        );
        return self_rule.decision;
    }

    if redirect_policy::is_media_provider_package(&package_name)
        && !caller_signal.has_external_caller_signal
        && !has_anonymous_redirect_owner_hint
    {
        return writer_trace.allow(
            "self_rule_miss",
            &caller_signal.effective_caller_package,
            paths::monotonic_ms().saturating_sub(caller_started_ms),
            0,
        );
    }

    if is_media_provider_internal_without_caller(
        &package_name,
        caller_signal.has_external_caller_signal,
        &caller_signal.effective_caller_package,
    ) && !has_anonymous_redirect_owner_hint
    {
        return writer_trace.allow(
            "internal_thread",
            "",
            paths::monotonic_ms().saturating_sub(caller_started_ms),
            0,
        );
    }

    let caller_context = resolve_system_writer_caller_context(
        hub,
        caller_signal.into_resolve_request(
            &package_name,
            self_uid,
            is_shared_uid,
            &normalized_path,
            user_id,
            is_write_operation,
        ),
    );
    let SystemWriterCallerContext {
        effective_caller_uid,
        effective_caller_package,
        user_id,
        is_caller_from_inferred,
    } = caller_context;

    let caller_ms = paths::monotonic_ms().saturating_sub(caller_started_ms);
    if effective_caller_package.is_empty() {
        writer::log_system_writer_caller_unresolved(&package_name, effective_caller_uid, pathname);
        return writer_trace.allow("caller_empty", &effective_caller_package, caller_ms, 0);
    }

    AuditTrail::instance().update_caller_package(&effective_caller_package);

    if user_id < 0 {
        writer::log_system_writer_user_unresolved(
            &effective_caller_package,
            effective_caller_uid,
            pathname,
        );
        return writer_trace.allow("user_empty", &effective_caller_package, caller_ms, 0);
    }

    let resolved_path = paths::resolve_user_path(&normalized_path, user_id);
    let config = SettingsHub::instance();
    let enablement = resolve_system_writer_enablement(
        config,
        &effective_caller_package,
        effective_caller_uid,
        user_id,
    );
    if !enablement.is_enabled() {
        if let Some(owner_rule) = resolve_disabled_caller_private_owner_backend_rule(
            &package_name,
            &effective_caller_package,
            effective_caller_uid,
            user_id,
            &normalized_path,
            &resolved_path,
        ) {
            let enable_ms = enablement.enable_ms.saturating_add(owner_rule.enable_ms);
            writer_trace.log(
                "private_owner_sqlite_backend",
                &effective_caller_package,
                caller_ms,
                enable_ms,
                SystemWriterPolicyTiming::default(),
                &owner_rule.decision,
            );
            return owner_rule.decision;
        }
        writer::log_system_writer_redirect_disabled(
            &effective_caller_package,
            effective_caller_uid,
            pathname,
        );
        return writer_trace.allow(
            "disabled",
            &effective_caller_package,
            caller_ms,
            enablement.enable_ms,
        );
    }

    if !writer::is_path_in_user_storage(&resolved_path, user_id) {
        return writer_trace.allow(
            "outside_storage",
            &effective_caller_package,
            caller_ms,
            enablement.enable_ms,
        );
    }

    if is_data_media
        && is_system_writer_default_sandbox_path(
            &resolved_path,
            &effective_caller_package,
            user_id,
            &package_name,
        )
    {
        return writer_trace.allow(
            "private_backend_input",
            &effective_caller_package,
            caller_ms,
            enablement.enable_ms,
        );
    }

    process_system_writer_policy(SystemWriterPolicyRequest {
        package_name: &package_name,
        caller_package: &effective_caller_package,
        caller_uid: effective_caller_uid,
        user_id,
        normalized_path: &normalized_path,
        resolved_path: &resolved_path,
        pathname,
        is_data_media,
        is_caller_from_inferred,
        redirect_enablement: enablement.redirect,
        reload_ms,
        caller_ms,
        enable_ms: enablement.enable_ms,
        perf_started_ms,
        is_write_operation,
    })
}

pub fn record_redirect_hit(hub: &InterceptHub, op_name: &str, from_path: &str, to_path: &str) {
    log::trace!("{}: {} -> {}", op_name, from_path, to_path);
    hub.increment_total_redirected();
}

fn reload_writer_config_by_fingerprint(reload_started_ms: i64) -> i64 {
    let last_ms = WRITER_LAST_CONFIG_RELOAD_CHECK_MS.load(Ordering::Relaxed) as i64;
    if reload_started_ms.saturating_sub(last_ms) < WRITER_CONFIG_RELOAD_INTERVAL_MS {
        return 0;
    }
    if WRITER_LAST_CONFIG_RELOAD_CHECK_MS
        .compare_exchange(
            last_ms as u64,
            reload_started_ms as u64,
            Ordering::Relaxed,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return 0;
    }

    // MediaProvider 可能在进程重启或配置目录重建期间漏掉 inotify 事件。
    // 限流的指纹探测使调用方缓存始终对应磁盘上的配置版本。
    SettingsHub::instance().reload_if_changed();
    crate::hook::refresh_runtime_config_from_settings();
    paths::monotonic_ms().saturating_sub(reload_started_ms)
}

struct SystemWriterSelfRule {
    exit_reason: &'static str,
    decision: RedirectDecision,
    enable_ms: i64,
    mapping_ms: i64,
    fallback_ms: i64,
}

fn resolve_system_writer_self_explicit_rule(
    package_name: &str,
    self_uid: i32,
    user_id: i32,
    normalized_path: &str,
    is_data_media: bool,
    is_write_operation: bool,
    include_mappings: bool,
) -> Option<SystemWriterSelfRule> {
    if !(redirect_policy::is_system_writer_package(package_name)
        || redirect_policy::is_shared_uid_process(self_uid))
        || self_uid < writer::ANDROID_APP_UID_START
        || user_id < 0
        || normalized_path.is_empty()
    {
        return None;
    }

    let enable_started_ms = paths::monotonic_ms();
    let config = SettingsHub::instance();
    let self_rule_package = resolve_system_writer_self_rule_package(package_name, self_uid, config);
    if self_rule_package.is_empty() {
        return None;
    }
    let self_redirect_package = if redirect_policy::is_media_provider_package(package_name) {
        package_name
    } else {
        self_rule_package.as_str()
    };

    let enablement = config.get_user_redirect_enablement(&self_rule_package, self_uid, user_id);
    let enable_ms = paths::monotonic_ms().saturating_sub(enable_started_ms);
    if !enablement.is_enabled() {
        return None;
    }

    let resolved_path = paths::resolve_user_path(normalized_path, user_id);
    if !writer::is_path_in_user_storage(&resolved_path, user_id)
        || is_system_writer_default_sandbox_path(
            &resolved_path,
            self_redirect_package,
            user_id,
            package_name,
        )
    {
        return None;
    }

    let mapping_started_ms = paths::monotonic_ms();
    let mut mapping_ms = 0;
    if include_mappings {
        let caller_mappings = writer::get_caller_mappings(&self_rule_package, self_uid);
        let mapped_path = writer::map_path_by_caller_mappings(&resolved_path, &caller_mappings);
        mapping_ms = paths::monotonic_ms().saturating_sub(mapping_started_ms);
        if !mapped_path.is_empty() && mapped_path != resolved_path {
            if is_write_operation
                && !writer::is_path_excluded_by_caller_real_paths(
                    &mapped_path,
                    &self_rule_package,
                    self_uid,
                )
                && writer::is_path_read_only_by_caller_paths(
                    &mapped_path,
                    &self_rule_package,
                    self_uid,
                )
            {
                log::debug!(
                    "writer self readonly deny mapped caller={} uid={} from={} to={}",
                    self_rule_package,
                    self_uid,
                    resolved_path,
                    mapped_path
                );
                return Some(SystemWriterSelfRule {
                    exit_reason: "self_mapping_read_only",
                    decision: RedirectDecision {
                        action: RedirectAction::DenyReadOnly,
                        new_path: mapped_path,
                        is_mapping: true,
                    },
                    enable_ms,
                    mapping_ms,
                    fallback_ms: 0,
                });
            }
            let new_path = resolve_system_writer_output_path(
                normalized_path,
                &mapped_path,
                is_data_media,
                package_name,
            );
            log::debug!(
                "writer self map caller={} uid={} from={} to={}",
                self_rule_package,
                self_uid,
                resolved_path,
                new_path
            );
            return Some(SystemWriterSelfRule {
                exit_reason: "self_mapping",
                decision: RedirectDecision {
                    action: RedirectAction::Redirect,
                    new_path,
                    is_mapping: true,
                },
                enable_ms,
                mapping_ms,
                fallback_ms: 0,
            });
        }
    }

    if is_write_operation
        && writer::is_caller_path_read_only(&resolved_path, &self_rule_package, self_uid)
    {
        log::debug!(
            "writer self readonly deny caller={} uid={} path={}",
            self_rule_package,
            self_uid,
            resolved_path
        );
        return Some(SystemWriterSelfRule {
            exit_reason: "self_read_only",
            decision: RedirectDecision {
                action: RedirectAction::DenyReadOnly,
                new_path: resolved_path,
                is_mapping: false,
            },
            enable_ms,
            mapping_ms,
            fallback_ms: 0,
        });
    }

    if !is_write_operation
        && writer::is_caller_path_read_only(&resolved_path, &self_rule_package, self_uid)
    {
        return Some(SystemWriterSelfRule {
            exit_reason: "self_read_only_read",
            decision: RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            },
            enable_ms,
            mapping_ms,
            fallback_ms: 0,
        });
    }

    if !writer::is_path_sandboxed_by_caller_paths(&resolved_path, &self_rule_package, self_uid) {
        return None;
    }

    let fallback_started_ms = paths::monotonic_ms();
    let redirect_target = resolve_system_writer_self_redirect_target(
        &self_rule_package,
        self_redirect_package,
        self_uid,
        user_id,
    );
    let sandbox_path =
        writer::map_path_by_caller_fallback(&resolved_path, &redirect_target, user_id);
    let fallback_ms = paths::monotonic_ms().saturating_sub(fallback_started_ms);
    if sandbox_path.is_empty() || sandbox_path == resolved_path {
        return None;
    }

    let new_path = resolve_system_writer_output_path(
        normalized_path,
        &sandbox_path,
        is_data_media,
        package_name,
    );
    log::debug!(
        "writer self sandbox caller={} uid={} from={} to={}",
        self_rule_package,
        self_uid,
        resolved_path,
        new_path
    );
    Some(SystemWriterSelfRule {
        exit_reason: "self_sandbox",
        decision: RedirectDecision {
            action: RedirectAction::Redirect,
            new_path,
            is_mapping: false,
        },
        enable_ms,
        mapping_ms,
        fallback_ms,
    })
}

fn resolve_system_writer_self_rule_package(
    package_name: &str,
    self_uid: i32,
    config: &SettingsHub,
) -> String {
    let user_id = platform::user_id_from_uid(self_uid);
    if redirect_policy::is_system_writer_package(package_name) {
        if redirect_policy::is_media_provider_package(package_name) {
            for candidate in redirect_policy::media_provider_package_aliases(package_name) {
                let enablement = config.get_user_redirect_enablement(candidate, self_uid, user_id);
                if enablement.is_enabled() {
                    return candidate.to_string();
                }
            }
        } else {
            let enablement = config.get_user_redirect_enablement(package_name, self_uid, user_id);
            if enablement.is_enabled() {
                return package_name.to_string();
            }
        }
    }

    let mut candidates = redirect_policy::get_packages_for_uid(self_uid)
        .into_iter()
        .filter(|pkg| redirect_policy::is_system_writer_package(pkg))
        .filter(|pkg| {
            config
                .get_user_redirect_enablement(pkg, self_uid, user_id)
                .is_enabled()
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    if candidates.len() == 1 {
        candidates.remove(0)
    } else {
        String::new()
    }
}

fn resolve_system_writer_self_redirect_target(
    config_package: &str,
    redirect_package: &str,
    self_uid: i32,
    user_id: i32,
) -> String {
    if config_package == redirect_package {
        return writer::resolve_system_writer_redirect_target(
            config_package,
            self_uid,
            user_id,
            false,
        );
    }

    if !SettingsHub::instance()
        .get_user_redirect_enablement(config_package, self_uid, user_id)
        .is_enabled()
    {
        return String::new();
    }

    let target = paths::resolve_user_path(
        &paths::normalize(&paths::default_redirect_target(redirect_package, user_id)),
        user_id,
    );
    if target.is_empty() || paths::has_unsafe_segments(&target) {
        return String::new();
    }
    target
}

fn resolve_android_private_path_owner(normalized_path: &str) -> Option<String> {
    let owner = paths::extract_android_private_path_owner(normalized_path);
    if owner.is_empty() || redirect_policy::is_media_intermediate_package(&owner) {
        None
    } else {
        Some(owner)
    }
}

fn resolve_package_uid(package_name: &str) -> i32 {
    redirect_policy::get_fresh_uid_for_package(package_name)
}

struct PrivateOwnerBackendRule {
    decision: RedirectDecision,
    enable_ms: i64,
}

fn resolve_disabled_caller_private_owner_backend_rule(
    package_name: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
    normalized_path: &str,
    resolved_path: &str,
) -> Option<PrivateOwnerBackendRule> {
    if user_id < 0
        || !redirect_policy::is_media_provider_package(package_name)
        || !writer::is_path_in_user_storage(resolved_path, user_id)
        || !paths::is_sqlite_database_or_sidecar_path(normalized_path)
    {
        return None;
    }

    let owner_package = resolve_android_private_path_owner(normalized_path)?;
    if owner_package == caller_package || redirect_policy::is_system_writer_package(&owner_package)
    {
        return None;
    }

    let owner_uid = resolve_package_uid(&owner_package);
    if owner_uid < writer::ANDROID_APP_UID_START || platform::user_id_from_uid(owner_uid) != user_id
    {
        return None;
    }

    let enablement = resolve_system_writer_enablement(
        SettingsHub::instance(),
        &owner_package,
        owner_uid,
        user_id,
    );
    if !enablement.is_enabled() {
        return None;
    }

    let backend_path = writer::storage_to_data_media_path(resolved_path);
    if backend_path.is_empty() || backend_path == resolved_path {
        return None;
    }

    log::debug!(
        "writer: private owner sqlite backend owner={} owner_uid={} caller={} caller_uid={} from={} to={}",
        owner_package,
        owner_uid,
        caller_package,
        caller_uid,
        resolved_path,
        backend_path
    );
    crate::monitor::remember_private_path_caller_hint(
        normalized_path,
        &owner_package,
        caller_package,
        caller_uid,
        user_id,
    );
    Some(PrivateOwnerBackendRule {
        decision: RedirectDecision {
            action: RedirectAction::Redirect,
            new_path: backend_path,
            is_mapping: false,
        },
        enable_ms: enablement.enable_ms,
    })
}

fn resolve_system_writer_output_path(
    _request_path: &str,
    output_path: &str,
    is_data_media_input: bool,
    package_name: &str,
) -> String {
    if is_data_media_input {
        return writer::storage_to_data_media_path(output_path);
    }

    if !redirect_policy::is_media_provider_package(package_name) {
        return output_path.to_string();
    }

    writer::storage_to_data_media_path(output_path)
}

fn resolve_system_writer_mapping_output_path(
    output_path: &str,
    is_data_media_input: bool,
    caller_package: &str,
    user_id: i32,
    package_name: &str,
) -> String {
    if is_data_media_input
        || is_system_writer_android_private_storage_path(
            output_path,
            caller_package,
            user_id,
            package_name,
        )
    {
        return writer::storage_to_data_media_path(output_path);
    }

    output_path.to_string()
}

fn resolve_system_writer_private_backend_path(
    resolved_path: &str,
    caller_package: &str,
    user_id: i32,
    package_name: &str,
) -> String {
    if is_system_writer_default_sandbox_path(resolved_path, caller_package, user_id, package_name) {
        return writer::storage_to_data_media_path(resolved_path);
    }

    String::new()
}

fn resolve_system_writer_mapping_target_backend_path(
    resolved_path: &str,
    caller_package: &str,
    user_id: i32,
    package_name: &str,
) -> String {
    if is_system_writer_android_private_storage_path(
        resolved_path,
        caller_package,
        user_id,
        package_name,
    ) {
        return writer::storage_to_data_media_path(resolved_path);
    }

    String::new()
}

fn is_system_writer_default_sandbox_path(
    resolved_path: &str,
    caller_package: &str,
    user_id: i32,
    package_name: &str,
) -> bool {
    if caller_package.is_empty() || user_id < 0 {
        return false;
    }
    if !redirect_policy::is_system_writer_package(package_name) {
        return false;
    }

    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(resolved_path, &storage_root) else {
        return false;
    };
    let sandbox_root = format!("Android/data/{}/sdcard", caller_package);
    paths::matches(&sandbox_root, relative, true)
}

fn is_system_writer_android_private_storage_path(
    resolved_path: &str,
    caller_package: &str,
    user_id: i32,
    package_name: &str,
) -> bool {
    if caller_package.is_empty() || user_id < 0 {
        return false;
    }
    if !redirect_policy::is_system_writer_package(package_name) {
        return false;
    }

    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(resolved_path, &storage_root) else {
        return false;
    };
    let private_roots = [
        format!("Android/data/{}", caller_package),
        format!("Android/media/{}", caller_package),
        format!("Android/obb/{}", caller_package),
    ];

    private_roots
        .iter()
        .any(|root| paths::matches(root, relative, true))
}

// 路径已在 Android/data、Android/media 或 Android/obb 下，无需重定向
fn is_already_private_path(path: &str) -> bool {
    let prefix = "/storage/emulated/";
    if !path.as_bytes().starts_with(prefix.as_bytes()) {
        return false;
    }
    let after_prefix = &path[prefix.len()..];
    let Some(slash) = after_prefix.find('/') else {
        return false;
    };
    let relative = &after_prefix[slash + 1..];
    paths::matches("Android/data", relative, true)
        || paths::matches("Android/media", relative, true)
        || paths::matches("Android/obb", relative, true)
}
