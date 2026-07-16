sync_monitor_collector() {
  # FileMonitorOp is written by the private socket receiver in srx_daemon.
  stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
}

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
}
