const STATS_TAG: &str = "Stats";

pub fn record_runtime_activation() {
    crate::logging::write_log(crate::logging::Level::Info, STATS_TAG, "+1");
}
