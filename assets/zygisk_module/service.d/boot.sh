boot_guard_wait() {
  boot_id=""
  if [ -r /proc/sys/kernel/random/boot_id ]; then
    boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
  fi

  # 重启后，上一次开机留下的采集器 pid 文件与采集器状态文件一律不可信：pid 号会被
  # 复用，旧 pid 可能已经属于一个无关进程。两头的后果都实测过（A14 模拟器）：
  #   - 用它判活 → 本 boot 静默跳过采集器启动（state=on + 旧 pid 存活时，连
  #     `sync: starting debug collectors` 都不会出现，详细日志静默缺失）；
  #   - 用它停止 → 关闭开关时把无关进程 SIGKILL 掉。
  # pid 文件只能证明「本 boot 内启动过」，所以在本 boot 首次运行服务、且尚未启动任何
  # 采集器时清空它们（本函数是 service.sh 里最早接触采集器的位置）。服务被 srxctl
  # 重跑时本 boot 的 marker 已存在，不会清掉正在运行的采集器记录。
  # daemon 的 pid 文件不在此列：start_srx_daemon 按 comm 校验自愈，且它先于本函数执行。
  if [ -n "$boot_id" ] && [ ! -f "$LOGS_DIR/boot_${boot_id}.marker" ]; then
    rm -f "$MONITOR_COLLECTOR_PID_FILE" "$CONFIG_EVENT_COLLECTOR_PID_FILE" \
      "$PACKAGE_EVENT_COLLECTOR_PID_FILE" "$RUNNING_COLLECTOR_PID_FILE" \
      "$MEDIA_STATE_COLLECTOR_PID_FILE" "$APP_STATUS_COLLECTOR_PID_FILE" \
      "$APP_STATUS_SNAPSHOT_PID_FILE" "$STATS_COLLECTOR_PID_FILE" \
      "$APP_ERROR_RING_PID_FILE" "$APP_ERROR_WINDOW_PID_FILE" \
      "$DEBUG_COLLECTORS_STATE_FILE"
  fi

  timeout=180
  i=0
  while [ $i -lt $timeout ]; do
    if [ "$(getprop sys.boot_completed 2>/dev/null)" = "1" ]; then
      if [ -n "$boot_id" ]; then
        echo "$boot_id" > "$BOOT_OK_FILE"
        : > "$LOGS_DIR/boot_${boot_id}.marker"
        chmod 644 "$LOGS_DIR/boot_${boot_id}.marker" 2>/dev/null
      else
        echo "unknown" > "$BOOT_OK_FILE"
      fi
      rm -f "$BOOT_PENDING_FILE" "$BOOT_FAIL_COUNT_FILE"
      refresh_uid_map
      restart_media_provider_for_deferred_hooks
      start_log_collectors
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done

  if [ -n "$boot_id" ]; then
    echo "$boot_id" > "$BOOT_OK_FILE"
    : > "$LOGS_DIR/boot_${boot_id}.marker"
    chmod 644 "$LOGS_DIR/boot_${boot_id}.marker" 2>/dev/null
  else
    echo "unknown" > "$BOOT_OK_FILE"
  fi
  rm -f "$BOOT_PENDING_FILE" "$BOOT_FAIL_COUNT_FILE"
  refresh_uid_map
  start_log_collectors
  return 0
}

restart_media_provider_for_deferred_hooks() {
  if [ ! -f "$MEDIA_HOOK_DEFERRED_FILE" ]; then
    return 0
  fi
  sdk=$(getprop ro.build.version.sdk 2>/dev/null || true)
  if [ -n "$sdk" ] && [ "$sdk" -ge 37 ]; then
    rm -f "$MEDIA_HOOK_DEFERRED_FILE"
    log -p i -t Boot "skip MediaProvider restart for lazy Android $sdk provider"
    return 0
  fi
  rm -f "$MEDIA_HOOK_DEFERRED_FILE"

  media_pkgs=$(pm list packages 2>/dev/null |
    sed -n 's/^package://p' |
    grep -E '^(com\.android\.providers\.media|com\.android\.providers\.media\.module|com\.google\.android\.providers\.media\.module)$')
  media_procs="$media_pkgs android.process.media com.android.providers.downloads com.android.mtp"

  for package_name in $media_procs; do
    pids=$(pidof "$package_name" 2>/dev/null)
    for pid in $pids; do
      kill "$pid" 2>/dev/null || true
    done
  done

  sleep 1
  for package_name in $media_procs; do
    pids=$(pidof "$package_name" 2>/dev/null)
    for pid in $pids; do
      kill -9 "$pid" 2>/dev/null || true
    done
  done

  content query --uri content://media/external/file --projection _id --limit 1 >/dev/null 2>&1 || true
  content query --uri content://media/internal/file --projection _id --limit 1 >/dev/null 2>&1 || true
  log -p i -t Boot "restarted MediaProvider for deferred srx hooks"
}
