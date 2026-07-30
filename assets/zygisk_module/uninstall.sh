#!/system/bin/sh

MODDIR="/data/adb/modules/storage.redirect.x"
MONITOR_COLLECTOR_PID_FILE="$MODDIR/logs/.monitor_collector.pid"
RUNNING_COLLECTOR_PID_FILE="$MODDIR/logs/.running_collector.pid"
MEDIA_STATE_COLLECTOR_PID_FILE="$MODDIR/logs/.media_state_collector.pid"
APP_STATUS_COLLECTOR_PID_FILE="$MODDIR/logs/.app_status_collector.pid"
APP_STATUS_SNAPSHOT_PID_FILE="$MODDIR/logs/.app_status_snapshot.pid"
STATS_COLLECTOR_PID_FILE="$MODDIR/logs/.stats_collector.pid"
CONFIG_EVENT_COLLECTOR_PID_FILE="$MODDIR/logs/.config_event_collector.pid"
PACKAGE_EVENT_COLLECTOR_PID_FILE="$MODDIR/logs/.package_event_collector.pid"

stop_background_process() {
  target_pid="$1"
  if [ -z "$target_pid" ] || ! kill -0 "$target_pid" 2>/dev/null; then
    return 0
  fi

  children_file="/proc/$target_pid/task/$target_pid/children"
  if [ -r "$children_file" ]; then
    for child_pid in $(cat "$children_file" 2>/dev/null); do
      stop_background_process "$child_pid"
    done
  fi
  kill "$target_pid" 2>/dev/null
}

stop_collector_by_pid_file() {
  pid_file="$1"
  if [ ! -f "$pid_file" ]; then
    return 0
  fi

  pid=$(cat "$pid_file" 2>/dev/null)
  stop_background_process "$pid"
  rm -f "$pid_file"
}

stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$RUNNING_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$APP_STATUS_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
stop_collector_by_pid_file "$STATS_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$CONFIG_EVENT_COLLECTOR_PID_FILE"
stop_collector_by_pid_file "$PACKAGE_EVENT_COLLECTOR_PID_FILE"

# 仅允许删除已知的模块私有路径，防止误删无关目录。
safe_remove_known_path() {
  target="$1"
  case "$target" in
    /data/local/tmp/storage.redirect.x_stats|\
    /data/local/tmp/storage.redirect.x|\
    /data/adb/storage.redirect.x)
      rm -rf "$target" 2>/dev/null
      ;;
    *)
      ui_print "-- warn: skip unknown path=$target"
      ;;
  esac
}

safe_remove_known_path /data/local/tmp/storage.redirect.x_stats
safe_remove_known_path /data/local/tmp/storage.redirect.x
# stats 持久目录在模块目录之外，模块管理器不会自动清理，需要显式删除。
safe_remove_known_path /data/adb/storage.redirect.x

ui_print "-- Storage Redirect X uninstalled"
ui_print "-- temporary files cleaned"
