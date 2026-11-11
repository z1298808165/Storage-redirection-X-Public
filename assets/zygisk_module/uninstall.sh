#!/system/bin/sh

MODDIR="/data/adb/modules/storage.redirect.x"
MONITOR_COLLECTOR_PID_FILE="$MODDIR/logs/.monitor_collector.pid"
RUNNING_COLLECTOR_PID_FILE="$MODDIR/logs/.running_collector.pid"
MEDIA_STATE_COLLECTOR_PID_FILE="$MODDIR/logs/.media_state_collector.pid"
APP_STATUS_COLLECTOR_PID_FILE="$MODDIR/logs/.app_status_collector.pid"
APP_STATUS_SNAPSHOT_PID_FILE="$MODDIR/logs/.app_status_snapshot.pid"
STATS_COLLECTOR_PID_FILE="$MODDIR/logs/.stats_collector.pid"
CONFIG_EVENT_COLLECTOR_PID_FILE="$MODDIR/logs/.config_event_collector.pid"
PACKAGE_EVENT_COLLECTOR_PID_FILE="$MODDIR/logs/.package_event_collector.pid"
# 必须与 service.d/common.sh 的 SRX_COLLECTOR_TAG 一致：详细日志采集器给它的 logcat
# 打了这个 `exec -a` 标记（logcat 是真正的 ELF，能打；toybox 的多调用二进制不行）。
SRX_COLLECTOR_TAG="srx-debug-collector"

# 停止后台进程，第二个参数是信号名（缺省 TERM）。
#
# 与 service.d/common.sh 同款，同样是三个坑：
# 1. 不能用全局变量存目标 pid——递归调用会覆盖它，循环结束后那句 kill 打的是最后一个
#    子进程，父 shell 收不到任何信号；
# 2. 采集器子 shell 是 mksh，SIGTERM 在它阻塞于管道 `read` 时会被延后处理，所以采集器
#    一律用 KILL；
# 3. 递归优先读 `/proc/<pid>/task/<pid>/children`，该文件并非所有内核都提供（A14 雷电、
#    A17 AvdLite 都没有），缺失时退化为 `ps` 的 PPID 列——否则管道里的 inotifyd / awk
#    会留成孤儿。不要改用 `exec -a` 打标记：toybox 的 inotifyd 靠 argv[0] 分派 applet。
stop_background_process() {
  if [ -z "$1" ] || ! kill -0 "$1" 2>/dev/null; then
    return 0
  fi

  children_file="/proc/$1/task/$1/children"
  if [ -r "$children_file" ]; then
    child_pids=$(cat "$children_file" 2>/dev/null)
  else
    child_pids=$(/system/bin/ps -A -o PID,PPID 2>/dev/null | awk -v root="$1" '$2 == root { print $1 }')
  fi
  for child_pid in $child_pids; do
    stop_background_process "$child_pid" "${2:-TERM}"
  done
  case "${2:-TERM}" in
    KILL|9) kill -9 "$1" 2>/dev/null ;;
    *) kill "$1" 2>/dev/null ;;
  esac
}

stop_collector_by_pid_file() {
  pid_file="$1"
  if [ ! -f "$pid_file" ]; then
    return 0
  fi

  pid=$(cat "$pid_file" 2>/dev/null)
  stop_background_process "$pid" KILL
  rm -f "$pid_file"
}

# 子 shell 已死，没人会再重建管道；此时按标记收掉管道里剩下的 logcat（awk / 读循环会
# 因输入 EOF 自行退出）。inotifyd 不在这里处理：它由上面的 PPID 递归段收掉。
#
# 与 service.d/common.sh 同款：用一次 `ps` 取全表并做前缀匹配，不逐个读
# `/proc/<pid>/cmdline`（A14 模拟器上目标进程消失时读端会自旋不返回），也不子串匹配
# （`awk`/`ps` 自己的 argv 里也会出现该标记）。
reap_collector_processes() {
  for collector_tag in "$SRX_COLLECTOR_TAG"; do
    victims=$(/system/bin/ps -A -o PID,ARGS 2>/dev/null | awk -v tag="$collector_tag" '
      {
        args = $0
        sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", args)
        if (index(args, tag) == 1) print $1
      }')
    for victim in $victims; do
      kill -9 "$victim" 2>/dev/null
    done
  done
}

# 上一次开机留下的 pid 文件不可信：pid 号会被复用，文件里的数字可能已经属于一个毫无
# 关系的进程（A14 实测：拿它停止会 SIGKILL 掉无关进程）。`logs/.boot_ok` 记录的是最近
# 一次完成开机的 boot_id，只有与当前一致时才认为这些 pid 属于本 boot 内启动的采集器。
# 读不到当前 boot_id 时（极旧内核）保持原有行为，不因此跳过清理。
current_boot_id=""
if [ -r /proc/sys/kernel/random/boot_id ]; then
  current_boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
fi
recorded_boot_id=$(cat "$MODDIR/.boot_ok" 2>/dev/null)
pid_files_from_previous_boot=0
if [ -n "$current_boot_id" ] && [ "$current_boot_id" != "$recorded_boot_id" ]; then
  pid_files_from_previous_boot=1
  ui_print "-- warn: collector pid files belong to a previous boot; skip killing them"
fi

if [ "$pid_files_from_previous_boot" -eq 0 ]; then
  stop_collector_by_pid_file "$MONITOR_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$RUNNING_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$MEDIA_STATE_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$APP_STATUS_SNAPSHOT_PID_FILE"
  stop_collector_by_pid_file "$STATS_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$CONFIG_EVENT_COLLECTOR_PID_FILE"
  stop_collector_by_pid_file "$PACKAGE_EVENT_COLLECTOR_PID_FILE"
fi
reap_collector_processes

# 仅允许删除已知的模块私有路径，防止误删无关目录。
safe_remove_known_path() {
  target="$1"
  case "$target" in
    /data/local/tmp/storage.redirect.x_stats|\
    /data/local/tmp/storage.redirect.x|\
    /data/adb/storage.redirect.x)
      rm -rf "$target" 2>/dev/null
      ;;
    *)
      ui_print "-- warn: skip unknown path=$target"
      ;;
  esac
}

safe_remove_known_path /data/local/tmp/storage.redirect.x_stats
safe_remove_known_path /data/local/tmp/storage.redirect.x
# stats 持久目录在模块目录之外，模块管理器不会自动清理，需要显式删除。
safe_remove_known_path /data/adb/storage.redirect.x

ui_print "-- Storage Redirect X uninstalled"
ui_print "-- temporary files cleaned"
