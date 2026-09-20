start_debug_logcat_collector() {
  # 先回收上一轮遗留的孤儿采集器。`stop_background_process` 依赖 /proc/<pid>/children
  # 递归杀，而管道里的 logcat 常已被 init 收养，杀子 shell 时整个漏掉（pgid 为 0，
  # 进程组杀法同样无效）。不先回收就会每触发一次同步多留一个永久孤儿，持续冲掉
  # logcat 环形缓冲并重复写日志。详见 `reap_stale_logcat_collectors`。
  reap_stale_logcat_collectors
  (
    app_line_buf=0
    trim_app_status_log_if_needed
    while true; do
      # `exec -a` 给 logcat 打上可识别标记，使孤儿可被 `reap_stale_logcat_collectors`
      # 精确回收；标记必须与 common.sh 的 SRX_COLLECTOR_TAG 保持一致。多套一层
      # 子 shell 是因为 `exec` 会替换当前 shell，直接写在管道左侧会顶掉内层 shell。
      ( exec -a "$SRX_COLLECTOR_TAG" /system/bin/logcat -T 1 -v threadtime -s SRX:V AndroidRuntime:E DEBUG:F libc:F ) 2>/dev/null |
      awk '
        function level_text(level_char) {
          if (level_char == "V") return "Verbose"
          if (level_char == "D") return "Debug"
          if (level_char == "I") return "Info"
          if (level_char == "W") return "Warn"
          if (level_char == "E") return "Error"
          if (level_char == "F" || level_char == "A") return "Fatal"
          return level_char
        }
        function join_from(start,    i, text) {
          text = ""
          for (i = start; i <= NF; i++) {
            text = text == "" ? $i : text " " $i
          }
          return text
        }
        {
          if (NF < 6) next
          date_text = $1
          time_text = $2
          level_char = $5
          tag = $6
          sub(/:$/, "", tag)

          if (tag == "SRX") {
            if (date_text !~ /^[0-9][0-9]-[0-9][0-9]$/) next
            if (time_text !~ /^[0-9][0-9]:[0-9][0-9]:[0-9][0-9](\.[0-9]+)?$/) next
            if (level_char !~ /^[VDIWEAF]$/) next
            gsub("-", "/", date_text)
            sub(/\..*$/, "", time_text)
            line_text = join_from(7)
            if (line_text == "") next
            if (line_text ~ /^\[(Rs|Kt|Jv)(Verbose|Debug|Info|Warn|Error|Fatal)\][[:space:]]/) {
              printf "R|%s %s %s\n", date_text, time_text, line_text
            } else {
              printf "R|%s %s [Jv%s] %s\n", date_text, time_text, level_text(level_char), line_text
            }
            next
          }

          if (tag == "AndroidRuntime" || tag == "DEBUG" || tag == "libc") {
            print "A|" $0
          }
        }
      ' |
      while IFS= read -r tagged_line; do
        [ -n "$tagged_line" ] || continue
        kind=${tagged_line%%|*}
        line=${tagged_line#*|}
        case "$kind" in
          R)
            [ -n "$line" ] || continue
            printf '%s\n' "$line" >> "$RUNNING_LOG_FILE"
            ;;
          A)
            [ -n "$line" ] || continue
            case "$line" in ---------*) continue ;; esac
            printf '%s\n' "$line" >> "$APP_STATUS_LOG_FILE"
            app_line_buf=$((app_line_buf + 1))
            if [ "$app_line_buf" -ge 200 ]; then
              trim_app_status_log_if_needed
              app_line_buf=0
            fi
            ;;
        esac
      done
      sleep 1
    done
  ) &
  debug_logcat_pid="$!"
  for pid_file in "$RUNNING_COLLECTOR_PID_FILE" "$APP_STATUS_COLLECTOR_PID_FILE"; do
    echo "$debug_logcat_pid" > "$pid_file"
    chmod 644 "$pid_file"
  done
  rm -f "$STATS_COLLECTOR_PID_FILE"
}

stop_debug_logcat_collector() {
  stopped_pids=" "
  for pid_file in "$RUNNING_COLLECTOR_PID_FILE" "$APP_STATUS_COLLECTOR_PID_FILE" "$STATS_COLLECTOR_PID_FILE"; do
    [ -f "$pid_file" ] || continue
    pid=$(cat "$pid_file" 2>/dev/null)
    case "$stopped_pids" in
      *" $pid "*) ;;
      *)
        stop_background_process "$pid"
        stopped_pids="$stopped_pids$pid "
        ;;
    esac
    rm -f "$pid_file"
  done
}

start_debug_collectors() {
  is_verbose_logging_enabled || return 0
  ensure_debug_log_files
  stop_debug_logcat_collector
  stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
  start_debug_logcat_collector
  start_media_state_collector
  start_app_status_snapshot_collector
}

stop_debug_collectors() {
  stop_debug_logcat_collector
  stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
}

sync_debug_collectors() {
  if is_verbose_logging_enabled; then
    start_debug_collectors
  else
    stop_debug_collectors
  fi
}
