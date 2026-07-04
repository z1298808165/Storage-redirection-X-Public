get_media_provider_pid() {
  pidof com.google.android.providers.media.module 2>/dev/null | awk '{print $1}'
  pidof com.android.providers.media.module 2>/dev/null | awk '{print $1}'
  pidof com.android.providers.media 2>/dev/null | awk '{print $1}'
}

append_media_state_output() {
  now_text="$1"
  while IFS= read -r line; do
    printf '%s %s\n' "$now_text" "$line"
  done
}

should_write_media_detail() {
  reason="$1"
  now_sec=$(now_epoch_seconds)
  last_sec=$(cat "$MEDIA_STATE_DETAIL_TS_FILE" 2>/dev/null)
  last_sec=${last_sec:-0}

  if [ "$reason" = "pid_change" ] || [ "$reason" = "state_D" ] || [ "$reason" = "rss_high" ] || [ "$reason" = "threads_high" ]; then
    echo "$now_sec" > "$MEDIA_STATE_DETAIL_TS_FILE"
    return 0
  fi
  if [ "$last_sec" -gt 0 ] && [ $((now_sec - last_sec)) -ge 300 ]; then
    echo "$now_sec" > "$MEDIA_STATE_DETAIL_TS_FILE"
    return 0
  fi
  return 1
}

read_media_proc_value() {
  status_file="$1"
  key="$2"
  awk -v target="$key" '$1 == target":" { print $2; exit }' "$status_file" 2>/dev/null
}

append_media_proc_detail() {
  now_text="$1"
  pid="$2"
  reason="$3"

  printf '%s -- media detail begin pid=%s reason=%s\n' "$now_text" "$pid" "$reason"
  printf '%s -- /proc/%s/cmdline\n' "$now_text" "$pid"
  tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null |
    append_media_state_output "$now_text"

  printf '%s -- /proc/%s/status\n' "$now_text" "$pid"
  sed 's/^/  /' "/proc/$pid/status" 2>/dev/null |
    append_media_state_output "$now_text"

  printf '%s -- /proc/%s/task/*/status\n' "$now_text" "$pid"
  for status_file in "/proc/$pid"/task/*/status; do
    [ -r "$status_file" ] || continue
    tid=${status_file%/status}
    tid=${tid##*/}
    printf '%s ---- task %s status\n' "$now_text" "$tid"
    sed 's/^/  /' "$status_file" 2>/dev/null |
      append_media_state_output "$now_text"
  done

  printf '%s -- /proc/%s/fd\n' "$now_text" "$pid"
  ls -l "/proc/$pid/fd" 2>/dev/null |
    append_media_state_output "$now_text"

  printf '%s -- /proc/%s/mountinfo storage\n' "$now_text" "$pid"
  grep -E ' /storage| /mnt/user| /mnt/media_rw| /sdcard|storage.redirect.x' "/proc/$pid/mountinfo" 2>/dev/null |
    append_media_state_output "$now_text"

  printf '%s -- dumpsys meminfo %s\n' "$now_text" "$pid"
  dumpsys meminfo "$pid" 2>/dev/null |
    append_media_state_output "$now_text"

  printf '%s -- recent media logcat\n' "$now_text"
  logcat -d -t 120 -v threadtime 2>/dev/null |
    grep -E 'MediaProvider|MediaProviderWrapper|FuseDaemon|ExternalStorage|StorageRedirect|FileMonitorOp|SQLite|CursorWindow|binder' |
    append_media_state_output "$now_text"

  printf '%s -- recent oom/fuse dmesg\n' "$now_text"
  dmesg 2>/dev/null |
    tail -n 300 |
    grep -Ei 'oom|lowmem|killed process|fuse|media' |
    append_media_state_output "$now_text"

  printf '%s -- media detail end pid=%s\n' "$now_text" "$pid"
}

append_media_state_snapshot() {
  now_text=$(date '+%m/%d %H:%M:%S' 2>/dev/null)
  now_text=${now_text:-unknown}
  pid_list=$(get_media_provider_pid | sort -u)

  printf '%s -- media_state snapshot begin\n' "$now_text"
  printf '%s -- pidof media providers\n' "$now_text"
  {
    pidof com.google.android.providers.media.module 2>/dev/null
    pidof com.android.providers.media.module 2>/dev/null
    pidof com.android.providers.media 2>/dev/null
  } | append_media_state_output "$now_text"

  printf '%s -- ps -A -o PID,USER,ARGS media\n' "$now_text"
  ps -A -o PID,USER,ARGS 2>/dev/null |
    awk 'NR == 1 || $0 ~ /providers\.media|android\.process\.media/' |
    append_media_state_output "$now_text"

  if [ -z "$pid_list" ]; then
    printf '%s -- media provider missing\n' "$now_text"
  else
    last_pid=$(cat "$MEDIA_STATE_LAST_PID_FILE" 2>/dev/null)
    for pid in $pid_list; do
      status_file="/proc/$pid/status"
      if [ ! -r "$status_file" ]; then
        printf '%s -- media process gone pid=%s\n' "$now_text" "$pid"
        continue
      fi

      printf '%s -- /proc/%s/status summary\n' "$now_text" "$pid"
      grep -E '^(Name|State|Pid|PPid|Threads|VmRSS|VmSize):' "$status_file" 2>/dev/null |
        sed 's/^/  /' |
        append_media_state_output "$now_text"

      detail_reason="periodic"
      state=$(read_media_proc_value "$status_file" State)
      threads=$(read_media_proc_value "$status_file" Threads)
      vmrss=$(read_media_proc_value "$status_file" VmRSS)
      if [ -n "$last_pid" ] && [ "$last_pid" != "$pid" ]; then
        detail_reason="pid_change"
      elif [ "$state" = "D" ]; then
        detail_reason="state_D"
      elif [ "${vmrss:-0}" -ge 524288 ]; then
        detail_reason="rss_high"
      elif [ "${threads:-0}" -ge 256 ]; then
        detail_reason="threads_high"
      fi

      echo "$pid" > "$MEDIA_STATE_LAST_PID_FILE"
      if should_write_media_detail "$detail_reason"; then
        append_media_proc_detail "$now_text" "$pid" "$detail_reason"
      fi
    done
  fi

  printf '%s -- media_state snapshot end\n' "$now_text"
}

start_media_state_collector() {
  (
    while true; do
      append_media_state_snapshot | emit_private_log_stream "MediaState"
      sleep 30
    done
  ) &
  echo "$!" > "$MEDIA_STATE_COLLECTOR_PID_FILE"
  chmod 644 "$MEDIA_STATE_COLLECTOR_PID_FILE"
}
