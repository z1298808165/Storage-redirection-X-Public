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
  touch "$MODDIR/disable"
  exit 0
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
