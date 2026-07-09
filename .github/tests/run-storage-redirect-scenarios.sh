#!/usr/bin/env bash
set -euo pipefail

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

APP_ID="${APP_ID:-me.fakerqu.test.storageredirect}"
CONFIG="/data/adb/modules/storage.redirect.x/config/apps/${APP_ID}.json"
GLOBAL_CONFIG="/data/adb/modules/storage.redirect.x/config/global.json"
LOG_PATH="/data/adb/modules/storage.redirect.x/logs/running.log"
FILE_MONITOR_LOG_PATH="/data/adb/modules/storage.redirect.x/logs/file_monitor.log"
ACTION="me.fakerqu.test.storageredirection.TEST_CASE"
RESULT_DIR="/sdcard/Android/data/${APP_ID}/files/test_case_result"
INTERNAL_RESULT_DIR="/data/data/${APP_ID}/files/test_case_result"
REAL_ROOT="/storage/emulated/0"
BACKEND_ROOT="/data/media/0"
BACKEND_PRIVATE_ROOT="${BACKEND_ROOT}/Android/data/${APP_ID}/sdcard"
PRIVATE_ROOT="${BACKEND_PRIVATE_ROOT}"
BACKEND_RESULT_DIR="${BACKEND_ROOT}/Android/data/${APP_ID}/files/test_case_result"
SANDBOX_RESULT_DIR="${BACKEND_PRIVATE_ROOT}/Android/data/${APP_ID}/files/test_case_result"
TEST_FILE="srt_ci_probe.txt"
HOT_BEFORE_FILE="srt_hot_before.txt"
HOT_AFTER_FILE="srt_hot_after.txt"
READ_ONLY_FILE="srt_read_only_seed.txt"
ALLOW_KEEP_FILE="keep.txt"
ALLOW_PART_FILE="srt_ci_probe.part"
QMARK_SINGLE_FILE="srt_qmark_a.txt"
QMARK_DOUBLE_FILE="srt_qmark_ab.txt"
QMARK_FILE_SINGLE_FILE="srt_qmark_file_a.txt"
MOUNT_NS_STAR_MEDIA_FILE="srt_mountns_star_media.bin"
MOUNT_NS_QMARK_MEDIA_FILE="srt_mountns_qmark_media.bin"
FUSE_STAR_MEDIA_FILE="srt_fuse_star_media.bin"
FUSE_STAR_MISS_MEDIA_FILE="srt_fuse_star_miss_media.bin"
FUSE_QMARK_MEDIA_FILE="srt_fuse_qmark_media.bin"
FUSE_QMARK_MISS_MEDIA_FILE="srt_fuse_qmark_miss_media.bin"
FUSE_DCIM_MEDIA_FILE="srt_fuse_dcim_media.jpg"
READ_ONLY_HARDLINK="hardlink.txt"
READ_ONLY_SYMLINK="symlink.txt"
READ_ONLY_IMAGE_FILE="srt_read_only_media.jpg"
PAYLOAD="storage-redirect-test:file:ci"
READ_ONLY_PAYLOAD="storage-redirect-test:file:readonly"
READ_ONLY_IMAGE_B64="/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYxLjE5LjEwMAD/2wBDAAgEBAQEBAUFBQUFBQYGBgYGBgYGBgYGBgYHBwcICAgHBwcGBgcHCAgICAkJCQgICAgJCQoKCgwMCwsODg4RERT/xABLAAEBAAAAAAAAAAAAAAAAAAAACAEBAAAAAAAAAAAAAAAAAAAAABABAAAAAAAAAAAAAAAAAAAAABEBAAAAAAAAAAAAAAAAAAAAAP/AABEIAAIAAgMBIgACEQADEQD/2gAMAwEAAhEDEQA/AJ/AB//Z"

READ_ONLY_ROOT="${REAL_ROOT}/Download/SrtReadOnly"
BACKEND_READ_ONLY_ROOT="${BACKEND_ROOT}/Download/SrtReadOnly"
READ_ONLY_MEDIA_ROOT="${REAL_ROOT}/Pictures/SrtReadOnlyMedia"
PRIVATE_READ_ONLY_MEDIA_ROOT="${PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia"
MAPPED_READ_ONLY_REQUEST="${REAL_ROOT}/Download/SrtMapRO"
MAPPED_READ_ONLY_TARGET="${REAL_ROOT}/Pictures/SrtLocked"
ALLOW_ROOT="${REAL_ROOT}/Download/SrtAllow"
PRIVATE_ALLOW_ROOT="${PRIVATE_ROOT}/Download/SrtAllow"
LEGACY_ROOT="${REAL_ROOT}/Download/SrtLegacy"
PRIVATE_LEGACY_ROOT="${PRIVATE_ROOT}/Download/SrtLegacy"
QMARK_ROOT="${REAL_ROOT}/Download/SrtQMark"
PRIVATE_QMARK_ROOT="${PRIVATE_ROOT}/Download/SrtQMark"
FUSE_PLAIN_ROOT="${REAL_ROOT}/Download/SrtFusePlain"
PRIVATE_FUSE_PLAIN_ROOT="${PRIVATE_ROOT}/Download/SrtFusePlain"
FUSE_DCIM_ROOT="${REAL_ROOT}/DCIM/SrtFuseQQ"
PRIVATE_FUSE_DCIM_ROOT="${PRIVATE_ROOT}/DCIM/SrtFuseQQ"
FUSE_DCIM_ALLOWED_ROOT="${FUSE_DCIM_ROOT}/SrtAllowedAlpha"
PRIVATE_FUSE_DCIM_ALLOWED_ROOT="${PRIVATE_FUSE_DCIM_ROOT}/SrtAllowedAlpha"
FUSE_DCIM_OTHER_ROOT="${FUSE_DCIM_ROOT}/SrtOther"
PRIVATE_FUSE_DCIM_OTHER_ROOT="${PRIVATE_FUSE_DCIM_ROOT}/SrtOther"
FUSE_QMARK_ROOT="${REAL_ROOT}/Download/SrtFuseQa"
PRIVATE_FUSE_QMARK_ROOT="${PRIVATE_ROOT}/Download/SrtFuseQa"
FUSE_QMARK_MISS_ROOT="${REAL_ROOT}/Download/SrtFuseQab"
PRIVATE_FUSE_QMARK_MISS_ROOT="${PRIVATE_ROOT}/Download/SrtFuseQab"
FUSE_QMARK_MEDIA_ROOT="${REAL_ROOT}/Download/SrtFuseQb"
PRIVATE_FUSE_QMARK_MEDIA_ROOT="${PRIVATE_ROOT}/Download/SrtFuseQb"
FUSE_STAR_MEDIA_ROOT="${REAL_ROOT}/Download/SrtFuseMediaAlpha"
PRIVATE_FUSE_STAR_MEDIA_ROOT="${PRIVATE_ROOT}/Download/SrtFuseMediaAlpha"
FUSE_EXCLUDE_ROOT="${REAL_ROOT}/Download/SrtFuseExclude"
PRIVATE_FUSE_EXCLUDE_ROOT="${PRIVATE_ROOT}/Download/SrtFuseExclude"
FUSE_MAP_PARENT="${REAL_ROOT}/Download/SrtFuseMapParent"
FUSE_MAP_RW_REQUEST="${REAL_ROOT}/Download/SrtFuseMapRW"
FUSE_MAP_RO_REQUEST="${REAL_ROOT}/Download/SrtFuseMapRO"
FUSE_MAP_RW_TARGET="${FUSE_MAP_PARENT}/WritableTarget"
FUSE_MAP_RO_TARGET="${FUSE_MAP_PARENT}/LockedTarget"
FUSE_MULTI_ROOT="${REAL_ROOT}/Download/SrtFuseMulti"
PRIVATE_FUSE_MULTI_ROOT="${PRIVATE_ROOT}/Download/SrtFuseMulti"
MOUNT_NS_ALLOW_ROOT="${REAL_ROOT}/Download/SrtMountNsAllow"
PRIVATE_MOUNT_NS_ALLOW_ROOT="${PRIVATE_ROOT}/Download/SrtMountNsAllow"
MOUNT_NS_READ_ONLY_ROOT="${REAL_ROOT}/Download/SrtMountNsReadOnly"
PRIVATE_MOUNT_NS_READ_ONLY_ROOT="${PRIVATE_ROOT}/Download/SrtMountNsReadOnly"
MOUNT_NS_MAP_PARENT="${REAL_ROOT}/Download/SrtMountNsMapParent"
MOUNT_NS_MAP_RW_REQUEST="${REAL_ROOT}/Download/SrtMountNsMapRW"
MOUNT_NS_MAP_RO_REQUEST="${REAL_ROOT}/Download/SrtMountNsMapRO"
MOUNT_NS_MAP_RW_TARGET="${MOUNT_NS_MAP_PARENT}/WritableTarget"
MOUNT_NS_MAP_RO_TARGET="${MOUNT_NS_MAP_PARENT}/LockedTarget"
MONITOR_BASE_ROOT="${REAL_ROOT}/Download/SrtMonitor"
PRIVATE_MONITOR_BASE_ROOT="${PRIVATE_ROOT}/Download/SrtMonitor"
MONITOR_MAP_REQUEST="${REAL_ROOT}/Download/SrtMonitorMap"
MONITOR_MAP_TARGET="${REAL_ROOT}/Download/SrtMonitorMapped"
MONITOR_LOCKED_ROOT="${REAL_ROOT}/Download/SrtMonitorLocked"
MONITOR_WRITABLE_ROOT="${REAL_ROOT}/Download/SrtMonitorLocked/Writable"
PRIVATE_MONITOR_WRITABLE_ROOT="${PRIVATE_ROOT}/Download/SrtMonitorLocked/Writable"
MONITOR_RELATIVE_DATA_ROOT="${REAL_ROOT}/Pictures/SrtRelativeData"
PRIVATE_MONITOR_RELATIVE_DATA_ROOT="${PRIVATE_ROOT}/Pictures/SrtRelativeData"
MONITOR_NNNGRAM_ROOT="${REAL_ROOT}/Pictures/Nnngram"
PRIVATE_MONITOR_NNNGRAM_ROOT="${PRIVATE_ROOT}/Pictures/Nnngram"
SRT_FRESH_APP_PER_CASE="${SRT_FRESH_APP_PER_CASE:-1}"
SRT_RESULT_POLL_MS="${SRT_RESULT_POLL_MS:-150}"
SRT_APP_LAUNCH_SETTLE_MS="${SRT_APP_LAUNCH_SETTLE_MS:-800}"
SRT_MOUNT_CONFIRM_TIMEOUT_MS="${SRT_MOUNT_CONFIRM_TIMEOUT_MS:-15000}"
SRT_APP_MOUNT_CONFIRM_RETRIES="${SRT_APP_MOUNT_CONFIRM_RETRIES:-3}"
SRT_CONFIG_APPLY_TIMEOUT_MS="${SRT_CONFIG_APPLY_TIMEOUT_MS:-30000}"
SRT_SERVICE_CASE_SETTLE_MS="${SRT_SERVICE_CASE_SETTLE_MS:-50}"
SRT_FILE_MONITOR_ENABLED="${SRT_FILE_MONITOR_ENABLED:-0}"
SRT_FAIL_FAST="${SRT_FAIL_FAST:-0}"
SRT_SCENARIO_TIMEOUT_SECONDS="${SRT_SCENARIO_TIMEOUT_SECONDS:-600}"
LAST_MOUNT_CONFIRMED_PID=""
ADB_ROOT_MODE="${ADB_ROOT_MODE:-}"

detect_adb_root_mode() {
  if [ -n "$ADB_ROOT_MODE" ]; then
    return 0
  fi
  if adb shell "su 0 sh -c 'id'" >/dev/null 2>&1; then
    ADB_ROOT_MODE="su0"
  elif adb shell "su -c 'id'" >/dev/null 2>&1; then
    ADB_ROOT_MODE="su"
  elif adb shell magisk su -c id >/dev/null 2>&1; then
    ADB_ROOT_MODE="magisk"
  elif adb shell /system/bin/magisk su -c id >/dev/null 2>&1; then
    ADB_ROOT_MODE="system_magisk"
  elif adb shell /debug_ramdisk/magisk su -c id >/dev/null 2>&1; then
    ADB_ROOT_MODE="debug_magisk"
  else
    echo "No usable adb root shell found." >&2
    return 1
  fi
  export ADB_ROOT_MODE
}

test_app_uid() {
  adb shell "cmd package list packages -U '$APP_ID' 2>/dev/null | sed -n 's/.* uid://p' | head -1" | tr -d '\r'
}

fix_private_backend_permissions() {
  local uid
  uid="$(test_app_uid)"
  if [ -z "$uid" ]; then
    echo "private_backend_permission_fix_skipped: app uid not found for $APP_ID" >&2
    return 1
  fi
  adb_su "app_uid='$uid'; app_root='${BACKEND_ROOT}/Android/data/${APP_ID}'; sandbox='${BACKEND_PRIVATE_ROOT}'; mkdir -p \"\$sandbox\"; chown -R \"\$app_uid\":1023 \"\$app_root\" 2>/dev/null || true; find \"\$app_root\" -type d -exec chmod 2771 {} + 2>/dev/null || true; find \"\$app_root\" -type f -exec chmod 0664 {} + 2>/dev/null || true" >/dev/null
}

adb_root() {
  local command="PATH=/debug_ramdisk:/sbin:/data/adb/magisk:\$PATH; $1"
  local encoded runner
  detect_adb_root_mode
  encoded="$(printf '%s' "$command" | base64 | tr -d '\n')"
  runner="printf '%s' '$encoded' | base64 -d | sh"
  case "$ADB_ROOT_MODE" in
    su0) adb shell "su 0 sh -c \"$runner\"" ;;
    su) adb shell "su -c \"$runner\"" ;;
    magisk) adb shell magisk su -c "$runner" ;;
    system_magisk) adb shell /system/bin/magisk su -c "$runner" ;;
    debug_magisk) adb shell /debug_ramdisk/magisk su -c "$runner" ;;
    *) echo "No usable adb root shell found." >&2; return 1 ;;
  esac
}

adb_su() {
  adb_root "$1" | tr -d '\r'
}

adb_write_file() {
  local path="$1"
  local content="$2"
  local encoded
  encoded="$(printf '%s' "$content" | base64 | tr -d '\n')"
  adb_root "printf '%s' '$encoded' | base64 -d > '$path'"
}

wait_boot_completed() {
  adb wait-for-device
  adb shell 'while [ "$(getprop sys.boot_completed)" != "1" ]; do sleep 2; done'
}

write_config() {
  local content="$1"
  adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps" >/dev/null
  adb_write_file "$CONFIG" "$content" >/dev/null
}

write_global_config() {
  local content="$1"
  adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config" >/dev/null
  adb_write_file "$GLOBAL_CONFIG" "$content" >/dev/null
}

test_global_config() {
  local fuse_daemon_enabled="$1"
  local file_monitor_enabled="${2:-$SRT_FILE_MONITOR_ENABLED}"
  case "$file_monitor_enabled" in
    1|true|TRUE|yes|YES) file_monitor_enabled=true ;;
    *) file_monitor_enabled=false ;;
  esac
  printf '{"file_monitor_enabled":%s,"fuse_fix_enabled":true,"fuse_daemon_redirect_enabled":%s,"verbose_logging_enabled":true,"auto_enable_redirect_for_new_apps":true,"auto_enable_new_apps_template_id":"","app_config_auto_save":true}' "$file_monitor_enabled" "$fuse_daemon_enabled"
}

enable_fuse_daemon_config() {
  write_global_config "$(test_global_config true)"
}

disable_fuse_daemon_config() {
  write_global_config "$(test_global_config false)"
}

use_mount_namespace_fallback_config() {
  write_global_config "$(test_global_config false)"
}

apply_config() {
  disable_fuse_daemon_config
  case "$1" in
    1)
      adb_su "rm -f '$CONFIG'" >/dev/null
      ;;
    2)
      write_config '{"users":{"0":{"enabled":true}}}'
      ;;
    3)
      write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}'
      ;;
    4)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download"],"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}'
      ;;
    5)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download"]}}}'
      ;;
    6)
      write_config '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/SrtOther":"Download/SrtOtherMapped"}}}}'
      ;;
    7)
      write_config '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/SrtProbe":"Download/SrtMapOnlyMapped"}}}}'
      ;;
    8)
      write_config '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":[".xlDownload"]}}}'
      ;;
    9)
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtReadOnly"]}}}'
      ;;
    10)
      write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtMapRO":"Pictures/SrtLocked"},"read_only_paths":["Pictures/SrtLocked"]}}}'
      ;;
    11)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtAllow","!Download/SrtAllow/tmp","Download","!Download/*.part"]}}}'
      ;;
    12)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtLegacy"],"excluded_real_paths":["Download/SrtLegacy/tmp"]}}}'
      ;;
    13)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/srt_qmark_?.txt","Download/srt_qmark_file_?.txt"]}}}'
      ;;
    14)
      write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtLongest":"Download/SrtLongestBase","Download/SrtLongest/Deep":"Download/SrtLongestDeep"}}}}'
      ;;
    15)
      write_config '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":"Download/SrtPriority","path_mappings":{"Download/SrtPriority":"Download/SrtPriorityMapped"}}}}'
      ;;
    16)
      enable_fuse_daemon_config
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFusePlain","DCIM/SrtFuseQQ/SrtAllowed*","Download/SrtFuseQ?/Media","Download/SrtFuseMedia*/Drop"]}}}'
      ;;
    17)
      enable_fuse_daemon_config
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseExclude/Writable"],"read_only_paths":["Download/SrtFuseExclude","!Download/SrtFuseExclude/Writable"]}}}'
      ;;
    18)
      enable_fuse_daemon_config
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtFuseMapParent","!Download/SrtFuseMapParent/WritableTarget"],"path_mappings":{"Download/SrtFuseMapRW":"Download/SrtFuseMapParent/WritableTarget","Download/SrtFuseMapRO":"Download/SrtFuseMapParent/LockedTarget"}}}}'
      ;;
    19)
      enable_fuse_daemon_config
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseMulti/QQ/*","Download/SrtFuseMulti/WeChat/*"],"read_only_paths":["Download/SrtFuseMulti/Locked/*"]}}}'
      ;;
    20)
      use_mount_namespace_fallback_config
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMountNsAllow/Team*/Deep","Download/SrtMountNsAllow/Q?/Deep"]}}}'
      ;;
    21)
      use_mount_namespace_fallback_config
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsReadOnly/Team*/Deep"]}}}'
      ;;
    22)
      use_mount_namespace_fallback_config
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsMapParent","!Download/SrtMountNsMapParent/WritableTarget"],"path_mappings":{"Download/SrtMountNsMapRW":"Download/SrtMountNsMapParent/WritableTarget","Download/SrtMountNsMapRO":"Download/SrtMountNsMapParent/LockedTarget"}}}}'
      ;;
    23)
      write_global_config "$(test_global_config false true)"
      write_config '{"users":{"0":{"enabled":false}}}'
      ;;
    24)
      write_global_config "$(test_global_config false true)"
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
      ;;
    25)
      write_global_config "$(test_global_config true true)"
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
      ;;
    26)
      write_global_config "$(test_global_config false true)"
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
      ;;
    27)
      write_global_config "$(test_global_config true true)"
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
      ;;
    28)
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Pictures/SrtReadOnlyMedia"]}}}'
      ;;
    29)
      write_config '{"users":{"0":{"enabled":true}}}'
      ;;
    *)
      echo "unknown scenario: $1" >&2
      return 1
      ;;
  esac
}

target_path() {
  case "$1" in
    8) echo "${REAL_ROOT}/.xldownload/${TEST_FILE}" ;;
    14) echo "${REAL_ROOT}/Download/SrtLongest/Deep/${TEST_FILE}" ;;
    15) echo "${REAL_ROOT}/Download/SrtPriority/${TEST_FILE}" ;;
    *) echo "${REAL_ROOT}/Download/SrtProbe/${TEST_FILE}" ;;
  esac
}

logical_dir() {
  case "$1" in
    8) echo "${REAL_ROOT}/.xldownload" ;;
    14) echo "${REAL_ROOT}/Download/SrtLongest/Deep" ;;
    15) echo "${REAL_ROOT}/Download/SrtPriority" ;;
    *) echo "${REAL_ROOT}/Download/SrtProbe" ;;
  esac
}

expected_path() {
  case "$1" in
    1|5|6) echo "${REAL_ROOT}/Download/SrtProbe/${TEST_FILE}" ;;
    2) echo "${PRIVATE_ROOT}/Download/SrtProbe/${TEST_FILE}" ;;
    3|4) echo "${REAL_ROOT}/Download/Test/${TEST_FILE}" ;;
    7) echo "${REAL_ROOT}/Download/SrtMapOnlyMapped/${TEST_FILE}" ;;
    8) echo "${PRIVATE_ROOT}/.xldownload/${TEST_FILE}" ;;
    14) echo "${REAL_ROOT}/Download/SrtLongestDeep/${TEST_FILE}" ;;
    15) echo "${REAL_ROOT}/Download/SrtPriorityMapped/${TEST_FILE}" ;;
    *) return 1 ;;
  esac
}

scenario_title() {
  case "$1" in
    1) echo "未启用应用配置，验证默认真实路径写入" ;;
    2) echo "启用重定向，验证写入应用私有空间" ;;
    3) echo "启用路径映射，验证 SrtProbe 写入真实 Test" ;;
    4) echo "路径映射叠加真实路径放行，验证映射优先级" ;;
    5) echo "放行真实 Download，验证保持原路径写入" ;;
    6) echo "仅映射模式，未命中映射路径应保持真实路径写入" ;;
    7) echo "仅映射模式，命中映射路径应写入映射目标" ;;
    8) echo "仅映射模式叠加 sandboxed_paths，验证 .xlDownload 别名沙盒化" ;;
    9) echo "read_only_paths 允许读取但拒绝写入、删除、mkdir、rename" ;;
    10) echo "映射目标为只读路径时，映射请求写入应被拒绝" ;;
    11) echo "allowed_real_paths 内联排除与通配符排除规则" ;;
    12) echo "excluded_real_paths 旧字段兼容并入排除规则" ;;
    13) echo "allowed_real_paths 问号通配符规则" ;;
    14) echo "path_mappings 最长前缀匹配规则" ;;
    15) echo "映射优先于字符串形式 sandboxed_paths" ;;
    16) echo "Fuse daemon 混合模式：普通放行与通配符放行并存" ;;
    17) echo "Fuse daemon 混合模式：read_only_paths 支持 ! 排除优先" ;;
    18) echo "Fuse daemon 混合模式：映射最终目标决定只读权限" ;;
    19) echo "Fuse daemon 混合模式：同父级多通配符规则互不污染" ;;
    20) echo "默认 mount namespace：allowed_real_paths 通配符回退" ;;
    21) echo "默认 mount namespace：read_only_paths 通配符回退" ;;
    22) echo "默认 mount namespace：映射最终目标决定只读权限" ;;
    23) echo "文件监视：未启用重定向的普通应用与系统代写保存成功记录" ;;
    24) echo "文件监视：普通应用 fuse daemon 关闭时映射保存、只读失败与只读排除成功记录" ;;
    25) echo "文件监视：普通应用 fuse daemon 开启时保存成功、只读失败与只读排除成功记录" ;;
    26) echo "文件监视：系统代写 fuse daemon 关闭时保存成功、只读失败与只读排除成功记录" ;;
    27) echo "文件监视：系统代写 fuse daemon 开启时保存成功、只读失败与只读排除成功记录" ;;
    28) echo "MediaStore 查询：只读真实图片路径应对应用可见" ;;
    29) echo "配置热更新：运行中应用无需重启即可从默认重定向切换到路径映射" ;;
  esac
}

clean_targets() {
  sleep_ms $SRT_SERVICE_CASE_SETTLE_MS
  clean_results
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/SrtOther' '${REAL_ROOT}/Download/SrtOtherMapped' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/SrtOther' '${BACKEND_PRIVATE_ROOT}/Download/SrtOtherMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia'; find '${REAL_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' -maxdepth 1 -name '$TEST_FILE' -delete 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/Test/$HOT_BEFORE_FILE' '${REAL_ROOT}/Download/Test/$HOT_AFTER_FILE' '${BACKEND_PRIVATE_ROOT}/Download/Test/$HOT_BEFORE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/Test/$HOT_AFTER_FILE' 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$ALLOW_PART_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$ALLOW_PART_FILE' '${REAL_ROOT}/Download/$QMARK_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_SINGLE_FILE' '${REAL_ROOT}/Download/$QMARK_DOUBLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_DOUBLE_FILE'" >/dev/null
  adb_su "mkdir -p '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow/tmp' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow/tmp' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload'; chmod -R 777 '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia' 2>/dev/null || true; chmod 777 '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'; mkdir -p '${REAL_ROOT}/Download/SrtLegacy/tmp' '${REAL_ROOT}/Download/SrtQMark/Keep1' '${REAL_ROOT}/Download/SrtQMark/Keep12' '${REAL_ROOT}/Download/SrtLongest/Deep' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy/tmp' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark/Keep1' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark/Keep12' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'; chmod -R 777 '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped' 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$QMARK_FILE_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_FILE_SINGLE_FILE'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude' '${REAL_ROOT}/Download/SrtFuseMapParent' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti' '${REAL_ROOT}/DCIM/SrtFuseQQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti' '${BACKEND_PRIVATE_ROOT}/DCIM/SrtFuseQQ'; mkdir -p '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude/Locked' '${REAL_ROOT}/Download/SrtFuseExclude/Writable' '${REAL_ROOT}/Download/SrtFuseMapParent/WritableTarget' '${REAL_ROOT}/Download/SrtFuseMapParent/LockedTarget' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti/QQ' '${REAL_ROOT}/Download/SrtFuseMulti/WeChat' '${REAL_ROOT}/Download/SrtFuseMulti/Locked' '${REAL_ROOT}/Download/SrtFuseMulti/Other' '${FUSE_DCIM_ALLOWED_ROOT}' '${FUSE_DCIM_OTHER_ROOT}' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude/Locked' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude/Writable' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent/WritableTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent/LockedTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/QQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/WeChat' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/Locked' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/Other' '${PRIVATE_FUSE_DCIM_ALLOWED_ROOT}' '${PRIVATE_FUSE_DCIM_OTHER_ROOT}'; chmod -R 777 '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude' '${REAL_ROOT}/Download/SrtFuseMapParent' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti' '${REAL_ROOT}/DCIM/SrtFuseQQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti' '${BACKEND_PRIVATE_ROOT}/DCIM/SrtFuseQQ' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFuseQa' '${REAL_ROOT}/Download/SrtFuseQab' '${REAL_ROOT}/Download/SrtFuseQb' '${REAL_ROOT}/Download/SrtFuseMediaAlpha' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha'; mkdir -p '${REAL_ROOT}/Download/SrtFuseQa/Media' '${REAL_ROOT}/Download/SrtFuseQab/Media' '${REAL_ROOT}/Download/SrtFuseQb/Media' '${REAL_ROOT}/Download/SrtFuseMediaAlpha/Drop' '${REAL_ROOT}/Download/SrtFuseMediaAlpha/Other' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha/Drop' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha/Other'; chmod -R 777 '${REAL_ROOT}/Download/SrtFuseQa' '${REAL_ROOT}/Download/SrtFuseQab' '${REAL_ROOT}/Download/SrtFuseQb' '${REAL_ROOT}/Download/SrtFuseMediaAlpha' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO'; mkdir -p '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent/WritableTarget' '${REAL_ROOT}/Download/SrtMountNsMapParent/LockedTarget' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent/WritableTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent/LockedTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO'; chmod -R 777 '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO' 2>/dev/null || true" >/dev/null
  adb_su "mkdir -p '${REAL_ROOT}/Download/SrtMountNsAllow/TeamAlpha/Deep' '${REAL_ROOT}/Download/SrtMountNsAllow/Qa/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow/TeamAlpha/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow/Qa/Deep'; chmod -R 777 '${REAL_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram'; mkdir -p '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked/Writable' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked/Writable' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram'; chmod -R 777 '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram' 2>/dev/null || true" >/dev/null
  fix_private_backend_permissions
}

clean_results() {
  adb_su "rm -rf '$RESULT_DIR' '$INTERNAL_RESULT_DIR' '$BACKEND_RESULT_DIR' '$SANDBOX_RESULT_DIR'; find '$BACKEND_ROOT/Android/data/$APP_ID' '/data/data/$APP_ID' -path '*/files/test_case_result' -type d -prune -exec rm -rf {} + 2>/dev/null || true" >/dev/null
}

backup_global_config() {
  global_config_backup_ready=0
  if adb_su "test -f '$GLOBAL_CONFIG'" >/dev/null 2>&1; then
    original_global_config_exists=1
    original_global_config_b64="$(adb_su "base64 '$GLOBAL_CONFIG' 2>/dev/null | tr -d '\n'")"
  else
    original_global_config_exists=0
    original_global_config_b64=""
  fi
  global_config_backup_ready=1
}

restore_global_config() {
  if [ "${global_config_backup_ready:-0}" -ne 1 ]; then
    return 0
  fi
  if [ "${original_global_config_exists:-0}" -eq 1 ] && [ -n "${original_global_config_b64:-}" ]; then
    adb_root "printf '%s' '$original_global_config_b64' | base64 -d > '$GLOBAL_CONFIG'" >/dev/null 2>&1 || true
    adb_su "chmod 644 '$GLOBAL_CONFIG' 2>/dev/null || true" >/dev/null 2>&1 || true
  else
    adb_su "rm -f '$GLOBAL_CONFIG'" >/dev/null 2>&1 || true
  fi
}

backup_app_config() {
  app_config_backup_ready=0
  if adb_su "test -f '$CONFIG'" >/dev/null 2>&1; then
    original_app_config_exists=1
    original_app_config_b64="$(adb_su "base64 '$CONFIG' 2>/dev/null | tr -d '\n'")"
  else
    original_app_config_exists=0
    original_app_config_b64=""
  fi
  app_config_backup_ready=1
}

restore_app_config() {
  if [ "${app_config_backup_ready:-0}" -ne 1 ]; then
    return 0
  fi
  if [ "${original_app_config_exists:-0}" -eq 1 ] && [ -n "${original_app_config_b64:-}" ]; then
    adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps" >/dev/null 2>&1 || true
    adb_root "printf '%s' '$original_app_config_b64' | base64 -d > '$CONFIG'" >/dev/null 2>&1 || true
    adb_su "chmod 644 '$CONFIG' 2>/dev/null || true" >/dev/null 2>&1 || true
  else
    adb_su "rm -f '$CONFIG'" >/dev/null 2>&1 || true
  fi
}

supports_fuse_daemon_scenarios() {
  case "${RUN_FUSE_DAEMON_SCENARIOS:-auto}" in
    1|true|TRUE|yes|YES) return 0 ;;
    0|false|FALSE|no|NO) return 1 ;;
  esac
  adb_su "for file in /data/adb/modules/storage.redirect.x/bin/srx_daemon /data/adb/modules/storage.redirect.x/zygisk/arm64-v8a.so /data/adb/modules/storage.redirect.x/zygisk/x86_64.so; do [ -f \"\$file\" ] && grep -a -q 'fuse_daemon_redirect_enabled' \"\$file\" && exit 0; done; exit 1" >/dev/null 2>&1
}

build_scenario_list() {
  scenarios=()
  if [ -n "${SRT_SCENARIOS:-}" ]; then
    local normalized="${SRT_SCENARIOS//,/ }"
    normalized="${normalized//;/ }"
    local scenario
    for scenario in $normalized; do
      case "$scenario" in
        ''|*[!0-9]*)
          echo "invalid scenario: $scenario" >&2
          return 1
          ;;
      esac
      if [ "$scenario" -lt 1 ] || [ "$scenario" -gt 29 ]; then
        echo "invalid scenario: $scenario" >&2
        return 1
      fi
      scenarios+=("$scenario")
    done
    return 0
  fi

  scenarios=(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 29)
  if supports_fuse_daemon_scenarios; then
    scenarios+=(16 17 18 19)
  else
    echo "skip fuse daemon scenarios: module does not expose fuse_daemon_redirect_enabled or RUN_FUSE_DAEMON_SCENARIOS disabled"
  fi
  scenarios+=(20 21 22)
  scenarios+=(28)
  scenarios+=(23 24)
  if supports_fuse_daemon_scenarios; then
    scenarios+=(25 26 27)
  else
    scenarios+=(26)
    echo "skip file monitor fuse daemon scenarios: module does not expose fuse_daemon_redirect_enabled or RUN_FUSE_DAEMON_SCENARIOS disabled"
  fi
}

remove_test_target_artifacts() {
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/SrtOther' '${REAL_ROOT}/Download/SrtOtherMapped' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/SrtOther' '${BACKEND_PRIVATE_ROOT}/Download/SrtOtherMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia'" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$ALLOW_PART_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$ALLOW_PART_FILE' '${REAL_ROOT}/Download/$QMARK_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_SINGLE_FILE' '${REAL_ROOT}/Download/$QMARK_DOUBLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_DOUBLE_FILE'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$QMARK_FILE_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_FILE_SINGLE_FILE'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude' '${REAL_ROOT}/Download/SrtFuseMapParent' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti' '${REAL_ROOT}/DCIM/SrtFuseQQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti' '${BACKEND_PRIVATE_ROOT}/DCIM/SrtFuseQQ'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFuseQa' '${REAL_ROOT}/Download/SrtFuseQab' '${REAL_ROOT}/Download/SrtFuseQb' '${REAL_ROOT}/Download/SrtFuseMediaAlpha' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram'" >/dev/null
}

remove_mediastore_rows_by_pattern() {
  local collection="$1"
  local name_regex="$2"
  local path_regex="$3"

  adb_su "content query --uri '$collection' --projection _id:_display_name:_data:relative_path 2>/dev/null || true" |
    while IFS= read -r row; do
      [[ "$row" =~ _id=([0-9]+) ]] || continue
      local id="${BASH_REMATCH[1]}"
      [[ "$row" =~ $name_regex ]] || continue
      [[ "$row" =~ $path_regex ]] || continue
      adb shell content delete --uri "$collection/$id" >/dev/null 2>&1 || true
    done
}

remove_random_mediastore_rows() {
  local app_regex="${APP_ID//./\\.}"
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_image_[0-9]+( \([0-9]+\))?\.jpg(,|$)' "relative_path=Pictures/|_data=.*/Pictures/|_data=.*/Android/data/${app_regex}/sdcard/Pictures/"
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_fuse_dcim_media( \([0-9]+\))?\.jpg(,|$)' "relative_path=DCIM/SrtFuseQQ/|_data=.*/DCIM/SrtFuseQQ/|_data=.*/Android/data/${app_regex}/sdcard/DCIM/SrtFuseQQ/"
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=srt_read_only_media( \([0-9]+\))?\.jpg(,|$)' "relative_path=Pictures/SrtReadOnlyMedia/|_data=.*/Pictures/SrtReadOnlyMedia/|_data=.*/Android/data/${app_regex}/sdcard/Pictures/SrtReadOnlyMedia/"
  remove_mediastore_rows_by_pattern "content://media/external/video/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_video_[0-9]+( \([0-9]+\))?\.mp4(,|$)' "relative_path=Movies/|_data=.*/Movies/|_data=.*/Android/data/${app_regex}/sdcard/Movies/"
  remove_mediastore_rows_by_pattern "content://media/external/audio/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_audio_[0-9]+( \([0-9]+\))?\.mp3(,|$)' "relative_path=Music/|_data=.*/Music/|_data=.*/Android/data/${app_regex}/sdcard/Music/"
  remove_mediastore_rows_by_pattern "content://media/external/file" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_file_[0-9]+( \([0-9]+\))?\.txt(,|$)' "relative_path=Documents/|_data=.*/Documents/|_data=.*/Android/data/${app_regex}/sdcard/Documents/"
  remove_mediastore_rows_by_pattern "content://media/external/downloads" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?(srt_download_[0-9]+( \([0-9]+\))?\.bin|srt_ci_probe( \([0-9]+\))?\.part|srt_qmark_a( \([0-9]+\))?\.txt|srt_qmark_ab( \([0-9]+\))?\.txt|srt_qmark_file_a( \([0-9]+\))?\.txt|srt_mountns_star_media( \([0-9]+\))?\.bin|srt_mountns_qmark_media( \([0-9]+\))?\.bin|srt_fuse_star_media( \([0-9]+\))?\.bin|srt_fuse_star_miss_media( \([0-9]+\))?\.bin|srt_fuse_qmark_media( \([0-9]+\))?\.bin|srt_fuse_qmark_miss_media( \([0-9]+\))?\.bin)(,|$)' "relative_path=Download/|_data=.*/Download/|_data=.*/Android/data/${app_regex}/sdcard/Download/"
  remove_mediastore_rows_by_pattern "content://media/external/downloads" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_monitor_[A-Za-z0-9_.-]+( \([0-9]+\))?\.bin(,|$)' "relative_path=Download/SrtMonitor|relative_path=Download/SrtMonitorMap|relative_path=Download/SrtMonitorMapped|relative_path=Download/SrtMonitorLocked|_data=.*/Download/SrtMonitor|_data=.*/Android/data/${app_regex}/sdcard/Download/SrtMonitor"
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_monitor_[A-Za-z0-9_.-]+( \([0-9]+\))?\.jpg(,|$)' "relative_path=Pictures/SrtRelativeData|_data=.*/Pictures/SrtRelativeData|_data=.*/Android/data/${app_regex}/sdcard/Pictures/SrtRelativeData|relative_path=Pictures/Nnngram|_data=.*/Pictures/Nnngram|_data=.*/Android/data/${app_regex}/sdcard/Pictures/Nnngram"
}

remove_random_physical_media_files() {
  adb_su "find '$BACKEND_ROOT/Pictures' '$BACKEND_PRIVATE_ROOT/Pictures' -maxdepth 1 -type f \( -name 'srt_image_[0-9]*.jpg' -o -name '.pending-*srt_image_[0-9]*.jpg' -o -name '.trashed-*srt_image_[0-9]*.jpg' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/DCIM/SrtFuseQQ' '$BACKEND_PRIVATE_ROOT/DCIM/SrtFuseQQ' -type f \( -name 'srt_fuse_dcim_media*.jpg' -o -name '.pending-*srt_fuse_dcim_media*.jpg' -o -name '.trashed-*srt_fuse_dcim_media*.jpg' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '$BACKEND_ROOT/Pictures/SrtReadOnlyMedia' '$BACKEND_PRIVATE_ROOT/Pictures/SrtReadOnlyMedia' 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/Movies' '$BACKEND_PRIVATE_ROOT/Movies' -maxdepth 1 -type f \( -name 'srt_video_[0-9]*.mp4' -o -name '.pending-*srt_video_[0-9]*.mp4' -o -name '.trashed-*srt_video_[0-9]*.mp4' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/Music' '$BACKEND_PRIVATE_ROOT/Music' -maxdepth 1 -type f \( -name 'srt_audio_[0-9]*.mp3' -o -name '.pending-*srt_audio_[0-9]*.mp3' -o -name '.trashed-*srt_audio_[0-9]*.mp3' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/Documents' '$BACKEND_PRIVATE_ROOT/Documents' -maxdepth 1 -type f \( -name 'srt_file_[0-9]*.txt' -o -name '.pending-*srt_file_[0-9]*.txt' -o -name '.trashed-*srt_file_[0-9]*.txt' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/Download' '$BACKEND_PRIVATE_ROOT/Download' -maxdepth 1 -type f \( -name 'srt_download_[0-9]*.bin' -o -name '.pending-*srt_download_[0-9]*.bin' -o -name '.trashed-*srt_download_[0-9]*.bin' -o -name 'srt_ci_probe*.part' -o -name '.pending-*srt_ci_probe*.part' -o -name '.trashed-*srt_ci_probe*.part' -o -name 'srt_qmark*.txt' -o -name '.pending-*srt_qmark*.txt' -o -name '.trashed-*srt_qmark*.txt' -o -name 'srt_mountns_*_media.bin' -o -name '.pending-*srt_mountns_*_media.bin' -o -name '.trashed-*srt_mountns_*_media.bin' -o -name 'srt_fuse_*_media.bin' -o -name '.pending-*srt_fuse_*_media.bin' -o -name '.trashed-*srt_fuse_*_media.bin' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "find '$BACKEND_ROOT/Download/SrtMonitor' '$BACKEND_ROOT/Download/SrtMonitorMap' '$BACKEND_ROOT/Download/SrtMonitorMapped' '$BACKEND_ROOT/Download/SrtMonitorLocked' '$BACKEND_ROOT/Pictures/SrtRelativeData' '$BACKEND_ROOT/Pictures/Nnngram' '$BACKEND_PRIVATE_ROOT/Download/SrtMonitor' '$BACKEND_PRIVATE_ROOT/Download/SrtMonitorMap' '$BACKEND_PRIVATE_ROOT/Download/SrtMonitorMapped' '$BACKEND_PRIVATE_ROOT/Download/SrtMonitorLocked' '$BACKEND_PRIVATE_ROOT/Pictures/SrtRelativeData' '$BACKEND_PRIVATE_ROOT/Pictures/Nnngram' -type f \( -name 'srt_monitor_*.bin' -o -name 'srt_monitor_*.jpg' -o -name '.pending-*srt_monitor_*' -o -name '.trashed-*srt_monitor_*' \) -delete 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '$BACKEND_RESULT_DIR' '$BACKEND_ROOT/Android/data/$APP_ID/files/srt_file_tests' '$INTERNAL_RESULT_DIR' '/data/data/$APP_ID/files/srt_file_tests' '$SANDBOX_RESULT_DIR' '$BACKEND_PRIVATE_ROOT/Android/data/$APP_ID/files/srt_file_tests' 2>/dev/null || true" >/dev/null
}

restart_media_provider() {
  local sdk
  sdk="$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  if [ -n "$sdk" ] && [ "$sdk" -le 34 ]; then
    echo "skip_media_provider_restart sdk=${sdk}: restarting MediaProvider can detach emulated storage on this emulator" >&2
    return 0
  fi

  adb shell am force-stop com.android.providers.media.module >/dev/null 2>&1 || true
  adb shell am force-stop com.google.android.providers.media.module >/dev/null 2>&1 || true
  adb_su "pkill -f com.android.providers.media.module 2>/dev/null || true; pkill -f com.google.android.providers.media.module 2>/dev/null || true" >/dev/null 2>&1 || true
  sleep 2
}

ensure_monitor_collector() {
  adb_su "touch /data/adb/modules/storage.redirect.x/config/apps '$GLOBAL_CONFIG' '$CONFIG' 2>/dev/null || true" >/dev/null 2>&1 || true
  adb_su "/data/adb/modules/storage.redirect.x/bin/srxctl ensure-collectors" >/dev/null 2>&1 || true
}

clear_file_monitor_log() {
  adb_su "mkdir -p '/data/adb/modules/storage.redirect.x/logs'; : > '$FILE_MONITOR_LOG_PATH'" >/dev/null 2>&1 || true
}

file_monitor_watch_capacity_limited() {
  local matches
  matches="$(adb_su "grep -E 'daemon monitor watch limit reached|capacity_limited=true' /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null || true")"
  [ -n "$matches" ]
}

assert_file_monitor_enabled_for_scenario() {
  local scenario="$1"
  local label="$2"
  if adb_su "grep -Eq '\"file_monitor_enabled\"[[:space:]]*:[[:space:]]*true' '$GLOBAL_CONFIG' 2>/dev/null"; then
    return 0
  fi
  echo "file_monitor_disabled scenario=${scenario} label=${label}: file_monitor_enabled must be true for monitor record tests" >&2
  adb_su "cat '$GLOBAL_CONFIG' 2>/dev/null || true" | sed 's/^/global_config: /' >&2
  return 1
}

prepare_file_monitor_assertion() {
  local scenario="$1"
  local label="$2"
  echo "monitor_prepare scenario=${scenario} label=${label}"
  assert_file_monitor_enabled_for_scenario "$scenario" "$label" || return 1
  adb logcat -c >/dev/null 2>&1 || true
  clear_file_monitor_log
  ensure_monitor_collector
  sleep_ms "$SRT_SERVICE_CASE_SETTLE_MS"
}

wait_file_monitor_log_line() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local expected="$4"
  local timeout_seconds="${5:-30}"
  local allow_capacity_limited_miss="${6:-0}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    local matches
    case "$expected" in
      success)
        matches="$(adb_su "grep -F -- '$file_name' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | grep -Fv -- 'ret=-1' | grep -Fv -- 'op=close_write' || true")"
        if [ -n "$matches" ]; then
          echo "monitor_log_found scenario=${scenario} label=${label} file=${file_name} expected=${expected}"
          return 0
        fi
        ;;
      failure)
        matches="$(adb_su "grep -F -- '$file_name' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | grep -F -- 'ret=-1' | grep -F -- 'deny_reason=read_only_rule' || true")"
        if [ -n "$matches" ]; then
          echo "monitor_log_found scenario=${scenario} label=${label} file=${file_name} expected=${expected}"
          return 0
        fi
        ;;
    esac
    sleep_ms 200
  done

  if [ "$allow_capacity_limited_miss" = "1" ] && file_monitor_watch_capacity_limited; then
    echo "monitor_log_skipped scenario=${scenario} label=${label} file=${file_name} expected=${expected} reason=watch-capacity-limited"
    return 0
  fi
  echo "monitor_log_timeout scenario=${scenario} label=${label} file=${file_name} expected=${expected}"
  adb_su "tail -80 '$FILE_MONITOR_LOG_PATH' 2>/dev/null || true" | sed 's/^/monitor_log_tail: /'
  return 1
}

expect_file_monitor_success_record() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local allow_capacity_limited_miss="${4:-0}"
  wait_file_monitor_log_line "$scenario" "$label" "$file_name" "success" 30 "$allow_capacity_limited_miss"
}

expect_file_monitor_failure_record() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local timeout_seconds="${4:-30}"
  wait_file_monitor_log_line "$scenario" "$label" "$file_name" "failure" "$timeout_seconds"
}

expect_no_read_only_failure_record() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local matches
  matches="$(adb_su "grep -F -- '$file_name' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | grep -F -- 'ret=-1' | grep -F -- 'deny_reason=read_only_rule' || true")"
  if [ -n "$matches" ]; then
    echo "file_monitor unexpected read-only failure: scenario=${scenario} label=${label} file=${file_name}" >&2
    printf '%s\n' "$matches" | sed 's/^/monitor_read_only_hit: /' >&2
    return 1
  fi
}

cleanup_test_artifacts() {
  local status=$?
  if [ "${cleanup_done:-0}" -eq 1 ]; then
    return "$status"
  fi
  cleanup_done=1
  set +e
  echo "== cleanup test artifacts =="
  adb shell am force-stop "$APP_ID" >/dev/null 2>&1
  restore_app_config >/dev/null 2>&1
  restore_global_config >/dev/null 2>&1
  clean_results >/dev/null 2>&1
  remove_test_target_artifacts >/dev/null 2>&1
  remove_random_mediastore_rows >/dev/null 2>&1
  remove_random_physical_media_files >/dev/null 2>&1
  restart_media_provider >/dev/null 2>&1
  return "$status"
}

latest_result() {
  adb_su "extra=\$(find '$BACKEND_ROOT/Android/data/$APP_ID' '/data/data/$APP_ID' -path '*/files/test_case_result/result_*.txt' -type f 2>/dev/null); ls -t '$RESULT_DIR'/result_*.txt '$INTERNAL_RESULT_DIR'/result_*.txt '$BACKEND_RESULT_DIR'/result_*.txt '$SANDBOX_RESULT_DIR'/result_*.txt \$extra 2>/dev/null | head -1" | tail -1
}

wait_service_result() {
  local timeout_seconds="$1"
  local seconds=$((SRT_RESULT_POLL_MS / 1000))
  local remainder=$((SRT_RESULT_POLL_MS % 1000))
  local poll_delay
  printf -v poll_delay '%d.%03d' "$seconds" "$remainder"
  adb_su "deadline=\$(date +%s); deadline=\$((deadline + $timeout_seconds)); while [ \$(date +%s) -lt \$deadline ]; do for file in '$RESULT_DIR/result_current.txt' '$INTERNAL_RESULT_DIR/result_current.txt' '$BACKEND_RESULT_DIR/result_current.txt' '$SANDBOX_RESULT_DIR/result_current.txt' \$(find '$BACKEND_ROOT/Android/data/$APP_ID' '/data/data/$APP_ID' -path '*/files/test_case_result/result_current.txt' -type f 2>/dev/null); do if [ -s \"\$file\" ]; then cat \"\$file\"; exit 0; fi; done; sleep $poll_delay; done; exit 1"
}

wait_app_mount_confirmed() {
  local label="$1"
  local expect_mount="${2:-1}"
  if [ "$expect_mount" -ne 1 ]; then
    return 0
  fi
  if [ "$SRT_MOUNT_CONFIRM_TIMEOUT_MS" -le 0 ]; then
    return 1
  fi
  local timeout_seconds=$(((SRT_MOUNT_CONFIRM_TIMEOUT_MS + 999) / 1000))
  local output
  if output="$(adb_su "deadline=\$((\$(date +%s) + $timeout_seconds)); pid=''; while [ \$(date +%s) -le \$deadline ]; do pid=\$(pidof '$APP_ID' 2>/dev/null | awk '{print \$1}'); [ -n \"\$pid\" ] && break; sleep 0.1; done; if [ -z \"\$pid\" ]; then echo pid_not_found; exit 2; fi; confirmed=\"app mount confirmed pid=\$pid\"; daemon=\"daemon mount pkg=$APP_ID pid=\$pid op=Reload ok=true\"; marker=\"marker ok path=/data/user/0/$APP_ID/.srx_mount_status_\$pid\"; while [ \$(date +%s) -le \$deadline ]; do if logcat -d -t 300 -s StorageRedirect:V SRX:V 2>/dev/null | grep -Eq \"(\$confirmed|\$daemon|\$marker)\"; then echo confirmed_pid=\$pid; exit 0; fi; if tail -240 '$LOG_PATH' 2>/dev/null | grep -Eq \"(\$confirmed|\$daemon|\$marker)\"; then echo confirmed_pid=\$pid; exit 0; fi; sleep 0.1; done; echo pid=\$pid; exit 1")"; then
    local confirmed_pid
    confirmed_pid="$(grep -E '^confirmed_pid=' <<<"$output" | tail -1 | cut -d= -f2)"
    if [ -n "$confirmed_pid" ] && app_mountinfo_has_expected_paths "$label" "$confirmed_pid"; then
      LAST_MOUNT_CONFIRMED_PID="$confirmed_pid"
      return 0
    fi
    echo "mount confirm missing expected mountinfo: $label pid=${confirmed_pid:-missing}"
    return 1
  fi
  if grep -Fq "pid_not_found" <<<"$output"; then
    echo "mount confirm skipped: app pid not found for $label"
  else
    echo "mount confirm timeout: $label $(grep -E '^pid=' <<<"$output" | tail -1)"
  fi
  return 1
}

scenario_from_label() {
  sed -n 's/.*scenario-\([0-9][0-9]*\).*/\1/p' <<<"$1" | head -1
}

label_expects_mount() {
  local scenario
  scenario="$(scenario_from_label "$1")"
  case "$scenario" in
    ""|1|23) return 1 ;;
    *) return 0 ;;
  esac
}

expected_mount_paths_for_label() {
  local scenario
  scenario="$(scenario_from_label "$1")"
  case "$scenario" in
    3)
      printf '%s\n' "${REAL_ROOT}/Download/SrtProbe"
      ;;
    4)
      printf '%s\n' "${REAL_ROOT}/Download" "${REAL_ROOT}/Download/SrtProbe"
      ;;
  esac
}

app_mountinfo_has_expected_paths() {
  local label="$1"
  local pid="$2"
  local expected path command output
  expected="$(expected_mount_paths_for_label "$label")"
  [ -z "$expected" ] && return 0
  [ -n "$pid" ] || return 1

  command="pid='$pid'; "
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    command="${command}grep -Fq ' ${path} ' \"/proc/\$pid/mountinfo\" || { echo missing=${path}; exit 1; }; "
  done <<<"$expected"

  if output="$(adb_su "$command")"; then
    return 0
  fi
  printf '%s\n' "$output" | sed "s/^/mountinfo_check ${label}: /"
  return 1
}

ensure_current_app_mount_confirmed() {
  local label="$1"
  local current_pid
  current_pid="$(app_pid)"
  if [ -n "$current_pid" ] && [ "$current_pid" = "${LAST_MOUNT_CONFIRMED_PID:-}" ] && app_mountinfo_has_expected_paths "$label" "$current_pid"; then
    return 0
  fi
  echo "mount confirm refresh: ${label} before=${LAST_MOUNT_CONFIRMED_PID:-none} after=${current_pid:-missing}"
  wait_app_mount_confirmed "$label" 1
}

wait_config_applied() {
  local label="$1"
  local timeout_seconds=$(((SRT_CONFIG_APPLY_TIMEOUT_MS + 999) / 1000))
  local output
  if output="$(adb_su "deadline=\$((\$(date +%s) + $timeout_seconds)); while [ \$(date +%s) -le \$deadline ]; do if tail -240 '$LOG_PATH' 2>/dev/null | grep -Eq 'config (reloaded|loaded) .*apps=[1-9]'; then exit 0; fi; sleep 0.1; done; tail -80 '$LOG_PATH' 2>/dev/null; exit 1")"; then
    return 0
  fi
  echo "config apply timeout: $label" >&2
  printf '%s\n' "$output" | sed 's/^/config_log_tail: /' >&2
  return 1
}

service_case_timeout_seconds() {
  case "$1" in
    all) echo "${ALL_TEST_TIMEOUT_SECONDS:-240}" ;;
    *) echo "${TEST_CASE_TIMEOUT_SECONDS:-75}" ;;
  esac
}

sleep_ms() {
  local ms=${1:-0}
  local seconds=$((ms / 1000))
  local remainder=$((ms % 1000))
  local delay
  printf -v delay '%d.%03d' $seconds $remainder
  sleep $delay
}

prepare_service_case() {
  local label="$1"
  case "$SRT_FRESH_APP_PER_CASE" in
    1|true|TRUE|yes|YES) ;;
    *) return 0 ;;
  esac
  local expect_mount=0
  if label_expects_mount "$label"; then
    expect_mount=1
  fi
  start_app_and_confirm_mount "$label" "$expect_mount" || return 1
  wait_storage_ready "$label" 30 >/dev/null || return 1
}

start_app_and_confirm_mount() {
  local label="$1"
  local expect_mount="${2:-1}"
  local attempt max_attempts
  max_attempts="$SRT_APP_MOUNT_CONFIRM_RETRIES"
  case "$max_attempts" in
    ''|*[!0-9]*) max_attempts=1 ;;
  esac
  [ "$max_attempts" -gt 0 ] || max_attempts=1

  for attempt in $(seq 1 "$max_attempts"); do
    LAST_MOUNT_CONFIRMED_PID=""
    adb shell am force-stop "$APP_ID" >/dev/null || true
    sleep 0.5
    adb logcat -c >/dev/null 2>&1 || true
    adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null
    adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null
    if wait_app_mount_confirmed "$label" "$expect_mount"; then
      return 0
    fi
    echo "mount confirm retry: ${label} attempt=${attempt}/${max_attempts}"
    sleep_ms "$SRT_APP_LAUNCH_SETTLE_MS"
  done

  echo "mount confirm failed: ${label} attempts=${max_attempts}"
  return 1
}

wait_storage_ready() {
  local label="$1"
  local timeout_seconds="${2:-90}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb shell "sm list-volumes all 2>/dev/null | grep -q 'emulated;0 mounted' && test -d '$REAL_ROOT' && ls -ld '$REAL_ROOT' >/dev/null 2>&1" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for emulated storage: ${label}"
  print_storage_state "${label}-storage-timeout"
  return 1
}

media_provider_query_ready() {
  local uri="$1"
  local output
  output="$(adb shell content query --uri "$uri" --projection _id --where '_id=-1' 2>&1 || true)"
  if grep -Eq 'Error while accessing provider:media|Volume external_primary not found|IllegalArgumentException|Unknown URL|Unsupported Uri' <<<"$output"; then
    return 1
  fi
  return 0
}

wait_media_provider_ready() {
  local label="$1"
  local timeout_seconds="${2:-120}"
  local deadline=$((SECONDS + timeout_seconds))
  local uris=(
    "content://media/external_primary/images/media"
    "content://media/external_primary/video/media"
    "content://media/external_primary/audio/media"
    "content://media/external_primary/file"
    "content://media/external_primary/downloads"
    "content://media/external/images/media"
    "content://media/external/video/media"
    "content://media/external/audio/media"
    "content://media/external/file"
    "content://media/external/downloads"
  )

  while [ "$SECONDS" -lt "$deadline" ]; do
    local ready=1
    local uri
    for uri in "${uris[@]}"; do
      if ! media_provider_query_ready "$uri"; then
        ready=0
        break
      fi
    done
    if [ "$ready" -eq 1 ]; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for MediaProvider: ${label}"
  print_storage_state "${label}-media-provider-timeout"
  return 1
}

print_storage_state() {
  local label="$1"
  echo "=== storage state: ${label} ==="
  adb shell "date; getprop ro.build.version.sdk; getprop ro.build.version.release; getprop sys.boot_completed; getprop dev.bootcomplete; getprop init.svc.sdcard; getprop init.svc.media; sm list-volumes all 2>/dev/null || true; df -h /storage/emulated/0 /sdcard 2>&1 || true; ls -ld /storage /storage/emulated /storage/emulated/0 /sdcard 2>&1 || true; mount | grep -E ' /storage|/mnt/runtime|/mnt/user|sdcard|fuse|srx' || true" || true
  adb_su "id; ls -ld /mnt/user/0 /mnt/user/0/emulated /mnt/user/0/emulated/0 /mnt/runtime/default/emulated/0 2>&1 || true; cat /proc/mounts | grep -E ' /storage|/mnt/runtime|/mnt/user|sdcard|fuse|srx' || true" || true
}

run_service_case() {
  local scenario="$1"
  local label="$2"
  local test_case="$3"
  local pass_pattern="$4"
  shift 4
  local output_file="scenario-${scenario}-${label}-result.txt"

  prepare_service_case "scenario-${scenario}-${label}" || return 1
  sleep_ms "$SRT_SERVICE_CASE_SETTLE_MS"
  clean_results
  local start_output
  if ! start_output="$(adb shell am broadcast -n "${APP_ID}/.receiver.TestCaseReceiver" -a "$ACTION" --es test_case "$test_case" "$@" 2>&1)"; then
    echo "service_start_failed scenario=${scenario} label=${label} test_case=${test_case}"
    printf '%s\n' "$start_output" | sed 's/^/service_start: /'
    return 1
  fi
  if ! grep -Eq 'Broadcast completed|result=0|cmp=' <<<"$start_output"; then
    echo "service_start_unexpected scenario=${scenario} label=${label} test_case=${test_case}"
    printf '%s\n' "$start_output" | sed 's/^/service_start: /'
  fi
  if label_expects_mount "scenario-${scenario}-${label}-service"; then
    ensure_current_app_mount_confirmed "scenario-${scenario}-${label}-service" || return 1
  fi

  local timeout_seconds
  timeout_seconds="$(service_case_timeout_seconds "$test_case")"
  if wait_service_result "$timeout_seconds" | tee "$output_file"; then
    cat "$output_file" >>"scenario-${scenario}-result.txt"
    if [ -z "$pass_pattern" ]; then
      return 0
    fi
    if grep -q "$pass_pattern" "$output_file"; then
      return 0
    fi
    return 1
  fi

  echo "result_timeout scenario=${scenario} test_case=${test_case}"
  adb shell am force-stop "$APP_ID" >/dev/null || true
  return 1
}

run_write_case() {
  local scenario="$1"
  local label="$2"
  local path="$3"
  local payload="${4:-$PAYLOAD}"
  run_service_case "$scenario" "$label" "file_write" '^PASS \[file_write\]' --es file_path "$path" --es payload "$payload" --es expected_payload "$payload"
}

run_create_case() {
  local scenario="$1"
  local label="$2"
  local path="$3"
  run_service_case "$scenario" "$label" "file_create" '^PASS \[file_create\]' --es file_path "$path"
}

run_mediastore_download_create_case() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local relative_path="${4:-}"
  local attempt
  for attempt in 1 2 3; do
    wait_storage_ready "scenario-${scenario}-${label}-mediastore-storage" 30 >/dev/null || return 1
    wait_media_provider_ready "scenario-${scenario}-${label}-mediastore-provider" 60 >/dev/null || return 1
    if [ -n "$relative_path" ]; then
      if run_service_case "$scenario" "$label" "mediastore_create_download" '^PASS \[mediastore_create_download\]' --es file_name "$file_name" --es relative_path "$relative_path"; then
        return 0
      fi
    else
      if run_service_case "$scenario" "$label" "mediastore_create_download" '^PASS \[mediastore_create_download\]' --es file_name "$file_name"; then
        return 0
      fi
    fi
    if [ "$attempt" -eq 3 ]; then
      return 1
    fi
    echo "mediastore_download_create_retry scenario=${scenario} label=${label} attempt=${attempt}"
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
}

run_mediastore_download_create_denied_case() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local relative_path="${4:-}"
  wait_storage_ready "scenario-${scenario}-${label}-mediastore-storage" 30 >/dev/null || return 1
  wait_media_provider_ready "scenario-${scenario}-${label}-mediastore-provider" 60 >/dev/null || return 1
  if [ -n "$relative_path" ]; then
    run_service_case "$scenario" "$label" "mediastore_create_download_denied" '^PASS \[mediastore_create_download_denied\]' --es file_name "$file_name" --es relative_path "$relative_path"
  else
    run_service_case "$scenario" "$label" "mediastore_create_download_denied" '^PASS \[mediastore_create_download_denied\]' --es file_name "$file_name"
  fi
}

run_write_test() {
  local scenario="$1"
  local path
  path="$(target_path "$scenario")"
  for attempt in 1 2; do
    if run_write_case "$scenario" "write" "$path" "$PAYLOAD"; then
      return 0
    fi
    if [ "$attempt" -eq 2 ]; then
      return 1
    fi
    echo "write_retry scenario=${scenario} attempt=${attempt}"
    adb shell am force-stop "$APP_ID" >/dev/null || true
    adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null
    wait_storage_ready "scenario-${scenario}-write-retry"
    clean_targets
  done
}

check_app_view() {
  local scenario="$1"
  local dir
  dir="$(logical_dir "$scenario")"
  expect_app_entry "$scenario" "app-view" "$dir"

  local mapped_real_dir="${REAL_ROOT}/Download/Test"
  case "$scenario" in
    3)
      expect_no_app_entry "$scenario" "app-mapped-real-view" "$mapped_real_dir"
      ;;
    4)
      expect_app_entry "$scenario" "app-mapped-real-view" "$mapped_real_dir"
      ;;
  esac
}

expect_app_entry() {
  local scenario="$1"
  local label="$2"
  local dir="$3"

  for attempt in 1 2 3 4 5; do
    if run_service_case "$scenario" "$label" "file_list_dir" '^PASS \[file_list_dir\]' --es file_dir "$dir" &&
      grep -q "entries=.*${TEST_FILE}" "scenario-${scenario}-${label}-result.txt"; then
      echo "app_view scenario=${scenario} logical_dir=${dir} expected_entry=${TEST_FILE}"
      return 0
    fi
    echo "app_view_retry scenario=${scenario} logical_dir=${dir} attempt=${attempt}"
    sleep_ms "$SRT_RESULT_POLL_MS"
  done

  return 1
}

expect_no_app_entry() {
  local scenario="$1"
  local label="$2"
  local dir="$3"

  for attempt in 1 2 3; do
    run_service_case "$scenario" "$label" "file_list_dir" "" --es file_dir "$dir"
    if grep -q "entries=.*${TEST_FILE}" "scenario-${scenario}-${label}-result.txt"; then
      echo "app_view scenario=${scenario} logical_dir=${dir} forbidden_entry_visible=${TEST_FILE}"
      return 1
    fi
    sleep_ms "$SRT_RESULT_POLL_MS"
  done

  echo "app_view scenario=${scenario} logical_dir=${dir} forbidden_entry=${TEST_FILE}"
}

find_written_file() {
  adb_su "for dir in '${REAL_ROOT}/Download' '${REAL_ROOT}/Pictures' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${PRIVATE_ROOT}/Download' '${PRIVATE_ROOT}/Pictures' '${PRIVATE_ROOT}/.xldownload' '${PRIVATE_ROOT}/.xlDownload'; do find \"\$dir\" -maxdepth 5 \\( -name '$TEST_FILE' -o -name '$HOT_BEFORE_FILE' -o -name '$HOT_AFTER_FILE' -o -name '$READ_ONLY_FILE' -o -name '$ALLOW_KEEP_FILE' -o -name '$ALLOW_PART_FILE' \\) -print 2>/dev/null || true; done | sort"
}

check_file_exists() {
  local label="$1"
  local path="$2"
  if adb_su "test -f '$path'"; then
    echo "file_exists label=${label} path=${path}"
    return 0
  fi
  echo "file_missing label=${label} path=${path}"
  return 1
}

check_file_missing() {
  local label="$1"
  local path="$2"
  if adb_su "test ! -e '$path'"; then
    echo "file_absent label=${label} path=${path}"
    return 0
  fi
  echo "file_unexpected label=${label} path=${path}"
  adb_su "ls -ld '$path' 2>/dev/null || true" || true
  return 1
}

check_file_location() {
  local scenario="$1" actual expected
  expected="$(expected_path "$scenario")"
  actual="$(find_written_file | tr '\n' ';')"
  echo "scenario=${scenario} expected_path=${expected} actual=${actual}"
  check_file_exists "scenario-${scenario}-expected" "$expected"
}

seed_read_only_targets() {
  adb_su "mkdir -p '$BACKEND_READ_ONLY_ROOT'; rm -f '$BACKEND_READ_ONLY_ROOT/write_denied.txt' '$BACKEND_READ_ONLY_ROOT/renamed.txt' '$BACKEND_READ_ONLY_ROOT/$READ_ONLY_HARDLINK' '$BACKEND_READ_ONLY_ROOT/$READ_ONLY_SYMLINK'; rm -rf '$BACKEND_READ_ONLY_ROOT/newdir'; printf '%s' '$READ_ONLY_PAYLOAD' > '$BACKEND_READ_ONLY_ROOT/$READ_ONLY_FILE'; chmod -R 777 '$BACKEND_READ_ONLY_ROOT' 2>/dev/null || true" >/dev/null
}

run_mediastore_image_create_case() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local relative_path="${4:-}"
  if [ -n "$relative_path" ]; then
    run_service_case "$scenario" "$label" "mediastore_create_image" '^PASS \[mediastore_create_image\]' --es file_name "$file_name" --es relative_path "$relative_path"
  else
    run_service_case "$scenario" "$label" "mediastore_create_image" '^PASS \[mediastore_create_image\]' --es file_name "$file_name"
  fi
}

run_mediastore_image_relative_data_create_case() {
  local scenario="$1"
  local label="$2"
  local file_name="$3"
  local relative_data_dir="$4"
  local attempt
  for attempt in 1 2 3; do
    wait_storage_ready "scenario-${scenario}-${label}-mediastore-relative-storage" 30 >/dev/null || return 1
    wait_media_provider_ready "scenario-${scenario}-${label}-mediastore-relative-provider" 60 >/dev/null || return 1
    if run_service_case "$scenario" "$label" "mediastore_create_image_relative_data" '^PASS \[mediastore_create_image_relative_data\]' --es file_name "$file_name" --es relative_path "$relative_data_dir"; then
      return 0
    fi
    [ "$attempt" -lt 3 ] || return 1
    echo "mediastore_image_relative_data_retry scenario=${scenario} label=${label} attempt=${attempt}"
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
}

check_read_only_artifacts() {
  check_file_exists "read-only-seed" "$READ_ONLY_ROOT/$READ_ONLY_FILE" &&
    check_file_missing "read-only-write" "$READ_ONLY_ROOT/write_denied.txt" &&
    check_file_missing "read-only-hardlink" "$READ_ONLY_ROOT/$READ_ONLY_HARDLINK" &&
    check_file_missing "read-only-symlink" "$READ_ONLY_ROOT/$READ_ONLY_SYMLINK" &&
    check_file_missing "read-only-mkdir" "$READ_ONLY_ROOT/newdir" &&
    check_file_missing "read-only-rename-target" "$READ_ONLY_ROOT/renamed.txt"
}

run_read_only_scenario() {
  local scenario="$1"
  run_service_case "$scenario" "read-only-read" "file_read" '^PASS \[file_read\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es expected_payload "$READ_ONLY_PAYLOAD" &&
    run_service_case "$scenario" "read-only-stat" "file_stat" '^PASS \[file_stat\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" &&
    run_service_case "$scenario" "read-only-access" "file_access" '^PASS \[file_access\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" &&
    run_service_case "$scenario" "read-only-write-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$READ_ONLY_ROOT/write_denied.txt" --es payload "$PAYLOAD" &&
    run_service_case "$scenario" "read-only-truncate-denied" "file_truncate_denied" '^PASS \[file_truncate_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es length "4" &&
    run_service_case "$scenario" "read-only-ftruncate-denied" "file_ftruncate_denied" '^PASS \[file_ftruncate_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es length "8" &&
    run_service_case "$scenario" "read-only-chmod-denied" "file_chmod_denied" '^PASS \[file_chmod_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es mode "0600" &&
    run_service_case "$scenario" "read-only-fchmod-denied" "file_fchmod_denied" '^PASS \[file_fchmod_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es mode "0600" &&
    run_service_case "$scenario" "read-only-link-denied" "file_link_denied" '^PASS \[file_link_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es target_file_path "$READ_ONLY_ROOT/$READ_ONLY_HARDLINK" &&
    run_service_case "$scenario" "read-only-symlink-denied" "file_symlink_denied" '^PASS \[file_symlink_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es target_file_path "$READ_ONLY_ROOT/$READ_ONLY_SYMLINK" &&
    run_service_case "$scenario" "read-only-mkdir-denied" "file_mkdir_denied" '^PASS \[file_mkdir_denied\]' --es file_dir "$READ_ONLY_ROOT/newdir" &&
    run_service_case "$scenario" "read-only-rename-denied" "file_rename_denied" '^PASS \[file_rename_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" --es target_file_path "$READ_ONLY_ROOT/renamed.txt" &&
    run_service_case "$scenario" "read-only-delete-denied" "file_delete_denied" '^PASS \[file_delete_denied\]' --es file_path "$READ_ONLY_ROOT/$READ_ONLY_FILE" &&
    check_read_only_artifacts
}

wait_mediastore_read_only_image() {
  local logical_path="$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  local deadline=$((SECONDS + 30))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb_su "content query --uri content://media/external/images/media --projection _id:_display_name:_data:relative_path 2>/dev/null | grep -F -- '$READ_ONLY_IMAGE_FILE' | grep -F -- 'Pictures/SrtReadOnlyMedia' >/dev/null"; then
      echo "mediastore_row_ready path=${logical_path}"
      return 0
    fi
    sleep_ms 500
  done
  echo "mediastore_row_missing path=${logical_path}" >&2
  adb_su "content query --uri content://media/external/images/media --projection _id:_display_name:_data:relative_path 2>/dev/null | grep -F -- '$READ_ONLY_IMAGE_FILE' || true" | sed 's/^/mediastore_row: /' >&2
  return 1
}

prepare_read_only_media_image() {
  local backend_dir="$BACKEND_ROOT/Pictures/SrtReadOnlyMedia"
  local backend_path="$backend_dir/$READ_ONLY_IMAGE_FILE"
  local logical_path="$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=srt_read_only_media( \([0-9]+\))?\.jpg(,|$)' 'relative_path=Pictures/SrtReadOnlyMedia/|_data=.*/Pictures/SrtReadOnlyMedia/'
  adb_su "mkdir -p '$backend_dir'; rm -f '$BACKEND_PRIVATE_ROOT/Pictures/SrtReadOnlyMedia/$READ_ONLY_IMAGE_FILE'; printf '%s' '$READ_ONLY_IMAGE_B64' | base64 -d > '$backend_path'; chmod -R 777 '$backend_dir' 2>/dev/null || true" >/dev/null
  adb shell content insert --uri content://media/external/images/media --bind _data:s:"$logical_path" --bind _display_name:s:"$READ_ONLY_IMAGE_FILE" --bind mime_type:s:image/jpeg >/dev/null 2>&1 || true
  adb shell am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d "file://${logical_path}" >/dev/null 2>&1 || true
  wait_mediastore_read_only_image
}

run_mediastore_read_only_query_scenario() {
  local scenario="$1"
  local logical_path="$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  local private_path="$PRIVATE_READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  wait_mediastore_read_only_image &&
    run_service_case "$scenario" "read-only-image-query" "mediastore_query_read_only_image" '^PASS \[mediastore_query_read_only_image\]' --es file_name "$READ_ONLY_IMAGE_FILE" --es expected_path "$logical_path" &&
    run_service_case "$scenario" "read-only-image-list" "file_list_dir" '^PASS \[file_list_dir\]' --es file_dir "$READ_ONLY_MEDIA_ROOT" &&
    grep -q "entries=.*${READ_ONLY_IMAGE_FILE}" "scenario-${scenario}-read-only-image-list-result.txt" &&
    run_service_case "$scenario" "read-only-image-file-read" "file_read" '^PASS \[file_read\]' --es file_path "$logical_path" &&
    check_file_exists "read-only-media-real" "$logical_path" &&
    check_file_missing "read-only-media-private" "$private_path"
}

prepare_mapped_read_only_targets() {
  adb_su "mkdir -p '$MAPPED_READ_ONLY_REQUEST' '$MAPPED_READ_ONLY_TARGET'; rm -f '$MAPPED_READ_ONLY_REQUEST/$TEST_FILE' '$MAPPED_READ_ONLY_TARGET/$TEST_FILE'; chmod -R 777 '$MAPPED_READ_ONLY_REQUEST' '$MAPPED_READ_ONLY_TARGET' 2>/dev/null || true" >/dev/null
}

run_mapped_read_only_scenario() {
  local scenario="$1"
  prepare_mapped_read_only_targets
  run_service_case "$scenario" "mapped-read-only-write-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$MAPPED_READ_ONLY_REQUEST/$TEST_FILE" --es payload "$PAYLOAD" &&
    check_file_missing "mapped-read-only-request" "$MAPPED_READ_ONLY_REQUEST/$TEST_FILE" &&
    check_file_missing "mapped-read-only-target" "$MAPPED_READ_ONLY_TARGET/$TEST_FILE"
}

run_allow_exclusion_scenario() {
  local scenario="$1"
  local keep_path="$ALLOW_ROOT/$ALLOW_KEEP_FILE"
  local keep_private="$PRIVATE_ALLOW_ROOT/$ALLOW_KEEP_FILE"
  local tmp_path="$ALLOW_ROOT/tmp/$TEST_FILE"
  local tmp_private="$PRIVATE_ALLOW_ROOT/tmp/$TEST_FILE"
  local part_path="$REAL_ROOT/Download/$ALLOW_PART_FILE"
  local part_private="$PRIVATE_ROOT/Download/$ALLOW_PART_FILE"

  run_write_case "$scenario" "allow-real-write" "$keep_path" "$PAYLOAD" &&
    check_file_exists "allow-real" "$keep_path" &&
    check_file_missing "allow-real-private" "$keep_private" &&
    run_write_case "$scenario" "allow-excluded-dir-write" "$tmp_path" "$PAYLOAD" &&
    check_file_exists "allow-excluded-dir-private" "$tmp_private" &&
    check_file_missing "allow-excluded-dir-real" "$tmp_path" &&
    run_mediastore_download_create_case "$scenario" "allow-excluded-glob-download-create" "$ALLOW_PART_FILE" &&
    check_file_exists "allow-excluded-glob-private" "$part_private" &&
    check_file_missing "allow-excluded-glob-real" "$part_path"
}

run_legacy_exclusion_scenario() {
  local scenario="$1"
  local keep_path="$LEGACY_ROOT/$ALLOW_KEEP_FILE"
  local keep_private="$PRIVATE_LEGACY_ROOT/$ALLOW_KEEP_FILE"
  local tmp_path="$LEGACY_ROOT/tmp/$TEST_FILE"
  local tmp_private="$PRIVATE_LEGACY_ROOT/tmp/$TEST_FILE"

  run_write_case "$scenario" "legacy-allow-real-write" "$keep_path" "$PAYLOAD" &&
    check_file_exists "legacy-allow-real" "$keep_path" &&
    check_file_missing "legacy-allow-private" "$keep_private" &&
    run_write_case "$scenario" "legacy-excluded-write" "$tmp_path" "$PAYLOAD" &&
    check_file_exists "legacy-excluded-private" "$tmp_private" &&
    check_file_missing "legacy-excluded-real" "$tmp_path"
}

run_qmark_wildcard_scenario() {
  local scenario="$1"
  local single_path="$REAL_ROOT/Download/$QMARK_SINGLE_FILE"
  local single_private="$PRIVATE_ROOT/Download/$QMARK_SINGLE_FILE"
  local double_path="$REAL_ROOT/Download/$QMARK_DOUBLE_FILE"
  local double_private="$PRIVATE_ROOT/Download/$QMARK_DOUBLE_FILE"
  local file_single_path="$REAL_ROOT/Download/$QMARK_FILE_SINGLE_FILE"
  local file_single_private="$PRIVATE_ROOT/Download/$QMARK_FILE_SINGLE_FILE"

  run_mediastore_download_create_case "$scenario" "qmark-single-char-download-create" "$QMARK_SINGLE_FILE" &&
    check_file_exists "qmark-single-char-real" "$single_path" &&
    check_file_missing "qmark-single-char-private" "$single_private" &&
    run_mediastore_download_create_case "$scenario" "qmark-two-char-download-create" "$QMARK_DOUBLE_FILE" &&
    check_file_exists "qmark-two-char-private" "$double_private" &&
    check_file_missing "qmark-two-char-real" "$double_path" &&
    run_write_case "$scenario" "qmark-single-char-file-write" "$file_single_path" "$PAYLOAD" &&
    check_file_exists "qmark-file-single-char-real" "$file_single_path" &&
    check_file_missing "qmark-file-single-char-private" "$file_single_private"
}

check_fuse_daemon_started() {
  local scenario="$1"
  for _ in 1 2 3 4 5; do
    if adb_su "grep -Eq 'fuse redirect mount start pkg=${APP_ID}|mount request cfg pkg=${APP_ID} fuse_daemon=true|app mount confirmed pid=' '$LOG_PATH' 2>/dev/null"; then
      echo "fuse_daemon_started scenario=${scenario}"
      return 0
    fi
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
  echo "fuse_daemon_missing scenario=${scenario}; continuing with behavioral checks"
  return 0
}

check_scoped_fuse_daemon_started() {
  local scenario="$1"
  local mount_root="$2"
  local strict="${3:-1}"
  local saw_fallback=0
  local saw_session_failed=0
  for _ in $(seq 1 20); do
    if adb_su "grep -F -- 'fuse redirect mount start pkg=${APP_ID}' '$LOG_PATH' 2>/dev/null | grep -F -- 'mp=${mount_root}' >/dev/null"; then
      echo "scoped_fuse_started scenario=${scenario} root=${mount_root}"
      return 0
    fi
    if adb_su "grep -F -- 'daemon hybrid fuse no scoped service mounted' '$LOG_PATH' 2>/dev/null | grep -F -- 'pkg=${APP_ID}' >/dev/null"; then
      saw_fallback=1
    fi
    if adb_su "grep -F -- 'fuse redirect session ended' '$LOG_PATH' 2>/dev/null | grep -F -- 'mp=${mount_root}' >/dev/null"; then
      saw_session_failed=1
    fi
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
  if [ "$strict" != "1" ]; then
    if [ "$saw_fallback" = "1" ]; then
      echo "scoped_fuse_fallback scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
      return 0
    fi
    if [ "$saw_session_failed" = "1" ]; then
      echo "scoped_fuse_session_failed scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
      return 0
    fi
    echo "scoped_fuse_start_log_not_observed scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
    return 0
  fi
  if [ "$saw_fallback" = "1" ]; then
    echo "scoped_fuse_fallback scenario=${scenario} root=${mount_root}" >&2
    return 1
  fi
  if [ "$saw_session_failed" = "1" ]; then
    echo "scoped_fuse_session_failed scenario=${scenario} root=${mount_root}" >&2
    return 1
  fi
  echo "scoped_fuse_missing scenario=${scenario} root=${mount_root}" >&2
  adb_su "grep -F -- '${APP_ID}' '$LOG_PATH' 2>/dev/null | tail -80 || true" | sed 's/^/fuse_tail: /'
  return 1
}

run_fuse_daemon_allow_wildcard_scenario() {
  local scenario="$1"
  local plain_path="$FUSE_PLAIN_ROOT/$TEST_FILE"
  local plain_private="$PRIVATE_FUSE_PLAIN_ROOT/$TEST_FILE"
  local wildcard_path="$FUSE_DCIM_ALLOWED_ROOT/$FUSE_DCIM_MEDIA_FILE"
  local wildcard_private="$PRIVATE_FUSE_DCIM_ALLOWED_ROOT/$FUSE_DCIM_MEDIA_FILE"
  local other_path="$FUSE_DCIM_OTHER_ROOT/$FUSE_DCIM_MEDIA_FILE"
  local other_private="$PRIVATE_FUSE_DCIM_OTHER_ROOT/$FUSE_DCIM_MEDIA_FILE"
  local qmark_path="$FUSE_QMARK_ROOT/Media/$TEST_FILE"
  local qmark_private="$PRIVATE_FUSE_QMARK_ROOT/Media/$TEST_FILE"
  local qmark_miss_path="$FUSE_QMARK_MISS_ROOT/Media/$TEST_FILE"
  local qmark_miss_private="$PRIVATE_FUSE_QMARK_MISS_ROOT/Media/$TEST_FILE"
  local star_media_path="$FUSE_STAR_MEDIA_ROOT/Drop/$FUSE_STAR_MEDIA_FILE"
  local star_media_private="$PRIVATE_FUSE_STAR_MEDIA_ROOT/Drop/$FUSE_STAR_MEDIA_FILE"
  local star_miss_media_path="$FUSE_STAR_MEDIA_ROOT/Other/$FUSE_STAR_MISS_MEDIA_FILE"
  local star_miss_media_private="$PRIVATE_FUSE_STAR_MEDIA_ROOT/Other/$FUSE_STAR_MISS_MEDIA_FILE"
  local qmark_media_path="$FUSE_QMARK_MEDIA_ROOT/Media/$FUSE_QMARK_MEDIA_FILE"
  local qmark_media_private="$PRIVATE_FUSE_QMARK_MEDIA_ROOT/Media/$FUSE_QMARK_MEDIA_FILE"
  local qmark_miss_media_path="$FUSE_QMARK_MISS_ROOT/Media/$FUSE_QMARK_MISS_MEDIA_FILE"
  local qmark_miss_media_private="$PRIVATE_FUSE_QMARK_MISS_ROOT/Media/$FUSE_QMARK_MISS_MEDIA_FILE"

  check_fuse_daemon_started "$scenario" &&
    run_write_case "$scenario" "plain-allow-write" "$plain_path" "$PAYLOAD" &&
    check_file_exists "fuse-plain-real" "$plain_path" &&
    check_file_missing "fuse-plain-private" "$plain_private" &&
    run_mediastore_image_create_case "$scenario" "wildcard-allow-image-create" "$FUSE_DCIM_MEDIA_FILE" "DCIM/SrtFuseQQ/SrtAllowedAlpha" &&
    check_file_exists "fuse-wildcard-real" "$wildcard_path" &&
    check_file_missing "fuse-wildcard-private" "$wildcard_private" &&
    run_mediastore_image_create_case "$scenario" "wildcard-other-image-create" "$FUSE_DCIM_MEDIA_FILE" "DCIM/SrtFuseQQ/SrtOther" &&
    check_file_exists "fuse-wildcard-other-private" "$other_private" &&
    check_file_missing "fuse-wildcard-other-real" "$other_path" &&
    run_write_case "$scenario" "qmark-allow-write" "$qmark_path" "$PAYLOAD" &&
    check_file_exists "fuse-qmark-real" "$qmark_path" &&
    check_file_missing "fuse-qmark-private" "$qmark_private" &&
    run_write_case "$scenario" "qmark-miss-write" "$qmark_miss_path" "$PAYLOAD" &&
    check_file_exists "fuse-qmark-miss-private" "$qmark_miss_private" &&
    check_file_missing "fuse-qmark-miss-real" "$qmark_miss_path" &&
    run_mediastore_download_create_case "$scenario" "fuse-star-media-download-create" "$FUSE_STAR_MEDIA_FILE" "Download/SrtFuseMediaAlpha/Drop" &&
    check_file_exists "fuse-star-media-real" "$star_media_path" &&
    check_file_missing "fuse-star-media-private" "$star_media_private" &&
    run_mediastore_download_create_case "$scenario" "fuse-star-media-miss-download-create" "$FUSE_STAR_MISS_MEDIA_FILE" "Download/SrtFuseMediaAlpha/Other" &&
    check_file_exists "fuse-star-media-miss-private" "$star_miss_media_private" &&
    check_file_missing "fuse-star-media-miss-real" "$star_miss_media_path" &&
    run_mediastore_download_create_case "$scenario" "fuse-qmark-media-download-create" "$FUSE_QMARK_MEDIA_FILE" "Download/SrtFuseQb/Media" &&
    check_file_exists "fuse-qmark-media-real" "$qmark_media_path" &&
    check_file_missing "fuse-qmark-media-private" "$qmark_media_private" &&
    run_mediastore_download_create_case "$scenario" "fuse-qmark-media-miss-download-create" "$FUSE_QMARK_MISS_MEDIA_FILE" "Download/SrtFuseQab/Media" &&
    check_file_exists "fuse-qmark-media-miss-private" "$qmark_miss_media_private" &&
    check_file_missing "fuse-qmark-media-miss-real" "$qmark_miss_media_path"
}

run_fuse_daemon_read_only_exclusion_scenario() {
  local scenario="$1"
  local locked_path="$FUSE_EXCLUDE_ROOT/Locked/$TEST_FILE"
  local writable_path="$FUSE_EXCLUDE_ROOT/Writable/$TEST_FILE"

  check_fuse_daemon_started "$scenario" &&
    run_service_case "$scenario" "read-only-excluded-write" "file_write" '^PASS \[file_write\]' --es file_path "$writable_path" --es payload "$PAYLOAD" --es expected_payload "$PAYLOAD" &&
    check_file_exists "fuse-read-only-excluded-real" "$writable_path" &&
    run_service_case "$scenario" "read-only-locked-write-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$locked_path" --es payload "$PAYLOAD" &&
    check_file_missing "fuse-read-only-locked-real" "$locked_path"
}

run_fuse_daemon_mapping_read_only_scenario() {
  local scenario="$1"
  local rw_request="$FUSE_MAP_RW_REQUEST/$TEST_FILE"
  local rw_target="$FUSE_MAP_RW_TARGET/$TEST_FILE"
  local ro_request="$FUSE_MAP_RO_REQUEST/$TEST_FILE"
  local ro_target="$FUSE_MAP_RO_TARGET/$TEST_FILE"

  check_fuse_daemon_started "$scenario" &&
    run_write_case "$scenario" "mapping-target-excluded-write" "$rw_request" "$PAYLOAD" &&
    check_file_exists "fuse-mapping-rw-target" "$rw_target" &&
    check_file_missing "fuse-mapping-rw-request" "$rw_request" &&
    run_service_case "$scenario" "mapping-target-read-only-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$ro_request" --es payload "$PAYLOAD" &&
    check_file_missing "fuse-mapping-ro-target" "$ro_target" &&
    check_file_missing "fuse-mapping-ro-request" "$ro_request"
}

run_fuse_daemon_multi_wildcard_scenario() {
  local scenario="$1"
  local qq_path="$FUSE_MULTI_ROOT/QQ/$TEST_FILE"
  local qq_private="$PRIVATE_FUSE_MULTI_ROOT/QQ/$TEST_FILE"
  local wechat_path="$FUSE_MULTI_ROOT/WeChat/$TEST_FILE"
  local wechat_private="$PRIVATE_FUSE_MULTI_ROOT/WeChat/$TEST_FILE"
  local locked_path="$FUSE_MULTI_ROOT/Locked/$TEST_FILE"
  local other_path="$FUSE_MULTI_ROOT/Other/$TEST_FILE"
  local other_private="$PRIVATE_FUSE_MULTI_ROOT/Other/$TEST_FILE"

  check_fuse_daemon_started "$scenario" &&
    run_write_case "$scenario" "multi-qq-write" "$qq_path" "$PAYLOAD" &&
    check_file_exists "fuse-multi-qq-real" "$qq_path" &&
    check_file_missing "fuse-multi-qq-private" "$qq_private" &&
    run_write_case "$scenario" "multi-wechat-write" "$wechat_path" "$PAYLOAD" &&
    check_file_exists "fuse-multi-wechat-real" "$wechat_path" &&
    check_file_missing "fuse-multi-wechat-private" "$wechat_private" &&
    run_service_case "$scenario" "multi-locked-write-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$locked_path" --es payload "$PAYLOAD" &&
    check_file_missing "fuse-multi-locked-real" "$locked_path" &&
    run_write_case "$scenario" "multi-other-write" "$other_path" "$PAYLOAD" &&
    check_file_exists "fuse-multi-other-private" "$other_private" &&
    check_file_missing "fuse-multi-other-real" "$other_path"
}

set_mount_namespace_read_only_seed() {
  local root="${BACKEND_ROOT}/Download/SrtMountNsReadOnly"
  adb_su "mkdir -p '$root'; rm -f '$root/write_denied.txt'; printf '%s' '$READ_ONLY_PAYLOAD' > '$root/$READ_ONLY_FILE'; chmod -R 777 '$root' 2>/dev/null || true" >/dev/null
}

run_mount_namespace_allow_wildcard_fallback_scenario() {
  local scenario="$1"
  local control_path="$REAL_ROOT/Download/SrtProbe/$TEST_FILE"
  local control_private="$PRIVATE_ROOT/Download/SrtProbe/$TEST_FILE"
  local star_path="$MOUNT_NS_ALLOW_ROOT/TeamAlpha/Deep/$TEST_FILE"
  local star_private="$PRIVATE_MOUNT_NS_ALLOW_ROOT/TeamAlpha/Deep/$TEST_FILE"
  local qmark_path="$MOUNT_NS_ALLOW_ROOT/Qa/Deep/$TEST_FILE"
  local qmark_private="$PRIVATE_MOUNT_NS_ALLOW_ROOT/Qa/Deep/$TEST_FILE"
  local star_media_path="$MOUNT_NS_ALLOW_ROOT/TeamAlpha/Deep/$MOUNT_NS_STAR_MEDIA_FILE"
  local star_media_private="$PRIVATE_MOUNT_NS_ALLOW_ROOT/TeamAlpha/Deep/$MOUNT_NS_STAR_MEDIA_FILE"
  local qmark_media_path="$MOUNT_NS_ALLOW_ROOT/Qa/Deep/$MOUNT_NS_QMARK_MEDIA_FILE"
  local qmark_media_private="$PRIVATE_MOUNT_NS_ALLOW_ROOT/Qa/Deep/$MOUNT_NS_QMARK_MEDIA_FILE"

  run_write_case "$scenario" "control-private-write" "$control_path" "$PAYLOAD" &&
    check_file_exists "mount-ns-control-private" "$control_private" &&
    check_file_missing "mount-ns-control-real" "$control_path" &&
    run_write_case "$scenario" "star-fallback-write" "$star_path" "$PAYLOAD" &&
    check_file_exists "star-fallback-real" "$star_path" &&
    check_file_missing "star-fallback-private" "$star_private" &&
    run_write_case "$scenario" "qmark-fallback-write" "$qmark_path" "$PAYLOAD" &&
    check_file_exists "qmark-fallback-real" "$qmark_path" &&
    check_file_missing "qmark-fallback-private" "$qmark_private" &&
    run_mediastore_download_create_case "$scenario" "star-fallback-media-create" "$MOUNT_NS_STAR_MEDIA_FILE" "Download/SrtMountNsAllow/TeamAlpha/Deep" &&
    check_file_exists "star-fallback-media-real" "$star_media_path" &&
    check_file_missing "star-fallback-media-private" "$star_media_private" &&
    run_mediastore_download_create_case "$scenario" "qmark-fallback-media-create" "$MOUNT_NS_QMARK_MEDIA_FILE" "Download/SrtMountNsAllow/Qa/Deep" &&
    check_file_exists "qmark-fallback-media-real" "$qmark_media_path" &&
    check_file_missing "qmark-fallback-media-private" "$qmark_media_private"
}

run_mount_namespace_read_only_wildcard_fallback_scenario() {
  local scenario="$1"
  local seed_path="$MOUNT_NS_READ_ONLY_ROOT/$READ_ONLY_FILE"
  local seed_private="$PRIVATE_MOUNT_NS_READ_ONLY_ROOT/$READ_ONLY_FILE"
  local denied_path="$MOUNT_NS_READ_ONLY_ROOT/write_denied.txt"
  local denied_private="$PRIVATE_MOUNT_NS_READ_ONLY_ROOT/write_denied.txt"

  run_service_case "$scenario" "fallback-read" "file_read" '^PASS \[file_read\]' --es file_path "$seed_path" --es expected_payload "$READ_ONLY_PAYLOAD" &&
    check_file_missing "mount-ns-seed-private" "$seed_private" &&
    run_service_case "$scenario" "fallback-write-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$denied_path" --es payload "$PAYLOAD" &&
    check_file_missing "mount-ns-denied-real" "$denied_path" &&
    check_file_missing "mount-ns-denied-private" "$denied_private"
}

run_mount_namespace_mapping_read_only_scenario() {
  local scenario="$1"
  local rw_request="$MOUNT_NS_MAP_RW_REQUEST/$TEST_FILE"
  local rw_target="$MOUNT_NS_MAP_RW_TARGET/$TEST_FILE"
  local ro_request="$MOUNT_NS_MAP_RO_REQUEST/$TEST_FILE"
  local ro_target="$MOUNT_NS_MAP_RO_TARGET/$TEST_FILE"

  run_write_case "$scenario" "mapping-target-write" "$rw_request" "$PAYLOAD" &&
    check_file_exists "mount-ns-mapping-rw-target" "$rw_target" &&
    check_file_missing "mount-ns-mapping-rw-request" "$rw_request" &&
    run_service_case "$scenario" "mapping-target-read-only-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$ro_request" --es payload "$PAYLOAD" &&
    check_file_missing "mount-ns-mapping-ro-target" "$ro_target" &&
    check_file_missing "mount-ns-mapping-ro-request" "$ro_request"
}

monitor_file_name() {
  local scenario="$1"
  local label="$2"
  printf 'srt_monitor_%s_%s.bin' "$scenario" "$label" | tr -c 'A-Za-z0-9_.-' '_'
}

run_file_monitor_write_success_case() {
  local scenario="$1"
  local label="$2"
  local path="$3"
  local expected_path="${4:-$path}"
  local private_path="${5:-}"
  local allow_capacity_limited_miss="${6:-0}"
  local require_monitor_record="${7:-1}"
  local monitor_skip_reason="${8:-ordinary-app-disabled-direct-write}"
  local file_name
  local attempt
  file_name="$(basename "$path")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  for attempt in 1 2; do
    if run_write_case "$scenario" "$label" "$path" "$PAYLOAD" &&
      check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path" &&
      { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path"; } &&
      { [ "$require_monitor_record" != "1" ] || expect_file_monitor_success_record "$scenario" "$label" "$file_name" "$allow_capacity_limited_miss"; }; then
      if [ "$require_monitor_record" != "1" ]; then
        echo "monitor_success_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=${monitor_skip_reason}"
      fi
      return 0
    fi
    [ "$attempt" -lt 2 ] || break
    echo "file_monitor_write_success_retry scenario=${scenario} label=${label} attempt=${attempt}"
    ensure_current_app_mount_confirmed "scenario-${scenario}-${label}-retry" || return 1
    wait_storage_ready "scenario-${scenario}-${label}-retry" 30 >/dev/null || return 1
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
  return 1
}

run_file_monitor_write_denied_case() {
  local scenario="$1"
  local label="$2"
  local path="$3"
  local missing_path="${4:-$path}"
  local file_name
  local attempt
  file_name="$(basename "$path")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  for attempt in 1 2; do
    if run_service_case "$scenario" "$label" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$path" --es payload "$PAYLOAD" &&
      check_file_missing "scenario-${scenario}-${label}-missing" "$missing_path"; then
      echo "monitor_failure_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=ordinary-app-inotify"
      return 0
    fi
    [ "$attempt" -lt 2 ] || break
    echo "file_monitor_write_denied_retry scenario=${scenario} label=${label} attempt=${attempt}"
    ensure_current_app_mount_confirmed "scenario-${scenario}-${label}-retry" || return 1
    wait_storage_ready "scenario-${scenario}-${label}-retry" 30 >/dev/null || return 1
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
  return 1
}

run_file_monitor_mediastore_success_case() {
  local scenario="$1"
  local label="$2"
  local relative_path="$3"
  local expected_path="$4"
  local private_path="${5:-}"
  local require_monitor_record="${6:-1}"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_download_create_case "$scenario" "$label" "$file_name" "$relative_path" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path/$file_name" &&
    { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path/$file_name"; } &&
    { [ "$require_monitor_record" != "1" ] || expect_file_monitor_success_record "$scenario" "$label" "$file_name"; } &&
    { [ "$require_monitor_record" = "1" ] || echo "monitor_success_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=disabled-profile-mediastore-create"; }
}

run_file_monitor_mediastore_relative_data_success_case() {
  local scenario="$1"
  local label="$2"
  local relative_data_dir="$3"
  local expected_path="$4"
  local private_path="${5:-}"
  local require_monitor_record="${6:-1}"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label" | sed 's/\.bin$/.jpg/')"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_image_relative_data_create_case "$scenario" "$label" "$file_name" "$relative_data_dir" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path/$file_name" &&
    { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path/$file_name"; } &&
    expect_no_read_only_failure_record "$scenario" "$label" "$file_name" &&
    { [ "$require_monitor_record" != "1" ] || expect_file_monitor_success_record "$scenario" "$label" "$file_name"; } &&
    { [ "$require_monitor_record" = "1" ] || echo "monitor_success_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=disabled-profile-mediastore-relative-data"; }
}

run_file_monitor_mediastore_denied_case() {
  local scenario="$1"
  local label="$2"
  local relative_path="$3"
  local missing_path="$4"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_download_create_denied_case "$scenario" "$label" "$file_name" "$relative_path" &&
    check_file_missing "scenario-${scenario}-${label}-missing" "$missing_path/$file_name" &&
    expect_file_monitor_failure_record "$scenario" "$label" "$file_name"
}

run_file_monitor_disabled_redirect_scenario() {
  local scenario="$1"
  local file_name
  file_name="$(monitor_file_name "$scenario" "disabled_regular")"
  run_file_monitor_write_success_case "$scenario" "disabled-regular-write" "$MONITOR_BASE_ROOT/$file_name" "$MONITOR_BASE_ROOT/$file_name" "$PRIVATE_MONITOR_BASE_ROOT/$file_name" 1 0 &&
    run_file_monitor_mediastore_success_case "$scenario" "disabled-system-writer-create" "Download/SrtMonitor" "$MONITOR_BASE_ROOT" "$PRIVATE_MONITOR_BASE_ROOT" 0 &&
    run_file_monitor_mediastore_relative_data_success_case "$scenario" "disabled-nnngram-relative-data" "/Pictures/Nnngram" "$MONITOR_NNNGRAM_ROOT" "$PRIVATE_MONITOR_NNNGRAM_ROOT" 0
}

run_file_monitor_regular_scenario() {
  local scenario="$1"
  local allow_file map_file locked_file writable_file
  allow_file="$(monitor_file_name "$scenario" "regular_allow")"
  map_file="$(monitor_file_name "$scenario" "regular_map")"
  locked_file="$(monitor_file_name "$scenario" "regular_locked")"
  writable_file="$(monitor_file_name "$scenario" "regular_writable")"

  if [ "$scenario" = "25" ]; then
    run_file_monitor_write_success_case "$scenario" "regular-allow-write" "$MONITOR_BASE_ROOT/$allow_file" "$MONITOR_BASE_ROOT/$allow_file" "$PRIVATE_MONITOR_BASE_ROOT/$allow_file" 1 0 "ordinary-app-scoped-fuse-direct-write" || return 1
    check_scoped_fuse_daemon_started "$scenario" "$MONITOR_LOCKED_ROOT" || return 1
  else
    echo "regular_allow_write_skipped scenario=${scenario} reason=mount-namespace-allowed-real-direct-write-is-platform-permission-sensitive"
  fi

  run_file_monitor_write_success_case "$scenario" "regular-mapped-write" "$MONITOR_MAP_REQUEST/$map_file" "$MONITOR_MAP_TARGET/$map_file" "" 0 0 "ordinary-app-direct-mapped-write" &&
    run_file_monitor_write_denied_case "$scenario" "regular-read-only-denied" "$MONITOR_LOCKED_ROOT/$locked_file" "$MONITOR_LOCKED_ROOT/$locked_file" &&
    run_file_monitor_write_success_case "$scenario" "regular-read-only-excluded-write" "$MONITOR_WRITABLE_ROOT/$writable_file" "$MONITOR_WRITABLE_ROOT/$writable_file" "$PRIVATE_MONITOR_WRITABLE_ROOT/$writable_file" 1 0 "ordinary-app-read-only-exclusion-direct-write"
}

run_file_monitor_mediastore_scenario() {
  local scenario="$1"
  restart_media_provider
  wait_storage_ready "scenario-${scenario}-mediastore-storage" 60 >/dev/null || return 1
  wait_media_provider_ready "scenario-${scenario}-mediastore-provider" 120 >/dev/null || return 1
  run_file_monitor_mediastore_success_case "$scenario" "media-allow-create" "Download/SrtMonitor" "$MONITOR_BASE_ROOT" "$PRIVATE_MONITOR_BASE_ROOT" &&
    { [ "$scenario" != "27" ] || check_scoped_fuse_daemon_started "$scenario" "$MONITOR_LOCKED_ROOT"; } &&
    run_file_monitor_mediastore_relative_data_success_case "$scenario" "media-relative-data-create" "Pictures/SrtRelativeData" "$MONITOR_RELATIVE_DATA_ROOT" "$PRIVATE_MONITOR_RELATIVE_DATA_ROOT" &&
    run_file_monitor_mediastore_relative_data_success_case "$scenario" "media-nnngram-relative-data" "/Pictures/Nnngram" "$MONITOR_NNNGRAM_ROOT" "$PRIVATE_MONITOR_NNNGRAM_ROOT" &&
    run_file_monitor_mediastore_success_case "$scenario" "media-mapped-create" "Download/SrtMonitorMap" "$MONITOR_MAP_TARGET" &&
    run_file_monitor_mediastore_denied_case "$scenario" "media-read-only-denied" "Download/SrtMonitorLocked" "$MONITOR_LOCKED_ROOT" &&
    run_file_monitor_mediastore_success_case "$scenario" "media-read-only-excluded-create" "Download/SrtMonitorLocked/Writable" "$MONITOR_WRITABLE_ROOT" "$PRIVATE_MONITOR_WRITABLE_ROOT"
}

app_pid() {
  adb shell "pidof '$APP_ID' 2>/dev/null | awk '{print \$1}'" | tr -d '\r' | tail -1
}

resume_hot_reload_app() {
  local scenario="$1"
  local expected_pid="$2"
  local current_pid
  adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null
  wait_storage_ready "scenario-${scenario}-hot-reload-resume" 30 >/dev/null
  current_pid="$(app_pid)"
  if [ -z "$current_pid" ] || [ "$current_pid" != "$expected_pid" ]; then
    echo "config_hot_reload_pid_changed scenario=${scenario} before=${expected_pid} after=${current_pid:-missing}" >&2
    return 1
  fi
}

run_config_hot_reload_scenario() {
  local scenario="$1"
  local previous_fresh_app_per_case="$SRT_FRESH_APP_PER_CASE"
  SRT_FRESH_APP_PER_CASE=0
  trap 'SRT_FRESH_APP_PER_CASE="$previous_fresh_app_per_case"; trap - RETURN' RETURN

  local initial_pid before_request before_private after_request after_mapped after_private attempt current_pid
  initial_pid="$(app_pid)"
  if [ -z "$initial_pid" ]; then
    echo "config_hot_reload_pid_missing: app is not running before scenario $scenario" >&2
    return 1
  fi

  before_request="${REAL_ROOT}/Download/SrtProbe/${HOT_BEFORE_FILE}"
  before_private="${PRIVATE_ROOT}/Download/SrtProbe/${HOT_BEFORE_FILE}"
  after_request="${REAL_ROOT}/Download/SrtProbe/${HOT_AFTER_FILE}"
  after_mapped="${REAL_ROOT}/Download/Test/${HOT_AFTER_FILE}"
  after_private="${PRIVATE_ROOT}/Download/SrtProbe/${HOT_AFTER_FILE}"

  resume_hot_reload_app "$scenario" "$initial_pid" || {
    return 1
  }
  run_write_case "$scenario" "hot-initial-private" "$before_request" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-initial-private" "$before_private" &&
    check_file_missing "scenario-${scenario}-initial-real" "$before_request" || {
      return 1
    }

  echo "config_hot_reload_update scenario=${scenario}: switch default redirect to path mapping without restarting app"
  write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}'
  wait_config_applied "scenario-${scenario}-hot-reload"

  for attempt in $(seq 1 20); do
    current_pid="$(app_pid)"
    if [ -z "$current_pid" ] || [ "$current_pid" != "$initial_pid" ]; then
      echo "config_hot_reload_pid_changed scenario=${scenario} before=${initial_pid} after=${current_pid:-missing}" >&2
      return 1
    fi

    adb_su "rm -f '$after_request' '$after_mapped' '$after_private' 2>/dev/null || true" >/dev/null
    resume_hot_reload_app "$scenario" "$initial_pid" || {
      return 1
    }
    if run_write_case "$scenario" "hot-update-mapped-${attempt}" "$after_request" "$PAYLOAD" &&
      check_file_exists "scenario-${scenario}-hot-mapped" "$after_mapped" &&
      check_file_missing "scenario-${scenario}-hot-request" "$after_request" &&
      check_file_missing "scenario-${scenario}-hot-private" "$after_private"; then
      current_pid="$(app_pid)"
      if [ -z "$current_pid" ] || [ "$current_pid" != "$initial_pid" ]; then
        echo "config_hot_reload_pid_changed_after_apply scenario=${scenario} before=${initial_pid} after=${current_pid:-missing}" >&2
        return 1
      fi
      echo "config_hot_reload_applied scenario=${scenario} pid=${initial_pid} attempt=${attempt}"
      return 0
    fi
    sleep 1
  done

  echo "config_hot_reload_timeout scenario=${scenario} pid=${initial_pid}" >&2
  return 1
}

check_health() {
  wait_storage_ready "health" 30 >/dev/null || return 1
  wait_media_provider_ready "health" 60 >/dev/null || return 1
  adb shell "count=\$(ps -A | grep -c 'com.google.android.providers.media.module' || true); echo media_count=\$count; ps -A | grep 'com.google.android.providers.media.module' || true; pid=\$(pidof com.google.android.providers.media.module 2>/dev/null || true); echo media_pid=\$pid; if [ -n \"\$pid\" ]; then echo threads=\$(ls /proc/\$pid/task 2>/dev/null | wc -l); ps -A -o PID,RSS,NAME | grep 'com.google.android.providers.media.module' || true; fi" | tee media-health.txt
  local count
  count="$(sed -n 's/^media_count=//p' media-health.txt | tail -1)"
  [ -z "$count" ] || [ "$count" -le 10 ]
}

print_diagnostics() {
  local scenario="$1"
  echo "=== scenario ${scenario} diagnostics ==="
  print_storage_state "scenario-${scenario}-failure"
  adb_su "echo ===global_config===; cat '$GLOBAL_CONFIG' 2>/dev/null || true; echo; echo ===app_config===; cat '$CONFIG' 2>/dev/null || true; echo; echo ===module_state===; ls -la /data/adb/modules/storage.redirect.x 2>/dev/null || true; echo; mount | grep -E 'srx|storage.redirect|fuse' || true; echo; echo ===logs===; for log in running.log app_status.log file_monitor.log media_provider_state.log; do echo ---\$log---; tail -80 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true; done; echo ===files===; for dir in '${REAL_ROOT}/Download' '${REAL_ROOT}/Pictures' '${REAL_ROOT}/DCIM' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${PRIVATE_ROOT}/Download' '${PRIVATE_ROOT}/Pictures' '${PRIVATE_ROOT}/DCIM' '${PRIVATE_ROOT}/.xldownload' '${PRIVATE_ROOT}/.xlDownload'; do find \"\$dir\" -maxdepth 5 \\( -name '$TEST_FILE' -o -name '$HOT_BEFORE_FILE' -o -name '$HOT_AFTER_FILE' -o -name '$READ_ONLY_FILE' -o -name '$ALLOW_KEEP_FILE' -o -name '$ALLOW_PART_FILE' \\) -printf '%p %s %u:%g\\n' 2>/dev/null || true; done | sort; echo ===results===; cat '$RESULT_DIR'/result_*.txt '$INTERNAL_RESULT_DIR'/result_*.txt '$BACKEND_RESULT_DIR'/result_*.txt '$SANDBOX_RESULT_DIR'/result_*.txt 2>/dev/null || true" || true
  adb logcat -d -t 1200 | grep -Ei 'StorageRedirectTest|srx|StorageRedirect|Magisk|zygisk|FATAL EXCEPTION|AndroidRuntime|PhantomProcessRecord|ExternalStorage|StorageManager|MediaProvider|vold|sdcard|fuse|Transport endpoint' | tail -260 || true
}

capture_test_flow_artifacts() {
  adb logcat -d >test-flow-logcat.txt 2>/dev/null || true
  adb_su "echo ===global_config===; cat '$GLOBAL_CONFIG' 2>/dev/null || true; echo; echo ===app_config===; cat '$CONFIG' 2>/dev/null || true; echo; echo ===module_state===; ls -la /data/adb/modules/storage.redirect.x 2>/dev/null || true; echo; mount | grep -E 'srx|storage.redirect|fuse' || true; echo; echo ===logs===; for log in running.log app_status.log file_monitor.log media_provider_state.log; do echo ---\$log---; if [ \"\$log\" = file_monitor.log ]; then tail -1000 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true; else tail -240 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true; fi; done" >test-flow-module-state.txt 2>/dev/null || true
  {
    echo "===app_pids==="
    adb shell "pidof '$APP_ID' 2>/dev/null || true"
    for pid in $(adb shell "pidof '$APP_ID' 2>/dev/null" | tr -d '\r'); do
      echo "--- /proc/${pid}/mountinfo ---"
      adb_su "cat '/proc/${pid}/mountinfo' 2>/dev/null | grep -E 'SrtProbe|Download/Test|SrtMonitor|/storage|/mnt/runtime|/mnt/user|/mnt/installer|/mnt/androidwritable|/mnt/pass_through|fuse|srx' || true"
    done
  } >test-flow-app-mountinfo.txt 2>/dev/null || true
}

run_standard_scenario() {
  local scenario="$1"
  echo "step 5/7: 从应用进程写入文件"
  if ! run_write_test "$scenario"; then
    return 1
  fi
  echo "step 6/7: 校验应用视角可见文件"
  if ! check_app_view "$scenario"; then
    return 1
  fi
  echo "step 7/7: 校验 root 视角物理落点"
  if ! check_file_location "$scenario"; then
    return 1
  fi
}

run_scenario() {
  local scenario="$1"
  local targets_prepared_before_start=0
  : >"scenario-${scenario}-result.txt"
  echo "step 1/7: 应用场景配置"
  adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null
  apply_config "$scenario"
  if [ "$scenario" != "1" ]; then
    wait_config_applied "scenario-${scenario}"
  fi
  case "$scenario" in
    9)
      echo "step 2/7: 清理并预置只读源文件"
      clean_targets
      seed_read_only_targets
      targets_prepared_before_start=1
      ;;
    28)
      echo "step 2/7: 清理并预置只读媒体源文件"
      clean_targets
      prepare_read_only_media_image
      targets_prepared_before_start=1
      ;;
    20|21|22)
      echo "step 2/7: 清理并预置 mount namespace 回退目标"
      clean_targets
      if [ "$scenario" = "21" ]; then
        set_mount_namespace_read_only_seed
      fi
      targets_prepared_before_start=1
      ;;
  esac
  if [ "$targets_prepared_before_start" -eq 0 ]; then
    echo "step 2/7: 清理并预置测试目标"
    clean_targets
    targets_prepared_before_start=1
  fi
  echo "step 3/7: 重启测试应用"
  adb shell am force-stop "$APP_ID" >/dev/null || true
  local expect_mount=1
  if ! label_expects_mount "scenario-${scenario}"; then
    expect_mount=0
  fi
  start_app_and_confirm_mount "scenario-${scenario}" "$expect_mount" || return 1
  echo "step 4/7: 等待共享存储可用"
  wait_storage_ready "scenario-${scenario}"
  clean_results
  case "$scenario" in
    9)
      echo "step 5/7: 执行只读路径读取和拒绝类用例"
      run_read_only_scenario "$scenario"
      ;;
    10)
      echo "step 5/7: 预置映射只读路径并执行拒绝类用例"
      run_mapped_read_only_scenario "$scenario"
      ;;
    11)
      echo "step 5/7: 执行放行、排除目录写入和通配符排除创建"
      run_allow_exclusion_scenario "$scenario"
      ;;
    12)
      echo "step 5/7: 执行旧 excluded_real_paths 字段兼容验证"
      run_legacy_exclusion_scenario "$scenario"
      ;;
    13)
      echo "step 5/7: 执行问号通配符放行验证"
      run_qmark_wildcard_scenario "$scenario"
      ;;
    16)
      echo "step 5/7: 执行 FUSE 普通放行与通配符放行混合验证"
      run_fuse_daemon_allow_wildcard_scenario "$scenario"
      ;;
    17)
      echo "step 5/7: 执行 FUSE 只读路径 ! 排除优先验证"
      run_fuse_daemon_read_only_exclusion_scenario "$scenario"
      ;;
    18)
      echo "step 5/7: 执行 FUSE 映射最终目标只读判定验证"
      run_fuse_daemon_mapping_read_only_scenario "$scenario"
      ;;
    19)
      echo "step 5/7: 执行 FUSE 同父级多通配符规则验证"
      run_fuse_daemon_multi_wildcard_scenario "$scenario"
      ;;
    20)
      echo "step 5/7: 执行默认 mount namespace 允许路径通配符回退验证"
      run_mount_namespace_allow_wildcard_fallback_scenario "$scenario"
      ;;
    21)
      echo "step 5/7: 执行默认 mount namespace 只读路径通配符回退验证"
      run_mount_namespace_read_only_wildcard_fallback_scenario "$scenario"
      ;;
    22)
      echo "step 5/7: 执行默认 mount namespace 映射最终目标只读判定验证"
      run_mount_namespace_mapping_read_only_scenario "$scenario"
      ;;
    28)
      echo "step 5/7: 执行 MediaStore 只读真实图片路径查询可见性验证"
      run_mediastore_read_only_query_scenario "$scenario"
      ;;
    29)
      echo "step 5/7: 修改配置并验证运行中应用热更新"
      run_config_hot_reload_scenario "$scenario"
      ;;
    23)
      echo "step 5/7: 执行未启用重定向普通应用与系统代写文件监视记录验证"
      run_file_monitor_disabled_redirect_scenario "$scenario"
      ;;
    24|25)
      echo "step 5/7: 执行普通应用文件监视记录矩阵验证"
      run_file_monitor_regular_scenario "$scenario"
      ;;
    26|27)
      echo "step 5/7: 执行系统代写文件监视记录矩阵验证"
      run_file_monitor_mediastore_scenario "$scenario"
      ;;
    *)
      run_standard_scenario "$scenario"
      ;;
  esac
}

cleanup_done=0
global_config_backup_ready=0
app_config_backup_ready=0
if [ "${SRT_SKIP_FINAL_CLEANUP:-0}" != "1" ]; then
  trap cleanup_test_artifacts EXIT
fi

wait_boot_completed
backup_global_config
backup_app_config
adb shell pm grant "$APP_ID" android.permission.READ_EXTERNAL_STORAGE >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.WRITE_EXTERNAL_STORAGE >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_IMAGES >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_VIDEO >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_AUDIO >/dev/null 2>&1 || true
fix_private_backend_permissions || true
restart_media_provider
wait_storage_ready "initial"
wait_media_provider_ready "initial"
adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null

fail=0
build_scenario_list

export APP_ID CONFIG GLOBAL_CONFIG LOG_PATH FILE_MONITOR_LOG_PATH ACTION RESULT_DIR INTERNAL_RESULT_DIR REAL_ROOT BACKEND_ROOT PRIVATE_ROOT BACKEND_PRIVATE_ROOT BACKEND_RESULT_DIR SANDBOX_RESULT_DIR TEST_FILE HOT_BEFORE_FILE HOT_AFTER_FILE READ_ONLY_FILE ALLOW_KEEP_FILE ALLOW_PART_FILE QMARK_SINGLE_FILE QMARK_DOUBLE_FILE QMARK_FILE_SINGLE_FILE MOUNT_NS_STAR_MEDIA_FILE MOUNT_NS_QMARK_MEDIA_FILE FUSE_STAR_MEDIA_FILE FUSE_STAR_MISS_MEDIA_FILE FUSE_QMARK_MEDIA_FILE FUSE_QMARK_MISS_MEDIA_FILE FUSE_DCIM_MEDIA_FILE READ_ONLY_HARDLINK READ_ONLY_SYMLINK READ_ONLY_IMAGE_FILE PAYLOAD READ_ONLY_PAYLOAD READ_ONLY_IMAGE_B64 READ_ONLY_ROOT BACKEND_READ_ONLY_ROOT READ_ONLY_MEDIA_ROOT PRIVATE_READ_ONLY_MEDIA_ROOT MAPPED_READ_ONLY_REQUEST MAPPED_READ_ONLY_TARGET ALLOW_ROOT PRIVATE_ALLOW_ROOT LEGACY_ROOT PRIVATE_LEGACY_ROOT QMARK_ROOT PRIVATE_QMARK_ROOT FUSE_PLAIN_ROOT PRIVATE_FUSE_PLAIN_ROOT FUSE_DCIM_ROOT PRIVATE_FUSE_DCIM_ROOT FUSE_DCIM_OTHER_ROOT PRIVATE_FUSE_DCIM_OTHER_ROOT FUSE_QMARK_ROOT PRIVATE_FUSE_QMARK_ROOT FUSE_QMARK_MISS_ROOT PRIVATE_FUSE_QMARK_MISS_ROOT FUSE_QMARK_MEDIA_ROOT PRIVATE_FUSE_QMARK_MEDIA_ROOT FUSE_STAR_MEDIA_ROOT PRIVATE_FUSE_STAR_MEDIA_ROOT FUSE_EXCLUDE_ROOT PRIVATE_FUSE_EXCLUDE_ROOT FUSE_MAP_PARENT FUSE_MAP_RW_REQUEST FUSE_MAP_RO_REQUEST FUSE_MAP_RW_TARGET FUSE_MAP_RO_TARGET FUSE_MULTI_ROOT PRIVATE_FUSE_MULTI_ROOT MOUNT_NS_ALLOW_ROOT PRIVATE_MOUNT_NS_ALLOW_ROOT MOUNT_NS_READ_ONLY_ROOT PRIVATE_MOUNT_NS_READ_ONLY_ROOT MOUNT_NS_MAP_PARENT MOUNT_NS_MAP_RW_REQUEST MOUNT_NS_MAP_RO_REQUEST MOUNT_NS_MAP_RW_TARGET MOUNT_NS_MAP_RO_TARGET MONITOR_BASE_ROOT PRIVATE_MONITOR_BASE_ROOT MONITOR_MAP_REQUEST MONITOR_MAP_TARGET MONITOR_LOCKED_ROOT MONITOR_WRITABLE_ROOT PRIVATE_MONITOR_WRITABLE_ROOT MONITOR_RELATIVE_DATA_ROOT PRIVATE_MONITOR_RELATIVE_DATA_ROOT SRT_FRESH_APP_PER_CASE SRT_RESULT_POLL_MS SRT_APP_LAUNCH_SETTLE_MS SRT_MOUNT_CONFIRM_TIMEOUT_MS SRT_APP_MOUNT_CONFIRM_RETRIES SRT_CONFIG_APPLY_TIMEOUT_MS SRT_SERVICE_CASE_SETTLE_MS SRT_FILE_MONITOR_ENABLED SRT_FAIL_FAST SRT_SCENARIO_TIMEOUT_SECONDS LAST_MOUNT_CONFIRMED_PID ADB_ROOT_MODE
export -f detect_adb_root_mode adb_root adb_su adb_write_file test_app_uid fix_private_backend_permissions wait_boot_completed restart_media_provider write_config write_global_config test_global_config enable_fuse_daemon_config disable_fuse_daemon_config use_mount_namespace_fallback_config apply_config target_path logical_dir expected_path scenario_title clean_targets clean_results latest_result wait_service_result wait_app_mount_confirmed scenario_from_label label_expects_mount expected_mount_paths_for_label app_mountinfo_has_expected_paths ensure_current_app_mount_confirmed wait_config_applied service_case_timeout_seconds sleep_ms prepare_service_case start_app_and_confirm_mount wait_storage_ready media_provider_query_ready wait_media_provider_ready print_storage_state run_service_case run_write_case run_create_case run_mediastore_download_create_case run_mediastore_image_create_case run_mediastore_image_relative_data_create_case run_mediastore_download_create_denied_case run_write_test check_app_view expect_app_entry expect_no_app_entry find_written_file check_file_exists check_file_missing check_file_location seed_read_only_targets check_read_only_artifacts run_read_only_scenario wait_mediastore_read_only_image prepare_read_only_media_image run_mediastore_read_only_query_scenario prepare_mapped_read_only_targets run_mapped_read_only_scenario run_allow_exclusion_scenario run_legacy_exclusion_scenario run_qmark_wildcard_scenario check_fuse_daemon_started check_scoped_fuse_daemon_started run_fuse_daemon_allow_wildcard_scenario run_fuse_daemon_read_only_exclusion_scenario run_fuse_daemon_mapping_read_only_scenario run_fuse_daemon_multi_wildcard_scenario set_mount_namespace_read_only_seed run_mount_namespace_allow_wildcard_fallback_scenario run_mount_namespace_read_only_wildcard_fallback_scenario run_mount_namespace_mapping_read_only_scenario ensure_monitor_collector clear_file_monitor_log file_monitor_watch_capacity_limited assert_file_monitor_enabled_for_scenario prepare_file_monitor_assertion wait_file_monitor_log_line expect_file_monitor_success_record expect_file_monitor_failure_record expect_no_read_only_failure_record monitor_file_name run_file_monitor_write_success_case run_file_monitor_write_denied_case run_file_monitor_mediastore_success_case run_file_monitor_mediastore_relative_data_success_case run_file_monitor_mediastore_denied_case run_file_monitor_disabled_redirect_scenario run_file_monitor_regular_scenario run_file_monitor_mediastore_scenario app_pid resume_hot_reload_app run_config_hot_reload_scenario check_health print_diagnostics capture_test_flow_artifacts run_standard_scenario run_scenario

for scenario in "${scenarios[@]}"; do
  echo "::group::scenario ${scenario}: $(scenario_title "$scenario")"
  if ! timeout --foreground "${SRT_SCENARIO_TIMEOUT_SECONDS}s" bash -c 'run_scenario "$1"' _ "$scenario"; then
    echo "scenario ${scenario}: failed or timed out"
    timeout --foreground 90s bash -c 'print_diagnostics "$1"' _ "$scenario" || true
    fail=1
    if [ "$SRT_FAIL_FAST" = "1" ]; then
      echo "SRT_FAIL_FAST=1: stop after first failed scenario in this shard"
      echo "::endgroup::"
      break
    fi
  fi
  echo "::endgroup::"
done
check_health || fail=1
capture_test_flow_artifacts

if [ "${SRT_SKIP_FINAL_CLEANUP:-0}" != "1" ]; then
  trap - EXIT
  cleanup_test_artifacts
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

exit 0
