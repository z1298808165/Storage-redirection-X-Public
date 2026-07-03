start_monitor_collector() {
  is_file_monitor_enabled || return 0
  (
    rotate_check_lines=0
    rotate_log_file_if_needed "$FILE_MONITOR_LOG_FILE" "$MAX_MONITOR_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
    while true; do
      logcat -T 1 -v raw -s FileMonitorOp:I 2>/dev/null |
      while IFS= read -r line; do
        [ -n "$line" ] || continue
        case "$line" in ---------*) continue ;; esac
        printf '%s\n' "$line" >> "$FILE_MONITOR_LOG_FILE"
        rotate_check_lines=$((rotate_check_lines + 1))
        if [ "$rotate_check_lines" -ge 100 ]; then
          rotate_log_file_if_needed "$FILE_MONITOR_LOG_FILE" "$MAX_MONITOR_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
          rotate_check_lines=0
        fi
      done
      sleep 1
    done
  ) &
  echo "$!" > "$MONITOR_COLLECTOR_PID_FILE"
  chmod 644 "$MONITOR_COLLECTOR_PID_FILE"
}

sync_monitor_collector() {
  if is_file_monitor_enabled; then
    ensure_log_files
    if is_pid_file_alive "$MONITOR_COLLECTOR_PID_FILE"; then
      return 0
    fi
    start_monitor_collector
  else
    stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
  fi
}

start_log_collectors() {
  ensure_log_files
  stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$CONFIG_EVENT_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$PACKAGE_EVENT_COLLECTOR_PID_FILE"
  sync_monitor_collector
  start_config_event_collector
  start_package_event_collector
  if command -v sync_debug_collectors >/dev/null 2>&1; then
    sync_debug_collectors
  fi
}
