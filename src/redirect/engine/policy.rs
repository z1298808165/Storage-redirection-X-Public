use super::{
    WRITER_ALLOWED_LOG_COUNT, WRITER_ALLOWED_LOG_STEP, resolve_system_writer_mapping_output_path,
    resolve_system_writer_output_path, resolve_system_writer_private_backend_path, should_log_step,
};
use crate::config::{SettingsHub, UserRedirectEnablement};
use crate::platform::paths;
use crate::redirect::router::{RedirectAction, RedirectDecision};
use crate::redirect::writer;
use std::sync::atomic::Ordering;

use super::trace::{SystemWriterPolicyTiming, SystemWriterPolicyTrace};

pub(super) struct SystemWriterPolicyRequest<'a> {
    pub(super) package_name: &'a str,
    pub(super) caller_package: &'a str,
    pub(super) caller_uid: i32,
    pub(super) user_id: i32,
    pub(super) normalized_path: &'a str,
    pub(super) resolved_path: &'a str,
    pub(super) pathname: &'a str,
    pub(super) is_data_media: bool,
    pub(super) is_caller_from_inferred: bool,
    pub(super) redirect_enablement: UserRedirectEnablement,
    pub(super) reload_ms: i64,
    pub(super) caller_ms: i64,
    pub(super) enable_ms: i64,
    pub(super) perf_started_ms: i64,
    pub(super) is_write_operation: bool,
}

struct SystemWriterPolicyContext<'a> {
    package_name: &'a str,
    caller_package: &'a str,
    caller_uid: i32,
    user_id: i32,
    normalized_path: &'a str,
    resolved_path: &'a str,
    is_data_media: bool,
    is_caller_from_inferred: bool,
    is_write_operation: bool,
}

struct MappingStageResult {
    decision: Option<RedirectDecision>,
    mapping_ms: i64,
}

struct FallbackStageResult {
    target_empty: bool,
    fallback_empty: bool,
    decision: RedirectDecision,
    fallback_ms: i64,
}

pub(super) struct SystemWriterEnablement {
    pub(super) redirect: UserRedirectEnablement,
    pub(super) enable_ms: i64,
}

impl SystemWriterEnablement {
    pub(super) fn is_enabled(&self) -> bool {
        self.redirect.is_enabled()
    }
}

pub(super) fn resolve_system_writer_enablement(
    config: &SettingsHub,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
) -> SystemWriterEnablement {
    let enable_started_ms = paths::monotonic_ms();
    let redirect = config.get_user_redirect_enablement(caller_package, caller_uid, user_id);
    let enable_ms = paths::monotonic_ms().saturating_sub(enable_started_ms);

    SystemWriterEnablement {
        redirect,
        enable_ms,
    }
}

pub(super) fn process_system_writer_policy(
    request: SystemWriterPolicyRequest<'_>,
) -> RedirectDecision {
    let SystemWriterPolicyRequest {
        package_name,
        caller_package,
        caller_uid,
        user_id,
        normalized_path,
        resolved_path,
        pathname,
        is_data_media,
        is_caller_from_inferred,
        redirect_enablement,
        reload_ms,
        caller_ms,
        enable_ms,
        perf_started_ms,
        is_write_operation,
    } = request;

    let trace = SystemWriterPolicyTrace {
        package_name,
        caller_package,
        caller_uid,
        user_id,
        normalized_path,
        resolved_path,
        pathname,
        is_caller_from_inferred,
        enabled_in_memory: redirect_enablement.enabled_in_memory,
        enabled_in_raw: redirect_enablement.enabled_in_raw,
        reload_ms,
        caller_ms,
        enable_ms,
        perf_started_ms,
    };

    let context = SystemWriterPolicyContext {
        package_name,
        caller_package,
        caller_uid,
        user_id,
        normalized_path,
        resolved_path,
        is_data_media,
        is_caller_from_inferred,
        is_write_operation,
    };

    let mapping_result = evaluate_system_writer_mapping_stage(&context, &trace);
    let mapping_ms = mapping_result.mapping_ms;
    if let Some(decision) = mapping_result.decision {
        return decision;
    }

    let is_path_excluded =
        writer::is_path_excluded_by_caller_real_paths(resolved_path, caller_package, caller_uid);
    let direct_read_only_check_path = if is_write_operation {
        writer::read_only_check_path_by_caller_paths(resolved_path, caller_package, caller_uid)
    } else {
        String::new()
    };
    if !direct_read_only_check_path.is_empty() {
        log::debug!(
            "writer: readonly deny caller={} uid={} path={}",
            caller_package,
            caller_uid,
            direct_read_only_check_path
        );
        return trace.finish(
            "read_only",
            RedirectDecision {
                action: RedirectAction::DenyReadOnly,
                is_mapping: direct_read_only_check_path != resolved_path,
                new_path: direct_read_only_check_path,
            },
            SystemWriterPolicyTiming {
                mapping_ms,
                ..Default::default()
            },
        );
    }

    let private_backend = resolve_system_writer_private_backend_path(
        resolved_path,
        caller_package,
        user_id,
        package_name,
    );
    if !private_backend.is_empty() && private_backend != resolved_path {
        log::debug!(
            "writer: private backend caller={} uid={} from={} to={}",
            caller_package,
            caller_uid,
            resolved_path,
            private_backend
        );
        return trace.redirect(
            "private_backend",
            private_backend,
            false,
            SystemWriterPolicyTiming {
                mapping_ms,
                ..Default::default()
            },
        );
    }

    if redirect_enablement.is_mapping_mode_only {
        if writer::is_path_sandboxed_by_caller_paths(resolved_path, caller_package, caller_uid) {
            let fallback = resolve_system_writer_fallback_redirect(&context);
            if fallback.decision.is_redirect() {
                log::debug!(
                    "writer: map-only sandbox caller={} uid={} from={} to={}",
                    caller_package,
                    caller_uid,
                    resolved_path,
                    fallback.decision.new_path
                );
                return trace.finish(
                    "map_only_sandbox",
                    fallback.decision,
                    SystemWriterPolicyTiming {
                        mapping_ms,
                        fallback_ms: fallback.fallback_ms,
                        ..Default::default()
                    },
                );
            }
        }

        return trace.allow(
            "map_only_miss",
            SystemWriterPolicyTiming {
                mapping_ms,
                ..Default::default()
            },
        );
    }

    let is_read_allowed_by_rule =
        writer::is_path_allowed_by_caller_real_paths(resolved_path, caller_package, caller_uid)
            || writer::is_path_read_only_excluded_by_caller_paths(
                resolved_path,
                caller_package,
                caller_uid,
            )
            || !is_write_operation
                && writer::is_path_read_only_by_caller_paths(
                    resolved_path,
                    caller_package,
                    caller_uid,
                );
    let allow_started_ms = paths::monotonic_ms();
    if is_path_excluded {
        log::debug!(
            "writer: excl hit caller={} uid={} path={}",
            caller_package,
            caller_uid,
            resolved_path
        );
    } else if is_read_allowed_by_rule {
        let allow_ms = paths::monotonic_ms().saturating_sub(allow_started_ms);
        let allowed_count = WRITER_ALLOWED_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_step(allowed_count, WRITER_ALLOWED_LOG_STEP) {
            log::debug!(
                "writer: real allow caller={} uid={} path={} n={}",
                caller_package,
                caller_uid,
                resolved_path,
                allowed_count
            );
        }
        return trace.allow(
            "allowed",
            SystemWriterPolicyTiming {
                mapping_ms,
                allow_ms,
                ..Default::default()
            },
        );
    }
    let allow_ms = paths::monotonic_ms().saturating_sub(allow_started_ms);

    let fallback = resolve_system_writer_fallback_redirect(&context);
    if fallback.target_empty {
        writer::log_system_writer_redirect_disabled(caller_package, caller_uid, pathname);
        return trace.allow(
            "target_empty",
            SystemWriterPolicyTiming {
                mapping_ms,
                allow_ms,
                fallback_ms: fallback.fallback_ms,
            },
        );
    }

    if fallback.fallback_empty {
        return trace.allow(
            "fallback_empty",
            SystemWriterPolicyTiming {
                mapping_ms,
                allow_ms,
                fallback_ms: fallback.fallback_ms,
            },
        );
    }

    log::debug!(
        "writer: default redirect caller={} uid={} from={} to={}",
        caller_package,
        caller_uid,
        resolved_path,
        fallback.decision.new_path
    );
    trace.finish(
        "fallback",
        fallback.decision,
        SystemWriterPolicyTiming {
            mapping_ms,
            allow_ms,
            fallback_ms: fallback.fallback_ms,
        },
    )
}

fn resolve_system_writer_fallback_redirect(
    context: &SystemWriterPolicyContext<'_>,
) -> FallbackStageResult {
    let fallback_started_ms = paths::monotonic_ms();
    let redirect_target = writer::resolve_system_writer_redirect_target(
        context.caller_package,
        context.caller_uid,
        context.user_id,
        context.is_caller_from_inferred,
    );
    if redirect_target.is_empty() {
        return FallbackStageResult {
            target_empty: true,
            fallback_empty: false,
            decision: RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            },
            fallback_ms: paths::monotonic_ms().saturating_sub(fallback_started_ms),
        };
    }

    let fallback_path = writer::map_path_by_caller_fallback(
        context.resolved_path,
        &redirect_target,
        context.user_id,
    );
    if fallback_path.is_empty() || fallback_path == context.resolved_path {
        return FallbackStageResult {
            target_empty: false,
            fallback_empty: true,
            decision: RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            },
            fallback_ms: paths::monotonic_ms().saturating_sub(fallback_started_ms),
        };
    }

    let new_path = resolve_system_writer_output_path(
        context.normalized_path,
        &fallback_path,
        context.is_data_media,
        context.package_name,
    );
    FallbackStageResult {
        target_empty: false,
        fallback_empty: false,
        decision: RedirectDecision {
            action: RedirectAction::Redirect,
            new_path,
            is_mapping: false,
        },
        fallback_ms: paths::monotonic_ms().saturating_sub(fallback_started_ms),
    }
}

fn evaluate_system_writer_mapping_stage(
    context: &SystemWriterPolicyContext<'_>,
    trace: &SystemWriterPolicyTrace<'_>,
) -> MappingStageResult {
    let mapping_started_ms = paths::monotonic_ms();
    let caller_mappings = writer::get_caller_mappings(context.caller_package, context.caller_uid);
    let mapped_path = writer::map_path_by_caller_mappings(context.resolved_path, &caller_mappings);
    let mapping_ms = paths::monotonic_ms().saturating_sub(mapping_started_ms);

    if !mapped_path.is_empty() && mapped_path != context.resolved_path {
        if context.is_write_operation
            && writer::is_caller_path_read_only(
                &mapped_path,
                context.caller_package,
                context.caller_uid,
            )
        {
            log::debug!(
                "writer: readonly deny mapped caller={} uid={} from={} to={}",
                context.caller_package,
                context.caller_uid,
                context.resolved_path,
                mapped_path
            );
            return MappingStageResult {
                decision: Some(trace.finish(
                    "mapping_read_only",
                    RedirectDecision {
                        action: RedirectAction::DenyReadOnly,
                        new_path: mapped_path,
                        is_mapping: true,
                    },
                    SystemWriterPolicyTiming {
                        mapping_ms,
                        ..Default::default()
                    },
                )),
                mapping_ms,
            };
        }
        let new_path = resolve_system_writer_mapping_output_path(
            &mapped_path,
            context.is_data_media,
            context.caller_package,
            context.user_id,
            context.package_name,
        );
        log::debug!(
            "writer: caller map caller={} uid={} from={} to={}",
            context.caller_package,
            context.caller_uid,
            context.resolved_path,
            new_path
        );
        return MappingStageResult {
            decision: Some(trace.redirect(
                "mapping",
                new_path,
                true,
                SystemWriterPolicyTiming {
                    mapping_ms,
                    ..Default::default()
                },
            )),
            mapping_ms,
        };
    }

    for mapping in &caller_mappings {
        if !paths::is_same_or_child(context.resolved_path, &mapping.final_path) {
            continue;
        }
        let private_mapping_backend = super::resolve_system_writer_mapping_target_backend_path(
            context.resolved_path,
            context.caller_package,
            context.user_id,
            context.package_name,
        );
        if !private_mapping_backend.is_empty() && private_mapping_backend != context.resolved_path {
            log::debug!(
                "writer: mapping target private backend caller={} uid={} path={} to={}",
                context.caller_package,
                context.caller_uid,
                context.resolved_path,
                private_mapping_backend
            );
            return MappingStageResult {
                decision: Some(trace.redirect(
                    "mapping_target_private_backend",
                    private_mapping_backend,
                    false,
                    SystemWriterPolicyTiming {
                        mapping_ms,
                        ..Default::default()
                    },
                )),
                mapping_ms,
            };
        }
        log::debug!(
            "writer: map target allow caller={} uid={} path={}",
            context.caller_package,
            context.caller_uid,
            context.resolved_path
        );
        return MappingStageResult {
            decision: Some(trace.allow(
                "mapping_target",
                SystemWriterPolicyTiming {
                    mapping_ms,
                    ..Default::default()
                },
            )),
            mapping_ms,
        };
    }

    MappingStageResult {
        decision: None,
        mapping_ms,
    }
}
