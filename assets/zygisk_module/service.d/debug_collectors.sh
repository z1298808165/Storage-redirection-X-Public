start_running_collector() {
  (
    rotate_check_lines=0
    rotate_log_file_if_needed "$RUNNING_LOG_FILE" "$MAX_RUNNING_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
    while true; do
      logcat -T 1 -v threadtime -s \
        StorageRedirect:V SRX:V 2>/dev/null |
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
        {
          if (NF < 6) {
            next
          }
          date_text = $1
          time_text = $2
          level_char = $5
          if (date_text !~ /^[0-9][0-9]-[0-9][0-9]$/) {
            next
          }
          if (time_text !~ /^[0-9][0-9]:[0-9][0-9]:[0-9][0-9](\.[0-9]+)?$/) {
            next
          }
          if (level_char !~ /^[VDIWEAF]$/) {
            next
          }
          is_java = $6 == "SRX:"
          source_prefix = is_java ? "Jv" : "Rs"
          message_start = is_java ? 6 : 7
          gsub("-", "/", date_text)
          sub(/\..*$/, "", time_text)
          line_text = ""
          for (i = message_start; i <= NF; i++) {
            if (line_text == "") {
              line_text = $i
            } else {
              line_text = line_text " " $i
            }
          }
          if (line_text == "") {
            next
          }
          if (line_text ~ /^\[(Rs|Kt)(Verbose|Debug|Info|Warn|Error|Fatal)\][[:space:]]/) {
            printf "%s %s %s\n", date_text, time_text, line_text
          } else {
            printf "%s %s [%s%s] %s\n", date_text, time_text, source_prefix, level_text(level_char), line_text
          }
        }
      ' |
      while IFS= read -r line; do
        [ -n "$line" ] || continue
        printf '%s\n' "$line" >> "$RUNNING_LOG_FILE"
        rotate_check_lines=$((rotate_check_lines + 1))
        if [ "$rotate_check_lines" -ge 100 ]; then
          rotate_log_file_if_needed "$RUNNING_LOG_FILE" "$MAX_RUNNING_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
          rotate_check_lines=0
        fi
      done
      sleep 1
    done
  ) &
  echo "$!" > "$RUNNING_COLLECTOR_PID_FILE"
  chmod 644 "$RUNNING_COLLECTOR_PID_FILE"
}

start_app_status_collector() {
  (
    line_buf=0
    trim_app_status_log_if_needed
    while true; do
      logcat -T 1 -v threadtime -s AndroidRuntime:E DEBUG:F libc:F 2>/dev/null |
      while IFS= read -r line; do
        [ -n "$line" ] || continue
        case "$line" in ---------*) continue ;; esac
        printf '%s\n' "$line" >> "$APP_STATUS_LOG_FILE"
        line_buf=$((line_buf + 1))
        if [ "$line_buf" -ge 200 ]; then
          trim_app_status_log_if_needed
          line_buf=0
        fi
      done
      sleep 1
    done
  ) &
  echo "$!" > "$APP_STATUS_COLLECTOR_PID_FILE"
  chmod 644 "$APP_STATUS_COLLECTOR_PID_FILE"
}

start_stats_collector() {
  if [ ! -f "$STATS_FILE" ]; then
    echo "0" > "$STATS_FILE"
  fi
  chmod 644 "$STATS_FILE"

  (
    while true; do
      logcat -T 1 -v raw -s Stats:I 2>/dev/null |
      awk -v stats_file="$STATS_FILE" '
        BEGIN {
          total = 0
          if ((getline current < stats_file) > 0) total = current + 0
          close(stats_file)
        }
        /^\+[0-9]+$/ {
          total += substr($0, 2) + 0
          dirty = 1
          events++
          if (events >= 50) {
            print total > stats_file
            close(stats_file)
            events = 0
            dirty = 0
          }
        }
        END {
          if (dirty) {
            print total > stats_file
            close(stats_file)
          }
        }
      '
      sleep 1
    done
  ) &
  echo "$!" > "$STATS_COLLECTOR_PID_FILE"
  chmod 644 "$STATS_COLLECTOR_PID_FILE"
}

start_debug_logcat_collector() {
  if [ ! -f "$STATS_FILE" ]; then
    echo "0" > "$STATS_FILE"
  fi
  chmod 644 "$STATS_FILE"

  (
    rotate_check_lines=0
    app_line_buf=0
    rotate_log_file_if_needed "$RUNNING_LOG_FILE" "$MAX_RUNNING_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
    trim_app_status_log_if_needed
    while true; do
      logcat -T 1 -v threadtime -s \
        StorageRedirect:V SRX:V Stats:I AndroidRuntime:E DEBUG:F libc:F 2>/dev/null |
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
            if (text == "") {
              text = $i
            } else {
              text = text " " $i
            }
          }
          return text
        }
        {
          if (NF < 6) {
            next
          }
          date_text = $1
          time_text = $2
          level_char = $5
          tag = $6
          sub(/:$/, "", tag)

          if (tag == "StorageRedirect" || tag == "SRX") {
            if (date_text !~ /^[0-9][0-9]-[0-9][0-9]$/) {
              next
            }
            if (time_text !~ /^[0-9][0-9]:[0-9][0-9]:[0-9][0-9](\.[0-9]+)?$/) {
              next
            }
            if (level_char !~ /^[VDIWEAF]$/) {
              next
            }
            is_java = tag == "SRX"
            source_prefix = is_java ? "Jv" : "Rs"
            gsub("-", "/", date_text)
            sub(/\..*$/, "", time_text)
            line_text = join_from(7)
            if (line_text == "") {
              next
            }
            if (line_text ~ /^\[(Rs|Kt)(Verbose|Debug|Info|Warn|Error|Fatal)\][[:space:]]/) {
              printf "R|%s %s %s\n", date_text, time_text, line_text
            } else {
              printf "R|%s %s [%s%s] %s\n", date_text, time_text, source_prefix, level_text(level_char), line_text
            }
            next
          }

          if (tag == "Stats") {
            line_text = join_from(7)
            if (line_text ~ /^\+[0-9]+$/) {
              printf "S|%s\n", line_text
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
            rotate_check_lines=$((rotate_check_lines + 1))
            if [ "$rotate_check_lines" -ge 100 ]; then
              rotate_log_file_if_needed "$RUNNING_LOG_FILE" "$MAX_RUNNING_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
              rotate_check_lines=0
            fi
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
          S)
            case "$line" in
              +[0-9]*)
                stats_delta=${line#+}
                stats_total=$(cat "$STATS_FILE" 2>/dev/null | awk '{ print $1 + 0 }')
                stats_total=${stats_total:-0}
                stats_total=$((stats_total + stats_delta))
                echo "$stats_total" > "$STATS_FILE"
                ;;
            esac
            ;;
        esac
      done
      sleep 1
    done
  ) &
  debug_logcat_pid="$!"
  for pid_file in "$RUNNING_COLLECTOR_PID_FILE" "$APP_STATUS_COLLECTOR_PID_FILE" "$STATS_COLLECTOR_PID_FILE"; do
    echo "$debug_logcat_pid" > "$pid_file"
    chmod 644 "$pid_file"
  done
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
