use super::{REDIRECT_DECISION_COUNT, REDIRECT_SAMPLE_STEP, REDIRECT_SLOW_MS, should_log_step};
use crate::platform::paths;
use crate::redirect::router::{RedirectAction, RedirectDecision};
use crate::redirect::thumbnail_diag::{self, ThumbnailDecisionDiag};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Default)]
pub(super) struct SystemWriterPolicyTiming {
    pub(super) mapping_ms: i64,
    pub(super) allow_ms: i64,
    pub(super) fallback_ms: i64,
}

pub(super) struct SystemWriterTrace<'a> {
    pub(super) package_name: &'a str,
    pub(super) pathname: &'a str,
    pub(super) normalized_path: &'a str,
    pub(super) reload_ms: i64,
    pub(super) perf_started_ms: i64,
}

impl SystemWriterTrace<'_> {
    pub(super) fn allow(
        &self,
        exit_reason: &'static str,
        caller_package: &str,
        caller_ms: i64,
        enable_ms: i64,
    ) -> RedirectDecision {
        self.allow_with_timing(
            exit_reason,
            caller_package,
            caller_ms,
            enable_ms,
            SystemWriterPolicyTiming::default(),
        )
    }

    fn allow_with_timing(
        &self,
        exit_reason: &'static str,
        caller_package: &str,
        caller_ms: i64,
        enable_ms: i64,
        timing: SystemWriterPolicyTiming,
    ) -> RedirectDecision {
        let decision = RedirectDecision {
            action: RedirectAction::Allow,
            new_path: String::new(),
            is_mapping: false,
        };
        self.log(
            exit_reason,
            caller_package,
            caller_ms,
            enable_ms,
            timing,
            &decision,
        );
        decision
    }

    pub(super) fn log(
        &self,
        exit_reason: &'static str,
        caller_package: &str,
        caller_ms: i64,
        enable_ms: i64,
        timing: SystemWriterPolicyTiming,
        decision: &RedirectDecision,
    ) {
        log_writer_redirect_perf(&WriterRedirectPerf {
            package_name: self.package_name,
            exit_reason,
            caller_package,
            path: self.pathname,
            normalized_path: self.normalized_path,
            reload_ms: self.reload_ms,
            caller_ms,
            enable_ms,
            mapping_ms: timing.mapping_ms,
            allow_ms: timing.allow_ms,
            fallback_ms: timing.fallback_ms,
            perf_started_ms: self.perf_started_ms,
            decision,
        });
    }
}

pub(super) struct SystemWriterPolicyTrace<'a> {
    pub(super) package_name: &'a str,
    pub(super) caller_package: &'a str,
    pub(super) caller_uid: i32,
    pub(super) user_id: i32,
    pub(super) normalized_path: &'a str,
    pub(super) resolved_path: &'a str,
    pub(super) pathname: &'a str,
    pub(super) is_caller_from_inferred: bool,
    pub(super) enabled_in_memory: bool,
    pub(super) enabled_in_raw: bool,
    pub(super) reload_ms: i64,
    pub(super) caller_ms: i64,
    pub(super) enable_ms: i64,
    pub(super) perf_started_ms: i64,
}

impl SystemWriterPolicyTrace<'_> {
    pub(super) fn allow(
        &self,
        exit_reason: &'static str,
        timing: SystemWriterPolicyTiming,
    ) -> RedirectDecision {
        self.finish(
            exit_reason,
            RedirectDecision {
                action: RedirectAction::Allow,
                new_path: String::new(),
                is_mapping: false,
            },
            timing,
        )
    }

    pub(super) fn redirect(
        &self,
        exit_reason: &'static str,
        new_path: String,
        is_mapping: bool,
        timing: SystemWriterPolicyTiming,
    ) -> RedirectDecision {
        self.finish(
            exit_reason,
            RedirectDecision {
                action: RedirectAction::Redirect,
                new_path,
                is_mapping,
            },
            timing,
        )
    }

    pub(super) fn finish(
        &self,
        exit_reason: &'static str,
        decision: RedirectDecision,
        timing: SystemWriterPolicyTiming,
    ) -> RedirectDecision {
        log_writer_redirect_perf(&WriterRedirectPerf {
            package_name: self.package_name,
            exit_reason,
            caller_package: self.caller_package,
            path: self.pathname,
            normalized_path: self.normalized_path,
            reload_ms: self.reload_ms,
            caller_ms: self.caller_ms,
            enable_ms: self.enable_ms,
            mapping_ms: timing.mapping_ms,
            allow_ms: timing.allow_ms,
            fallback_ms: timing.fallback_ms,
            perf_started_ms: self.perf_started_ms,
            decision: &decision,
        });
        thumbnail_diag::log_system_writer_decision(&ThumbnailDecisionDiag {
            proc_package: self.package_name,
            caller_package: self.caller_package,
            caller_uid: self.caller_uid,
            user_id: self.user_id,
            resolved_path: self.resolved_path,
            enabled_in_memory: self.enabled_in_memory,
            enabled_in_raw: self.enabled_in_raw,
            is_caller_from_inferred: self.is_caller_from_inferred,
            exit_reason,
            decision: &decision,
        });
        decision
    }
}

pub(super) struct WriterRedirectPerf<'a> {
    pub(super) package_name: &'a str,
    pub(super) exit_reason: &'a str,
    pub(super) caller_package: &'a str,
    pub(super) path: &'a str,
    pub(super) normalized_path: &'a str,
    pub(super) reload_ms: i64,
    pub(super) caller_ms: i64,
    pub(super) enable_ms: i64,
    pub(super) mapping_ms: i64,
    pub(super) allow_ms: i64,
    pub(super) fallback_ms: i64,
    // 保存起始时间戳而非预计算的 total_ms，将 clock_gettime 推迟到 gate 内部，
    // 避免非采样路径上的无效系统调用。
    pub(super) perf_started_ms: i64,
    pub(super) decision: &'a RedirectDecision,
}

pub(super) fn log_redirect_perf(
    mode: &str,
    package_name: &str,
    exit_reason: &str,
    path: &str,
    caller_package: &str,
    started_ms: i64,
    decision: &RedirectDecision,
) {
    let total_ms = paths::monotonic_ms().saturating_sub(started_ms);
    if !should_log_redirect_perf(total_ms) {
        return;
    }
    log::info!(
        "perf redirect mode={} pkg={} caller={} exit={} action={} path={} to={} total_ms={}",
        mode,
        package_name,
        caller_package,
        exit_reason,
        redirect_action_text(decision),
        path,
        decision.new_path,
        total_ms
    );
}

pub(super) fn log_writer_redirect_perf(perf: &WriterRedirectPerf<'_>) {
    // 在 gate 内部计算耗时，避免非采样路径上的 clock_gettime 调用。
    let total_ms = paths::monotonic_ms().saturating_sub(perf.perf_started_ms);
    if !should_log_redirect_perf(total_ms) {
        return;
    }
    log::info!(
        "perf redirect mode=writer pkg={} caller={} exit={} action={} path={} normalized={} to={} reload_ms={} caller_ms={} enable_ms={} mapping_ms={} allow_ms={} fallback_ms={} total_ms={}",
        perf.package_name,
        perf.caller_package,
        perf.exit_reason,
        redirect_action_text(perf.decision),
        perf.path,
        perf.normalized_path,
        perf.decision.new_path,
        perf.reload_ms,
        perf.caller_ms,
        perf.enable_ms,
        perf.mapping_ms,
        perf.allow_ms,
        perf.fallback_ms,
        total_ms
    );
}

fn should_log_redirect_perf(total_ms: i64) -> bool {
    if total_ms >= REDIRECT_SLOW_MS {
        return true;
    }
    let count = REDIRECT_DECISION_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    should_log_step(count, REDIRECT_SAMPLE_STEP)
}

fn redirect_action_text(decision: &RedirectDecision) -> &'static str {
    match decision.action {
        RedirectAction::Allow => "allow",
        RedirectAction::Redirect => "redirect",
        RedirectAction::DenyReadOnly => "deny-readonly",
    }
}
