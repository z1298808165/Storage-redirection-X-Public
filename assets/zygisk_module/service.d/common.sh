ensure_log_file() {
  file="$1"
  touch "$file"
  chmod 666 "$file"
}

ensure_log_files() {
  ensure_log_file "$FILE_MONITOR_LOG_FILE"
  ensure_log_file "$PACKAGE_EVENT_LOG_FILE"
  [ -n "$RECENT_SOURCE_HINT_FILE" ] && ensure_log_file "$RECENT_SOURCE_HINT_FILE"
  [ -n "$RECENT_PATH_CALLER_HINT_FILE" ] && ensure_log_file "$RECENT_PATH_CALLER_HINT_FILE"
}

ensure_debug_log_files() {
  ensure_log_file "$RUNNING_LOG_FILE"
  ensure_log_file "$MEDIA_STATE_LOG_FILE"
  ensure_log_file "$APP_STATUS_LOG_FILE"
  if [ -f "$LOGS_DIR/app_crash.log" ] && [ ! -s "$APP_STATUS_LOG_FILE" ]; then
    cat "$LOGS_DIR/app_crash.log" >> "$APP_STATUS_LOG_FILE" 2>/dev/null
  fi
  rm -f "$LOGS_DIR/media_provider.log" "$LOGS_DIR/app_crash.log" "$MEDIA_STATE_LAST_PID_FILE" "$MEDIA_STATE_DETAIL_TS_FILE"
}

is_verbose_logging_enabled() {
  [ -f "$CONFIG_DIR/global.json" ] || return 1
  grep -Eq '"verbose_logging_enabled"[[:space:]]*:[[:space:]]*true' "$CONFIG_DIR/global.json" 2>/dev/null
}

is_file_monitor_enabled() {
  [ -f "$CONFIG_DIR/global.json" ] || return 1
  grep -Eq '"file_monitor_enabled"[[:space:]]*:[[:space:]]*true' "$CONFIG_DIR/global.json" 2>/dev/null
}

rotate_log_file_if_needed() {
  file="$1"
  max_bytes="$2"
  backups="$3"

  if [ -z "$max_bytes" ] || [ "$max_bytes" -le 0 ] || [ ! -f "$file" ]; then
    return 0
  fi

  file_size=$(get_file_size "$file")
  if [ "$file_size" -le "$max_bytes" ]; then
    return 0
  fi

  lock_dir="${file}.rotate.lock"
  if ! mkdir "$lock_dir" 2>/dev/null; then
    return 0
  fi

  file_size=$(get_file_size "$file")
  if [ "$file_size" -le "$max_bytes" ]; then
    rmdir "$lock_dir" 2>/dev/null
    return 0
  fi

  backups=${backups:-2}
  if [ "$backups" -le 0 ]; then
    : > "$file"
    chmod 666 "$file" 2>/dev/null
    rmdir "$lock_dir" 2>/dev/null
    return 0
  fi

  rm -f "$file.$backups"
  i=$((backups - 1))
  while [ "$i" -ge 1 ]; do
    if [ -f "$file.$i" ]; then
      next=$((i + 1))
      mv "$file.$i" "$file.$next" 2>/dev/null
      chmod 666 "$file.$next" 2>/dev/null
    fi
    i=$((i - 1))
  done

  mv "$file" "$file.1" 2>/dev/null
  chmod 666 "$file.1" 2>/dev/null
  ensure_log_file "$file"
  rmdir "$lock_dir" 2>/dev/null
}

get_file_size() {
  file="$1"
  if [ ! -f "$file" ]; then
    echo 0
    return 0
  fi

  file_size=$(stat -c '%s' "$file" 2>/dev/null)
  if [ -z "$file_size" ]; then
    file_size=$(wc -c < "$file" 2>/dev/null | awk '{print $1}')
  fi
  if [ -z "$file_size" ]; then
    echo 0
    return 0
  fi
  echo "$file_size"
}

now_epoch_seconds() {
  date '+%s' 2>/dev/null || echo 0
}

trim_app_status_log_if_needed() {
  rotate_log_file_if_needed "$APP_STATUS_LOG_FILE" "$MAX_APP_STATUS_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
}

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
  wait "$target_pid" 2>/dev/null
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

is_process_alive() {
  pid="$1"
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

is_pid_file_alive() {
  pid_file="$1"
  [ -f "$pid_file" ] || return 1
  pid=$(cat "$pid_file" 2>/dev/null)
  is_process_alive "$pid"
}

refresh_uid_map() {
  force_refresh="$1"
  now_sec=$(now_epoch_seconds)
  last_sec=$(cat "$UID_MAP_LAST_REFRESH_FILE" 2>/dev/null)
  last_sec=${last_sec:-0}
  if [ "$force_refresh" != "force" ] && [ -f "$SYSTEM_WRITER_UIDS_FILE" ] && [ $((now_sec - last_sec)) -lt 60 ]; then
    return 0
  fi

  mkdir -p "$CONFIG_DIR"
  tmp_uids_file="${SYSTEM_WRITER_UIDS_FILE}.tmp"

  {
    echo "# package:uid"
    cmd package list packages -U 2>/dev/null |
      sed -n 's/^package:\([^ ]*\).* uid:\([0-9][0-9]*\).*/\1:\2/p' |
      sort -u
  } > "$tmp_uids_file"

  entry_count=$(grep -c '^[^#].*:[0-9][0-9]*$' "$tmp_uids_file" 2>/dev/null)
  if [ "$entry_count" -gt 0 ]; then
    mv "$tmp_uids_file" "$SYSTEM_WRITER_UIDS_FILE"
    chmod 644 "$SYSTEM_WRITER_UIDS_FILE"
    echo "$now_sec" > "$UID_MAP_LAST_REFRESH_FILE"
  else
    rm -f "$tmp_uids_file"
  fi
}
