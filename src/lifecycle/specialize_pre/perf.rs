const SPECIALIZE_SLOW_MS: i64 = 20;

pub(super) struct SpecializePerf<'a> {
    pub(super) package_name: &'a str,
    pub(super) exit_reason: &'a str,
    pub(super) pid: i32,
    pub(super) uid: i32,
    pub(super) app_count: usize,
    pub(super) should_redirect: bool,
    pub(super) should_monitor: bool,
    pub(super) is_system_writer: bool,
    pub(super) is_hook_redirect: bool,
    pub(super) allow_count: usize,
    pub(super) excluded_count: usize,
    pub(super) mapping_count: usize,
    pub(super) payload_bytes: usize,
    pub(super) config_init_ms: i64,
    pub(super) config_reload_ms: i64,
    pub(super) shared_uid_ms: i64,
    pub(super) decision_ms: i64,
    pub(super) writer_context_ms: i64,
    pub(super) enabled_scan_ms: i64,
    pub(super) route_ms: i64,
    pub(super) payload_ms: i64,
    pub(super) send_ms: i64,
    pub(super) total_ms: i64,
}

/// specialize 各阶段耗时与规模统计的累积容器。
///
/// `pre_app_specialize` 有多个退出点，原先每个退出点都要重复罗列
/// [`SpecializePerf`] 的全部字段，实际只有 `exit_reason` 和“该阶段是否已经
/// 执行过”不同，既冗长又容易漏填或写错字段顺序。这里按阶段逐步累积，未执行
/// 的阶段保持 0，退出点只需给出退出原因。
pub(super) struct SpecializePerfStages {
    /// specialize 进入时刻，用于计算 `total_ms`
    pub(super) started_ms: i64,
    pub(super) allow_count: usize,
    pub(super) excluded_count: usize,
    pub(super) mapping_count: usize,
    pub(super) payload_bytes: usize,
    pub(super) config_init_ms: i64,
    pub(super) config_reload_ms: i64,
    pub(super) shared_uid_ms: i64,
    pub(super) decision_ms: i64,
    pub(super) writer_context_ms: i64,
    pub(super) enabled_scan_ms: i64,
    pub(super) route_ms: i64,
    pub(super) payload_ms: i64,
    pub(super) send_ms: i64,
}

impl SpecializePerfStages {
    pub(super) fn new(started_ms: i64) -> Self {
        Self {
            started_ms,
            allow_count: 0,
            excluded_count: 0,
            mapping_count: 0,
            payload_bytes: 0,
            config_init_ms: 0,
            config_reload_ms: 0,
            shared_uid_ms: 0,
            decision_ms: 0,
            writer_context_ms: 0,
            enabled_scan_ms: 0,
            route_ms: 0,
            payload_ms: 0,
            send_ms: 0,
        }
    }
}

pub(super) fn log_specialize_perf(perf: &SpecializePerf<'_>) {
    if perf.total_ms < SPECIALIZE_SLOW_MS
        && !perf.should_redirect
        && !perf.should_monitor
        && !perf.is_system_writer
        && perf.app_count < 100
    {
        return;
    }

    log::info!(
        "perf specialize pkg={} pid={} uid={} exit={} apps={} redirect={} monitor={} writer={} hook_redirect={} allow={} excl={} map={} payload={} init_ms={} reload_ms={} uid_ms={} decision_ms={} writer_ms={} enabled_scan_ms={} route_ms={} payload_ms={} send_ms={} total_ms={}",
        perf.package_name,
        perf.pid,
        perf.uid,
        perf.exit_reason,
        perf.app_count,
        perf.should_redirect,
        perf.should_monitor,
        perf.is_system_writer,
        perf.is_hook_redirect,
        perf.allow_count,
        perf.excluded_count,
        perf.mapping_count,
        perf.payload_bytes,
        perf.config_init_ms,
        perf.config_reload_ms,
        perf.shared_uid_ms,
        perf.decision_ms,
        perf.writer_context_ms,
        perf.enabled_scan_ms,
        perf.route_ms,
        perf.payload_ms,
        perf.send_ms,
        perf.total_ms
    );
}
