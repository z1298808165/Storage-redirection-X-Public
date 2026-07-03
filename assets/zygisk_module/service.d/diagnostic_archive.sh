#!/system/bin/sh

MODDIR=${0%/*}
MODDIR=${MODDIR%/service.d}
[ -n "$MODDIR" ] || MODDIR="/data/adb/modules/storage.redirect.x"

CONFIG_DIR="$MODDIR/config"
APPS_CONFIG_DIR="$CONFIG_DIR/apps"
LOGS_DIR="$MODDIR/logs"
STATE_DIR=""
PROC_DIR=""
ANR_DIR=""
TOMBSTONE_DIR=""
PROGRESS_FILE=""

die() {
  update_progress 98 error "$*"
  echo "diagnostic_archive: $*" >&2
  exit 1
}

is_managed_tmp_path() {
  value="$1"
  case "$value" in
    /data/local/tmp/srx_*) ;;
    *) return 1 ;;
  esac
  case "$value" in
    *..*|*"//"*|*'|'*) return 1 ;;
  esac
  return 0
}

safe_name() {
  printf '%s' "$1" | sed 's/[^A-Za-z0-9_.-]/_/g' | cut -c 1-120
}

update_progress() {
  [ -n "$PROGRESS_FILE" ] || return 0
  is_managed_tmp_path "$PROGRESS_FILE" || return 0
  percent="$1"
  phase="$2"
  message="$3"
  tmp_progress="$PROGRESS_FILE.tmp"
  printf '%s|%s|%s\n' "$percent" "$phase" "$message" > "$tmp_progress" 2>/dev/null &&
    mv "$tmp_progress" "$PROGRESS_FILE" 2>/dev/null || true
}

capture_cmd() {
  out_file="$1"
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout 8 "$@" > "$out_file" 2>&1 || true
  else
    "$@" > "$out_file" 2>&1 || true
  fi
}

copy_file_if_exists() {
  src="$1"
  dst="$2"
  [ -f "$src" ] || return 0
  cp -p "$src" "$dst" 2>/dev/null || true
}

add_package_candidate() {
  package_name="$1"
  case "$package_name" in
    ""|*[!A-Za-z0-9_.-]*) return 0 ;;
  esac
  echo "$package_name" >> "$STATE_DIR/package-candidates.raw"
}

configured_packages() {
  [ -d "$APPS_CONFIG_DIR" ] || return 0
  for config_file in "$APPS_CONFIG_DIR"/*.json; do
    [ -f "$config_file" ] || continue
    basename "$config_file" .json
  done
}

collect_package_candidates() {
  : > "$STATE_DIR/package-candidates.raw"

  for package_name in \
    org.srx.manager \
    storage.redirect.x \
    com.android.providers.media.module \
    com.google.android.providers.media.module \
    com.android.providers.media \
    android.process.media \
    com.android.externalstorage \
    com.android.providers.downloads \
    com.android.providers.downloads.ui \
    com.android.documentsui \
    com.google.android.documentsui \
    com.android.mtp \
    bin.mt.plus \
    com.bin.mt.plus \
    com.mi.android.globalFileexplorer \
    com.android.fileexplorer \
    com.google.android.apps.nbu.files \
    com.mixplorer \
    pl.solidexplorer2 \
    com.lonelycatgames.Xplore \
    com.alphainventor.filemanager; do
    add_package_candidate "$package_name"
  done

  configured_packages | while IFS= read -r package_name; do
    add_package_candidate "$package_name"
  done

  if [ -f "$LOGS_DIR/file_monitor.log" ]; then
    tail -n 800 "$LOGS_DIR/file_monitor.log" 2>/dev/null |
      awk -F'|' 'NF >= 3 { print $2; print $3 }' |
      tr ',' '\n' |
      while IFS= read -r package_name; do
        add_package_candidate "$package_name"
      done
  fi

  sort -u "$STATE_DIR/package-candidates.raw" > "$STATE_DIR/package-candidates.txt" 2>/dev/null || true
}

collect_basic_files() {
  mkdir -p "$stage/logs" "$stage/config/apps"

  if [ -d "$LOGS_DIR" ]; then
    find "$LOGS_DIR" -maxdepth 1 -type f \
      ! -name ".*.pid" \
      ! -name ".uid_map_last_refresh" \
      ! -name ".app_status_*" \
      -exec cp -p {} "$stage/logs/" \; 2>/dev/null || true
  fi

  copy_file_if_exists "$MODDIR/module.prop" "$stage/module.prop"
  copy_file_if_exists "$MODDIR/stats" "$stage/stats"
  copy_file_if_exists "$CONFIG_DIR/global.json" "$stage/config/global.json"
  copy_file_if_exists "$CONFIG_DIR/file_monitor_filters.json" "$stage/config/file_monitor_filters.json"
  copy_file_if_exists "$CONFIG_DIR/templates.json" "$stage/config/templates.json"

  if [ -d "$APPS_CONFIG_DIR" ]; then
    find "$APPS_CONFIG_DIR" -maxdepth 1 -type f -name "*.json" \
      -exec cp -p {} "$stage/config/apps/" \; 2>/dev/null || true
  fi
}

collect_device_state() {
  {
    echo "diagnostic_archive_version=3"
    echo "progress_protocol=1"
    echo "created_at=$(date '+%Y-%m-%d %H:%M:%S %z' 2>/dev/null || date 2>/dev/null)"
    echo "id:"
    id
    echo
    echo "uname:"
    uname -a
    echo
    echo "boot_id:"
    cat /proc/sys/kernel/random/boot_id 2>/dev/null
    echo
    echo "getenforce:"
    getenforce 2>/dev/null || true
    echo
    echo "selected getprop:"
    getprop 2>/dev/null |
      grep -E '^\[(ro\.build|ro\.product|ro\.system|ro\.vendor|ro\.odm|ro\.hardware|ro\.zygote|ro\.dalvik|sys\.boot_completed|persist\.sys\.(locale|timezone)|init\.svc\.(zygote|zygote64|media|mediaextractor|mediametrics|vold|storaged|installd|logd|surfaceflinger))'
  } > "$STATE_DIR/device.txt" 2>&1

  {
    echo "module status:"
    /system/bin/sh "$MODDIR/bin/srxctl" status 2>/dev/null || true
    echo
    echo "module dir:"
    ls -la "$MODDIR" 2>/dev/null
    echo
    echo "module dir with selinux:"
    ls -laZ "$MODDIR" 2>/dev/null || true
    echo
    echo "logs dir:"
    ls -la "$LOGS_DIR" 2>/dev/null
    echo
    echo "config dir:"
    ls -la "$CONFIG_DIR" 2>/dev/null
    echo
    echo "service.d:"
    ls -la "$MODDIR/service.d" 2>/dev/null
  } > "$STATE_DIR/module.txt" 2>&1

  {
    echo "configured packages:"
    configured_packages | sort -u
    echo
    echo "config file count:"
    find "$APPS_CONFIG_DIR" -maxdepth 1 -type f -name "*.json" 2>/dev/null | wc -l
  } > "$STATE_DIR/config-summary.txt" 2>&1
}

collect_process_state() {
  {
    ps -A -o PID,PPID,USER,NAME,ARGS 2>/dev/null ||
      ps -A -o PID,USER,ARGS 2>/dev/null ||
      ps -A 2>/dev/null
  } > "$STATE_DIR/processes.txt" 2>&1

  ps -A -o PID,NAME 2>/dev/null > "$STATE_DIR/process-names.txt" 2>/dev/null || true

  {
    echo "media provider pids:"
    for package_name in \
      com.android.providers.media.module \
      com.google.android.providers.media.module \
      com.android.providers.media \
      android.process.media; do
      echo "## $package_name"
      pidof "$package_name" 2>/dev/null || true
    done
  } > "$STATE_DIR/media-pids.txt" 2>&1

  {
    echo "package list with uid:"
    cmd package list packages -U 2>/dev/null || true
    echo
    echo "user packages with uid and path:"
    pm list packages -3 -f -U 2>/dev/null || true
    echo
    echo "system packages with uid and path:"
    pm list packages -s -f -U 2>/dev/null || true
  } > "$STATE_DIR/package-uids.txt" 2>&1
}

collect_mount_state() {
  {
    echo "mount:"
    mount 2>/dev/null
    echo
    echo "/proc/self/mountinfo:"
    cat /proc/self/mountinfo 2>/dev/null
    echo
    echo "/proc/mounts:"
    cat /proc/mounts 2>/dev/null
    echo
    echo "df:"
    df 2>/dev/null
  } > "$STATE_DIR/mounts.txt" 2>&1

  {
    for path in \
      /storage \
      /storage/emulated \
      /storage/emulated/0 \
      /sdcard \
      /mnt/user \
      /mnt/runtime \
      /mnt/media_rw \
      /data/media \
      /data/media/0; do
      echo "## $path"
      ls -la "$path" 2>/dev/null || true
      ls -laZ "$path" 2>/dev/null || true
    done
  } > "$STATE_DIR/storage-paths.txt" 2>&1
}

collect_proc_for_pid() {
  pid="$1"
  label="$2"
  [ -n "$pid" ] || return 0
  [ -d "/proc/$pid" ] || return 0
  prefix="$PROC_DIR/$(safe_name "$label")_$pid"

  {
    echo "label=$label"
    echo "pid=$pid"
    echo
    echo "cmdline:"
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null
    echo
    echo
    echo "status:"
    cat "/proc/$pid/status" 2>/dev/null
    echo
    echo "stat:"
    cat "/proc/$pid/stat" 2>/dev/null
    echo
    echo "wchan:"
    cat "/proc/$pid/wchan" 2>/dev/null
    echo
    echo "limits:"
    cat "/proc/$pid/limits" 2>/dev/null
  } > "$prefix-status.txt" 2>&1

  cat "/proc/$pid/mountinfo" > "$prefix-mountinfo.txt" 2>/dev/null || true
  grep -E ' /storage| /mnt/user| /mnt/runtime| /mnt/media_rw| /sdcard| /data/media|storage.redirect.x|fuse' \
    "/proc/$pid/mountinfo" > "$prefix-mountinfo-storage.txt" 2>/dev/null || true

  {
    ls -l "/proc/$pid/fd" 2>/dev/null | head -n 80
  } > "$prefix-fd.txt" 2>&1

  {
    count=0
    for task_dir in "/proc/$pid"/task/*; do
      [ -d "$task_dir" ] || continue
      task_id=${task_dir##*/}
      echo "## task $task_id"
      grep -E '^(Name|State|Tgid|Pid|PPid|TracerPid|Uid|Gid|FDSize|Vm|Threads|Sig|Cap|voluntary|nonvoluntary)' \
        "$task_dir/status" 2>/dev/null || true
      printf 'Wchan:\t'
      cat "$task_dir/wchan" 2>/dev/null || true
      echo
      count=$((count + 1))
      [ "$count" -lt 80 ] || break
    done
  } > "$prefix-threads.txt" 2>&1
}

collect_relevant_proc_state() {
  [ -f "$STATE_DIR/package-candidates.txt" ] || return 0
  : > "$STATE_DIR/package-pids.txt"
  detail_count=0
  max_detail_pids=10
  matches="$STATE_DIR/package-pids.raw"
  sorted_matches="$STATE_DIR/package-pids.sorted"
  : > "$matches"

  awk '
    NR == FNR {
      if ($1 != "") candidates[$1] = 1
      next
    }
    NR > 1 && NF >= 2 {
      pid = $1
      name = $2
      for (package_name in candidates) {
        if (name == package_name || index(name, package_name ":") == 1) {
          print package_name "|" pid
        }
      }
    }
  ' "$STATE_DIR/package-candidates.txt" "$STATE_DIR/process-names.txt" >> "$matches" 2>/dev/null || true

  for package_name in \
    com.android.providers.media.module \
    com.google.android.providers.media.module \
    com.android.providers.media \
    android.process.media \
    com.android.documentsui \
    com.google.android.documentsui \
    bin.mt.plus \
    com.bin.mt.plus; do
    pidof "$package_name" 2>/dev/null | tr ' ' '\n' |
      awk -v package_name="$package_name" '$1 ~ /^[0-9]+$/ { print package_name "|" $1 }'
  done >> "$matches"

  sort -u "$matches" > "$sorted_matches" 2>/dev/null || true
  while IFS='|' read -r package_name pid; do
    [ -n "$package_name" ] || continue
    [ -n "$pid" ] || continue
    if [ "$detail_count" -ge "$max_detail_pids" ]; then
      echo "proc detail collection capped at $max_detail_pids pids" >> "$STATE_DIR/package-pids.txt"
      return 0
    fi
    echo "$package_name $pid" >> "$STATE_DIR/package-pids.txt"
    progress=$((54 + detail_count * 10 / max_detail_pids))
    update_progress "$progress" proc "正在采集进程细节 $((detail_count + 1))/$max_detail_pids"
    collect_proc_for_pid "$pid" "$package_name"
    detail_count=$((detail_count + 1))
  done < "$sorted_matches"
}

collect_dumpsys_state() {
  capture_cmd "$STATE_DIR/dumpsys-mount.txt" dumpsys mount
  capture_cmd "$STATE_DIR/dumpsys-activity-processes.txt" dumpsys activity processes
  capture_cmd "$STATE_DIR/dumpsys-activity-top.txt" dumpsys activity top
  capture_cmd "$STATE_DIR/dumpsys-window.txt" dumpsys window
  capture_cmd "$STATE_DIR/dumpsys-user.txt" dumpsys user
  capture_cmd "$STATE_DIR/dumpsys-media-provider.txt" dumpsys media.provider
  capture_cmd "$STATE_DIR/dumpsys-media-metrics.txt" dumpsys media.metrics
  capture_cmd "$STATE_DIR/dumpsys-meminfo-summary.txt" dumpsys meminfo -s

  {
    for package_name in \
      com.android.providers.media.module \
      com.google.android.providers.media.module \
      com.android.providers.media \
      com.android.externalstorage \
      com.android.providers.downloads \
      com.android.documentsui \
      com.google.android.documentsui; do
      echo "## dumpsys package $package_name"
      dumpsys package "$package_name" 2>/dev/null | head -n 180
      echo
    done
  } > "$STATE_DIR/dumpsys-key-packages.txt" 2>&1
}

collect_logcat_state() {
  logcat -g > "$stage/logcat-buffers.txt" 2>&1 || true
  logcat -b main,system,crash -d -t 4000 -v threadtime \
    > "$stage/logcat-main-system-crash.txt" 2>&1 || true
  logcat -b events -d -t 1500 -v threadtime \
    > "$stage/logcat-events.txt" 2>&1 || true
  logcat -d -t 2500 -v threadtime -s \
    StorageRedirect:V SRX:V FileMonitorOp:I Stats:I AndroidRuntime:E DEBUG:F libc:F \
    ActivityManager:I WindowManager:I MediaProvider:V ExternalStorage:V DocumentsUI:V Vold:V \
    > "$stage/logcat-srx-filtered.txt" 2>&1 || true
}

collect_kernel_state() {
  dmesg 2>/dev/null | tail -n 1200 > "$stage/dmesg-tail.txt" 2>/dev/null || true
  dmesg 2>/dev/null |
    tail -n 1600 |
    grep -Ei 'srx|zygisk|fuse|sdcard|media|storage|vold|binder|oom|lowmem|killed process|avc|denied' \
    > "$stage/dmesg-filtered.txt" 2>/dev/null || true

  {
    echo "/proc/meminfo"
    cat /proc/meminfo 2>/dev/null
    echo
    echo "/proc/pressure/cpu"
    cat /proc/pressure/cpu 2>/dev/null
    echo
    echo "/proc/pressure/io"
    cat /proc/pressure/io 2>/dev/null
    echo
    echo "/proc/pressure/memory"
    cat /proc/pressure/memory 2>/dev/null
  } > "$STATE_DIR/kernel-pressure.txt" 2>&1

  {
    echo "/sys/kernel/debug/binder/stats"
    cat /sys/kernel/debug/binder/stats 2>/dev/null | head -n 600
    echo
    echo "/sys/kernel/debug/binder/state"
    cat /sys/kernel/debug/binder/state 2>/dev/null | head -n 1000
  } > "$STATE_DIR/binder.txt" 2>&1
}

copy_anr_files() {
  {
    echo "/data/anr"
    ls -la /data/anr 2>/dev/null || true
    echo
    ls -lt /data/anr 2>/dev/null || true
  } > "$ANR_DIR/list.txt" 2>&1

  ls -t /data/anr 2>/dev/null | head -n 5 | while IFS= read -r file_name; do
    src="/data/anr/$file_name"
    [ -f "$src" ] || continue
    dst="$ANR_DIR/$(safe_name "$file_name").head.txt"
    head -n 1200 "$src" > "$dst" 2>/dev/null || true
  done
}

copy_tombstone_files() {
  {
    echo "/data/tombstones"
    ls -la /data/tombstones 2>/dev/null || true
    echo
    ls -lt /data/tombstones 2>/dev/null || true
  } > "$TOMBSTONE_DIR/list.txt" 2>&1

  ls -t /data/tombstones 2>/dev/null | head -n 5 | while IFS= read -r file_name; do
    src="/data/tombstones/$file_name"
    [ -f "$src" ] || continue
    dst="$TOMBSTONE_DIR/$(safe_name "$file_name").head"
    head -c 131072 "$src" > "$dst" 2>/dev/null ||
      dd if="$src" of="$dst" bs=4096 count=32 2>/dev/null || true
  done
}

finalize_archive() {
  {
    echo "files:"
    (cd "$stage" && find . -maxdepth 5 -type f 2>/dev/null | sort)
  } > "$stage/manifest.txt" 2>&1

  if (cd "$stage" && tar -czf "$archive" *); then
    chmod 644 "$archive" 2>/dev/null || true
    rm -rf "$stage"
    return 0
  fi

  rc=$?
  rm -rf "$stage" "$archive"
  return "$rc"
}

stage="$1"
archive="$2"
PROGRESS_FILE="$3"
[ -n "$stage" ] || die "missing stage path"
[ -n "$archive" ] || die "missing archive path"
is_managed_tmp_path "$stage" || die "unsafe stage path"
is_managed_tmp_path "$archive" || die "unsafe archive path"
if [ -n "$PROGRESS_FILE" ]; then
  is_managed_tmp_path "$PROGRESS_FILE" || die "unsafe progress path"
fi

STATE_DIR="$stage/state"
PROC_DIR="$stage/proc"
ANR_DIR="$stage/anr"
TOMBSTONE_DIR="$stage/tombstones"

rm -rf "$stage" "$archive"
[ -z "$PROGRESS_FILE" ] || rm -f "$PROGRESS_FILE" "$PROGRESS_FILE.tmp" 2>/dev/null || true
update_progress 1 init "正在准备日志包"
mkdir -p "$STATE_DIR" "$PROC_DIR" "$ANR_DIR" "$TOMBSTONE_DIR" || die "failed to create stage"

update_progress 8 files "正在复制模块日志和配置"
collect_basic_files
update_progress 14 packages "正在分析相关应用"
collect_package_candidates
update_progress 22 device "正在采集设备和模块状态"
collect_device_state
update_progress 32 process "正在采集进程状态"
collect_process_state
update_progress 42 mounts "正在采集存储挂载状态"
collect_mount_state
update_progress 54 proc "正在采集相关进程细节"
collect_relevant_proc_state
update_progress 66 dumpsys "正在采集系统服务快照"
collect_dumpsys_state
update_progress 78 logcat "正在截取系统日志"
collect_logcat_state
update_progress 86 kernel "正在采集内核和压力状态"
collect_kernel_state
update_progress 91 anr "正在截取 ANR 记录"
copy_anr_files
update_progress 94 tombstones "正在截取崩溃记录"
copy_tombstone_files

update_progress 97 archive "正在压缩日志包"
finalize_archive || die "failed to create archive"
update_progress 98 done "日志包已生成，正在准备写入目标文件"
