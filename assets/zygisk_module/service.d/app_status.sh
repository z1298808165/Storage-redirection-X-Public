append_app_status_output() {
  now_text="$1"
  while IFS= read -r line; do
    printf '%s %s\n' "$now_text" "$line"
  done
}

get_package_pids() {
  package_name="$1"
  {
    pidof "$package_name" 2>/dev/null
    ps -A -o PID,NAME 2>/dev/null | awk -v target="$package_name" '
      NR > 1 && NF >= 2 {
        if ($2 == target || index($2, target ":") == 1) {
          print $1
        }
      }
    '
  } | tr ' ' '\n' | awk 'NF > 0 && $1 ~ /^[0-9]+$/ { print }' | sort -u
}

append_package_raw_state() {
  now_text="$1"
  package_name="$2"
  config_file="$3"

  printf '%s -- package begin %s\n' "$now_text" "$package_name"
  printf '%s -- config %s\n' "$now_text" "$config_file"
  sed 's/^/  /' "$config_file" 2>/dev/null | append_app_status_output "$now_text"

  printf '%s -- cmd package list packages -U | grep %s\n' "$now_text" "$package_name"
  cmd package list packages -U 2>/dev/null |
    grep -F "package:$package_name " |
    append_app_status_output "$now_text"

  printf '%s -- pidof %s\n' "$now_text" "$package_name"
  pidof "$package_name" 2>/dev/null |
    append_app_status_output "$now_text"

  printf '%s -- ps -A -o PID,NAME | grep %s\n' "$now_text" "$package_name"
  ps -A -o PID,NAME 2>/dev/null | awk -v target="$package_name" '
    NR == 1 { print; next }
    NF >= 2 && ($2 == target || index($2, target ":") == 1) { print }
  ' | append_app_status_output "$now_text"

  for pid in $(get_package_pids "$package_name"); do
    printf '%s -- /proc/%s/cmdline\n' "$now_text" "$pid"
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null |
      append_app_status_output "$now_text"

    printf '%s -- /proc/%s/status\n' "$now_text" "$pid"
    sed 's/^/  /' "/proc/$pid/status" 2>/dev/null |
      append_app_status_output "$now_text"

    printf '%s -- /proc/%s/fd\n' "$now_text" "$pid"
    ls -l "/proc/$pid/fd" 2>/dev/null |
      head -n 80 |
      append_app_status_output "$now_text"

    printf '%s -- /proc/%s/mountinfo storage\n' "$now_text" "$pid"
    grep -E ' /storage| /mnt/user| /mnt/media_rw| /sdcard' "/proc/$pid/mountinfo" 2>/dev/null |
      head -n 80 |
      append_app_status_output "$now_text"
  done

  printf '%s -- package end %s\n' "$now_text" "$package_name"
}

append_app_status_snapshot() {
  now_text=$(date '+%m/%d %H:%M:%S' 2>/dev/null)
  now_text=${now_text:-unknown}
  printf '%s -- app_status snapshot begin\n' "$now_text"
  for config_file in "$APPS_CONFIG_DIR"/*.json; do
    [ -f "$config_file" ] || continue
    package_name=$(basename "$config_file" .json)
    is_skipped_package "$package_name" && continue
    is_effective_package_config "$package_name" || continue
    append_package_raw_state "$now_text" "$package_name" "$config_file"
  done
  printf '%s -- app_status snapshot end\n' "$now_text"
}

start_app_status_snapshot_collector() {
  (
    while true; do
      append_app_status_snapshot | emit_private_log_stream "AppStatus"
      sleep 30
    done
  ) &
  echo "$!" > "$APP_STATUS_SNAPSHOT_PID_FILE"
  chmod 644 "$APP_STATUS_SNAPSHOT_PID_FILE"
}
