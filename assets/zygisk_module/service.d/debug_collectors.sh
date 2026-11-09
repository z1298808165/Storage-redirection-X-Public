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
      # 管道退出到这里意味着 logcat/awk/读循环异常终止（正常情况下永不退出）。
      # 留痕后由外层 while 重启——若反复出现，说明该设备环境持续杀死管道。
      log_collector_status "debug pipeline exited; restarting pid=$debug_logcat_pid"
      sleep 1
    done
  ) &
  debug_logcat_pid="$!"
  for pid_file in "$RUNNING_COLLECTOR_PID_FILE" "$APP_STATUS_COLLECTOR_PID_FILE"; do
    echo "$debug_logcat_pid" > "$pid_file"
    chmod 644 "$pid_file"
  done
  rm -f "$STATS_COLLECTOR_PID_FILE"
  log_collector_status "debug_collector started pid=$debug_logcat_pid"
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
  log_collector_status "debug_collector stopped"
}

start_debug_collectors() {
  is_verbose_logging_enabled || return 0
  ensure_debug_log_files
  stop_debug_logcat_collector
  stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
  # 启动步骤逐段落痕：这条链路由配置事件驱动，一旦中途返回就再也没有第二次机会
  # （周期看门狗已按规范移除），所以每段开始都要留一行，现场据此定位中断点。
  log_collector_status "debug start: logcat"
  start_debug_logcat_collector
  log_collector_status "debug start: media snapshot"
  start_media_state_collector
  log_collector_status "debug start: app snapshot"
  start_app_status_snapshot_collector
  log_collector_status "debug start: ready"
}

# 应用崩溃环形采集器：常驻、不受 verbose 门禁、稳态零开销。
#
# 背景：反馈设备上「图片加载不了」这类应用侧报错只存在于应用自己的日志里，且复现与
# 导出之间往往隔着一两分钟——按需导出的 logcat 快照覆盖不到复现瞬间。因此需要一份
# 常驻的落盘副本。
#
# ── 2026-11-09 真机实测后的两次修正（都有量化数据）──────────────────────────
# 修正 1：`*:E` → 白名单。
#   旧过滤器把全平台/OEM 的 Error 级噪声一起收进来：45 秒实测 1132 行，其中 1047 行
#   是我们自己的 tag，其余全是 HwcComposer / hwcomposer / MiuiWifiService /
#   AmlWifiScoreReportInjector / AlarmManager / HfLooper 之类；历史 ring 25793 行里
#   有 17746 行是 `com.android.phone` 一个 SQLite 建表失败在刷屏——真实应用错误一条
#   都没有。每天为此向 /data 写约 17MB。
# 修正 2：`-b main,crash` → 只订阅 `-b crash`。
#   白名单把落盘量降到 0（实测 60 秒 0 字节），但 CPU 几乎没降（0.317% → 0.283%）。
#   原因：logcat 的过滤发生在客户端——只要订阅了 main 缓冲，它就必须把 main 的全部
#   条目从 logd 收下来再逐条丢弃。也就是说 0.28% 单核（≈28 mW，约 0.7 Wh/天）是
#   「订阅 main」本身的价格，与收多少行无关。实测只订阅 crash 缓冲时是 0.000% CPU、
#   0 字节落盘——因为 crash 缓冲只在真崩溃时才有内容。
#
# 结论：常驻部分只订阅 crash（免费，覆盖最有价值的信号：Java 崩溃栈 + 原生 tombstone
# 头）；主缓冲的 Error / 图片组件日志改为按需窗口采集（见下方
# start_app_error_window_capture），需要时武装 N 分钟，成本与「真要看日志」严格对齐。
# 「任意应用任意错误」也可以走按需导出（diagnostic_archive.sh 会整体抓 logd 缓冲）。
start_app_error_ring_collector() {
  if is_process_alive "$(cat "$APP_ERROR_RING_PID_FILE" 2>/dev/null)"; then
    return 0
  fi
  rm -f "$APP_ERROR_RING_PID_FILE"
  # 过滤器版本迁移：旧版 `*:E` 与白名单版攒下的 ring 口径不同，混在一起无法判读，
  # 因此按版本号清一次（新版稳态几乎不产生内容，旧文件不会被自然轮转掉）。
  app_error_filter_state="$LOGS_DIR/.app_error_filter_version"
  if [ "$(cat "$app_error_filter_state" 2>/dev/null)" != "$APP_ERROR_RING_FILTER_VERSION" ]; then
    rm -f "$APP_ERROR_RING_LOG_FILE"
    printf '%s\n' "$APP_ERROR_RING_FILTER_VERSION" > "$app_error_filter_state" 2>/dev/null || true
    chmod 644 "$app_error_filter_state" 2>/dev/null || true
    log_collector_status "app_error_ring filter upgraded to $APP_ERROR_RING_FILTER_VERSION; old ring dropped"
  fi
  (
    exec -a "$SRX_APP_ERROR_COLLECTOR_TAG" /system/bin/logcat -b crash \
      -T 1 -v threadtime \
      -f "$APP_ERROR_RING_LOG_FILE" -r 4096 -n 5 \
      AndroidRuntime:E libc:F DEBUG:F '*:S'
  ) 2>/dev/null &
  echo "$!" > "$APP_ERROR_RING_PID_FILE"
  chmod 644 "$APP_ERROR_RING_PID_FILE" 2>/dev/null || true
  chmod 644 "$APP_ERROR_RING_LOG_FILE" 2>/dev/null || true
  log_collector_status "app_error_ring started pid=$(cat "$APP_ERROR_RING_PID_FILE" 2>/dev/null)"
}

# 主缓冲错误窗口采集（按需）。用法：`start_app_error_window_capture [分钟，默认 10]`
#
# 只在「有人要诊断」时武装：订阅 main 缓冲的价格是 0.28% 单核，不能常驻。窗口结束时
# 自动退出，也可用 `stop_app_error_window_capture` 提前结束。落盘到
# app-errors-window.log，与常驻的 crash ring 分开，避免混口径。
start_app_error_window_capture() {
  minutes="${1:-${APP_ERROR_WINDOW_MINUTES:-10}}"
  case "$minutes" in
    ''|*[!0-9]*) minutes=10 ;;
  esac
  [ "$minutes" -lt 1 ] && minutes=1
  [ "$minutes" -gt 60 ] && minutes=60
  if is_process_alive "$(cat "$APP_ERROR_WINDOW_PID_FILE" 2>/dev/null)"; then
    log_collector_status "app_error_window already running pid=$(cat "$APP_ERROR_WINDOW_PID_FILE" 2>/dev/null)"
    return 0
  fi
  rm -f "$APP_ERROR_WINDOW_PID_FILE"
  # 外层子 shell 负责「定时回收」，内层 `exec -a` 负责让 logcat 带上可识别标记
  # （与 crash ring 一致，便于 reap_stale_logcat_collectors 回收）。整个函数立即返回，
  # 不会阻塞调用方（配置事件收集器的 shell）。
  (
    (
      exec -a "$SRX_APP_WINDOW_COLLECTOR_TAG" /system/bin/logcat -b main \
      -T 1 -v threadtime \
      -f "$APP_ERROR_WINDOW_LOG_FILE" -r 4096 -n 2 \
      AndroidRuntime:E libc:F DEBUG:F StorageRedirect:E \
      MediaProvider:E MediaScanner:E MediaProvider2:E \
      ImageDecoder:V Bitmap:V Skia:V Glide:V Fresco:V Coil:V ImageLoader:V \
      ImagePipeLine:V chromium:V libEGL:V \
      SRX:V '*:S'
    ) 2>/dev/null &
    window_pid=$!
    echo "$window_pid" > "$APP_ERROR_WINDOW_PID_FILE"
    chmod 644 "$APP_ERROR_WINDOW_PID_FILE" 2>/dev/null || true
    chmod 644 "$APP_ERROR_WINDOW_LOG_FILE" 2>/dev/null || true
    log_collector_status "app_error_window started pid=$window_pid minutes=$minutes"
    # 一次等待一个已知截止点，然后释放订阅；这不是轮询。
    sleep $((minutes * 60))
    kill "$window_pid" 2>/dev/null || true
    rm -f "$APP_ERROR_WINDOW_PID_FILE"
    log_collector_status "app_error_window ended minutes=$minutes"
  ) >/dev/null 2>&1 &
  log_collector_status "app_error_window armed minutes=$minutes"
}

stop_app_error_window_capture() {
  stop_collector_by_pid_file "$APP_ERROR_WINDOW_PID_FILE"
}

stop_app_error_ring_collector() {
  stop_collector_by_pid_file "$APP_ERROR_RING_PID_FILE"
}

stop_debug_collectors() {
  stop_debug_logcat_collector
  stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
}

# sync 收敛语义（幂等 + 自愈）：
# - desired 与状态文件一致且进程存活 → 直接跳过（允许被周期性调用而不产生重启抖动）；
# - desired=on 但采集器死亡 → 自动重启（自愈此前「boot 后首个 sync 判定 off、
#   之后 verbose 翻转再无事件」导致的整个 boot 静默）；
# - desired=off 且已停 → 跳过。
# 状态文件是引导时序的安全网：boot 首个 sync 读到的 global.json 可能尚未反映
# 用户当前设置，后续周期重试会把它收敛到最新判定。
sync_debug_collectors() {
  if is_verbose_logging_enabled; then
    debug_collectors_sync_to "on"
  else
    debug_collectors_sync_to "off"
  fi
  # 顺带确保常驻 crash ring 在位：配置事件是它在异常退出后唯一的自愈机会
  # （幂等：存活时直接返回，不产生额外进程或唤醒）。
  start_app_error_ring_collector
}

debug_collectors_sync_to() {
  desired="$1"
  last_state=$(cat "$DEBUG_COLLECTORS_STATE_FILE" 2>/dev/null)
  if [ "$desired" = "off" ]; then
    if [ "$last_state" = "off" ]; then
      return 0
    fi
    log_collector_status "sync: stopping debug collectors (verbose off)"
    stop_debug_collectors
    printf '%s' "off" > "$DEBUG_COLLECTORS_STATE_FILE" 2>/dev/null
    return 0
  fi
  if [ "$last_state" = "on" ]; then
    running_pid=$(cat "$RUNNING_COLLECTOR_PID_FILE" 2>/dev/null)
    if is_process_alive "$running_pid"; then
      return 0
    fi
    log_collector_status "sync: debug collector dead, restarting"
  else
    log_collector_status "sync: starting debug collectors (verbose on)"
  fi
  start_debug_collectors
  printf '%s' "on" > "$DEBUG_COLLECTORS_STATE_FILE" 2>/dev/null
}
