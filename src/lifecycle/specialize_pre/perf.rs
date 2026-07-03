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
