get_media_provider_pid() {
  pidof com.google.android.providers.media.module 2>/dev/null | awk '{print $1}'
  pidof com.android.providers.media.module 2>/dev/null | awk '{print $1}'
  pidof com.android.providers.media 2>/dev/null | awk '{print $1}'
}

append_media_state_output() {
  now_text="$1"
  while IFS= read -r line; do
    printf '%s %s\n' "$now_text" "$line" >> "$MEDIA_STATE_LOG_FILE"
  done
}

should_write_media_detail() {
  reason="$1"
  now_sec=$(now_epoch_seconds)
  last_sec=$(cat "$MEDIA_STATE_DETAIL_TS_FILE" 2>/dev/null)
  last_sec=${last_sec:-0}

  if [ "$reason" = "pid_change" ] || [ "$reason" = "state_D" ]; then
    echo "$now_sec" > "$MEDIA_STATE_DETAIL_TS_FILE"
    return 0
  fi
  if [ $((now_sec - last_sec)) -ge 300 ]; then
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

  echo "$now_text -- media detail begin pid=$pid reason=$reason" >> "$MEDIA_STATE_LOG_FILE"
  echo "$now_text -- /proc/$pid/cmdline" >> "$MEDIA_STATE_LOG_FILE"
  tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null |
    append_media_state_output "$now_text"

  echo "$now_text -- /proc/$pid/status" >> "$MEDIA_STATE_LOG_FILE"
  sed 's/^/  /' "/proc/$pid/status" 2>/dev/null |
    append_media_state_output "$now_text"

  echo "$now_text -- /proc/$pid/task/*/status" >> "$MEDIA_STATE_LOG_FILE"
  for status_file in "/proc/$pid"/task/*/status; do
    [ -r "$status_file" ] || continue
    tid=${status_file%/status}
    tid=${tid##*/}
    echo "$now_text ---- task $tid status" >> "$MEDIA_STATE_LOG_FILE"
    sed 's/^/  /' "$status_file" 2>/dev/null |
      append_media_state_output "$now_text"
  done

  echo "$now_text -- /proc/$pid/fd" >> "$MEDIA_STATE_LOG_FILE"
  ls -l "/proc/$pid/fd" 2>/dev/null |
    append_media_state_output "$now_text"

  echo "$now_text -- /proc/$pid/mountinfo storage" >> "$MEDIA_STATE_LOG_FILE"
  grep -E ' /storage| /mnt/user| /mnt/media_rw| /sdcard|storage.redirect.x' "/proc/$pid/mountinfo" 2>/dev/null |
    append_media_state_output "$now_text"

  echo "$now_text -- dumpsys meminfo $pid" >> "$MEDIA_STATE_LOG_FILE"
  dumpsys meminfo "$pid" 2>/dev/null |
    append_media_state_output "$now_text"

  echo "$now_text -- recent media logcat" >> "$MEDIA_STATE_LOG_FILE"
  logcat -d -t 120 -v threadtime 2>/dev/null |
    grep -E 'MediaProvider|MediaProviderWrapper|FuseDaemon|ExternalStorage|StorageRedirect|FileMonitorOp|SQLite|CursorWindow|binder' |
    append_media_state_output "$now_text"

  echo "$now_text -- recent oom/fuse dmesg" >> "$MEDIA_STATE_LOG_FILE"
  dmesg 2>/dev/null |
    tail -n 300 |
    grep -Ei 'oom|lowmem|killed process|fuse|media' |
    append_media_state_output "$now_text"

  echo "$now_text -- media detail end pid=$pid" >> "$MEDIA_STATE_LOG_FILE"
}

append_media_state_snapshot() {
  now_text=$(date '+%m/%d %H:%M:%S' 2>/dev/null)
  now_text=${now_text:-unknown}
  pid_list=$(get_media_provider_pid | sort -u)

  echo "$now_text -- media_state snapshot begin" >> "$MEDIA_STATE_LOG_FILE"
  echo "$now_text -- pidof media providers" >> "$MEDIA_STATE_LOG_FILE"
  {
    pidof com.google.android.providers.media.module 2>/dev/null
    pidof com.android.providers.media.module 2>/dev/null
    pidof com.android.providers.media 2>/dev/null
  } | append_media_state_output "$now_text"

  echo "$now_text -- ps -A -o PID,USER,ARGS media" >> "$MEDIA_STATE_LOG_FILE"
  /system/bin/ps -A -o PID,USER,ARGS 2>/dev/null |
    awk 'NR == 1 || $0 ~ /providers\.media|android\.process\.media/' |
    append_media_state_output "$now_text"

  if [ -z "$pid_list" ]; then
    echo "$now_text -- media provider missing" >> "$MEDIA_STATE_LOG_FILE"
  else
    last_pid=$(cat "$MEDIA_STATE_LAST_PID_FILE" 2>/dev/null)
    for pid in $pid_list; do
      status_file="/proc/$pid/status"
      if [ ! -r "$status_file" ]; then
        echo "$now_text -- media process gone pid=$pid" >> "$MEDIA_STATE_LOG_FILE"
        continue
      fi

      echo "$now_text -- /proc/$pid/status summary" >> "$MEDIA_STATE_LOG_FILE"
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

  echo "$now_text -- media_state snapshot end" >> "$MEDIA_STATE_LOG_FILE"
}

start_media_state_collector() {
  # 事件驱动：只在采集器启动的时刻（详细日志打开、配置变化、开机）抓一次快照。
  #
  # 此前是常驻 `while true; sleep 120` 周期采样。存储健康的历史曲线对排障价值有限，
  # 而常驻采样每次唤醒都要读 /proc、跑 dumpsys meminfo 与 logcat，属于规范禁止的
  # 周期轮询。需要现场时由两条事件按需采集：诊断日志包导出（导出脚本实时采集
  # 挂载与进程状态）与停摆取证（daemon 侧 stall-forensics 事件触发）。
  #
  # **必须后台执行。** 快照里的 `dumpsys meminfo` / `logcat -d` 在 A17 上实测耗时
  # 47-58 秒（A14 上 8-10 秒），而调用方是配置事件循环：同步跑等于把「改配置」这件事
  # 按住近一分钟，期间的开开关关全部排队，看起来就像「开关没反应」。后台化不改变语义
  # ——快照只写自己的日志文件、不需要调用方上下文、也不是常驻进程。
  # pid 落盘是为了让下一轮 `start_debug_collectors` 的「先停旧」能收掉还没返回的上一轮。
  #
  # `</dev/null` 不是可选项：后台子 shell 默认继承调用方的 stdin，而调用方是配置事件
  # 循环——它的 stdin 正是 inotifyd 的管道。快照链路里任何一处裸 `read`（`append_media_
  # state_output` 的 `while read` 虽在管道内、但后续维护很容易写出裸 read）都会从那条
  # 管道抢走配置事件，表现为「开关偶尔失灵」。显式断开 stdin 把这类耦合一次性掐死。
  (
    snapshot_started=$(date +%s 2>/dev/null)
    rotate_log_file_if_needed "$MEDIA_STATE_LOG_FILE" "$MAX_MEDIA_STATE_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
    append_media_state_snapshot
    rotate_log_file_if_needed "$MEDIA_STATE_LOG_FILE" "$MAX_MEDIA_STATE_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
    # 带上耗时：后台化之后「何时跑完」不再有日志时间戳可推断，而这条耗时正是判断
    # `dumpsys meminfo` 有没有恶化的唯一现场数据（A17 首次 47-58 秒，稳态 3 秒）。
    log_collector_status "media_state snapshot captured in $(( $(date +%s 2>/dev/null) - ${snapshot_started:-0} ))s"
  ) </dev/null >/dev/null 2>&1 &
  echo "$!" > "$MEDIA_STATE_COLLECTOR_PID_FILE"
  chmod 644 "$MEDIA_STATE_COLLECTOR_PID_FILE" 2>/dev/null || true
}
