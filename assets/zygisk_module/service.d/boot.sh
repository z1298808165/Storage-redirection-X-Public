boot_guard_wait() {
  boot_id=""
  if [ -r /proc/sys/kernel/random/boot_id ]; then
    boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
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
      rm -f "$BOOT_PENDING_FILE"
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
  rm -f "$BOOT_PENDING_FILE"
  refresh_uid_map
  start_log_collectors
  return 0
}

restart_media_provider_for_deferred_hooks() {
  if [ ! -f "$MEDIA_HOOK_DEFERRED_FILE" ]; then
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
