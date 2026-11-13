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

# 按指定信号终止进程；缺省为 TERM，采集器一律传 KILL。
#
# 为什么采集器必须用 KILL：采集器子 shell 是 mksh，`SigCgt` 里带 SIGTERM（它捕获该
# 信号），而它平时正阻塞在管道的 `read` 上。A14 模拟器实测：对子 shell 发完 SIGTERM
# 后子 shell、logcat、awk 三者全部存活，而调用方已经把「已停止」写进状态文件与 pid
# 文件——`debug_collectors_sync_to off` 的 state 早退让此后任何配置事件都不会再尝试
# 停止，泄漏就此锁定到下次开机。KILL 不可捕获、不可延后，是这里唯一确定的选择。
kill_with_signal() {
  [ -n "$1" ] || return 1
  case "$2" in
    KILL|9) kill -9 "$1" 2>/dev/null ;;
    *) kill "$1" 2>/dev/null ;;
  esac
}

# 停止后台进程。第二个参数是信号名（缺省 TERM）。
#
# 三个坑都在这里，改之前先读完：
#
# 1. **不要用全局变量存目标 pid。** 旧实现是 `target_pid="$1"` + 递归 `stop_background_process
#    "$child_pid"`，而递归会把同一个全局 `target_pid` 覆盖成子进程的 pid 且返回后不恢复；
#    于是循环结束后那句 `kill "$target_pid"` 打的是最后一个**子**进程（已经是尸体），
#    父 shell 从头到尾没收到过信号。现在一律使用位置参数：函数返回后 `$1` 自动恢复，
#    递归不会污染调用者。
# 2. 递归段优先读 `/proc/<pid>/task/<pid>/children`，但该文件并非所有内核都提供
#    （A14 雷电、A17 AvdLite 上都不存在）。之前遇到这种情况就整段跳过递归，等于把
#    「子进程会被顺带杀掉」当成了运气；现在退化为 `ps` 的 PPID 列——同一次快照里的父子
#    关系是静态的，足够覆盖采集器这一层（subshell → logcat / awk / inotifyd）。
# 3. **不要为了「能按名字找到子进程」去给它们打 `exec -a` 标记**：`/system/bin/inotifyd`
#    是 toybox 的符号链接，toybox 靠 argv[0] 分派 applet，改掉 argv[0] 后它直接起不来。
#    2026-11-11 在 A17 上实测踩过：两个事件收集器的监视线程全部起不来，配置事件整条链路
#    静默失效，而表面上进程「起来又停掉」看不出异常。
stop_background_process() {
  if [ -z "$1" ] || ! kill -0 "$1" 2>/dev/null; then
    return 0
  fi

  # 先取子进程清单再动手：先杀父会让子进程被 init 收养，之后按 PPID 就再也找不到它们。
  children_file="/proc/$1/task/$1/children"
  if [ -r "$children_file" ]; then
    child_pids=$(cat "$children_file" 2>/dev/null)
  else
    child_pids=$(/system/bin/ps -A -o PID,PPID 2>/dev/null | awk -v root="$1" '$2 == root { print $1 }')
  fi
  for child_pid in $child_pids; do
    stop_background_process "$child_pid" "${2:-TERM}"
  done
  kill_with_signal "$1" "${2:-TERM}"
  wait "$1" 2>/dev/null
}

# 按 pid 文件停止采集器。第二个参数是信号名，缺省 KILL。
#
# 这里的目标一律是本模块自己的后台循环（config/package 事件收集器、媒体与应用状态快照、
# 详细日志采集器的子 shell），全是 mksh 进程。**默认必须是 KILL**：SIGTERM 会被延后到
# 它当前阻塞的那次 `read` 返回才处理，而这些循环恰恰长期阻塞在管道读上——发过 TERM 在
# 运行期等价于没杀（详见 stop_background_process 的注释）。早期版本用默认 TERM，等于
# `start_log_collectors` 里「先停旧再起新」的停从来没生效过，srxctl 重跑 service.sh 时
# 会留下两套事件循环。KILL 之后 `wait` 必定立即返回，不会再出现调用方被挂住的问题。
#
# 管道里的成员（inotifyd / awk / 读循环）由 stop_background_process 的递归段一并收掉，
# 不需要给它们单独打标记（toybox 的 inotifyd 也不能打，见那里的注释）。
stop_collector_by_pid_file() {
  pid_file="$1"
  if [ ! -f "$pid_file" ]; then
    return 0
  fi

  pid=$(cat "$pid_file" 2>/dev/null)
  stop_background_process "$pid" "${2:-KILL}"
  rm -f "$pid_file"
}

# 管道型采集器给自己的成员打 `exec -a` 标记，停止时按标记收尾。
#
# 前提：**只有真正的 ELF 可执行文件能这么打**。toybox 的多调用二进制（inotifyd 等）靠
# argv[0] 分派 applet，改掉 argv[0] 它就直接起不来（见 stop_background_process 第 3 条）。
# 所以这里只标记 logcat 一个，管道里其余成员由 stop_background_process 的递归段收掉。
SRX_COLLECTOR_TAG="srx-debug-collector"
# 应用错误环形采集器使用独立标记：它不受 verbose 门禁、常驻运行，
# 不能被 debug 采集器的例行重启（reap + 重启）误杀。
SRX_APP_ERROR_COLLECTOR_TAG="srx-apperror-collector"
# 主缓冲按需窗口采集器的标记（与常驻 crash ring 区分，便于现场辨认）。
SRX_APP_WINDOW_COLLECTOR_TAG="srx-apperror-window"

# 回收本模块遗留的 logcat 采集进程。可选参数为信号名，缺省 KILL。
#
# 只回收 `$SRX_COLLECTOR_TAG` 一个标记。**不要**把常驻 crash ring 纳入：本函数由
# `start_debug_logcat_collector` 调用，而后者会被「verbose 开关翻转」这类配置事件
# 触发——一并回收就等于每次改配置都静默杀掉常驻采集器（且不会重建），崩溃现场
# 从此丢失。crash ring 的生命周期由自己的 pid 文件 + 幂等启动管理。
#
# 注意本函数**只**能收掉 logcat 本身，收不掉采集器子 shell：子 shell 会在内层
# `while true` 的重建分支里 1 秒内拉起新的 logcat（实测 pid 8032 → 8468）。所以调用顺序
# 必须是「先让子 shell 死掉（stop_debug_logcat_collector 用 KILL 打 pid 文件里的 pid），
# 再调用本函数收管道里的 logcat」；单独调用本函数只会换来一个新 pid。
#
# 实现上两点不能改：
# 1. **用一次 `ps` 取全表，不要逐个读 `/proc/<pid>/cmdline`**：目标进程若在读取过程中
#    消失，`tr` 会在 R 态自旋不返回；本函数跑在配置事件循环里，一卡就等于此后所有配置
#    变更都不再被处理。前缀匹配而非子串匹配：`ps`/`awk` 自己的 argv 也可能含该标记。
# 2. **`ps` 必须写绝对路径 `/system/bin/ps`**：开机走 `busybox sh`（ash），它把 applet
#    当内建命令，`ps` 会落到 busybox 版——只认小写 `-o pid`，`-o PID,ARGS` 直接 rc=1。
#    错误被 `2>/dev/null` 吞掉后，表现就是「选不出进程、什么都没杀」，每轮开关泄漏一个
#    logcat。同一个坑还打掉了 `stop_background_process` 的 PPID 递归段。
#    其它用 `-o` 的 `ps`（media_state / app_status / diagnostic_archive / uninstall）同理。
reap_stale_logcat_collectors() {
  reap_signal="${1:-KILL}"
  reaped=0
  victims=$(/system/bin/ps -A -o PID,ARGS 2>/dev/null | awk -v tag="$SRX_COLLECTOR_TAG" '
    {
      args = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", args)
      if (index(args, tag) == 1) print $1
    }')
  # 先收集、后统一杀：不要在扫描过程中就地 kill，避免「刚杀完又去读它」这类自造竞态。
  for victim in $victims; do
    if kill_with_signal "$victim" "$reap_signal"; then
      reaped=$((reaped + 1))
    fi
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

  # 查询端限定 IPC 截止点；失败时不覆盖上一份快照。缓存保持 appId 格式供
  # 无 root 通道的 hook 读取，companion 授权始终另行查询完整 UID。
  {
    echo "# package:uid"
    "$MODDIR/bin/srx_daemon" package-query 2>/dev/null |
      awk -F: '{ printf "%s:%d\n", $1, $2 % 100000 }' |
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
