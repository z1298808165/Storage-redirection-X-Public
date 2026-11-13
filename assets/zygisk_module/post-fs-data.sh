#!/system/bin/sh

MODDIR=${0%/*}

LOGS_DIR="$MODDIR/logs"
PACKAGE_EVENT_LOG_FILE="$LOGS_DIR/package_events.log"
RECENT_SOURCE_HINT_FILE="$LOGS_DIR/.recent_source_hint"
RECENT_PATH_CALLER_HINT_FILE="$LOGS_DIR/.recent_path_caller_hint"
PACKAGE_EVENT_RECEIVER_READY_FILE="$LOGS_DIR/.package_event_receiver_ready"
CONFIG_DIR="$MODDIR/config"
BOOT_PENDING_FILE="$MODDIR/.boot_pending"
BOOT_OK_FILE="$MODDIR/.boot_ok"
BOOT_FAIL_COUNT_FILE="$MODDIR/.boot_fail_count"
BOOT_MODULE_VERSION_FILE="$MODDIR/.boot_module_version"
LOGS_CTX="u:object_r:shell_data_file:s0"
RUNTIME_DISABLE_FILE="$MODDIR/.runtime_disabled"

mkdir -p "$LOGS_DIR"
chmod 755 "$LOGS_DIR"
touch "$PACKAGE_EVENT_LOG_FILE"
chmod 666 "$PACKAGE_EVENT_LOG_FILE" 2>/dev/null
touch "$RECENT_SOURCE_HINT_FILE" "$RECENT_PATH_CALLER_HINT_FILE"
chmod 666 "$RECENT_SOURCE_HINT_FILE" "$RECENT_PATH_CALLER_HINT_FILE" 2>/dev/null
: > "$PACKAGE_EVENT_RECEIVER_READY_FILE"
chmod 666 "$PACKAGE_EVENT_RECEIVER_READY_FILE" 2>/dev/null
mkdir -p "$CONFIG_DIR"
mkdir -p "$CONFIG_DIR/apps"
chmod 755 "$CONFIG_DIR" "$CONFIG_DIR/apps" 2>/dev/null
find "$CONFIG_DIR" -type f -name '*.json' -exec chmod 644 {} \; 2>/dev/null
# stats 持久目录位于模块目录之外，模块升级不会删除它。
# daemon 写入时会自行 create_dir_all，这里提前创建只是保险。
mkdir -p /data/adb/storage.redirect.x
chmod 700 /data/adb/storage.redirect.x 2>/dev/null

# 运行时停用与模块管理器停用必须成对生效。旧版本只写入
# .runtime_disabled；用户在 KernelSU/Magisk 中重新启用模块后，删除 disable
# 即表示用户选择恢复运行，此时清理旧的运行时停用标记。
if [ -f "$RUNTIME_DISABLE_FILE" ] && [ ! -f "$MODDIR/disable" ]; then
  rm -f "$RUNTIME_DISABLE_FILE"
  printf '{"runtime_disabled":false}\n' > "$CONFIG_DIR/runtime_state.json"
  chmod 644 "$CONFIG_DIR/runtime_state.json" 2>/dev/null
fi

module_version=$(sed -n 's/^versionCode=//p; s/^version=//p' "$MODDIR/module.prop" 2>/dev/null | tr '\n' ' ')
if [ -n "$module_version" ]; then
  printf '%s\n' "$module_version" > "$BOOT_MODULE_VERSION_FILE"
  chmod 644 "$BOOT_MODULE_VERSION_FILE" 2>/dev/null
fi

if [ -f "$RUNTIME_DISABLE_FILE" ]; then
  exit 0
fi

boot_id=""
if [ -r /proc/sys/kernel/random/boot_id ]; then
  boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
fi

last_pending=""
if [ -f "$BOOT_PENDING_FILE" ]; then
  last_pending=$(cat "$BOOT_PENDING_FILE" 2>/dev/null)
fi

last_ok=""
if [ -f "$BOOT_OK_FILE" ]; then
  last_ok=$(cat "$BOOT_OK_FILE" 2>/dev/null)
fi

if [ -n "$last_pending" ] && [ "$last_pending" != "$boot_id" ] && [ "$last_pending" != "$last_ok" ]; then
  # 连续两次未完成启动才停用模块。
  #
  # 单次失败不一定是模块的问题：开机阶段上游服务未就绪、系统更新后首次启动更慢、
  # 用户中途重启，都会让上一轮 `.boot_pending` 停在那里而 `.boot_ok` 没写。原来的
  # 「一次就写 disable」会把这类偶发当成故障，把本来可用的设备永久停用，而且用户
  # 只能手动去管理器里重新启用。
  # 来源：上游 PR Kindness-Kismet/Storage-redirection-X-Public#66（贡献者 People-11）。
  fail_count=0
  if [ -f "$BOOT_FAIL_COUNT_FILE" ]; then
    fail_count=$(cat "$BOOT_FAIL_COUNT_FILE" 2>/dev/null || echo 0)
  fi
  case "$fail_count" in
    '' | *[!0-9]*) fail_count=0 ;;
  esac
  if [ "$fail_count" -ge 1 ]; then
    touch "$MODDIR/disable"
    rm -f "$BOOT_PENDING_FILE" "$BOOT_FAIL_COUNT_FILE"
    exit 0
  fi
  echo "$((fail_count + 1))" > "$BOOT_FAIL_COUNT_FILE"
  chmod 644 "$BOOT_FAIL_COUNT_FILE" 2>/dev/null
fi

if [ -n "$boot_id" ]; then
  echo "$boot_id" > "$BOOT_PENDING_FILE"
else
  echo "unknown" > "$BOOT_PENDING_FILE"
fi
rm -f "$BOOT_OK_FILE"

# 清理历史启动 marker，避免 logs 目录长期累积。
if [ -d "$LOGS_DIR" ]; then
  for marker_path in "$LOGS_DIR"/boot_*.marker; do
    [ -e "$marker_path" ] || break
    marker_name="${marker_path##*/}"
    if [ -n "$boot_id" ] && [ "$marker_name" = "boot_${boot_id}.marker" ]; then
      continue
    fi
    rm -f "$marker_path"
  done
fi

if command -v chcon >/dev/null 2>&1; then
  chcon -R "$LOGS_CTX" "$LOGS_DIR" 2>/dev/null
  chcon -R u:object_r:shell_data_file:s0 "$CONFIG_DIR" 2>/dev/null
fi

# system_server 经保留的模块目录 FD 创建查询端点；目录仅向 system_server 与 root 开放。
mkdir -p "$MODDIR/tmp/package_query"
chown 1000:1000 "$MODDIR/tmp/package_query"
chmod 700 "$MODDIR/tmp/package_query"
chcon u:object_r:shell_data_file:s0 "$MODDIR/tmp/package_query" 2>/dev/null

# 路径映射字段重命名迁移：current_path/target_path → request_path/final_path
MAPPING_MIGRATION_MARKER="$MODDIR/.mapping_fields_v1"
APPS_DIR="$CONFIG_DIR/apps"
if [ ! -f "$MAPPING_MIGRATION_MARKER" ] && [ -d "$APPS_DIR" ]; then
  for cfg in "$APPS_DIR"/*.json; do
    [ -f "$cfg" ] || continue
    if grep -q '"current_path"\|"target_path"' "$cfg" 2>/dev/null; then
      tmp_cfg="${cfg}.tmp"
      sed -e 's/"current_path"/"request_path"/g' \
          -e 's/"target_path"/"final_path"/g' \
          "$cfg" > "$tmp_cfg" && mv "$tmp_cfg" "$cfg"
    fi
  done
  touch "$MAPPING_MIGRATION_MARKER"
fi
