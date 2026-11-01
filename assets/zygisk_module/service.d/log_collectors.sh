sync_monitor_collector() {
  # FileMonitorOp 由 srx_daemon 中的私有 socket 接收器写入。
  stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
}

# 采集器状态收敛完全由事件驱动，不再挂周期看门狗。
#
# 收敛的三个触发源都是事件：
#   1. 开机启动：start_log_collectors 直接调用一次 sync_debug_collectors；
#   2. 配置变化：config_events.sh 监听 config 目录的 inotify 事件后调用 sync
#      （详细日志开关就写在 global.json 里，因此开关本身就是事件）；
#   3. 采集管道异常：debug 采集器内层循环在管道退出后自行重建，不依赖外部巡检。
#
# 此前这里还有一个 `while true; sleep 60` 的同步看门狗，作为「boot 首个 sync 判定
# verbose=off 后配置翻转再无事件到达」的兜底。该缺口已由 inotifyd 监听 config
# 目录堵死（事件不会因首次判定而消失），继续保留定时唤醒只会每秒醒来读文件，
# 属于规范禁止的周期轮询，故移除。

start_log_collectors() {
  ensure_log_files
  stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$CONFIG_EVENT_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$PACKAGE_EVENT_COLLECTOR_PID_FILE"
  start_config_event_collector
  start_package_event_collector
  if command -v sync_debug_collectors >/dev/null 2>&1; then
    sync_debug_collectors
  fi
  if command -v start_app_error_ring_collector >/dev/null 2>&1; then
    start_app_error_ring_collector
  fi
}
