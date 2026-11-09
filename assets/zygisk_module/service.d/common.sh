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

# 回收本模块遗留的 logcat 采集进程。
#
# 采集器的形态是 `( logcat ... | awk ... | while read ... ) &`。杀子 shell 时，管道里的
# `logcat` 往往已经被 init 收养（ppid=1）并从子 shell 的 children 列表里消失，于是
# `stop_background_process` 的递归杀法整个漏掉它，`pgid` 又普遍是 0（进程组杀法同样无效）。
# 结果是每触发一次 `sync_debug_collectors` 就多留一个永久孤儿：它会持续消费 logcat 环形
# 缓冲（把应用崩溃现场冲掉、直接损害排障能力），并把同一份日志重复写进 running.log。
#
# 因此采集器启动时给自己的 logcat 打上 `SRX_COLLECTOR_TAG` 标记（`exec -a`），这里按该
# 标记扫描 /proc 回收全部孤儿。标记同时也是幂等保证：不依赖 pid 文件是否被覆盖或丢失。
SRX_COLLECTOR_TAG="srx-debug-collector"
# 应用错误环形采集器使用独立标记：它不受 verbose 门禁、常驻运行，
# 不能被 debug 采集器的例行重启（reap + 重启）误杀。
SRX_APP_ERROR_COLLECTOR_TAG="srx-apperror-collector"
# 主缓冲按需窗口采集器的标记（与常驻 crash ring 区分，便于现场辨认）。
SRX_APP_WINDOW_COLLECTOR_TAG="srx-apperror-window"

reap_stale_logcat_collectors() {
  reaped=0
  # 只回收 debug 采集器自己的孤儿。**不要**把常驻的 crash ring 纳入回收：本函数由
  # `start_debug_logcat_collector` 调用，而后者会被「verbose 开关翻转」这类配置事件
  # 触发——一并回收就等于每次改配置都静默杀掉常驻采集器（且不会重建），崩溃现场
  # 从此丢失。crash ring 的生命周期由自己的 pid 文件 + 幂等启动管理。
  for collector_tag in "$SRX_COLLECTOR_TAG"; do
    for proc_dir in /proc/[0-9]*; do
      [ -r "$proc_dir/cmdline" ] || continue
      cmdline=$(tr '\0' ' ' < "$proc_dir/cmdline" 2>/dev/null)
      case "$cmdline" in
        "$collector_tag"*)
          kill "${proc_dir#/proc/}" 2>/dev/null && reaped=$((reaped + 1))
          ;;
      esac
    done
  done
  if [ "$reaped" -gt 0 ]; then
    log -p i -t Boot "reaped stale logcat collectors n=$reaped"
    log_collector_status "reaped stale logcat collectors n=$reaped"
  fi
  return 0
}

is_process_alive() {
  pid="$1"
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

# 采集器生命周期留痕：启动/停止/sync 决策/管道异常重启都落一行。
# 反馈设备上 debug 采集器曾在 verbose=true 的 boot 里静默死亡，导致 [Jv] 转发与
# FUSE 逐操作日志整体缺失却无从定位；有这份时间线才能从诊断包里直接读出死因。
log_collector_status() {
  status_message="$1"
  [ -n "$COLLECTOR_STATUS_LOG_FILE" ] || return 0
  printf '%s %s\n' "$(date '+%m-%d %H:%M:%S')" "$status_message" \
    >> "$COLLECTOR_STATUS_LOG_FILE" 2>/dev/null || true
  rotate_log_file_if_needed "$COLLECTOR_STATUS_LOG_FILE" 524288 2
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

  # package 查询走 binder，boot 早期 PMS 未就绪时可能长时间阻塞；该函数被
  # config 事件批与包事件循环同步调用，一旦阻塞会拖死整条采集循环。超时后
  # 本轮不落盘（entry_count 为空保护仍在），等待下一次对账重试。
  {
    echo "# package:uid"
    timeout "${UID_MAP_QUERY_TIMEOUT_SECONDS:-10}" cmd package list packages -U 2>/dev/null |
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
