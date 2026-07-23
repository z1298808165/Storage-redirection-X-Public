#!/system/bin/sh

MODDIR=${0%/*}

LOGS_DIR="$MODDIR/logs"
FILE_MONITOR_LOG_FILE="$LOGS_DIR/file_monitor.log"
PACKAGE_EVENT_LOG_FILE="$LOGS_DIR/package_events.log"
RECENT_SOURCE_HINT_FILE="$LOGS_DIR/.recent_source_hint"
RECENT_PATH_CALLER_HINT_FILE="$LOGS_DIR/.recent_path_caller_hint"
MAX_PACKAGE_EVENT_LOG_BYTES=524288
LOG_ROTATE_BACKUPS=2
MONITOR_COLLECTOR_PID_FILE="$LOGS_DIR/.monitor_collector.pid"
CONFIG_EVENT_COLLECTOR_PID_FILE="$LOGS_DIR/.config_event_collector.pid"
PACKAGE_EVENT_COLLECTOR_PID_FILE="$LOGS_DIR/.package_event_collector.pid"
CONFIG_STATE_FILE="$LOGS_DIR/.config_apps_state"
PACKAGE_EVENT_OFFSET_FILE="$LOGS_DIR/.package_events.offset"
PACKAGE_EVENT_RECEIVER_READY_FILE="$LOGS_DIR/.package_event_receiver_ready"
UID_MAP_LAST_REFRESH_FILE="$LOGS_DIR/.uid_map_last_refresh"
CONFIG_DIR="$MODDIR/config"
AUTO_NEW_APPS_BASELINE_FILE="$CONFIG_DIR/auto_new_apps_baseline"
SYSTEM_WRITER_UIDS_FILE="$CONFIG_DIR/system_writer_uids.list"
APPS_CONFIG_DIR="$CONFIG_DIR/apps"
BOOT_PENDING_FILE="$MODDIR/.boot_pending"
BOOT_OK_FILE="$MODDIR/.boot_ok"
RUNTIME_DISABLE_FILE="$MODDIR/.runtime_disabled"
MEDIA_HOOK_DEFERRED_FILE="$LOGS_DIR/.media_hook_deferred"

if [ -f "$RUNTIME_DISABLE_FILE" ]; then
  log -p i -t Boot "srx runtime disabled; skip service startup"
  exit 0
fi

mkdir -p "$LOGS_DIR"
chmod 755 "$LOGS_DIR"
touch "$PACKAGE_EVENT_LOG_FILE"
chmod 666 "$PACKAGE_EVENT_LOG_FILE" 2>/dev/null
touch "$RECENT_SOURCE_HINT_FILE" "$RECENT_PATH_CALLER_HINT_FILE"
chmod 666 "$RECENT_SOURCE_HINT_FILE" "$RECENT_PATH_CALLER_HINT_FILE" 2>/dev/null

mkdir -p "$CONFIG_DIR/apps"
chmod 755 "$CONFIG_DIR" "$CONFIG_DIR/apps" 2>/dev/null
find "$CONFIG_DIR" -type f -name '*.json' -exec chmod 644 {} \; 2>/dev/null
if command -v chcon >/dev/null 2>&1; then
  chcon -R u:object_r:shell_data_file:s0 "$CONFIG_DIR" 2>/dev/null
fi

start_srx_daemon() {
  daemon_bin="$MODDIR/bin/srx_daemon"
  daemon_pid_file="$LOGS_DIR/.srx_daemon.pid"
  if [ ! -x "$daemon_bin" ]; then
    log -p w -t Boot "srx daemon missing: $daemon_bin"
    return 0
  fi

  if [ -r "$daemon_pid_file" ]; then
    old_pid=$(cat "$daemon_pid_file" 2>/dev/null)
    if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
      log -p i -t Boot "srx daemon already running pid=$old_pid"
      return 0
    fi
  fi

  "$daemon_bin" >/dev/null 2>&1 &
  daemon_pid=$!
  echo "$daemon_pid" > "$daemon_pid_file"
  chmod 600 "$daemon_pid_file" 2>/dev/null
  log -p i -t Boot "srx daemon started pid=$daemon_pid"
}


# 配置 WebUI
WEBROOT_DIR="$MODDIR/webroot"
if [ -d "$WEBROOT_DIR" ]; then
  chmod 755 "$WEBROOT_DIR"
  find "$WEBROOT_DIR" -type d -exec chmod 755 {} \; 2>/dev/null
  find "$WEBROOT_DIR" -type f -exec chmod 644 {} \; 2>/dev/null
  if command -v chcon >/dev/null 2>&1; then
    chcon -R u:object_r:shell_data_file:s0 "$WEBROOT_DIR" 2>/dev/null
  fi
  log -p i -t Boot "webui ready"
fi

start_srx_daemon

SERVICE_DIR="$MODDIR/service.d"
RUNNING_LOG_FILE="$LOGS_DIR/running.log"
MEDIA_STATE_LOG_FILE="$LOGS_DIR/media_provider_state.log"
APP_STATUS_LOG_FILE="$LOGS_DIR/app_status.log"
MAX_RUNNING_LOG_BYTES=2097152
MAX_MEDIA_STATE_LOG_BYTES=10485760
MAX_APP_STATUS_LOG_BYTES=10485760
DIAGNOSTIC_SNAPSHOT_INTERVAL_SECONDS=120
RUNNING_COLLECTOR_PID_FILE="$LOGS_DIR/.running_collector.pid"
MEDIA_STATE_COLLECTOR_PID_FILE="$LOGS_DIR/.media_state_collector.pid"
APP_STATUS_COLLECTOR_PID_FILE="$LOGS_DIR/.app_status_collector.pid"
APP_STATUS_SNAPSHOT_PID_FILE="$LOGS_DIR/.app_status_snapshot.pid"
STATS_COLLECTOR_PID_FILE="$LOGS_DIR/.stats_collector.pid"
MEDIA_STATE_LAST_PID_FILE="$LOGS_DIR/.media_state_last_pid"
MEDIA_STATE_DETAIL_TS_FILE="$LOGS_DIR/.media_state_detail_ts"
SERVICE_PARTS="common.sh log_collectors.sh config_events.sh media_state.sh app_status.sh debug_collectors.sh boot.sh"

for service_name in $SERVICE_PARTS; do
  service_part="$SERVICE_DIR/$service_name"
  if [ ! -r "$service_part" ]; then
    log -p e -t Boot "missing service part: $service_part"
    exit 1
  fi
  . "$service_part"
done

boot_guard_wait &
