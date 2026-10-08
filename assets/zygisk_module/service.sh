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
    if [ -n "$old_pid" ] && daemon_process_matches "$old_pid"; then
      log -p i -t Boot "srx daemon already running pid=$old_pid"
      return 0
    fi
  fi

  for running_pid in $(pidof srx_daemon 2>/dev/null); do
    if daemon_process_matches "$running_pid"; then
      printf '%s\n' "$running_pid" > "$daemon_pid_file"
      chmod 600 "$daemon_pid_file" 2>/dev/null
      log -p i -t Boot "srx daemon pid file repaired pid=$running_pid"
      return 0
    fi
  done

  "$daemon_bin" >/dev/null 2>&1 &
  daemon_pid=$!
  echo "$daemon_pid" > "$daemon_pid_file"
  chmod 600 "$daemon_pid_file" 2>/dev/null
  log -p i -t Boot "srx daemon started pid=$daemon_pid"
}

daemon_process_matches() {
  pid="$1"
  [ -n "$pid" ] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  # 按 comm 而不是 exe 判定：daemon 的宿主会话（srx_fuse_host）与 scoped FUSE
  # 子进程（srx_fuse）都由 daemon fork 而来、exe 同样是 srx_daemon，只有 comm
  # 被 prctl 改过。若按 exe 判断，daemon 主进程崩溃后这些孤儿子进程会让存活检测
  # 误判为「还在运行」，watchdog 永不重启，reconcile 停摆、新应用只能走 scoped。
  [ "$(cat "/proc/$pid/comm" 2>/dev/null)" = "srx_daemon" ]
}

# 清理 daemon 的遗留子进程：它们继承了 daemon 的实例锁 fd（flock），daemon 主进程
# 崩溃后若不先杀掉，新 daemon 启动时 flock 冲突会 already_running 静默退出。
kill_daemon_children() {
  for child in srx_fuse_host srx_fuse; do
    for pid in $(pidof "$child" 2>/dev/null); do
      kill -9 "$pid" 2>/dev/null || true
    done
  done
}

daemon_watchdog() {
  while true; do
    if [ -f "$RUNTIME_DISABLE_FILE" ]; then
      exit 0
    fi
    daemon_alive=0
    if [ -r "$daemon_pid_file" ] && daemon_process_matches "$(cat "$daemon_pid_file" 2>/dev/null)"; then
      daemon_alive=1
    else
      for pid in $(pidof srx_daemon 2>/dev/null); do
        if daemon_process_matches "$pid"; then
          daemon_alive=1
          break
        fi
      done
    fi
    if [ "$daemon_alive" -eq 0 ]; then
      log -p w -t Boot "srx daemon watchdog: not running, cleanup and restart"
      kill_daemon_children
      sleep 1
      start_srx_daemon
      # 重启后的第一次检查保持短间隔，尽快发现启动失败。
      sleep 5
      continue
    fi
    sleep "${DAEMON_WATCHDOG_INTERVAL_SECONDS:-20}"
  done
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
daemon_watchdog &
