append_app_status_output() {
  now_text="$1"
  while IFS= read -r line; do
    printf '%s %s\n' "$now_text" "$line" >> "$APP_STATUS_LOG_FILE"
  done
}

get_package_pids() {
  package_name="$1"
  ps_state_file="$2"
  (
    awk -v target="$package_name" '
      NR > 1 && NF >= 2 {
        if ($2 == target || index($2, target ":") == 1) {
          print $1
        }
      }
    ' "$ps_state_file" 2>/dev/null
    pidof "$package_name" 2>/dev/null | tr ' ' '\n'
  ) | awk 'NF > 0 && $1 ~ /^[0-9]+$/ { print }' | sort -u
}

append_package_raw_state() {
  now_text="$1"
  package_name="$2"
  config_file="$3"
  package_state_file="$4"
  ps_state_file="$5"

  echo "$now_text -- package begin $package_name" >> "$APP_STATUS_LOG_FILE"
  echo "$now_text -- config $config_file" >> "$APP_STATUS_LOG_FILE"
  sed 's/^/  /' "$config_file" 2>/dev/null | append_app_status_output "$now_text"

  echo "$now_text -- cached package list packages -U | grep $package_name" >> "$APP_STATUS_LOG_FILE"
  grep -F "package:$package_name " "$package_state_file" 2>/dev/null |
    append_app_status_output "$now_text"

  echo "$now_text -- cached pids $package_name" >> "$APP_STATUS_LOG_FILE"
  get_package_pids "$package_name" "$ps_state_file" |
    append_app_status_output "$now_text"

  echo "$now_text -- cached ps -A -o PID,NAME | grep $package_name" >> "$APP_STATUS_LOG_FILE"
  awk -v target="$package_name" '
    NR == 1 { print; next }
    NF >= 2 && ($2 == target || index($2, target ":") == 1) { print }
  ' "$ps_state_file" 2>/dev/null | append_app_status_output "$now_text"

  for pid in $(get_package_pids "$package_name" "$ps_state_file"); do
    echo "$now_text -- /proc/$pid/cmdline" >> "$APP_STATUS_LOG_FILE"
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null |
      append_app_status_output "$now_text"

    echo "$now_text -- /proc/$pid/status" >> "$APP_STATUS_LOG_FILE"
    sed 's/^/  /' "/proc/$pid/status" 2>/dev/null |
      append_app_status_output "$now_text"

    echo "$now_text -- /proc/$pid/fd" >> "$APP_STATUS_LOG_FILE"
    ls -l "/proc/$pid/fd" 2>/dev/null |
      head -n 80 |
      append_app_status_output "$now_text"

    echo "$now_text -- /proc/$pid/mountinfo storage" >> "$APP_STATUS_LOG_FILE"
    grep -E ' /storage| /mnt/user| /mnt/media_rw| /sdcard' "/proc/$pid/mountinfo" 2>/dev/null |
      head -n 80 |
      append_app_status_output "$now_text"
  done

  echo "$now_text -- package end $package_name" >> "$APP_STATUS_LOG_FILE"
}

append_app_status_snapshot() {
  now_text=$(date '+%m/%d %H:%M:%S' 2>/dev/null)
  now_text=${now_text:-unknown}
  package_state_file="$LOGS_DIR/.app_status_packages.$$"
  ps_state_file="$LOGS_DIR/.app_status_ps.$$"
  cmd package list packages -U > "$package_state_file" 2>/dev/null || : > "$package_state_file"
  ps -A -o PID,NAME > "$ps_state_file" 2>/dev/null || : > "$ps_state_file"

  echo "$now_text -- app_status snapshot begin" >> "$APP_STATUS_LOG_FILE"
  for config_file in "$APPS_CONFIG_DIR"/*.json; do
    [ -f "$config_file" ] || continue
    package_name=$(basename "$config_file" .json)
    is_skipped_package "$package_name" && continue
    is_effective_package_config "$package_name" || continue
    append_package_raw_state "$now_text" "$package_name" "$config_file" "$package_state_file" "$ps_state_file"
  done
  echo "$now_text -- app_status snapshot end" >> "$APP_STATUS_LOG_FILE"
  rm -f "$package_state_file" "$ps_state_file"
}

start_app_status_snapshot_collector() {
  (
    trim_app_status_log_if_needed
    while true; do
      trim_app_status_log_if_needed
      append_app_status_snapshot
      trim_app_status_log_if_needed
      sleep "$DIAGNOSTIC_SNAPSHOT_INTERVAL_SECONDS"
    done
  ) &
  echo "$!" > "$APP_STATUS_SNAPSHOT_PID_FILE"
  chmod 644 "$APP_STATUS_SNAPSHOT_PID_FILE"
}
