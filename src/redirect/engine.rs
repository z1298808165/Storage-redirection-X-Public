// 重定向决策引擎：统一处理普通应用与系统代写进程的路径重定向
mod caller;
mod policy;
mod trace;

#[cfg(test)]
use self::caller::has_external_writer_caller_signal;
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
    if !caller_signal.has_external_caller_signal && !has_anonymous_mapping_request_owner_hint {
        if let Some(self_rule) = resolve_system_writer_self_explicit_rule(
            &package_name,
            self_uid,
            user_id,
            &normalized_path,
            is_data_media,
            is_write_operation,
            true,
        ) {
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
    hub.increment_global_redirect_count();
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

    // MediaProvider can miss inotify events across process restarts or config dir recreation.
    // A throttled fingerprint probe keeps caller caches tied to the on-disk config version.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppProfile, UserProfile, UserRedirectEnablement};
    use crate::domain::PathMapping;
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};

    static APP_ROUTER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn system_writer_policy_decision(
        caller_package: &str,
        caller_uid: i32,
        path: &str,
        is_mapping_mode_only: bool,
    ) -> RedirectDecision {
        system_writer_policy_decision_with_write(
            caller_package,
            caller_uid,
            path,
            is_mapping_mode_only,
            true,
        )
    }

    fn system_writer_policy_decision_with_write(
        caller_package: &str,
        caller_uid: i32,
        path: &str,
        is_mapping_mode_only: bool,
        is_write_operation: bool,
    ) -> RedirectDecision {
        process_system_writer_policy(SystemWriterPolicyRequest {
            package_name: "com.android.providers.media.module",
            caller_package,
            caller_uid,
            user_id: 0,
            normalized_path: path,
            resolved_path: path,
            pathname: path,
            is_data_media: false,
            is_caller_from_inferred: false,
            redirect_enablement: UserRedirectEnablement {
                enabled_in_memory: true,
                has_raw_config: true,
                enabled_in_raw: true,
                is_mapping_mode_only,
            },
            reload_ms: 0,
            caller_ms: 0,
            enable_ms: 0,
            perf_started_ms: paths::monotonic_ms(),
            is_write_operation,
        })
    }

    fn anonymous_media_writer_decision(path: &str) -> RedirectDecision {
        anonymous_media_writer_decision_with_current_uid(path, -1)
    }

    fn anonymous_media_writer_decision_with_current_uid(
        path: &str,
        current_uid: i32,
    ) -> RedirectDecision {
        let hub = InterceptHub::instance();
        hub.clear_current_caller();
        if current_uid >= writer::ANDROID_APP_UID_START {
            hub.set_current_caller_uid(current_uid);
        }
        process_system_writer_redirect(SystemWriterRedirectRequest {
            hub,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: 10226,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: path,
            perf_started_ms: paths::monotonic_ms(),
        })
    }

    fn anonymous_media_query_decision(path: &str) -> RedirectDecision {
        anonymous_media_query_decision_with_current_uid(path, -1)
    }

    fn anonymous_media_query_decision_with_current_uid(
        path: &str,
        current_uid: i32,
    ) -> RedirectDecision {
        let hub = InterceptHub::instance();
        hub.clear_current_caller();
        if current_uid >= writer::ANDROID_APP_UID_START {
            hub.set_current_caller_uid(current_uid);
        }
        process_system_writer_redirect(SystemWriterRedirectRequest {
            hub,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: 10226,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: false,
            pathname: path,
            perf_started_ms: paths::monotonic_ms(),
        })
    }

    fn configure_app_router(
        package_name: &str,
        app_uid: i32,
        allowed_real_paths: &[String],
        excluded_real_paths: &[String],
        sandboxed_paths: &[String],
        read_only_paths: &[String],
        path_mappings: &[PathMapping],
        is_mapping_mode_only: bool,
    ) {
        let user_id = platform::user_id_from_uid(app_uid);
        let redirect_target = platform::paths::default_redirect_target(package_name, user_id);
        let router = crate::redirect::PathRouter::instance();
        router.init();
        router.configure(
            package_name,
            app_uid,
            &redirect_target,
            allowed_real_paths,
            excluded_real_paths,
            sandboxed_paths,
            read_only_paths,
            path_mappings,
            is_mapping_mode_only,
        );
    }

    fn lock_app_router_test() -> MutexGuard<'static, ()> {
        APP_ROUTER_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[test]
    fn system_writer_backend_only_treats_default_sandbox_as_private_backend() {
        assert!(is_system_writer_default_sandbox_path(
            "/storage/emulated/0/Android/data/xyz.nextalone.nnngram/sdcard/Nnngram/a.jpg",
            "xyz.nextalone.nnngram",
            0,
            "com.android.providers.media.module",
        ));
        assert!(is_system_writer_default_sandbox_path(
            "/storage/emulated/0/Android/data/xyz.nextalone.nnngram/sdcard",
            "xyz.nextalone.nnngram",
            0,
            "com.android.providers.media.module",
        ));
    }

    #[test]
    fn system_writer_backend_does_not_treat_android_media_as_sandbox_backend() {
        assert!(!is_system_writer_default_sandbox_path(
            "/storage/emulated/0/Android/media/xyz.nextalone.nnngram/Nnngram/a.jpg",
            "xyz.nextalone.nnngram",
            0,
            "com.android.providers.media.module",
        ));
        assert!(!is_system_writer_default_sandbox_path(
            "/storage/emulated/0/Android/obb/xyz.nextalone.nnngram/main.obb",
            "xyz.nextalone.nnngram",
            0,
            "com.android.providers.media.module",
        ));
    }

    #[test]
    fn system_writer_mapping_target_can_still_use_android_media_backend() {
        assert!(is_system_writer_android_private_storage_path(
            "/storage/emulated/0/Android/media/xyz.nextalone.nnngram/cache/a.jpg",
            "xyz.nextalone.nnngram",
            0,
            "com.android.providers.media.module",
        ));
    }

    #[test]
    fn media_provider_public_scan_without_caller_is_internal() {
        assert!(is_media_provider_internal_without_caller(
            "com.android.providers.media.module",
            false,
            "",
        ));
        assert!(!is_media_provider_internal_without_caller(
            "com.android.providers.media.module",
            true,
            "",
        ));
        assert!(!is_media_provider_internal_without_caller(
            "com.android.providers.media.module",
            false,
            "org.srx.caller",
        ));
    }

    #[test]
    fn external_caller_signal_ignores_self_and_system_writer_packages() {
        assert!(!has_external_writer_caller_signal(
            "com.android.providers.media.module",
            10217,
            "com.android.providers.media.module",
            10217,
        ));
        assert!(!has_external_writer_caller_signal(
            "com.android.providers.media.module",
            10217,
            "com.android.providers.media",
            10218,
        ));
        assert!(has_external_writer_caller_signal(
            "com.android.providers.media.module",
            10217,
            "org.srx.testapp",
            10123,
        ));
        assert!(has_external_writer_caller_signal(
            "com.android.providers.media.module",
            10217,
            "",
            10123,
        ));
    }

    #[test]
    fn system_writer_output_path_uses_backend_for_media_provider() {
        assert_eq!(
            resolve_system_writer_output_path(
                "/storage/emulated/0/DCIM/a.jpg",
                "/storage/emulated/0/Pictures/a.jpg",
                false,
                "com.android.providers.media.module",
            ),
            "/data/media/0/Pictures/a.jpg"
        );
        assert_eq!(
            resolve_system_writer_output_path(
                "/data/media/0/DCIM/a.jpg",
                "/storage/emulated/0/Pictures/a.jpg",
                true,
                "org.srx.testapp",
            ),
            "/data/media/0/Pictures/a.jpg"
        );
        assert_eq!(
            resolve_system_writer_output_path(
                "/storage/emulated/0/DCIM/a.jpg",
                "/storage/emulated/0/Pictures/a.jpg",
                false,
                "org.srx.testapp",
            ),
            "/storage/emulated/0/Pictures/a.jpg"
        );
    }

    #[test]
    fn system_writer_mapping_output_keeps_public_storage_for_media_provider() {
        assert_eq!(
            resolve_system_writer_mapping_output_path(
                "/storage/emulated/0/Download/ThirdParty/WeChat/a.zip",
                false,
                "com.tencent.mm",
                0,
                "com.android.providers.media.module",
            ),
            "/storage/emulated/0/Download/ThirdParty/WeChat/a.zip"
        );
        assert_eq!(
            resolve_system_writer_mapping_output_path(
                "/storage/emulated/0/Android/media/com.tencent.mm/a.zip",
                false,
                "com.tencent.mm",
                0,
                "com.android.providers.media.module",
            ),
            "/data/media/0/Android/media/com.tencent.mm/a.zip"
        );
    }

    #[test]
    fn path_inference_without_caller_signal_only_applies_to_private_paths() {
        assert!(!is_already_private_path(
            "/storage/emulated/0/Download/Nnngram/app.apk"
        ));
        assert!(is_already_private_path(
            "/storage/emulated/0/Android/data/xyz.nextalone.nnngram/sdcard/Download/Nnngram/app.apk"
        ));
    }

    #[test]
    fn provider_passthrough_overrides_explicit_caller_decision() {
        let _passthrough = ProviderPassthroughGuard;
        crate::hook::enter_provider_passthrough();
        let _explicit = crate::hook::enter_explicit_caller_decision();

        assert!(should_bypass_system_writer_provider_passthrough(true));
        assert!(!should_bypass_system_writer_provider_passthrough(false));
    }

    #[test]
    fn media_provider_self_rules_only_apply_to_explicit_sandbox_paths() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.providers.media.module".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/.xlDownload".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let sandbox = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/.xlDownload",
            false,
            false,
            true,
        )
        .expect("self sandboxed path should redirect");
        assert!(sandbox.decision.is_redirect());
        assert_eq!(
            sandbox.decision.new_path,
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.xlDownload"
        );

        let plain = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/Pictures",
            false,
            false,
            true,
        );
        assert!(plain.is_none());

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn media_provider_self_sandbox_uses_raw_config_when_profile_cache_is_empty() {
        let hub = SettingsHub::instance();
        let config_dir = std::env::temp_dir().join(format!(
            "srx_engine_raw_self_{}_{}",
            std::process::id(),
            paths::monotonic_ms()
        ));
        let apps_dir = config_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).expect("create temp apps dir");
        std::fs::write(
            apps_dir.join("com.android.providers.media.module.json"),
            r#"{
                "users": {
                    "0": {
                        "enabled": true,
                        "mapping_mode_only": true,
                        "sandboxed_paths": [".xlDownload"]
                    }
                }
            }"#,
        )
        .expect("write media provider raw config");

        let previous_config_dir =
            hub.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());

        let sandbox = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/.xlDownload",
            false,
            true,
            true,
        )
        .expect("raw self sandboxed path should redirect");
        assert!(sandbox.decision.is_redirect());
        assert_eq!(
            sandbox.decision.new_path,
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.xlDownload"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
        hub.replace_test_config_dir(previous_config_dir);
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_self_sandbox_accepts_raw_config_from_package_alias() {
        let hub = SettingsHub::instance();
        let config_dir = std::env::temp_dir().join(format!(
            "srx_engine_raw_alias_{}_{}",
            std::process::id(),
            paths::monotonic_ms()
        ));
        let apps_dir = config_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).expect("create temp apps dir");
        std::fs::write(
            apps_dir.join("com.android.providers.media.module.json"),
            r#"{
                "users": {
                    "0": {
                        "enabled": true,
                        "mapping_mode_only": true,
                        "sandboxed_paths": [".xlDownload"]
                    }
                }
            }"#,
        )
        .expect("write aliased media provider raw config");

        let previous_config_dir =
            hub.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::new());

        let sandbox = resolve_system_writer_self_explicit_rule(
            "com.google.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/.xlDownload",
            false,
            true,
            true,
        )
        .expect("aliased raw self sandboxed path should redirect");
        assert!(sandbox.decision.is_redirect());
        assert_eq!(
            sandbox.decision.new_path,
            "/data/media/0/Android/data/com.google.android.providers.media.module/sdcard/.xlDownload"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
        hub.replace_test_config_dir(previous_config_dir);
        let _ = std::fs::remove_dir_all(config_dir);
    }

    #[test]
    fn media_provider_self_sandbox_applies_before_external_caller_rules() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.providers.media.module".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/.xlDownload".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let sandbox = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/.xlDownload/dp_so.log",
            false,
            false,
            false,
        )
        .expect("self sandboxed path should redirect even with an external caller");
        assert!(sandbox.decision.is_redirect());
        assert_eq!(
            sandbox.decision.new_path,
            "/storage/emulated/0/Android/data/com.android.providers.media.module/sdcard/.xlDownload/dp_so.log"
        );
        assert!(!sandbox.decision.is_mapping);

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn media_provider_external_uid_skips_self_sandbox_for_unconfigured_new_app() {
        let hub = SettingsHub::instance();
        let media_uid = 10217;
        let new_app_uid = 10335;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.providers.media.module".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/MIUI/Mirror".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.media.module".to_string(), media_uid),
            ("com.yek.android.kfc.activitys".to_string(), new_app_uid),
        ]));

        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(new_app_uid);
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: media_uid,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: "/storage/emulated/0/MIUI/Mirror/MainCache",
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!decision.is_redirect());
        assert_eq!(
            hub_instance.get_current_caller_package(),
            "com.yek.android.kfc.activitys"
        );
        assert_eq!(hub_instance.get_current_caller_uid(), new_app_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn media_provider_self_rule_denies_read_only_mapping_writes() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.providers.media.module".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Pictures/Locked".to_string()],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/QQ".to_string(),
                            "/storage/emulated/0/Pictures/Locked".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let write = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/Download/QQ/a.jpg",
            false,
            true,
            true,
        )
        .expect("self mapping write should be denied by read-only target");
        assert!(write.decision.is_denied());
        assert!(write.decision.is_mapping);

        let read = resolve_system_writer_self_explicit_rule(
            "com.android.providers.media.module",
            10217,
            0,
            "/storage/emulated/0/Download/QQ/a.jpg",
            false,
            false,
            true,
        )
        .expect("self mapping read should still redirect");
        assert!(read.decision.is_redirect());
        assert_eq!(
            read.decision.new_path,
            "/data/media/0/Pictures/Locked/a.jpg"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn excluded_allowed_child_redirects_to_sandbox() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mobileqq".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec!["/storage/emulated/0/Android".to_string()],
                        excluded_real_paths: vec![
                            "/storage/emulated/0/Android/.android_lq".to_string(),
                        ],
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let allowed = writer::is_path_allowed_by_caller_real_paths(
            "/storage/emulated/0/Android/.android_lq",
            "com.tencent.mobileqq",
            10123,
        );
        assert!(allowed);
        assert!(writer::is_path_excluded_by_caller_real_paths(
            "/storage/emulated/0/Android/.android_lq",
            "com.tencent.mobileqq",
            10123,
        ));

        let redirect_target =
            writer::resolve_system_writer_redirect_target("com.tencent.mobileqq", 10123, 0, false);
        let fallback_path = writer::map_path_by_caller_fallback(
            "/storage/emulated/0/Android/.android_lq",
            &redirect_target,
            0,
        );
        assert_eq!(
            fallback_path,
            "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard/Android/.android_lq"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_prefers_explicit_mapping() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/DCIM/MyApp".to_string(),
                            "/storage/emulated/0/Pictures/MyApp".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/DCIM/MyApp/a.jpg",
            false,
        );

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Pictures/MyApp/a.jpg"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_denies_read_only_write_when_mapping_target_is_read_only() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec!["/storage/emulated/0/DCIM/MyApp".to_string()],
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/Pictures/MyApp".to_string()],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/DCIM/MyApp".to_string(),
                            "/storage/emulated/0/Pictures/MyApp".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let path = "/storage/emulated/0/DCIM/MyApp/a.jpg";
        let write_decision = system_writer_policy_decision("org.srx.testapp", 10123, path, false);
        assert!(write_decision.is_denied());
        assert!(matches!(
            write_decision.action,
            RedirectAction::DenyReadOnly
        ));
        assert!(write_decision.is_mapping);
        assert_eq!(
            write_decision.new_path,
            "/storage/emulated/0/Pictures/MyApp/a.jpg"
        );

        let read_decision =
            system_writer_policy_decision_with_write("org.srx.testapp", 10123, path, false, false);
        assert!(read_decision.is_redirect());
        assert_eq!(
            read_decision.new_path,
            "/storage/emulated/0/Pictures/MyApp/a.jpg"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_allows_mapping_request_under_read_only_parent_when_target_is_excluded()
    {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec![
                            "/storage/emulated/0/Download".to_string(),
                            "!/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
                        ],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/QQ".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/QQ".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/QQ/a.jpg",
            false,
        );

        assert!(decision.is_redirect());
        assert!(!decision.is_denied());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/QQ/a.jpg"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_lets_exclude_override_read_only() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec!["/storage/emulated/0/DCIM/Private".to_string()],
                        excluded_real_paths: vec!["/storage/emulated/0/DCIM/Private".to_string()],
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec!["/storage/emulated/0/DCIM/Private".to_string()],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/DCIM/Private/a.jpg",
            false,
        );

        assert!(decision.is_redirect());
        assert!(!matches!(decision.action, RedirectAction::DenyReadOnly));

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_lets_read_only_exclusion_write_real() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec![
                            "/storage/emulated/0/Download/SrtMonitorLocked".to_string(),
                            "!/storage/emulated/0/Download/SrtMonitorLocked/Writable".to_string(),
                        ],
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/SrtMonitorLocked/Writable/a.bin",
            false,
        );

        assert!(!decision.is_redirect());
        assert!(!matches!(decision.action, RedirectAction::DenyReadOnly));

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_falls_back_to_default_sandbox() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/file.txt",
            false,
        );

        assert!(decision.is_redirect());
        assert!(!decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/data/media/0/Android/data/org.srx.testapp/sdcard/Download/file.txt"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_redirects_rule_miss_hidden_dir_with_allowed_list() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.mms".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec![
                            "/storage/emulated/0/DCIM".to_string(),
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Download".to_string(),
                            "/storage/emulated/0/MIUI".to_string(),
                            "/storage/emulated/0/Movies".to_string(),
                            "/storage/emulated/0/Music".to_string(),
                            "/storage/emulated/0/Pictures".to_string(),
                        ],
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let public_allowed = system_writer_policy_decision(
            "com.android.mms",
            10120,
            "/storage/emulated/0/Download/visible.txt",
            false,
        );
        assert!(!public_allowed.is_redirect());

        let hidden_rule_miss = system_writer_policy_decision(
            "com.android.mms",
            10120,
            "/storage/emulated/0/.CMRcs/chatbot/icon/avatar.bin",
            false,
        );
        assert!(hidden_rule_miss.is_redirect());
        assert!(!hidden_rule_miss.is_mapping);
        assert_eq!(
            hidden_rule_miss.new_path,
            "/data/media/0/Android/data/com.android.mms/sdcard/.CMRcs/chatbot/icon/avatar.bin"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_excluded_real_path_overrides_allowed_real_path() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mobileqq".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec!["/storage/emulated/0/Android".to_string()],
                        excluded_real_paths: vec![
                            "/storage/emulated/0/Android/.android_lq".to_string(),
                        ],
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let excluded_path = "/storage/emulated/0/Android/.android_lq/cache.db";
        let excluded =
            system_writer_policy_decision("com.tencent.mobileqq", 10123, excluded_path, false);
        assert!(excluded.is_redirect());
        assert!(!excluded.is_mapping);
        assert_eq!(
            excluded.new_path,
            "/data/media/0/Android/data/com.tencent.mobileqq/sdcard/Android/.android_lq/cache.db"
        );

        let allowed_path = "/storage/emulated/0/Android/Tencent/msflogs/log.txt";
        let allowed =
            system_writer_policy_decision("com.tencent.mobileqq", 10123, allowed_path, false);
        assert!(!allowed.is_redirect());

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_applies_media_store_pending_display_name_rules() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec![
                            "/storage/emulated/0/Download/srt_qmark_?.txt".to_string(),
                        ],
                        excluded_real_paths: vec![
                            "/storage/emulated/0/Download/*.part".to_string(),
                        ],
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let allowed_pending = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/.pending-1781788689-srt_qmark_a.txt",
            false,
        );
        assert!(!allowed_pending.is_redirect());

        let excluded_pending = system_writer_policy_decision(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/.pending-77-srt_ci_probe.part",
            false,
        );
        assert!(excluded_pending.is_redirect());
        assert_eq!(
            excluded_pending.new_path,
            "/data/media/0/Android/data/org.srx.testapp/sdcard/Download/.pending-77-srt_ci_probe.part"
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn system_writer_policy_maps_media_store_pending_request_path() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let decision = system_writer_policy_decision(
            "com.tencent.mm",
            10284,
            "/storage/emulated/0/Download/Weixin/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip",
            false,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783058689-storage.redirect.x-v1.2.55-local.zip"
        );
    }

    #[test]
    fn anonymous_media_writer_mkdir_maps_request_path_with_self_uid() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([
            (
                "com.android.providers.media.module".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: true,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: vec![PathMapping::new(
                                "/storage/emulated/0/Download/DLManager".to_string(),
                                "/storage/emulated/0/Download/ThirdParty/DLManager".to_string(),
                            )],
                        },
                    )]),
                },
            ),
            (
                "com.tencent.mm".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: vec![PathMapping::new(
                                "/storage/emulated/0/Download/Weixin".to_string(),
                                "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                            )],
                        },
                    )]),
                },
            ),
        ]));

        let decision = anonymous_media_writer_decision_with_current_uid(
            "/storage/emulated/0/Download/Weixin",
            10226,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat"
        );
    }

    #[test]
    fn media_provider_self_uid_mkdir_uses_mapping_owner_not_original_path() {
        let hub = SettingsHub::instance();
        let media_uid = 10217;
        let wechat_uid = 10284;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec![
                            "/storage/emulated/0/Android".to_string(),
                            "/storage/emulated/0/DCIM".to_string(),
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Movies".to_string(),
                            "/storage/emulated/0/Music".to_string(),
                            "/storage/emulated/0/Pictures".to_string(),
                        ],
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.media.module".to_string(), media_uid),
            ("com.tencent.mm".to_string(), wechat_uid),
        ]));

        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(media_uid);
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: media_uid,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: "/storage/emulated/0/Download/Weixin",
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat"
        );
        assert_eq!(hub_instance.get_current_caller_package(), "com.tencent.mm");
        assert_eq!(hub_instance.get_current_caller_uid(), wechat_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn media_provider_external_uid_pending_uses_mapping_owner_not_original_path() {
        let hub = SettingsHub::instance();
        let media_uid = 10217;
        let wechat_uid = 10284;
        let mt_uid = 10307;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec![
                            "/storage/emulated/0/Android".to_string(),
                            "/storage/emulated/0/DCIM".to_string(),
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Movies".to_string(),
                            "/storage/emulated/0/Music".to_string(),
                            "/storage/emulated/0/Pictures".to_string(),
                        ],
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([
            ("bin.mt.plus".to_string(), mt_uid),
            ("com.android.providers.media.module".to_string(), media_uid),
            ("com.tencent.mm".to_string(), wechat_uid),
        ]));

        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(mt_uid);
        hub_instance.set_current_caller_package("bin.mt.plus");
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: media_uid,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: "/storage/emulated/0/Download/Weixin/.pending-1783081226-storage.redirect.x.zip",
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat/.pending-1783081226-storage.redirect.x.zip"
        );
        assert_eq!(hub_instance.get_current_caller_package(), "com.tencent.mm");
        assert_eq!(hub_instance.get_current_caller_uid(), wechat_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn anonymous_media_query_maps_specific_mapping_request_owner() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.mm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/Weixin".to_string(),
                            "/storage/emulated/0/Download/ThirdParty/WeChat".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let decision =
            anonymous_media_query_decision("/storage/emulated/0/Download/Weixin/.nomedia");
        let self_uid_decision = anonymous_media_query_decision_with_current_uid(
            "/storage/emulated/0/Download/Weixin/.nomedia",
            10226,
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat/.nomedia"
        );
        assert!(self_uid_decision.is_redirect());
        assert!(self_uid_decision.is_mapping);
        assert_eq!(
            self_uid_decision.new_path,
            "/storage/emulated/0/Download/ThirdParty/WeChat/.nomedia"
        );
    }

    #[test]
    fn anonymous_media_query_ignores_broad_public_mapping_request_owner() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.tencent.lolm".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Android/data/com.tencent.lolm/files".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let decision = anonymous_media_query_decision(
            "/storage/emulated/0/Documents/MTManager/apks/coolapk.apk",
        );

        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!decision.is_redirect());
        assert!(!decision.is_mapping);
    }

    #[test]
    fn system_writer_policy_map_only_sandbox_redirects_only_configured_paths() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "org.srx.testapp".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/.xlDownload".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let sandboxed_path = "/storage/emulated/0/.xlDownload/task.log";
        let sandboxed =
            system_writer_policy_decision("org.srx.testapp", 10123, sandboxed_path, true);
        assert!(sandboxed.is_redirect());
        assert!(!sandboxed.is_mapping);
        assert_eq!(
            sandboxed.new_path,
            "/data/media/0/Android/data/org.srx.testapp/sdcard/.xlDownload/task.log"
        );

        let public_path = "/storage/emulated/0/Download/keep-visible.txt";
        let public = system_writer_policy_decision("org.srx.testapp", 10123, public_path, true);
        assert!(!public.is_redirect());

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn anonymous_media_writer_uses_media_provider_self_map_only_sandbox_rule() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "com.android.providers.media.module".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: vec!["/storage/emulated/0/.CMRcs".to_string()],
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));

        let sandboxed =
            anonymous_media_writer_decision("/storage/emulated/0/.CMRcs/chatbot/icon/avatar.bin");
        assert!(sandboxed.is_redirect());
        assert!(!sandboxed.is_mapping);
        assert_eq!(
            sandboxed.new_path,
            "/data/media/0/Android/data/com.android.providers.media.module/sdcard/.CMRcs/chatbot/icon/avatar.bin"
        );

        let public = anonymous_media_writer_decision("/storage/emulated/0/Download/visible.txt");
        assert!(!public.is_redirect());

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn anonymous_media_writer_uses_recent_provider_open_hint_for_pending_display_rules() {
        let _guard = lock_app_router_test();
        crate::monitor::clear_recent_private_owner_hint_for_tests();
        let previous_uid = redirect_policy::replace_test_uid_cache(HashMap::from([(
            "me.fakerqu.test.storageredirect".to_string(),
            10366,
        )]));
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            "me.fakerqu.test.storageredirect".to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: vec![
                            "/storage/emulated/0/Download/*.part".to_string(),
                        ],
                        sandboxed_paths: Vec::new(),
                        read_only_paths: vec![
                            "/storage/emulated/0/Download/SrtMonitorLocked".to_string(),
                        ],
                        path_mappings: vec![PathMapping::new(
                            "/storage/emulated/0/Download/SrtMonitorMap".to_string(),
                            "/storage/emulated/0/Download/SrtMonitorMapped".to_string(),
                        )],
                    },
                )]),
            },
        )]));

        let was_monitor_enabled = AuditTrail::instance().is_enabled();
        AuditTrail::instance().set_enabled(true);
        AuditTrail::instance().init("com.android.providers.media.module", 10226);
        AuditTrail::instance().record_provider_open_path(
            "/storage/emulated/0/Download/srt_ci_probe.part",
            10366,
            "me.fakerqu.test.storageredirect",
        );
        AuditTrail::instance().record_provider_open_path(
            "/storage/emulated/0/Download/SrtMonitorMap/srt_monitor_media-mapped-create.bin",
            10366,
            "me.fakerqu.test.storageredirect",
        );
        AuditTrail::instance().record_provider_open_path(
            "/storage/emulated/0/Download/SrtMonitorLocked/srt_monitor_media-read-only-denied.bin",
            10366,
            "me.fakerqu.test.storageredirect",
        );

        let pending = anonymous_media_writer_decision(
            "/storage/emulated/0/Download/.pending-1783044788-srt_ci_probe.part",
        );
        let mapped = anonymous_media_writer_decision(
            "/storage/emulated/0/Download/SrtMonitorMap/.pending-1783044789-srt_monitor_media-mapped-create.bin",
        );
        let read_only = anonymous_media_writer_decision(
            "/storage/emulated/0/Download/SrtMonitorLocked/.pending-1783044790-srt_monitor_media-read-only-denied.bin",
        );

        hub.restore_test_apps(previous_apps, previous_loaded);
        redirect_policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );
        AuditTrail::instance().set_enabled(was_monitor_enabled);
        crate::monitor::clear_recent_private_owner_hint_for_tests();

        assert!(pending.is_redirect());
        assert_eq!(
            pending.new_path,
            "/data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard/Download/.pending-1783044788-srt_ci_probe.part"
        );
        assert!(mapped.is_redirect());
        assert!(mapped.is_mapping);
        assert_eq!(
            mapped.new_path,
            "/storage/emulated/0/Download/SrtMonitorMapped/.pending-1783044789-srt_monitor_media-mapped-create.bin"
        );
        assert!(read_only.is_denied());
        assert_eq!(
            read_only.new_path,
            "/storage/emulated/0/Download/SrtMonitorLocked/.pending-1783044790-srt_monitor_media-read-only-denied.bin"
        );
    }

    #[test]
    fn media_writer_external_uid_redirects_root_hidden_file_to_caller_sandbox() {
        let hub = SettingsHub::instance();
        let package_name = "com.coolapk.market";
        let app_uid = 10282;
        let media_uid = 10226;
        let config_dir = std::env::temp_dir().join(format!(
            "srx_engine_duid_{}_{}",
            std::process::id(),
            paths::monotonic_ms()
        ));
        let apps_dir = config_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).expect("create temp apps dir");
        std::fs::write(
            apps_dir.join(format!("{package_name}.json")),
            r#"{
                "users": {
                    "0": {
                        "enabled": true,
                        "allowed_real_paths": ["DCIM", "Documents", "Movies", "Pictures"]
                    }
                }
            }"#,
        )
        .expect("write coolapk raw config");

        let previous_config_dir =
            hub.replace_test_config_dir(config_dir.to_string_lossy().into_owned());
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            package_name.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: false,
                        allowed_real_paths: vec![
                            "/storage/emulated/0/DCIM".to_string(),
                            "/storage/emulated/0/Documents".to_string(),
                            "/storage/emulated/0/Movies".to_string(),
                            "/storage/emulated/0/Pictures".to_string(),
                        ],
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([
            (package_name.to_string(), app_uid),
            ("com.android.providers.media.module".to_string(), media_uid),
        ]));

        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(app_uid);
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: media_uid,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: "/storage/emulated/0/.duid",
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);
        hub.replace_test_config_dir(previous_config_dir);
        let _ = std::fs::remove_dir_all(config_dir);

        assert!(decision.is_redirect());
        assert!(!decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/data/media/0/Android/data/com.coolapk.market/sdcard/.duid"
        );
        assert_eq!(hub_instance.get_current_caller_package(), package_name);
        assert_eq!(hub_instance.get_current_caller_uid(), app_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn anonymous_media_writer_private_path_uses_owner_package() {
        let hub = SettingsHub::instance();
        let package_name = "com.example.owner";
        let uid = 10274;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            package_name.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([(
            package_name.to_string(),
            uid,
        )]));

        let media_path = "/storage/emulated/0/Android/media/com.example.owner/Viewer/Cache.db-shm";
        let decision = anonymous_media_writer_decision(media_path);

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!decision.is_redirect());
        assert_eq!(
            InterceptHub::instance().get_current_caller_package(),
            package_name
        );
        assert_eq!(InterceptHub::instance().get_current_caller_uid(), uid);
        InterceptHub::instance().clear_current_caller();
    }

    #[test]
    fn disabled_external_caller_private_owner_sqlite_uses_owner_backend() {
        let hub = SettingsHub::instance();
        let owner_package = "com.example.owner";
        let owner_uid = 10274;
        let external_package = "com.example.viewer";
        let external_uid = 10164;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            owner_package.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([
            (owner_package.to_string(), owner_uid),
            (external_package.to_string(), external_uid),
        ]));

        let media_path = "/storage/emulated/0/Android/media/com.example.owner/Viewer/Cache.db-shm";
        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(external_uid);
        hub_instance.set_current_caller_package(external_package);
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: 10217,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: media_path,
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(decision.is_redirect());
        assert_eq!(
            decision.new_path,
            "/data/media/0/Android/media/com.example.owner/Viewer/Cache.db-shm"
        );
        assert_eq!(hub_instance.get_current_caller_package(), external_package);
        assert_eq!(hub_instance.get_current_caller_uid(), external_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn explicit_external_uid_non_sqlite_does_not_infer_private_path_owner_package() {
        let hub = SettingsHub::instance();
        let owner_package = "com.example.owner";
        let owner_uid = 10274;
        let external_uid = 10399;
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([(
            owner_package.to_string(),
            AppProfile {
                user_profiles: HashMap::from([(
                    0,
                    UserProfile {
                        is_enabled: true,
                        is_mapping_mode_only: true,
                        allowed_real_paths: Vec::new(),
                        excluded_real_paths: Vec::new(),
                        sandboxed_paths: Vec::new(),
                        read_only_paths: Vec::new(),
                        path_mappings: Vec::new(),
                    },
                )]),
            },
        )]));
        let previous_uid_cache = redirect_policy::replace_test_uid_cache(HashMap::from([(
            owner_package.to_string(),
            owner_uid,
        )]));

        let media_path = "/storage/emulated/0/Android/media/com.example.owner/Viewer/cache.bin";
        let hub_instance = InterceptHub::instance();
        hub_instance.clear_current_caller();
        hub_instance.set_current_caller_uid(external_uid);
        let decision = process_system_writer_redirect(SystemWriterRedirectRequest {
            hub: hub_instance,
            package_name: "com.android.providers.media.module".to_string(),
            self_uid: 10217,
            is_shared_uid: false,
            is_explicit_caller_decision: false,
            is_write_operation: true,
            pathname: media_path,
            perf_started_ms: paths::monotonic_ms(),
        });

        redirect_policy::restore_test_uid_cache(
            previous_uid_cache.0,
            previous_uid_cache.1,
            previous_uid_cache.2,
            previous_uid_cache.3,
        );
        hub.restore_test_apps(previous_apps, previous_loaded);

        assert!(!decision.is_redirect());
        assert!(hub_instance.get_current_caller_package().is_empty());
        assert_eq!(hub_instance.get_current_caller_uid(), external_uid);
        hub_instance.clear_current_caller();
    }

    #[test]
    fn wildcard_exclude_uses_actual_caller_config() {
        let hub = SettingsHub::instance();
        let (previous_apps, previous_loaded) = hub.replace_test_apps(HashMap::from([
            (
                "com.tencent.mobileqq".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: vec!["/storage/emulated/0/Android".to_string()],
                            excluded_real_paths: vec![
                                "/storage/emulated/0/Android/.android_*".to_string(),
                            ],
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: Vec::new(),
                        },
                    )]),
                },
            ),
            (
                "org.srx.otherapp".to_string(),
                AppProfile {
                    user_profiles: HashMap::from([(
                        0,
                        UserProfile {
                            is_enabled: true,
                            is_mapping_mode_only: false,
                            allowed_real_paths: Vec::new(),
                            excluded_real_paths: Vec::new(),
                            sandboxed_paths: Vec::new(),
                            read_only_paths: Vec::new(),
                            path_mappings: Vec::new(),
                        },
                    )]),
                },
            ),
        ]));

        let qq_lq_path = "/storage/emulated/0/Android/.android_lq";
        assert!(writer::is_path_allowed_by_caller_real_paths(
            qq_lq_path,
            "com.tencent.mobileqq",
            10123,
        ));
        assert!(writer::is_path_excluded_by_caller_real_paths(
            qq_lq_path,
            "com.tencent.mobileqq",
            10123,
        ));
        assert!(!writer::is_path_excluded_by_caller_real_paths(
            qq_lq_path,
            "org.srx.otherapp",
            10124,
        ));

        let redirect_target =
            writer::resolve_system_writer_redirect_target("com.tencent.mobileqq", 10123, 0, false);
        let fallback_path = writer::map_path_by_caller_fallback(qq_lq_path, &redirect_target, 0);
        assert_eq!(
            fallback_path,
            "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard/Android/.android_lq"
        );

        assert!(!writer::is_path_excluded_by_caller_real_paths(
            "/storage/emulated/0/Android/Tencent",
            "com.tencent.mobileqq",
            10123,
        ));

        hub.restore_test_apps(previous_apps, previous_loaded);
    }

    #[test]
    fn app_router_excluded_path_redirects_to_own_sandbox() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "com.tencent.mobileqq",
            10288,
            &["/storage/emulated/0/Documents".to_string()],
            &["/storage/emulated/0/Android/.android_lq".to_string()],
            &[],
            &[],
            &[],
            false,
        );

        let hub = InterceptHub::instance();
        hub.init("com.tencent.mobileqq", false, false);
        let decision = process_redirect_path(hub, "/storage/emulated/0/Android/.android_lq");
        assert!(decision.is_redirect());
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard/Android/.android_lq"
        );
    }

    #[test]
    fn app_router_read_only_write_is_denied_but_read_uses_mount_namespace() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &[],
            &[],
            &["/storage/emulated/0/Documents/Locked".to_string()],
            &[],
            false,
        );

        let write = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Documents/Locked/a.txt",
            true,
            paths::monotonic_ms(),
        );
        assert!(write.is_denied());
        assert!(matches!(write.action, RedirectAction::DenyReadOnly));

        let read = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Documents/Locked/a.txt",
            false,
            paths::monotonic_ms(),
        );
        assert!(!read.is_redirect());
        assert!(!read.is_denied());
    }

    #[test]
    fn app_router_read_only_exclusion_overrides_read_only_allow() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &[],
            &[],
            &[
                "/storage/emulated/0/Documents".to_string(),
                "!/storage/emulated/0/Documents/tmp".to_string(),
            ],
            &[],
            false,
        );

        let readonly_read = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Documents/report.txt",
            false,
            paths::monotonic_ms(),
        );
        assert!(!readonly_read.is_redirect());
        assert!(!readonly_read.is_denied());

        let excluded_read = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Documents/tmp/report.txt",
            false,
            paths::monotonic_ms(),
        );
        assert!(excluded_read.is_redirect());
        assert_eq!(
            excluded_read.new_path,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/Documents/tmp/report.txt"
        );

        let excluded_write = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Documents/tmp/report.txt",
            true,
            paths::monotonic_ms(),
        );
        assert!(excluded_write.is_redirect());
        assert!(!excluded_write.is_denied());
    }

    #[test]
    fn app_router_mapped_target_read_only_denies_mapping_request_write() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &[],
            &[],
            &["/storage/emulated/0/Pictures/Locked".to_string()],
            &[PathMapping::new(
                "/storage/emulated/0/Download/QQ".to_string(),
                "/storage/emulated/0/Pictures/Locked".to_string(),
            )],
            false,
        );

        let decision = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/QQ/a.jpg",
            true,
            paths::monotonic_ms(),
        );

        assert!(decision.is_denied());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Pictures/Locked/a.jpg"
        );
    }

    #[test]
    fn app_router_exclude_overrides_read_only_for_writes() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &["/storage/emulated/0/DCIM/Private".to_string()],
            &[],
            &["/storage/emulated/0/DCIM/Private".to_string()],
            &[],
            false,
        );

        let decision = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/DCIM/Private/a.jpg",
            true,
            paths::monotonic_ms(),
        );

        assert!(decision.is_redirect());
        assert!(!decision.is_denied());
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/DCIM/Private/a.jpg"
        );
    }

    #[test]
    fn app_router_map_only_unmatched_path_stays_public() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &[],
            &[],
            &[],
            &[PathMapping::new(
                "/storage/emulated/0/DCIM/App".to_string(),
                "/storage/emulated/0/Pictures/App".to_string(),
            )],
            true,
        );

        let decision = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/Download/public.txt",
            true,
            paths::monotonic_ms(),
        );

        assert!(!decision.is_redirect());
        assert!(!decision.is_denied());
    }

    #[test]
    fn app_router_map_only_mapping_request_redirects_to_target() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "idm.internet.download.manager.plus",
            10367,
            &[],
            &[],
            &[],
            &[],
            &[PathMapping::new(
                "/storage/emulated/0/Download/1DMP".to_string(),
                "/storage/emulated/0/Download/第三方下载/1DMP".to_string(),
            )],
            true,
        );

        let decision = process_app_mount_namespace_redirect(
            "idm.internet.download.manager.plus",
            10367,
            "/storage/emulated/0/Download/1DMP/file.bin",
            true,
            paths::monotonic_ms(),
        );

        assert!(decision.is_redirect());
        assert!(decision.is_mapping);
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Download/第三方下载/1DMP/file.bin"
        );
    }

    #[test]
    fn app_router_map_only_sandboxed_path_redirects_to_own_sandbox() {
        let _guard = lock_app_router_test();
        configure_app_router(
            "org.srx.testapp",
            10123,
            &[],
            &[],
            &["/storage/emulated/0/.xlDownload".to_string()],
            &[],
            &[],
            true,
        );

        let decision = process_app_mount_namespace_redirect(
            "org.srx.testapp",
            10123,
            "/storage/emulated/0/.xlDownload/task.log",
            true,
            paths::monotonic_ms(),
        );

        assert!(decision.is_redirect());
        assert_eq!(
            decision.new_path,
            "/storage/emulated/0/Android/data/org.srx.testapp/sdcard/.xlDownload/task.log"
        );
    }

    struct ProviderPassthroughGuard;

    impl Drop for ProviderPassthroughGuard {
        fn drop(&mut self) {
            crate::hook::exit_provider_passthrough();
        }
    }
}
