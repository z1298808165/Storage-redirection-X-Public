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

# 应用错误环形采集器：常驻、不受 verbose 门禁。
#
# 背景：反馈设备上「图片加载不了」的应用侧报错只存在于应用自己的日志里，且复现
# 与导出之间往往隔着一两分钟——按需导出的 logcat 快照（哪怕 20000 行）在高频
# OEM 日志下也覆盖不到复现瞬间。这里从开机起就把全标签 Error 级（含常见图片组件
# 定向、OEM 遥测标签静默）落进 logs/app-errors.log 环形文件（logcat 自带轮转，
# 上限约 24MB），诊断包采集时随 $LOGS_DIR 整体带走，导出时机不再敏感。
# 抑制清单按真机 ring 实测的噪声分布维护（ColorOS 遥测/UI/编解码标签每分钟
# 数百行）；AndroidRuntime 故意保留——它是真实崩溃信号，开机突发只有一分钟。
start_app_error_ring_collector() {
  if is_process_alive "$(cat "$APP_ERROR_RING_PID_FILE" 2>/dev/null)"; then
    return 0
  fi
  rm -f "$APP_ERROR_RING_PID_FILE"
  ( exec -a "$SRX_APP_ERROR_COLLECTOR_TAG" /system/bin/logcat -b main,crash \
      -T 1 -v threadtime \
      -f "$APP_ERROR_RING_LOG_FILE" -r 4096 -n 5 \
      OPLUS_KEVENT_RECORD:S midasd:S statsd:S oiface:S PERFHAL-BOOSTCONFIG:S \
      PerformanceService:S MIDASApplication:S metis_v2_FactManager:S \
      AEC.h:S AEC.AudioxService:S AMC:S SettingsTelemetryUtils:S artd:S libEGL:S \
      OPLUS_KEVENT_RECORD:S DynamicFramerate:S AppFeatureProviderUtils:S \
      ThemeUtils:S ANDR-PWR-OPT:S Parcel:S PERFHAL-CLIENT:S \
      LayerAvatar_masker_render:S BufferQueueProducer:S SDM:S HWUI:S \
      SchedAssist:S CCodecBufferChannel:S NfcRegisteredServicesCache:S \
      DeviceHalAidl:S QC2Comp:S PERFHAL-RESOURCEQS:S \
      WeatherS_WeatherHttpDataSource:S UnionPermissionController:S \
      NotificationBadgeManager:S LSPosedFramework:S OplusBracketLog:S \
      OplusContextUtil:S OplusPredictiveBackController:S \
      FastDynamicDrawable_CacheFrameProducer:S SettingsState:S \
      ImageDecoder:V Bitmap:V chromium:V ImagePipeLine:V Glide:V Fresco:V Coil:V \
      ImageLoader:V Skia:V SRX:V '*:E' ) 2>/dev/null &
  echo "$!" > "$APP_ERROR_RING_PID_FILE"
  chmod 644 "$APP_ERROR_RING_PID_FILE" 2>/dev/null || true
  chmod 644 "$APP_ERROR_RING_LOG_FILE" 2>/dev/null || true
  log_collector_status "app_error_ring started pid=$(cat "$APP_ERROR_RING_PID_FILE" 2>/dev/null)"
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
