// 挂载成功计数：发 socket 由 srx_logd 累加，避免与守护进程直写 STATS_FILE 冲突
const STATS_TAG: &str = "Stats";

pub(super) fn update_redirect_stats() {
    // srx_logd 按 "+N" 解析增量
    log::info!(target: STATS_TAG, "+1");
}
