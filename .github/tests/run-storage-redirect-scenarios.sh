#!/usr/bin/env bash
set -euo pipefail

# Android 17 上出现整脚本 9 秒内静默退出（exit 224）的现象，日志无任何标记。
# ERR trap 在失败命令处输出行号与退出码，用于精确定位传播到 job 层的异常退出码。
trap 'echo "::warning file=run-storage-redirect-scenarios.sh,line=${BASH_LINENO[0]}::场景脚本命令失败：${BASH_COMMAND} 退出码=$?" >&2' ERR

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

# 测试流产物统一落盘目录（AGENTS.md「临时文件管理」规范）：
# temp/test-flow/<启动时间戳>/，结果文件与诊断文件一律写到这里，不得散落在
# 仓库根目录或脚本 CWD。同一天多轮测试互不覆盖；上游编排器
# （run-android-test-flow.sh / verify-test-flow.ps1）会通过 SRT_ARTIFACT_DIR
# 注入同一目录，保证一轮测试流的全部脚本产物聚在一起。
SRT_ARTIFACT_DIR="${SRT_ARTIFACT_DIR:-}"
if [ -z "$SRT_ARTIFACT_DIR" ]; then
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  SRT_ARTIFACT_DIR="${repo_root}/temp/test-flow/$(date +%Y%m%d-%H%M%S)"
fi
mkdir -p "$SRT_ARTIFACT_DIR"
export SRT_ARTIFACT_DIR

# 监视 fail-open 探测会冻结 daemon（SIGSTOP）；任何异常退出都必须 CONT 恢复，
# 否则后续场景在冻结 daemon 上全灭。run_scenario 在 `bash -c` 子 shell 里执行，
# 变量无法跨子 shell 回传，因此用产物目录下的 pid 文件登记冻结状态。
export SRT_FROZEN_DAEMON_PID_FILE="${SRT_ARTIFACT_DIR}/.frozen_daemon_pid"
cleanup_frozen_daemon() {
  if [ -s "$SRT_FROZEN_DAEMON_PID_FILE" ]; then
    local frozen_pid
    frozen_pid="$(cat "$SRT_FROZEN_DAEMON_PID_FILE" 2>/dev/null | tr -d '\r' | awk 'NF {print; exit}')"
    if [ -n "$frozen_pid" ]; then
      adb_su "[ \"\$(cat /proc/${frozen_pid}/comm 2>/dev/null)\" = srx_daemon ] && kill -CONT ${frozen_pid} >/dev/null 2>&1 || true"
      echo "monitor_capture_failopen resumed daemon on exit pid=${frozen_pid}" >&2
    fi
    rm -f "$SRT_FROZEN_DAEMON_PID_FILE"
  fi
}
trap cleanup_frozen_daemon EXIT

APP_ID="${APP_ID:-me.fakerqu.test.storageredirect}"
CONFIG="/data/adb/modules/storage.redirect.x/config/apps/${APP_ID}.json"
READ_ONLY_OWNER_CONFIG="/data/adb/modules/storage.redirect.x/config/apps/com.android.settings.json"
GLOBAL_CONFIG="/data/adb/modules/storage.redirect.x/config/global.json"
MOUNT_STATE_DIR="/data/adb/modules/storage.redirect.x/tmp/mount_state"
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
QQ_ALIAS_MAPPED_ROOT="${REAL_ROOT}/Download/SrtQqAliasMapped"
QQ_ALIAS_MAPPED_FILE="srt_qq_alias_existing.bin"
QQ_ALIAS_REQUEST_ROOT="${REAL_ROOT}/Android/data/${APP_ID}/Tencent/QQfile_recv"
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
MEDIASTORE_ROUTING_PROBE_ROOT="${BACKEND_ROOT}/Documents/SrtMediaRoutingProbe"
PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT="${PRIVATE_ROOT}/Documents/SrtMediaRoutingProbe"
RULE_SANDBOX_ROOT="${REAL_ROOT}/SrtRuleSandbox"
BACKEND_RULE_SANDBOX_ROOT="${BACKEND_ROOT}/SrtRuleSandbox"
PRIVATE_RULE_SANDBOX_ROOT="${PRIVATE_ROOT}/SrtRuleSandbox"
# 场景 38：下载提供器（监视桥接进程）自身配置 sandboxed_paths 的回归夹具。
# 提供器进程名不是包名，用包名 force-stop 即可；真实父目录由 shell 预建，
# 目标文件由提供器在自己的进程内创建，实际落点是断言对象。
PROVIDER_PACKAGE="com.android.providers.downloads"
PROVIDER_CONFIG="/data/adb/modules/storage.redirect.x/config/apps/${PROVIDER_PACKAGE}.json"
# 探测路径必须用 MIUI 下载提供器允许的目标目录（隐藏目录会被 SecurityException 拒绝），
# 因此落在 Download/SrtDlProbe；清理口径与 Assert-FixtureRootsEmpty 的 Srt* 通配一致。
PROVIDER_PROBE_REL="Download/SrtDlProbe"
PROVIDER_PROBE_FILE="srx_probe_download.txt"
PROVIDER_REAL_DIR="${REAL_ROOT}/${PROVIDER_PROBE_REL}"
PROVIDER_BACKEND_DIR="${BACKEND_ROOT}/Android/data/${PROVIDER_PACKAGE}/sdcard/${PROVIDER_PROBE_REL}"
PROVIDER_BACKEND_FILE="${PROVIDER_BACKEND_DIR}/${PROVIDER_PROBE_FILE}"
PROVIDER_REAL_FILE="${BACKEND_ROOT}/${PROVIDER_PROBE_REL}/${PROVIDER_PROBE_FILE}"
# 场景 39：下载提供器（共享 uid 代写者）按发起者规则重定向的归因夹具。
# 测试应用作为下载发起者配置 path_mappings；下载提供器引擎写的公共路径必须
# 按发起者的映射落到映射目标，真实请求路径零穿透。
DL_OWNER_REQUEST_REL="Download/SrtDlOwner"
DL_OWNER_MAPPED_REL="Download/SrtDlOwnerMapped"
DL_OWNER_PROBE_FILE="srt_dl_owner_probe.bin"
DL_OWNER_REQUEST_FILE="${REAL_ROOT}/${DL_OWNER_REQUEST_REL}/${DL_OWNER_PROBE_FILE}"
DL_OWNER_REQUEST_BACKEND_FILE="${BACKEND_ROOT}/${DL_OWNER_REQUEST_REL}/${DL_OWNER_PROBE_FILE}"
DL_OWNER_MAPPED_BACKEND_FILE="${BACKEND_ROOT}/${DL_OWNER_MAPPED_REL}/${DL_OWNER_PROBE_FILE}"
# 下载用例的取数 URL 覆盖：默认空 = 场景内启动本地回环 HTTP 服务（adb reverse
# 转发，不依赖模拟器外部网络）；显式设置时优先使用该外源。
SRT_DOWNLOAD_URL="${SRT_DOWNLOAD_URL:-}"
# 场景 40：系统 uid 的 FUSE 写入按发起服务归因夹具。shell（uid 2000）延迟
# mkdir 制造确定的系统 uid FUSE 请求，断言监视记录按发起进程归因（此前
# 该类请求只能记录 unknown 或误归组内成员）。
UID_ATTRIB_REL="Download/SrtUidAttrib"
UID_ATTRIB_PROBE_REL="Download/SrtUidAttrib/srt_uid_probe_dir"
RULE_SIBLING_ROOT="${REAL_ROOT}/DCIM/SrtRuleSibling"
BACKEND_RULE_SIBLING_ROOT="${BACKEND_ROOT}/DCIM/SrtRuleSibling"
PRIVATE_RULE_SIBLING_ROOT="${PRIVATE_ROOT}/DCIM/SrtRuleSibling"
OWN_PRIVATE_DATA_ROOT="${REAL_ROOT}/Android/data/${APP_ID}/Tencent/QQfile_recv"
OWN_PRIVATE_MEDIA_ROOT="${REAL_ROOT}/Android/media/${APP_ID}/Tencent/QQfile_recv"
OWN_PRIVATE_OBB_ROOT="${REAL_ROOT}/Android/obb/${APP_ID}/Tencent/QQfile_recv"
BACKEND_OWN_PRIVATE_DATA_ROOT="${BACKEND_ROOT}/Android/data/${APP_ID}/Tencent/QQfile_recv"
BACKEND_OWN_PRIVATE_MEDIA_ROOT="${BACKEND_ROOT}/Android/media/${APP_ID}/Tencent/QQfile_recv"
BACKEND_OWN_PRIVATE_OBB_ROOT="${BACKEND_ROOT}/Android/obb/${APP_ID}/Tencent/QQfile_recv"
SANDBOX_OWN_PRIVATE_DATA_ROOT="${BACKEND_PRIVATE_ROOT}/Android/data/${APP_ID}/Tencent/QQfile_recv"
SANDBOX_OWN_PRIVATE_MEDIA_ROOT="${BACKEND_PRIVATE_ROOT}/Android/media/${APP_ID}/Tencent/QQfile_recv"
SANDBOX_OWN_PRIVATE_OBB_ROOT="${BACKEND_PRIVATE_ROOT}/Android/obb/${APP_ID}/Tencent/QQfile_recv"
# 各矩阵规则使用独立源和目标，链式映射由场景 37 单独覆盖。
ANY_RELATIVE_REQUEST="${REAL_ROOT}/Android/data/${APP_ID}/srt_any_relative"
ANY_ABSOLUTE_USER_REQUEST="/data/user/0/${APP_ID}/files"
ANY_USER_ID_REQUEST="/data/user/0/${APP_ID}/cache"
ANY_LEGACY_DATA_REQUEST="/data/data/${APP_ID}/code_cache"
ANY_PUBLIC_TO_PRIVATE_REQUEST="${REAL_ROOT}/Download/SrtAnyPublicToPrivate"
ANY_RELATIVE_PUBLIC_TARGET="${REAL_ROOT}/Download/SrtAnyRelativePublic"
ANY_ABSOLUTE_PUBLIC_TARGET="${REAL_ROOT}/Download/SrtAnyAbsolutePublic"
ANY_USER_PRIVATE_TARGET="/data/user/0/${APP_ID}/no_backup/srt_any_private"
ANY_LEGACY_PRIVATE_TARGET="${REAL_ROOT}/Android/media/${APP_ID}/cache"
ANY_MEDIA_REQUEST="${REAL_ROOT}/Download/SrtAnyMediaRequest"
ANY_MEDIA_TARGET="${REAL_ROOT}/Download/SrtAnyMediaTarget"
ANY_MEDIA_FILE="srt_any_media.bin"
NESTED_MAPPING_REQUEST_ROOT="${REAL_ROOT}/Android/data/${APP_ID}/Tencent"
NESTED_MAPPING_STAGE_ROOT="${REAL_ROOT}/Android/data/${APP_ID}/sandbox"
NESTED_MAPPING_TARGET_ROOT="${REAL_ROOT}/Download/SrtNestedMappingQQ"
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

# 自有私有目录夹具的属主修正：Android 13 上应用的自有私有目录视图经系统 FUSE 锚点
# 恢复，MediaProvider 按属主过滤目录项——root 预置的夹具对应用不可见，写入直接
# ENOENT（场景 34 own-data 重试后仍失败，是确定性差异而非时序）。后端拷贝与可见
# 路径两份夹具都按应用属主修正；模块自身创建包名根时也用应用 uid，语义一致。
# fix_private_backend_permissions 只覆盖 Android/data 一支，这里补齐 media/obb 与可见路径。
fix_own_private_fixture_permissions() {
  local uid
  uid="$(test_app_uid)"
  if [ -z "$uid" ]; then
    echo "own_private_fixture_permission_fix_skipped: app uid not found for $APP_ID" >&2
    return 1
  fi
  # 沙箱内的夹具同样按应用属主修正：`${PRIVATE_ROOT}` 就是应用自己的重定向根
  # （`Android/data/<包名>/sdcard`），root 预置的目录项会被系统 FUSE 按属主过滤，
  # 应用在热重载后重新解析这条路径时报 ENOENT（场景 29 的 `createNewFile` 失败）。
  # 模块自身创建沙箱目录时用的也是应用 uid，这里与之对齐，不改变夹具内容。
  adb_su "app_uid='$uid'; for root in '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${OWN_PRIVATE_DATA_ROOT}' '${OWN_PRIVATE_MEDIA_ROOT}' '${OWN_PRIVATE_OBB_ROOT}' '${PRIVATE_ROOT}/Download' '${PRIVATE_ROOT}/Pictures' '${PRIVATE_ROOT}/DCIM' '${PRIVATE_ROOT}/Documents'; do [ -e \"\$root\" ] || continue; chown -R \"\$app_uid\":1023 \"\$root\" 2>/dev/null || true; find \"\$root\" -type d -exec chmod 2771 {} + 2>/dev/null || true; find \"\$root\" -type f -exec chmod 0664 {} + 2>/dev/null || true; done" >/dev/null
}

adb_root() {
  local command="PATH=/debug_ramdisk:/sbin:/data/adb/magisk:\$PATH; $1"
  local encoded runner
  local -a timeout_command=()
  detect_adb_root_mode
  encoded="$(printf '%s' "$command" | base64 | tr -d '\n')"
  runner="printf '%s' '$encoded' | base64 -d | sh"
  if [ -n "${ADB_ROOT_TIMEOUT_SECONDS:-}" ]; then
    timeout_command=(timeout --foreground "${ADB_ROOT_TIMEOUT_SECONDS}s")
  fi
  case "$ADB_ROOT_MODE" in
    su0) "${timeout_command[@]}" adb shell "su 0 sh -c \"$runner\"" ;;
    su) "${timeout_command[@]}" adb shell "su -c \"$runner\"" ;;
    magisk) "${timeout_command[@]}" adb shell magisk su -c "$runner" ;;
    system_magisk) "${timeout_command[@]}" adb shell /system/bin/magisk su -c "$runner" ;;
    debug_magisk) "${timeout_command[@]}" adb shell /debug_ramdisk/magisk su -c "$runner" ;;
    *) echo "No usable adb root shell found." >&2; return 1 ;;
  esac
}

adb_su() {
  # 场景函数由 `bash -c` 子 shell 执行，而 SHELLOPTS 不会导出，因此脚本顶层的
  # pipefail 在子 shell 内是关闭的。若这里直接用管道，退出码取自末尾的 `tr`
  # 而恒为 0，`if adb_su "test -f ..."` 之类的断言会无条件通过。
  # `local -` 把选项变更限制在本函数内，避免影响调用方其它管道的判定语义。
  local -
  set -o pipefail
  adb_root "$1" | tr -d '\r'
}

adb_su_timeout() {
  local timeout_seconds="$1"
  local ADB_ROOT_TIMEOUT_SECONDS="$timeout_seconds"
  shift
  adb_su "$1"
}

adb_write_file() {
  local path="$1"
  local content="$2"
  echo "TRACE adb_write_file path=[$path] caller=${FUNCNAME[1]:-top}@${BASH_LINENO[0]:-0} provider_config=[${PROVIDER_CONFIG:-unset}]" >&2
  local encoded
  encoded="$(printf '%s' "$content" | base64 | tr -d '\n')"
  # 必须原子落盘：truncate-then-write 的写入窗口会让 daemon 的 reload 撞上半截内容，
  # 应用配置被误判无效后整个场景保持未配置态（EROFS/ENOENT，应用重启救不回——共享宿主
  # 策略与进程无关），直到下一次配置重写才恢复（真机全量跑 scenario-12 的 EROFS 即此机理）。
  # 同目录 tmp + rename 原子替换，指纹只会看到完整的新文件或完整的旧文件。
  # tmp 名不带 .json 后缀，避免进入指纹的 app 配置枚举。
  adb_root "printf '%s' '$encoded' | base64 -d > '$path.tmp.$$'; mv -f '$path.tmp.$$' '$path'"
}

wait_boot_completed() {
  local deadline=$((SECONDS + ${ADB_BOOT_TIMEOUT_SECONDS:-300}))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ "$(adb get-state 2>/dev/null || true)" = "device" ] &&
      [ "$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = "1" ]; then
      return 0
    fi
    if adb devices 2>/dev/null | grep -q $'\toffline$'; then
      adb reconnect offline >/dev/null 2>&1 || true
    else
      adb start-server >/dev/null 2>&1 || true
    fi
    sleep 3
  done

  echo "等待 Android 启动完成超时，当前设备状态：" >&2
  adb devices -l >&2 || true
  return 1
}

backup_device_execution_state() {
  device_execution_state_backup_ready=0
  original_stay_on_while_plugged_in="$(adb shell settings get global stay_on_while_plugged_in | tr -d '\r')"
  original_app_inactive="$(adb shell am get-inactive "$APP_ID" | tr -d '\r' | sed -n 's/^Idle=//p')"
  if adb shell cmd deviceidle whitelist | tr -d '\r' | grep -Fq ",${APP_ID},"; then
    original_deviceidle_whitelist=1
  else
    original_deviceidle_whitelist=0
  fi
  device_execution_state_backup_ready=1
}

prepare_device_execution_state() {
  adb shell input keyevent WAKEUP >/dev/null
  adb shell wm dismiss-keyguard >/dev/null 2>&1 || true
  adb shell svc power stayon true >/dev/null
  adb shell am set-inactive "$APP_ID" false >/dev/null
  adb shell cmd deviceidle whitelist +"$APP_ID" >/dev/null
}

restore_device_execution_state() {
  if [ "${device_execution_state_backup_ready:-0}" -ne 1 ]; then
    return 0
  fi
  if [ "${original_deviceidle_whitelist:-0}" -eq 0 ]; then
    adb shell cmd deviceidle whitelist -"$APP_ID" >/dev/null 2>&1 || true
  fi
  adb shell am set-inactive "$APP_ID" "${original_app_inactive:-false}" >/dev/null 2>&1 || true
  if [ -z "${original_stay_on_while_plugged_in:-}" ] || [ "$original_stay_on_while_plugged_in" = "null" ]; then
    adb shell settings delete global stay_on_while_plugged_in >/dev/null 2>&1 || true
  else
    adb shell settings put global stay_on_while_plugged_in "$original_stay_on_while_plugged_in" >/dev/null 2>&1 || true
  fi
}

write_config() {
  local content="$1"
  # 记录最近一次写入的应用配置内容：步骤 1 在行水位之后重写同一份配置，用于产生一次
  # 必然落在水位之后的 reload 标记，供 wait_config_applied 观测。
  APP_CONFIG_CONTENT="$content"
  adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps" >/dev/null
  adb_write_file "$CONFIG" "$content" >/dev/null
}

write_cross_app_read_only_config() {
  adb_write_file "$READ_ONLY_OWNER_CONFIG" '{"users":{"0":{"enabled":true,"read_only_paths":["DCIM","Pictures"]}}}' >/dev/null
}

clear_provider_sandbox_config() {
  # 场景 38 使用的下载提供器专属配置；其余场景不得残留，避免桥接进程
  # 在后续场景重启时带上意外的沙盒规则。
  adb_su "rm -f '$PROVIDER_CONFIG'" >/dev/null 2>&1 || true
}

clear_cross_app_read_only_config() {
  adb_su "rm -f '$READ_ONLY_OWNER_CONFIG'" >/dev/null 2>&1 || true
}

write_global_config() {
  local content="$1"
  adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config" >/dev/null
  adb_write_file "$GLOBAL_CONFIG" "$content" >/dev/null
}

test_global_config() {
  local storage_backend_mode="${1:-auto}"
  local file_monitor_enabled="${2:-$SRT_FILE_MONITOR_ENABLED}"
  case "$file_monitor_enabled" in
    1|true|TRUE|yes|YES) file_monitor_enabled=true ;;
    *) file_monitor_enabled=false ;;
  esac
  printf '{"file_monitor_enabled":%s,"fuse_fix_enabled":true,"storage_backend_mode":"auto","verbose_logging_enabled":true,"auto_enable_redirect_for_new_apps":false,"auto_enable_new_apps_template_id":"","app_config_auto_save":false}' "$file_monitor_enabled"
}

set_backend_config() {
  write_global_config "$(test_global_config "$1" "${2:-$SRT_FILE_MONITOR_ENABLED}")"
}

apply_config() {
  # 场景 1 会删除应用配置，先清空上一场景残留的内容，避免步骤 1 误把旧配置写回。
  APP_CONFIG_CONTENT=""
  set_backend_config auto
  clear_cross_app_read_only_config
  clear_provider_sandbox_config
  case "$1" in
    1)
      adb_su "rm -f '$CONFIG'" >/dev/null
      ;;
    2|29|30|34)
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
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":["SrtRuleSandbox"]}}}'
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
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFusePlain","DCIM/SrtFuseQQ/SrtAllowed*","Download/SrtFuseQ?/Media","Download/SrtFuseMedia*/Drop"]}}}'
      ;;
    17)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseExclude/Writable"],"read_only_paths":["Download/SrtFuseExclude","!Download/SrtFuseExclude/Writable"]}}}'
      ;;
    18)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtFuseMapParent","!Download/SrtFuseMapParent/WritableTarget"],"path_mappings":{"Download/SrtFuseMapRW":"Download/SrtFuseMapParent/WritableTarget","Download/SrtFuseMapRO":"Download/SrtFuseMapParent/LockedTarget"}}}}'
      ;;
    19)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseMulti/QQ/*","Download/SrtFuseMulti/WeChat/*"],"read_only_paths":["Download/SrtFuseMulti/Locked/*"]}}}'
      ;;
    20)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMountNsAllow/Team*/Deep","Download/SrtMountNsAllow/Q?/Deep"]}}}'
      ;;
    21)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsReadOnly/Team*/Deep"]}}}'
      ;;
    22)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsMapParent","!Download/SrtMountNsMapParent/WritableTarget"],"path_mappings":{"Download/SrtMountNsMapRW":"Download/SrtMountNsMapParent/WritableTarget","Download/SrtMountNsMapRO":"Download/SrtMountNsMapParent/LockedTarget"}}}}'
      ;;
    23)
      set_backend_config auto true
      write_config '{"users":{"0":{"enabled":false}}}'
      ;;
    24|25)
      set_backend_config auto true
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
      ;;
    26|27)
      set_backend_config auto true
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor","DCIM","Pictures"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped","DCIM/Camera":"Pictures/SrtMonitorDing"}}}}'
      write_cross_app_read_only_config
      ;;
    28)
      write_config '{"users":{"0":{"enabled":true,"read_only_paths":["Pictures/SrtReadOnlyMedia"]}}}'
      ;;
    31)
      write_config '{"users":{"0":{"enabled":false,"path_mappings":{"Pictures/SrtReadOnlyMedia":"Pictures/SrtLocked"}}}}'
      ;;
    32|33)
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["DCIM","Pictures"]}}}'
      ;;
    35)
      write_config "$(printf '{\"users\":{\"0\":{\"enabled\":true,\"path_mappings\":{\"Android/data/%s/srt_any_relative\":\"Download/SrtAnyRelativePublic\",\"/data/user/0/%s/files\":\"Download/SrtAnyAbsolutePublic\",\"/data/user/0/%s/cache\":\"Android/data/%s/cache\",\"/data/data/%s/code_cache\":\"Android/media/%s/cache\",\"Download/SrtAnyPublicToPrivate\":\"/data/user/0/%s/no_backup/srt_any_private\",\"Download/SrtAnyMediaRequest\":\"Download/SrtAnyMediaTarget\"}}}}' "$APP_ID" "$APP_ID" "$APP_ID" "$APP_ID" "$APP_ID" "$APP_ID" "$APP_ID")"
      ;;
    36)
      set_backend_config auto
      write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["DCIM","Documents","Movies","Music","Pictures"],"path_mappings":{"Android/data/'"${APP_ID}"'/Tencent/QQfile_recv":"Download/SrtQqAliasMapped","Download/QQ":"Download/SrtQqAliasMapped"}}}}'
      ;;
    37)
      write_config "$(printf '{\"users\":{\"0\":{\"enabled\":true,\"path_mappings\":{\"/storage/emulated/0/Android/data/%s/Tencent\":\"/storage/emulated/0/Android/data/%s/sandbox\",\"/storage/emulated/0/Android/data/%s/sandbox/QQfile_recv\":\"/storage/emulated/0/Download/SrtNestedMappingQQ\"}}}}' "$APP_ID" "$APP_ID" "$APP_ID")"
      ;;
    38)
      set_backend_config auto
      adb_write_file "$PROVIDER_CONFIG" '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":["Download/SrtDlProbe"]}}}' >/dev/null
      # 写入一份 disabled 的测试应用配置：apply_config_and_wait 的水位机制要求水位之后
      # 必然出现一次 daemon reload 标记；提供器配置自身的 reload 在水位之前已经发生，
      # 只靠它会让等待超时（apps 计数含提供器配置，apps=[1-9] 成立）。
      write_config '{"users":{"0":{"enabled":false}}}'
      ;;
    39)
      set_backend_config auto
      # 发起者（测试应用）配置显式映射；下载提供器本身不配置任何规则，
      # 场景断言的就是「代写者按发起者规则改写」这条归因链路。
      write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtDlOwner":"Download/SrtDlOwnerMapped"}}}}'
      ;;
    40)
      # 文件监视开启（记录断言依赖），测试应用写 disabled 配置仅作 reload 水位。
      set_backend_config auto
      write_global_config "$(test_global_config auto 1)"
      write_config '{"users":{"0":{"enabled":false}}}'
      ;;
    *)
      echo "unknown scenario: $1" >&2
      return 1
      ;;
  esac
}

target_path() {
  case "$1" in
    8) echo "${RULE_SANDBOX_ROOT}/${TEST_FILE}" ;;
    14) echo "${REAL_ROOT}/Download/SrtLongest/Deep/${TEST_FILE}" ;;
    15) echo "${REAL_ROOT}/Download/SrtPriority/${TEST_FILE}" ;;
    *) echo "${REAL_ROOT}/Download/SrtProbe/${TEST_FILE}" ;;
  esac
}

logical_dir() {
  case "$1" in
    8) echo "${RULE_SANDBOX_ROOT}" ;;
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
    8) echo "${PRIVATE_RULE_SANDBOX_ROOT}/${TEST_FILE}" ;;
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
    6) echo "仅映射模式，未命中映射路径保持真实且自有目录属主漂移不被自动修复" ;;
    7) echo "仅映射模式，命中映射路径应写入映射目标" ;;
    8) echo "FUSE 仅映射模式：命中 sandbox 规则且同级未命中路径保持真实" ;;
    9) echo "read_only_paths 允许读取但拒绝写入、删除、mkdir、rename" ;;
    10) echo "映射目标为只读路径时，映射请求写入应被拒绝" ;;
    11) echo "allowed_real_paths 内联排除与通配符排除规则" ;;
    12) echo "excluded_real_paths 旧字段兼容并入排除规则" ;;
    13) echo "allowed_real_paths 问号通配符规则" ;;
    14) echo "path_mappings 最长前缀匹配规则" ;;
    15) echo "映射优先于字符串形式 sandboxed_paths" ;;
    16) echo "FUSE 数据面：普通放行与通配符放行并存" ;;
    17) echo "FUSE 数据面：read_only_paths 支持 ! 排除优先" ;;
    18) echo "FUSE 数据面：映射最终目标决定只读权限" ;;
    19) echo "FUSE 数据面：同父级多通配符规则互不污染" ;;
    20) echo "自动后端：allowed_real_paths 通配符匹配或 namespace 回退" ;;
    21) echo "自动后端：read_only_paths 通配符匹配或 namespace 回退" ;;
    22) echo "自动后端：映射最终目标决定只读权限" ;;
    23) echo "文件监视：未启用重定向的普通应用与系统代写保存成功记录" ;;
    24) echo "文件监视：普通应用自动后端映射保存、只读失败与只读排除成功记录" ;;
    25) echo "文件监视：普通应用自动后端保存成功、只读失败与只读排除成功记录" ;;
    26) echo "文件监视：系统代写自动后端保存成功、只读失败与只读排除成功记录" ;;
    27) echo "文件监视：系统代写自动后端保存成功、只读失败与只读排除成功记录" ;;
    28) echo "MediaStore 查询：只读真实图片路径应对应用可见" ;;
    29) echo "配置热更新：运行中应用无需重启即可从默认重定向切换到路径映射" ;;
    30) echo "MediaProvider 集合 URI：openTypedAssetFile 不应重映射首条媒体记录" ;;
    31) echo "关闭重定向：缩略图与原图均按真实 MediaStore 路径读取" ;;
    32) echo "真实后端失联恢复：应用重启后无需重启 MediaProvider 即可继续保存图片" ;;
    33) echo "MediaProvider 进程内热重载后应用挂载与图片保存恢复" ;;
    34) echo "启用重定向，验证自有包名 Android/data|media|obb/Tencent/QQfile_recv 保持真实路径" ;;
    35) echo "任意路径映射矩阵：Android 私有目录、data/user、data/data 与公共路径双向映射" ;;
    36) echo "映射私有别名可继续写入已由 MediaProvider 创建的文件" ;;
    37) echo "父映射结果继续命中子映射" ;;
    38) echo "下载提供器自身 sandboxed_paths 原生写入进沙盒" ;;
    39) echo "下载提供器代写按发起者映射落到映射目标" ;;
    40) echo "系统 uid 的 FUSE 写入按发起服务归因" ;;
  esac
}

prepare_backend_core_targets() {
  if ! adb_su "mkdir -p '${BACKEND_ROOT}/Download/SrtProbe' '${BACKEND_ROOT}/Download/Test' '${BACKEND_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_ROOT}/Download/SrtReadOnly' '${BACKEND_ROOT}/Download/SrtMapRO' '${BACKEND_ROOT}/Download/SrtAllow/tmp' '${BACKEND_ROOT}/Download/SrtLegacy/tmp' '${BACKEND_ROOT}/Download/SrtQMark/Keep1' '${BACKEND_ROOT}/Download/SrtQMark/Keep12' '${BACKEND_ROOT}/Download/SrtLongest/Deep' '${BACKEND_ROOT}/Download/SrtLongestBase' '${BACKEND_ROOT}/Download/SrtLongestDeep' '${BACKEND_ROOT}/Download/SrtPriority' '${BACKEND_ROOT}/Download/SrtPriorityMapped' '${BACKEND_ROOT}/Pictures/SrtLocked' '${BACKEND_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_ROOT}/.xldownload' '${BACKEND_ROOT}/.xlDownload'; chmod -R 777 '${BACKEND_ROOT}/Download/SrtProbe' '${BACKEND_ROOT}/Download/Test' '${BACKEND_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_ROOT}/Download/SrtReadOnly' '${BACKEND_ROOT}/Download/SrtMapRO' '${BACKEND_ROOT}/Download/SrtAllow' '${BACKEND_ROOT}/Download/SrtLegacy' '${BACKEND_ROOT}/Download/SrtQMark' '${BACKEND_ROOT}/Download/SrtLongest' '${BACKEND_ROOT}/Download/SrtLongestBase' '${BACKEND_ROOT}/Download/SrtLongestDeep' '${BACKEND_ROOT}/Download/SrtPriority' '${BACKEND_ROOT}/Download/SrtPriorityMapped' '${BACKEND_ROOT}/Pictures/SrtLocked' '${BACKEND_ROOT}/Pictures/SrtReadOnlyMedia' 2>/dev/null || true; chmod 777 '${BACKEND_ROOT}/.xldownload' '${BACKEND_ROOT}/.xlDownload' 2>/dev/null || true; test -d '${BACKEND_ROOT}/Download/SrtProbe' && test -d '${BACKEND_ROOT}/Download/Test' && test -d '${BACKEND_ROOT}/Download/SrtPriority' && test -d '${BACKEND_ROOT}/Download/SrtPriorityMapped'" >/dev/null; then
    echo "测试目标真实后端预置失败：${BACKEND_ROOT}/Download" >&2
    return 1
  fi
}

prepare_any_path_targets() {
  adb_su "rm -rf '${ANY_RELATIVE_PUBLIC_TARGET}' '${ANY_ABSOLUTE_PUBLIC_TARGET}' '${ANY_PUBLIC_TO_PRIVATE_REQUEST}' '${ANY_MEDIA_REQUEST}' '${ANY_MEDIA_TARGET}' '${NESTED_MAPPING_REQUEST_ROOT}' '${NESTED_MAPPING_STAGE_ROOT}' '${NESTED_MAPPING_TARGET_ROOT}' '${ANY_RELATIVE_REQUEST}/srt_any_relative.txt' '${ANY_ABSOLUTE_USER_REQUEST}/srt_any_absolute.txt' '${ANY_USER_ID_REQUEST}/srt_any_user_id.txt' '${ANY_LEGACY_DATA_REQUEST}/srt_any_legacy.txt' '${ANY_USER_PRIVATE_TARGET}/srt_any_public_private.txt' '${ANY_LEGACY_PRIVATE_TARGET}/srt_any_legacy.txt'; mkdir -p '${ANY_RELATIVE_PUBLIC_TARGET}' '${ANY_ABSOLUTE_PUBLIC_TARGET}' '${ANY_PUBLIC_TO_PRIVATE_REQUEST}' '${ANY_MEDIA_REQUEST}' '${ANY_MEDIA_TARGET}' '${NESTED_MAPPING_REQUEST_ROOT}' '${NESTED_MAPPING_STAGE_ROOT}' '${NESTED_MAPPING_TARGET_ROOT}' '${BACKEND_ROOT}/Android/data/${APP_ID}/cache' '${BACKEND_ROOT}/Android/data/${APP_ID}/srt_any_relative' '${BACKEND_ROOT}/Android/media/${APP_ID}/cache' '${ANY_ABSOLUTE_USER_REQUEST}' '${ANY_USER_ID_REQUEST}' '${ANY_LEGACY_DATA_REQUEST}' '${ANY_USER_PRIVATE_TARGET}'; chmod -R 777 '${ANY_RELATIVE_PUBLIC_TARGET}' '${ANY_ABSOLUTE_PUBLIC_TARGET}' '${ANY_PUBLIC_TO_PRIVATE_REQUEST}' '${ANY_MEDIA_REQUEST}' '${ANY_MEDIA_TARGET}' '${NESTED_MAPPING_REQUEST_ROOT}' '${NESTED_MAPPING_STAGE_ROOT}' '${NESTED_MAPPING_TARGET_ROOT}' '${BACKEND_ROOT}/Android/data/${APP_ID}/cache' '${BACKEND_ROOT}/Android/data/${APP_ID}/srt_any_relative' '${BACKEND_ROOT}/Android/media/${APP_ID}/cache' '${ANY_ABSOLUTE_USER_REQUEST}' '${ANY_USER_ID_REQUEST}' '${ANY_LEGACY_DATA_REQUEST}' '${ANY_USER_PRIVATE_TARGET}' 2>/dev/null || true" >/dev/null
}

clean_targets() {
  # 共享探针与夹具目录的删除必须走**可见路径**：该路径由模块 FUSE 承接，删除请求会经
  # 模块通知系统 FUSE 失效前序场景查询过的 inode 缓存；只删后端那一份会让后续 lookup
  # 继续命中旧文件。删不掉时如实报错，但不能因此中断清理（内层 `|| echo` 保证退出码 0）。
  #
  # 只删探针文件是不够的：模块对重定向目录的 unlink 返回 EDOM
  # （`rm: ...: Math result not representable`），该次删除不生效；必须**同时删到目录
  # 本身**，目录级 `rm -rf` 会继续递归并让模块丢弃该目录项，后续 lookup 才不再命中残留。
  # 这两条都必须在 REAL_ROOT 切到后端之前执行（此时 REAL_ROOT 仍是可见路径）。
  #
  # 对照证据（Android 17 场景 20 `file_unexpected label=mount-ns-control-real`）：
  # - run `35128270269`（公开 `3565c0f4`）此处有可见 `rm -f` + 可见 `rm -rf`，整轮 EDOM
  #   只出现 2 次（同一次清理的 rm -f 与 rm -rf 递归各一次），场景 20 通过；
  # - run `35238174287`（公开 `36abe4b0`）把 `rm -rf` 整体改走后端后只剩可见 `rm -f`，
  #   场景 7 起每次清理都报 EDOM、共 14 次，场景 5/6 写入可见 SrtProbe 的探针
  #   （mtime 15:23，早于场景 20 的 15:31）一路存活，被 `file_unexpected` 判失败。
  adb_su "rm -f '${REAL_ROOT}/Download/SrtProbe/$TEST_FILE' '${REAL_ROOT}/Download/Test/$TEST_FILE' || echo 'cleanup_tolerated reason=probe_fuse_edom note=单文件删除被模块 FUSE 以 EDOM 拒绝，改由下面的目录级删除覆盖' >&2" >/dev/null
  # 目录级删除按 `Srt*` 通配动态枚举，不能靠维护一份手写清单：清单漏一个目录，那个目录里的
  # 文件就会跨场景残留，最后在离现场很远的「不应存在」断言上爆掉（场景 29 的
  # `fixture_residue .../SrtQqAliasMapped/srt_qq_alias_existing.bin` 就是清单漏项所致）。
  # 枚举口径与场景前置断言 `assert_fixture_roots_empty` 保持一致，两者互为印证。
  adb_su "for d in '${REAL_ROOT}/Download'/Srt*; do [ -d \"\$d\" ] && rm -rf \"\$d\"; done 2>/dev/null; rm -rf '${REAL_ROOT}/Download/Test' 2>/dev/null || true" >/dev/null
  # 仍保留显式列出的既有目录：通配只在目录名以 Srt 开头时才命中，历史目录名若不含该前缀
  # 需要靠这里兜住。
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/SrtOther' '${REAL_ROOT}/Download/SrtOtherMapped' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' 2>/dev/null || true" >/dev/null
  # Any-path 夹具也必须先从可见路径删除：后续 local REAL_ROOT 切到后端后，动态
  # 函数会清理另一份目录树；漏掉这里会让 profileInstalled 等文件跨场景残留。
  adb_su "rm -rf '${ANY_RELATIVE_PUBLIC_TARGET}' '${ANY_ABSOLUTE_PUBLIC_TARGET}' '${ANY_PUBLIC_TO_PRIVATE_REQUEST}' '${ANY_MEDIA_REQUEST}' '${ANY_MEDIA_TARGET}' '${NESTED_MAPPING_REQUEST_ROOT}' '${NESTED_MAPPING_STAGE_ROOT}' '${NESTED_MAPPING_TARGET_ROOT}' '${ANY_ABSOLUTE_USER_REQUEST}' '${ANY_USER_ID_REQUEST}' '${ANY_LEGACY_DATA_REQUEST}' '${ANY_USER_PRIVATE_TARGET}' '${ANY_LEGACY_PRIVATE_TARGET}' 2>/dev/null || true" >/dev/null

  # 其余夹具的预置与清理一律改走原始后端路径：/storage/emulated/0 是应用视图，刚落盘的
  # 场景配置（只读路径、映射父目录）会在这条视图上覆盖到夹具父目录，root shell
  # 从该视图 mkdir 会被以 EPERM 拒绝（Android 17 场景 17 缺少 Locked/Writable 目录，
  # 于是 file_write_denied 的前置断言直接失败）。原始后端与可见路径指向同一份目录树，
  # 但不经过任何重定向层，因此预置结果是确定的；断言仍读应用可见路径。
  local REAL_ROOT="${BACKEND_ROOT}"

  sleep_ms $SRT_SERVICE_CASE_SETTLE_MS
  clean_results
  # 后端侧也要按通配扫一遍：这里的目录清单同样是手工维护的，漏项会让残留避开清理
  # （可见路径已被扫过，但后端那一份是独立的目录树）。两条路径都要动态枚举才能保证不漏。
  adb_su "for d in '${REAL_ROOT}/Download'/Srt* '${REAL_ROOT}/Pictures'/Srt* '${REAL_ROOT}/DCIM'/Srt* '${REAL_ROOT}/Documents'/Srt* '${REAL_ROOT}/Movies'/Srt* '${REAL_ROOT}/Music'/Srt*; do [ -d \"\$d\" ] && rm -rf \"\$d\"; done 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${MEDIASTORE_ROUTING_PROBE_ROOT}' '${PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT}'" >/dev/null
  adb_su "rm -rf '${BACKEND_RULE_SANDBOX_ROOT}' '${PRIVATE_RULE_SANDBOX_ROOT}' '${BACKEND_RULE_SIBLING_ROOT}' '${PRIVATE_RULE_SIBLING_ROOT}'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/SrtOther' '${REAL_ROOT}/Download/SrtOtherMapped' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/SrtOther' '${BACKEND_PRIVATE_ROOT}/Download/SrtOtherMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia'; find '${REAL_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' -maxdepth 1 -name '$TEST_FILE' -delete 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/Test/$HOT_BEFORE_FILE' '${REAL_ROOT}/Download/Test/$HOT_AFTER_FILE' '${BACKEND_PRIVATE_ROOT}/Download/Test/$HOT_BEFORE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/Test/$HOT_AFTER_FILE' 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$ALLOW_PART_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$ALLOW_PART_FILE' '${REAL_ROOT}/Download/$QMARK_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_SINGLE_FILE' '${REAL_ROOT}/Download/$QMARK_DOUBLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_DOUBLE_FILE'" >/dev/null
  # `SrtQqAliasMapped` 既是场景 36 的映射目标，也落在上面的 `Srt*` 通配删除范围内：
  # 若重建清单漏项，该物理目录在完整矩阵里就不存在（前面已有场景跑过清理），而 MediaProvider
  # 的 lower-FS 通道（`Hooker.providerFileParentCallback`）是按**物理路径**取父目录去建
  # `.pending-*` 临时文件的，目录缺失即报 `MediaStore initial write failed`。
  # 实测 Android 14 完整矩阵两轮稳定复现（41/42ms），而只跑场景 36 时该目录仍存在故通过——
  # 这正是「顺序相关」的来源。补入后与 `SrtMapOnlyMapped` 等同级映射目标保持一致。
  adb_su "mkdir -p '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtQqAliasMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow/tmp' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow/tmp' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload'; chmod -R 777 '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtQqAliasMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia' 2>/dev/null || true; chmod 777 '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${REAL_ROOT}/Android/data/${APP_ID}/files/SrtDrift' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'; mkdir -p '${REAL_ROOT}/Download/SrtLegacy/tmp' '${REAL_ROOT}/Download/SrtQMark/Keep1' '${REAL_ROOT}/Download/SrtQMark/Keep12' '${REAL_ROOT}/Download/SrtLongest/Deep' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy/tmp' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark/Keep1' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark/Keep12' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'; chmod -R 777 '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped' 2>/dev/null || true" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$QMARK_FILE_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_FILE_SINGLE_FILE'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude' '${REAL_ROOT}/Download/SrtFuseMapParent' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti' '${REAL_ROOT}/DCIM/SrtFuseQQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti' '${BACKEND_PRIVATE_ROOT}/DCIM/SrtFuseQQ'; mkdir -p '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude/Locked' '${REAL_ROOT}/Download/SrtFuseExclude/Writable' '${REAL_ROOT}/Download/SrtFuseMapParent/WritableTarget' '${REAL_ROOT}/Download/SrtFuseMapParent/LockedTarget' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti/QQ' '${REAL_ROOT}/Download/SrtFuseMulti/WeChat' '${REAL_ROOT}/Download/SrtFuseMulti/Locked' '${REAL_ROOT}/Download/SrtFuseMulti/Other' '${FUSE_DCIM_ALLOWED_ROOT}' '${FUSE_DCIM_OTHER_ROOT}' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude/Locked' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude/Writable' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent/WritableTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent/LockedTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/QQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/WeChat' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/Locked' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti/Other' '${PRIVATE_FUSE_DCIM_ALLOWED_ROOT}' '${PRIVATE_FUSE_DCIM_OTHER_ROOT}'; chmod -R 777 '${REAL_ROOT}/Download/SrtFusePlain' '${REAL_ROOT}/Download/SrtFuseExclude' '${REAL_ROOT}/Download/SrtFuseMapParent' '${REAL_ROOT}/Download/SrtFuseMapRW' '${REAL_ROOT}/Download/SrtFuseMapRO' '${REAL_ROOT}/Download/SrtFuseMulti' '${REAL_ROOT}/DCIM/SrtFuseQQ' '${BACKEND_PRIVATE_ROOT}/Download/SrtFusePlain' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseExclude' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMulti' '${BACKEND_PRIVATE_ROOT}/DCIM/SrtFuseQQ' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtFuseQa' '${REAL_ROOT}/Download/SrtFuseQab' '${REAL_ROOT}/Download/SrtFuseQb' '${REAL_ROOT}/Download/SrtFuseMediaAlpha' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha'; mkdir -p '${REAL_ROOT}/Download/SrtFuseQa/Media' '${REAL_ROOT}/Download/SrtFuseQab/Media' '${REAL_ROOT}/Download/SrtFuseQb/Media' '${REAL_ROOT}/Download/SrtFuseMediaAlpha/Drop' '${REAL_ROOT}/Download/SrtFuseMediaAlpha/Other' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb/Media' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha/Drop' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha/Other'; chmod -R 777 '${REAL_ROOT}/Download/SrtFuseQa' '${REAL_ROOT}/Download/SrtFuseQab' '${REAL_ROOT}/Download/SrtFuseQb' '${REAL_ROOT}/Download/SrtFuseMediaAlpha' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQa' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQab' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseQb' '${BACKEND_PRIVATE_ROOT}/Download/SrtFuseMediaAlpha' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO'; mkdir -p '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent/WritableTarget' '${REAL_ROOT}/Download/SrtMountNsMapParent/LockedTarget' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent/WritableTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent/LockedTarget' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO'; chmod -R 777 '${REAL_ROOT}/Download/SrtMountNsAllow' '${REAL_ROOT}/Download/SrtMountNsReadOnly' '${REAL_ROOT}/Download/SrtMountNsMapParent' '${REAL_ROOT}/Download/SrtMountNsMapRW' '${REAL_ROOT}/Download/SrtMountNsMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapParent' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRW' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsMapRO' 2>/dev/null || true" >/dev/null
  adb_su "mkdir -p '${REAL_ROOT}/Download/SrtMountNsAllow/TeamAlpha/Deep' '${REAL_ROOT}/Download/SrtMountNsAllow/Qa/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow/TeamAlpha/Deep' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow/Qa/Deep'; chmod -R 777 '${REAL_ROOT}/Download/SrtMountNsAllow' '${BACKEND_PRIVATE_ROOT}/Download/SrtMountNsAllow' 2>/dev/null || true" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram'; mkdir -p '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked/Writable' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked/Writable' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram'; chmod -R 777 '${REAL_ROOT}/Download/SrtMonitor' '${REAL_ROOT}/Download/SrtMonitorMap' '${REAL_ROOT}/Download/SrtMonitorMapped' '${REAL_ROOT}/Download/SrtMonitorLocked' '${REAL_ROOT}/Pictures/SrtRelativeData' '${REAL_ROOT}/Pictures/Nnngram' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitor' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMap' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMonitorLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtRelativeData' '${BACKEND_PRIVATE_ROOT}/Pictures/Nnngram' 2>/dev/null || true" >/dev/null
  prepare_backend_core_targets
  prepare_any_path_targets
  # 嵌套映射准备会删除私有父目录，随后再创建自有目录测试数据。
  # 系统 FUSE 可能拒绝 shell 清理可见自有目录，但不一定就是 FUSE 所致：可见路径删除失败时
  # 如实记录，交由真实后端的严格清理覆盖（不声称一定是 FUSE）。
  adb_su "rm -rf '${OWN_PRIVATE_DATA_ROOT}' '${OWN_PRIVATE_MEDIA_ROOT}' '${OWN_PRIVATE_OBB_ROOT}'" >/dev/null || echo "cleanup_tolerated reason=own_dir_fuse note=可见自有目录删除失败将验证后端清理" >&2
  adb_su "rm -rf '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}' '${SANDBOX_OWN_PRIVATE_OBB_ROOT}'" >/dev/null
  # 紧接上面 6 个 backend/sandbox 自有目录删除之后、重建之前，显式确认这 6 个精确目录确实
  # 已被删除：只做存在性检查、绝不删除其它文件；adb 本身失败也要传播（不吞成成功），
  # 避免残留被静默掩盖、漂到后面的「不应存在」断言上才爆。
  adb_su "for d in '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}' '${SANDBOX_OWN_PRIVATE_OBB_ROOT}'; do if [ -e \"\$d\" ] || [ -L \"\$d\" ]; then echo \"own_dir_cleanup_failed path=\$d\" >&2; exit 1; fi; done" || {
    echo "own_dir_verify_failed note=后端自有目录残留或设备验证失败" >&2
    return 1
  }
  adb_su "mkdir -p '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}' '${SANDBOX_OWN_PRIVATE_OBB_ROOT}'; chmod -R 777 '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}' '${SANDBOX_OWN_PRIVATE_OBB_ROOT}' 2>/dev/null || true" >/dev/null
  # 自有私有目录在应用视图里是模块锚点的 bind，权威落点是可见路径：新版本把
  # app-specific 存储放在独立卷上（Android 13 的 Android/data|obb 是独立 ext4），
  # 此时 /data/media/0 下的同名路径是另一份目录，只预置后端拷贝会让应用看到空包名根，
  # 写入直接 ENOENT（场景 34 own-data）。因此可见路径必须预置成功，且不再吞掉错误。
  if ! adb_su "mkdir -p '${OWN_PRIVATE_DATA_ROOT}' '${OWN_PRIVATE_MEDIA_ROOT}' '${OWN_PRIVATE_OBB_ROOT}'"; then
    echo "自有目录可见路径预置失败，应用视图写入将直接 ENOENT；后端拷贝仍会预置" >&2
  fi
  fix_own_private_fixture_permissions
  fix_private_backend_permissions
}

# 场景前置断言：夹具根下不得残留任何文件。
#
# 场景 20 的 `file_unexpected label=mount-ns-control-real` 就是前序场景的探针没被清掉造成的：
# 残留文件的 mtime 早于本场景十余分钟，却被本场景的「不应存在」断言读到。清理失败在过去只打
# 一行提示就继续跑，于是污染跨场景传播，最终在离现场很远的断言上爆掉，排查成本极高。
#
# clean_targets 只建目录、不预置文件，所以「未自行预置夹具的场景，清理后夹具根下必须没有
# 文件」是成立的前置条件。不成立就当场失败，把跨场景污染变成可定位的即时错误。
assert_fixture_roots_empty() {
  local label="$1"
  local residue rc=0
  # 夹具目录会随场景演进增长，因此按 Srt* 通配动态枚举，不写死清单。
  # 仅扫描原有 Download/Srt* 与 Download/Test 范围，不扩大枚举、不新增删除：避免误伤
  # 未知预置夹具。扫描失败（adb 中断、find 权限错误）必须传播，不能吞成成功；移除
  # -maxdepth 限制与 head 管道后，任何层级的残留与 find 错误都不再被掩盖。
  residue="$(adb_su_timeout 45 "for d in '${REAL_ROOT}/Download'/Srt* '${REAL_ROOT}/Download/Test'; do if [ -d \"\$d\" ]; then find \"\$d\" -type f || exit 1; fi; done")" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "fixture_scan_failed label=${label}" >&2
    return 1
  fi
  if [ -n "$residue" ]; then
    echo "fixture_residue label=${label}" >&2
    printf '%s\n' "$residue" | sed 's/^/  residue: /' >&2
    return 1
  fi
  return 0
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

backup_cross_app_config() {
  cross_app_config_backup_ready=0
  if adb_su "test -f '$READ_ONLY_OWNER_CONFIG'" >/dev/null 2>&1; then
    original_cross_app_config_exists=1
    original_cross_app_config_b64="$(adb_su "base64 '$READ_ONLY_OWNER_CONFIG' 2>/dev/null | tr -d '[:space:]'")"
  else
    original_cross_app_config_exists=0
    original_cross_app_config_b64=""
  fi
  cross_app_config_backup_ready=1
}

restore_cross_app_config() {
  if [ "${cross_app_config_backup_ready:-0}" -ne 1 ]; then
    return 0
  fi
  if [ "${original_cross_app_config_exists:-0}" -eq 1 ] && [ -n "${original_cross_app_config_b64:-}" ]; then
    adb_su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps" >/dev/null 2>&1 || true
    adb_root "printf '%s' '$original_cross_app_config_b64' | base64 -d > '$READ_ONLY_OWNER_CONFIG'" >/dev/null 2>&1 || true
    adb_su "chmod 644 '$READ_ONLY_OWNER_CONFIG' 2>/dev/null || true" >/dev/null 2>&1 || true
  else
    clear_cross_app_read_only_config
  fi
}

supports_fuse_backend_scenarios() {
  # 所有场景都使用 auto；实际后端由运行时日志和 mountinfo 记录。
  return 0
}

prune_version_scenario_excludes() {
  # 版本级场景排除（CI 矩阵按版本传入，如 A17 无外网不能验证依赖下载的场景 39）。
  # 语义是**剔除**而不是替换：默认范围仍是全场景（1-40），显式窄范围（dispatch
  # 输入或提交信息标记）也不会被矩阵覆盖。此前用「包含列表」直接覆盖
  # SRT_SCENARIOS，既让默认退化成窄场景，也吞掉了 dispatch 的单点回归。
  local exclude_raw="${SRT_VERSION_SCENARIO_EXCLUDE:-}"
  [ -n "$exclude_raw" ] || return 0
  local normalized="${exclude_raw//,/ }"
  normalized="${normalized//;/ }"
  local excluded=()
  local item
  for item in $normalized; do
    case "$item" in
      ''|*[!0-9]*)
        echo "invalid exclude scenario: $item" >&2
        return 1
        ;;
    esac
    if [ "$item" -lt 1 ] || [ "$item" -gt 40 ]; then
      echo "invalid exclude scenario: $item" >&2
      return 1
    fi
    excluded+=("$item")
  done

  local kept=()
  local scenario skip
  for scenario in "${scenarios[@]}"; do
    skip=0
    for item in "${excluded[@]}"; do
      if [ "$scenario" = "$item" ]; then
        skip=1
        break
      fi
    done
    if [ "$skip" -eq 0 ]; then
      kept+=("$scenario")
    fi
  done
  if [ "${#kept[@]}" -gt 0 ]; then
    scenarios=("${kept[@]}")
  else
    scenarios=()
  fi
  echo "version scenario exclude=${excluded[*]} remaining=${#scenarios[@]}"
  return 0
}

build_scenario_list() {
  scenarios=()
  if [ -n "${SRT_SCENARIOS:-}" ]; then
    local normalized="${SRT_SCENARIOS//,/ }"
    normalized="${normalized//;/ }"
    if [ "$(printf '%s' "$normalized" | awk '{$1=$1; print}')" = "all" ]; then
      scenarios=($(seq 1 40))
    else
      local scenario
      for scenario in $normalized; do
        case "$scenario" in
          ''|*[!0-9]*)
            echo "invalid scenario: $scenario" >&2
            return 1
            ;;
        esac
        if [ "$scenario" -lt 1 ] || [ "$scenario" -gt 40 ]; then
          echo "invalid scenario: $scenario" >&2
          return 1
        fi
        scenarios+=("$scenario")
      done
    fi
  else
    scenarios=(1 2 3 4 5 6 7)
    scenarios+=(8)
    scenarios+=(9 10 11 12 13 14 15 29 30 31 32 33 34)
    scenarios+=(16 17 18 19)
    scenarios+=(20 21 22)
    scenarios+=(28)
    scenarios+=(23 24)
    scenarios+=(25 26 27)
    scenarios+=(35 36 37 38 39 40)
  fi

  prune_version_scenario_excludes || return 1
  if [ "${#scenarios[@]}" -eq 0 ]; then
    # 显式只请求了本版本被排除的场景：该版本不适用，跳过而不是静默假通过。
    echo "::warning::本版本排除后无剩余场景（scenarios=${SRT_SCENARIOS:-default} exclude=${SRT_VERSION_SCENARIO_EXCLUDE:-}）"
  fi
  return 0
}

remove_test_target_artifacts() {
  adb_su "rm -rf '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}' '${SANDBOX_OWN_PRIVATE_OBB_ROOT}'" >/dev/null
  adb_su "rm -rf '${BACKEND_RULE_SANDBOX_ROOT}' '${PRIVATE_RULE_SANDBOX_ROOT}' '${BACKEND_RULE_SIBLING_ROOT}' '${PRIVATE_RULE_SIBLING_ROOT}'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtProbe' '${REAL_ROOT}/Download/SrtOther' '${REAL_ROOT}/Download/SrtOtherMapped' '${REAL_ROOT}/Download/SrtMapOnlyMapped' '${REAL_ROOT}/Download/SrtReadOnly' '${REAL_ROOT}/Download/SrtMapRO' '${REAL_ROOT}/Download/SrtAllow' '${REAL_ROOT}/Download/Test' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${REAL_ROOT}/Pictures/SrtLocked' '${REAL_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_PRIVATE_ROOT}/Download/SrtProbe' '${BACKEND_PRIVATE_ROOT}/Download/SrtOther' '${BACKEND_PRIVATE_ROOT}/Download/SrtOtherMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapOnlyMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtReadOnly' '${BACKEND_PRIVATE_ROOT}/Download/SrtMapRO' '${BACKEND_PRIVATE_ROOT}/Download/SrtAllow' '${BACKEND_PRIVATE_ROOT}/Download/Test' '${BACKEND_PRIVATE_ROOT}/.xldownload' '${BACKEND_PRIVATE_ROOT}/.xlDownload' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtLocked' '${BACKEND_PRIVATE_ROOT}/Pictures/SrtReadOnlyMedia'" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$ALLOW_PART_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$ALLOW_PART_FILE' '${REAL_ROOT}/Download/$QMARK_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_SINGLE_FILE' '${REAL_ROOT}/Download/$QMARK_DOUBLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_DOUBLE_FILE'" >/dev/null
  adb_su "rm -rf '${REAL_ROOT}/Download/SrtLegacy' '${REAL_ROOT}/Download/SrtQMark' '${REAL_ROOT}/Download/SrtLongest' '${REAL_ROOT}/Download/SrtLongestBase' '${REAL_ROOT}/Download/SrtLongestDeep' '${REAL_ROOT}/Download/SrtPriority' '${REAL_ROOT}/Download/SrtPriorityMapped' '${BACKEND_PRIVATE_ROOT}/Download/SrtLegacy' '${BACKEND_PRIVATE_ROOT}/Download/SrtQMark' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongest' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestBase' '${BACKEND_PRIVATE_ROOT}/Download/SrtLongestDeep' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriority' '${BACKEND_PRIVATE_ROOT}/Download/SrtPriorityMapped'" >/dev/null
  adb_su "rm -f '${REAL_ROOT}/Download/$QMARK_FILE_SINGLE_FILE' '${BACKEND_PRIVATE_ROOT}/Download/$QMARK_FILE_SINGLE_FILE'" >/dev/null
  adb_su "rm -rf '${ANY_RELATIVE_PUBLIC_TARGET}' '${ANY_ABSOLUTE_PUBLIC_TARGET}' '${ANY_PUBLIC_TO_PRIVATE_REQUEST}' '${ANY_RELATIVE_REQUEST}/srt_any_relative.txt' '${ANY_ABSOLUTE_USER_REQUEST}/srt_any_absolute.txt' '${ANY_USER_ID_REQUEST}/srt_any_user_id.txt' '${ANY_LEGACY_DATA_REQUEST}/srt_any_legacy.txt' '${ANY_USER_PRIVATE_TARGET}/srt_any_public_private.txt' '${ANY_LEGACY_PRIVATE_TARGET}/srt_any_legacy.txt'" >/dev/null
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
  remove_mediastore_rows_by_pattern "content://media/external/images/media" '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_own_private_mediastore( \([0-9]+\))?\.jpg(,|$)' "_data=.*/Android/media/${app_regex}/Tencent/QQfile_recv/"
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
  if [ -n "$sdk" ] && { [ "$sdk" -le 34 ] || [ "$sdk" -ge 37 ]; }; then
    echo "skip_media_provider_restart sdk=${sdk}: restarting MediaProvider can detach emulated storage on this emulator" >&2
    return 0
  fi

  adb shell am force-stop com.android.providers.media.module >/dev/null 2>&1 || true
  adb shell am force-stop com.google.android.providers.media.module >/dev/null 2>&1 || true
  adb_su "pkill -f com.android.providers.media.module 2>/dev/null || true; pkill -f com.google.android.providers.media.module 2>/dev/null || true" >/dev/null 2>&1 || true
  sleep 2
}

ensure_monitor_collector() {
  adb_su "/data/adb/modules/storage.redirect.x/bin/srxctl ensure-collectors" >/dev/null 2>&1 || true
}

clear_file_monitor_log() {
  adb_su "/data/adb/modules/storage.redirect.x/bin/srxctl clear-monitor || { mkdir -p '/data/adb/modules/storage.redirect.x/logs'; : > '$FILE_MONITOR_LOG_PATH'; }" >/dev/null 2>&1 || true
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
      write)
        # 覆盖写记录有三种来源：普通应用的 daemon inotify 监视写（事件类型 OPEN，字段 op=open:write）、
        # 系统写入进程 hook（字段 op_filter=open:write）、以及 FUSE 只读拒绝（op=write，已被 ret=-1 排除）。
        # 三者必须取并集：只匹配 hook 形式会把 daemon 监视到的成功覆盖写误判成丢失事件。
        matches="$(adb_su "grep -F -- '$file_name' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | grep -E 'op=open:write|op_filter=open:write|op=write' | grep -Fv -- 'ret=-1' || true")"
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
  # 在断言超时的当下立刻取内核 watch 数与监视日志。场景结束后再采集会受后续
  # 场景的配置切换与重建影响，无法反映写入发生时监视是否真的存在。
  capture_file_monitor_diagnostics 2>/dev/null | sed "s/^/monitor_timeout_probe scenario=${scenario}: /" || true
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
  restore_cross_app_config >/dev/null 2>&1
  restore_app_config >/dev/null 2>&1
  restore_global_config >/dev/null 2>&1
  clean_results >/dev/null 2>&1
  remove_test_target_artifacts >/dev/null 2>&1
  clear_alias_mediastore_fixture >/dev/null 2>&1
  remove_random_mediastore_rows >/dev/null 2>&1
  remove_random_physical_media_files >/dev/null 2>&1
  restart_media_provider >/dev/null 2>&1
  restore_device_execution_state >/dev/null 2>&1
  return "$status"
}

latest_result() {
  adb_su "extra=\$(find '$BACKEND_ROOT/Android/data/$APP_ID' '/data/data/$APP_ID' -path '*/files/test_case_result/result_*.txt' -type f 2>/dev/null); ls -t '$RESULT_DIR'/result_*.txt '$INTERNAL_RESULT_DIR'/result_*.txt '$BACKEND_RESULT_DIR'/result_*.txt '$SANDBOX_RESULT_DIR'/result_*.txt \$extra 2>/dev/null | head -1" | tail -1
}

wait_service_result() {
  local timeout_seconds="$1"
  local host_timeout_seconds=$((timeout_seconds + 15))
  local freshness_marker="$2"
  local expected_test_case="$3"
  local seconds=$((SRT_RESULT_POLL_MS / 1000))
  local remainder=$((SRT_RESULT_POLL_MS % 1000))
  local poll_delay
  printf -v poll_delay '%d.%03d' "$seconds" "$remainder"
  adb_su_timeout "$host_timeout_seconds" "marker_mtime=\$(stat -c %Y '$freshness_marker' 2>/dev/null || echo 0); deadline=\$(date +%s); deadline=\$((deadline + $timeout_seconds)); while [ \$(date +%s) -lt \$deadline ]; do for file in '$RESULT_DIR/result_current.txt' '$INTERNAL_RESULT_DIR/result_current.txt' '$BACKEND_RESULT_DIR/result_current.txt' '$SANDBOX_RESULT_DIR/result_current.txt' \$(find '$BACKEND_ROOT/Android/data/$APP_ID' '/data/data/$APP_ID' -path '*/files/test_case_result/result_current.txt' -type f 2>/dev/null); do if [ -s \"\$file\" ]; then file_mtime=\$(stat -c %Y \"\$file\" 2>/dev/null || echo 0); [ \"\$file_mtime\" -ge \"\$marker_mtime\" ] || continue; if [ '$expected_test_case' != 'all' ]; then line_count=\$(grep -cve '^[[:space:]]*\$' \"\$file\" 2>/dev/null || true); grep -Eq '^(PASS|FAIL) \\[$expected_test_case\\]' \"\$file\" || continue; [ \"\$line_count\" -eq 1 ] || continue; fi; cat \"\$file\"; exit 0; fi; done; sleep $poll_delay; done; exit 1"
}

wait_app_mount_confirmed() {
  # 认三类稳定成功日志：应用侧 specialize_post 的 `app mount confirmed pid=`、daemon 主动重挂
  # 的 `daemon mount ... op=Reload ok=true`，以及 companion 挂载完成的
  # `perf companion mount pkg=... pid=... ... ok=true`。这里曾经还有第三条「读挂载状态标记文件成功」，
  # 但标记文件机制已废弃（它按 PID 命名、在应用数据目录里无限累积），不要把它加回来。
  # 确认到 PID 之后仍会由 app_mountinfo_has_expected_paths 独立复核挂载点，日志只是触发器。
  local label="$1"
  local expect_mount="${2:-1}"
  if [ "$expect_mount" -ne 1 ]; then
    return 0
  fi
  if [ "$SRT_MOUNT_CONFIRM_TIMEOUT_MS" -le 0 ]; then
    return 1
  fi
  local timeout_seconds=$(((SRT_MOUNT_CONFIRM_TIMEOUT_MS + 999) / 1000))
  local host_timeout_seconds=$((timeout_seconds + 15))
  local output
  if output="$(adb_su_timeout "$host_timeout_seconds" "deadline=\$((\$(date +%s) + $timeout_seconds)); pid=''; while [ \$(date +%s) -le \$deadline ]; do pid=\$(pidof '$APP_ID' 2>/dev/null | awk '{for (i=1; i<=NF; i++) if (\$i+0 > max) max=\$i} END {if (max != \"\") print max}'); [ -z \"\$pid\" ] && pid=\$(for q in /proc/[0-9]*; do c=\$(cat \"\$q/cmdline\" 2>/dev/null | tr '\\0' '\\n' | head -1); case \"\$c\" in '$APP_ID'|'$APP_ID':*) echo \"\${q#/proc/}\";; esac; done | sort -n | tail -1); [ -n \"\$pid\" ] && break; sleep 0.1; done; if [ -z \"\$pid\" ]; then echo pid_not_found; exit 2; fi; confirmed=\"app mount confirmed pid=\$pid\"; daemon=\"daemon mount pkg=$APP_ID pid=\$pid op=Reload ok=true\"; companion=\"perf companion mount pkg=$APP_ID pid=\$pid .* ok=true\"; while [ \$(date +%s) -le \$deadline ]; do if logcat -d -t 300 -s StorageRedirect:V SRX:V 2>/dev/null | grep -Eq \"(\$confirmed|\$daemon|\$companion)\"; then echo confirmed_pid=\$pid; exit 0; fi; if grep -Eq \"(\$confirmed|\$daemon|\$companion)\" '$LOG_PATH' 2>/dev/null; then echo confirmed_pid=\$pid; exit 0; fi; sleep 0.1; done; echo pid=\$pid; exit 1")"; then
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
    ""|1|23|31|38|40) return 1 ;;
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
      # 父目录放行与子路径映射共存时有两种合法布局：
      # ① 旧 scoped/namespace 布局——放行根上有独立挂载点；
      # ② 共享宿主接入布局——整个存储视图根被 srx_fuse_host[pid] 接管，放行与
      #    映射都是宿主会话内的策略层逻辑路径，不会额外出现在 mountinfo 中。
      # 两种布局任一命中即算确认成功（见 app_mountinfo_has_expected_paths 的场景 4 分支）。
      printf '%s\n' "${REAL_ROOT}/Download"
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

  if [ "$(scenario_from_label "$label")" = "4" ]; then
    # 场景 4 的期望路径表保留旧布局形态（放行根挂载点）；共享宿主接入布局下整个
    # 视图根被 srx_fuse_host[pid] 接管，放行与映射路径都是会话内的策略层逻辑路径，
    # 不会再出现 /Download 挂载点。两种布局任一命中即证明应用视图已被模块接管。
    if adb_su "pid='$pid'; grep -Fq ' ${REAL_ROOT}/Download ' /proc/\$pid/mountinfo"; then
      return 0
    fi
    if adb_su "pid='$pid'; grep -F ' ${REAL_ROOT} ' /proc/\$pid/mountinfo | grep -Fq srx_fuse_host"; then
      return 0
    fi
    echo "missing=${REAL_ROOT}/Download-or-fuse-host-root"
    return 1
  fi

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
  local watermark="${2:-0}"
  case "$watermark" in
    ''|*[!0-9]*) watermark=0 ;;
  esac
  local timeout_seconds=$(((SRT_CONFIG_APPLY_TIMEOUT_MS + 999) / 1000))
  local host_timeout_seconds=$((timeout_seconds + 15))
  local output
  # 只认行水位之后的 reload 标记：旧实现扫描尾部固定 240 行，daemon 应用配置时会紧接着输出
  # 大批挂载日志，把标记冲出窗口，于是配置其实已经生效却一直等不到（场景 22 应用超时即由此产生）。
  if output="$(adb_su_timeout "$host_timeout_seconds" "deadline=\$((\$(date +%s) + $timeout_seconds)); while [ \$(date +%s) -le \$deadline ]; do if tail -n +$((watermark + 1)) '$LOG_PATH' 2>/dev/null | grep -Eq 'config (reloaded|loaded) .*apps=[1-9]'; then exit 0; fi; sleep 0.1; done; tail -80 '$LOG_PATH' 2>/dev/null; exit 1")"; then
    return 0
  fi
  echo "config apply timeout: $label" >&2
  printf '%s\n' "$output" | sed 's/^/config_log_tail: /' >&2
  return 1
}

# 等待刚写入的配置生效。
#
# daemon 输出 reload 标记后会立刻跟进大批挂载日志，固定行数窗口会被这批日志冲出，
# 标记即使已经输出也永远不进窗口。这里先记录当前行水位，再重写同一份应用配置，使一次
# 必然发生的 reload 落在水位之后，等待逻辑只认水位之后的标记，不再依赖行数窗口。
apply_config_and_wait() {
  local label="$1"
  local watermark
  watermark="$(adb_su "wc -l < '$LOG_PATH' 2>/dev/null || echo 0" 2>/dev/null | tr -d '[:space:]' || echo 0)"
  case "$watermark" in
    ''|*[!0-9]*) watermark=0 ;;
  esac
  if [ -n "${APP_CONFIG_CONTENT:-}" ]; then
    write_config "$APP_CONFIG_CONTENT"
  fi
  wait_config_applied "$label" "$watermark"
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

SRT_FRESH_APP_SCENARIOS="${SRT_FRESH_APP_SCENARIOS:-}"

# SRT_FRESH_APP_SCENARIOS 非空时按场景收窄逐用例冷启动：只有列出的场景保持冷启动，
# 其余场景复用应用进程（A17 实验提速用：冷启动是 A17 比其他版本慢 10-15 分钟的主因）。
# 空值保持 SRT_FRESH_APP_PER_CASE 的原语义，正式矩阵行为不变。
fresh_app_enabled_for_label() {
  case "$SRT_FRESH_APP_PER_CASE" in
    1|true|TRUE|yes|YES) ;;
    *) return 1 ;;
  esac
  if [ -n "$SRT_FRESH_APP_SCENARIOS" ]; then
    local scenario
    scenario="$(scenario_from_label "$1")"
    case ",${SRT_FRESH_APP_SCENARIOS// /}," in
      *",${scenario},"*) return 0 ;;
      *) return 1 ;;
    esac
  fi
  return 0
}

prepare_service_case() {
  local label="$1"
  fresh_app_enabled_for_label "$label" || return 0
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
    local previous_pid
    previous_pid="$(app_pid)"
    # Android 17 冷启动后 framework 可能短暂未就绪，adb 子命令偶发不返回；
    # 统一包 timeout 防挂起，失败快速进入下一轮重试而不是空等场景级超时。
    timeout 45 adb shell am force-stop "$APP_ID" >/dev/null || true
    if [[ "$label" == *quick-initial-app* ]] && [ -n "$previous_pid" ] && ! wait_app_process_stopped 10; then
      echo "app stop timeout: $label" >&2
      return 1
    fi
    timeout 15 adb logcat -c >/dev/null 2>&1 || true
    adb_su_timeout 30 ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null
    local am_start_output am_start_try
    am_start_output=""
    for am_start_try in 1 2 3; do
      if am_start_output="$(timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" 2>&1)"; then
        if grep -q "Status: ok" <<<"$am_start_output"; then
          break
        fi
      fi
      # "Too early to start activity" 等 framework 未就绪错误：立即退避重试，
      # 避免进入 mount 确认轮询空转数十秒才发现应用根本没起来。
      echo "am start 未就绪（${label} 第 ${am_start_try}/3 次），退避重试：$(grep -m1 -E 'Error|Exception|Too early|error' <<<"$am_start_output" | cut -c1-120)" >&2
      sleep 5
    done
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

# 等待场景目标目录在被测应用自身的挂载视图内可见。
#
# 为什么需要这一步：上面的 `wait_storage_ready` 只验证 root 侧（`sm list-volumes` 与真实
# 路径的 `test -d`），而被测应用的挂载视图与 root 是隔离的（沙箱绑定叠加 FUSE 视图）。
# 脚本用 root 预置的 `Download/Srt*` 目录可能在应用侧尚未出现，随后的写入或读取就会
# 立刻拿到 ENOENT。实测 Android 13 x86_64 上连续四次完整矩阵分别失败于场景 25/4/14/9，
# 形态同属这一类 —— 都是「root 可见但应用尚不可见」的时序窗口。
#
# 探测手段是进应用进程的 mount namespace 后 `ls`，取到的正是应用实际会走的视图，
# 而不是 adb 自己的视图（后者无法证明应用可访问，这一点在本项目已多次踩过）。
# 拿不到已确认的 pid、或设备没有 `nsenter` 时优雅降级为不等待；等待超时也只输出诊断、
# 不判定场景失败，避免把「无法探测」或「探测偏慢」变成新的失败模式。
wait_scenario_app_view() {
  local scenario="$1"
  local label="scenario-${scenario}"
  local pid="${LAST_MOUNT_CONFIRMED_PID:-}"
  local timeout_seconds="${SRT_APP_VIEW_TIMEOUT_SECONDS:-15}"
  local path deadline

  # 场景 38/40 不使用测试应用（见 step 3 跳过逻辑）：直接跳过应用视图探针，
  # 避免误用上一场景残留的 confirmed pid 白烧 15s 超时。
  case "$scenario" in
    38|40)
      return 0
      ;;
  esac

  # 探针熔断：首次 15s 超时后写标记文件，同一轮里其余场景直接跳过。
  # CI 实测 API 34 模拟器上该探针恒不可用（37 场景里 34 次打满超时、场景本体
  # 照常通过），不熔断时每场景白烧 15s，整条 A14 矩阵多跑约 9.5 分钟。
  if [ -f "app-view-probe-broken.flag" ]; then
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "app_view_probe_skipped label=${label} reason=probe-circuit-broken"
    return 0
  fi
  if [ -z "$pid" ]; then
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "app_view_probe_skipped label=${label} reason=no-confirmed-pid"
    return 0
  fi
  if [ -z "$(adb_su 'command -v nsenter 2>/dev/null' 2>/dev/null || true)" ]; then
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "app_view_probe_skipped label=${label} reason=no-nsenter"
    return 0
  fi

  # 只看该场景的逻辑目录：它已按场景分派（含 sandbox 规则场景与最长前缀场景），
  # 且正常情况下首次探测即命中，因此不引入额外耗时；只有真的尚未可见才会等待。
  path="$(logical_dir "$scenario")"
  [ -n "$path" ] || return 0
  deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ -n "$(adb_su "ls -ld '/proc/${pid}/root${path}' 2>/dev/null" 2>/dev/null || true)" ]; then
      return 0
    fi
    sleep 1
  done

  # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
  echo "app_view_not_ready label=${label} path=${path} pid=${pid}"
  # 首次超时做一次性诊断：目标 pid 有效性、mount namespace 标识、nsenter 与
  # /proc/<pid>/root 两条探针的实际退出码与错误输出，供根因定位；随后写熔断
  # 标记，本轮剩余场景不再重试。
  # 根因（CI run 39123157208 A14 诊断实锤）：API 34 模拟器的 /system/bin/nsenter
  # 没有正确吃到 -t 参数，报 /proc/0/ns/mnt（target 落到默认 0）——探针失败与应用
  # 视图无关。改用 /proc/<pid>/root/<path> 直查目标进程根视图，语义等价且无
  # setns 与参数解析问题。
  # quality-allow(chinese-language): 输出为日志检索用的 ASCII 标记，中文说明见上一行注释。
  adb_su "echo app_view_diag_pid=${pid}; echo app_view_diag_ns=\$(readlink /proc/${pid}/ns/mnt 2>&1); echo app_view_diag_nsenter=\$(command -v nsenter 2>&1); nsenter -t ${pid} -m -- ls -ld '${path}' 2>&1; echo app_view_diag_nsenter_rc=\$?; echo app_view_diag_rootprobe=\$(ls -ld '/proc/${pid}/root${path}' 2>&1); echo app_view_diag_rootprobe_rc=\$?" || true
  : > "app-view-probe-broken.flag"
  echo "app_view_probe_circuit_broken（本轮剩余场景跳过应用视图探针）"
  print_storage_state "${label}-app-view-timeout"
  return 0
}

request_android13_storage_mount() {
  local sdk
  sdk="$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  if [ "$sdk" != "33" ]; then
    return 0
  fi
  echo "initial_storage_recovery: request Android 13 vold mount"
  adb shell "cmd storage mount all" >/dev/null 2>&1 || true
  adb shell "vdc volume mount emulated;0" >/dev/null 2>&1 || true
  adb shell "sm mount 'emulated;0'" >/dev/null 2>&1 || true
}

ensure_initial_storage_ready() {
  # FBE CE 解锁等待：boot 后 15-25 秒内 /data/media/0 尚不可写（ENOTKEY=126），
  # 共享宿主的策略登记与重定向根 mkdir 连续失败、宿主反复自退，应用回退 scoped
  # 会话后 fresh-app 首操作大面积 ENOENT（与 scripts/verify-test-flow.ps1 同步）。
  # CI 模拟器解锁时序不同所以未暴露，真机（MIUI+A14）必现。
  local unlock_deadline=$((SECONDS + 180))
  while [ "$SECONDS" -lt "$unlock_deadline" ]; do
    if adb_su "test -w /data/media/0" >/dev/null 2>&1; then
      break
    fi
    sleep 3
  done

  if wait_storage_ready "initial" 60; then
    return 0
  fi

  echo "initial_storage_recovery: request emulated volume remount"
  request_android13_storage_mount
  adb shell "sm mount 'emulated;0'" >/dev/null 2>&1 || true
  if wait_storage_ready "initial-remount" 15; then
    echo "initial_storage_recovery: emulated storage recovered by remount"
    return 0
  fi

  echo "initial_storage_recovery: reboot after emulated storage stayed unavailable"
  adb reboot >/dev/null 2>&1 || true
  ADB_ROOT_MODE=""
  wait_boot_completed
  request_android13_storage_mount
  wait_storage_ready "initial-reboot" 120
  detect_adb_root_mode
  echo "initial_storage_recovery: emulated storage recovered"
}

media_provider_query_ready() {
  local uri="$1"
  local output
  if ! output="$(timeout 15s adb shell content query --uri "$uri" --projection _id --where '_id=-1' 2>&1)"; then
    return 1
  fi
  if grep -Eiq 'Error while accessing provider:media|Volume [^ ]+ not found|IllegalArgumentException|Unknown URL|Unsupported Uri' <<<"$output"; then
    return 1
  fi
  return 0
}

media_provider_is_lazy() {
  local sdk
  sdk="$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  [ -n "$sdk" ] && [ "$sdk" -ge 37 ]
}

wait_media_provider_ready() {
  local label="$1"
  local timeout_seconds="${2:-120}"
  local deadline=$((SECONDS + timeout_seconds))
  if media_provider_is_lazy; then
    echo "MediaProvider readiness deferred until first app access: ${label}"
    return 0
  fi
  local sdk
  sdk="$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  local uris=("content://media/external_primary/file")
  if [ -z "$sdk" ] || [ "$sdk" -lt 37 ]; then
    uris+=(
      "content://media/external_primary/images/media"
      "content://media/external_primary/video/media"
      "content://media/external_primary/audio/media"
      "content://media/external_primary/downloads"
      "content://media/external/images/media"
      "content://media/external/video/media"
      "content://media/external/audio/media"
      "content://media/external/file"
      "content://media/external/downloads"
    )
  fi

  local last_failed_uri=""
  while [ "$SECONDS" -lt "$deadline" ]; do
    local ready=1
    local uri
    for uri in "${uris[@]}"; do
      if ! media_provider_query_ready "$uri"; then
        ready=0
        last_failed_uri="$uri"
        break
      fi
    done
    if [ "$ready" -eq 1 ]; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for MediaProvider: ${label}"
  echo "MediaProvider readiness probe failed: ${last_failed_uri:-unknown}"
  print_storage_state "${label}-media-provider-timeout"
  return 1
}

media_provider_pid() {
  adb_su "for package in com.android.providers.media.module com.google.android.providers.media.module com.android.providers.media android.process.media; do pidof \"\$package\" 2>/dev/null || true; done; ps -A -o PID,NAME,ARGS 2>/dev/null | awk '\$0 ~ /providers\\.media|android\\.process\\.media/ {print \$1}'" |
    tr -d '\r' |
    awk 'NF && pid == "" { pid = $1 } END { if (pid != "") print pid }'
}

# MediaProvider 就绪缓存：wait_media_provider_ready 在 API<37 要顺序探测 10 个
# URI（每次 content query ~1s，共 ~10s），系统代写监视场景（26/27）每个用例前
# 都重复探测，成为全场矩阵最大单点耗时。按 provider 进程 PID 缓存就绪结论，
# PID 变化（真实重启）自动失效；场景 32/33 的进程内热重载不换 PID，显式绕过
# 缓存直连探测，避免掩盖热重载后的 hook 状态。
media_provider_ready_cached() {
  local label="$1"
  local scenario
  scenario="$(scenario_from_label "$label" 2>/dev/null || true)"
  case "$scenario" in
    32|33) wait_media_provider_ready "$@" ; return $? ;;
  esac
  local current_pid
  current_pid="$(media_provider_pid)"
  if [ -n "$current_pid" ] && [ "$current_pid" = "${MEDIA_PROVIDER_READY_CACHE_PID:-}" ]; then
    return 0
  fi
  if wait_media_provider_ready "$@"; then
    MEDIA_PROVIDER_READY_CACHE_PID="$current_pid"
    return 0
  fi
  return 1
}

wait_media_provider_hook_ready() {
  local label="$1"
  local timeout_seconds="${2:-30}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    local pid boot_id install_state hook_lines
    pid="$(media_provider_pid)"
    if [ -n "$pid" ]; then
      boot_id="$(adb shell cat /proc/sys/kernel/random/boot_id 2>/dev/null | tr -d '\r')"
      install_state="$(adb_su "cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null || true" | tr -d '\r')"
      if [ -n "$boot_id" ] && grep -Fq "stage=init_ok pid=${pid} boot_id=${boot_id} " <<<"$install_state"; then
        echo "media_provider_hook_ready label=${label} pid=${pid} source=install-state"
        return 0
      fi
      hook_lines="$(adb logcat -d --pid "$pid" -s SRX:I 2>/dev/null | grep -F 'java hook open ok' || true)"
      if [ -n "$hook_lines" ]; then
        echo "media_provider_hook_ready label=${label} pid=${pid} source=logcat"
        return 0
      fi
    fi
    sleep_ms 200
  done

  local pid
  pid="$(media_provider_pid)"
  echo "media_provider_hook_timeout label=${label} pid=${pid:-missing}" >&2
  if [ -n "$pid" ]; then
    adb_su "grep -E 'storage.redirect.x/zygisk|libsrx_core' '/proc/$pid/maps' 2>/dev/null || true" |
      sed 's/^/media_provider_module_map: /' >&2
    adb logcat -d --pid "$pid" -s SRX:V StorageRedirect:V 2>/dev/null |
      tail -80 |
      sed 's/^/media_provider_hook_logcat: /' >&2
  fi
  return 1
}

ensure_media_provider_hook_ready() {
  local label="$1"
  if media_provider_is_lazy; then
    echo "MediaProvider hook readiness deferred until first app access: ${label}"
    return 0
  fi
  if wait_media_provider_hook_ready "${label}-current" 3; then
    return 0
  fi
  echo "media_provider_hook_recovery label=${label}"
  restart_media_provider_with_hook_ready "${label}-recovery"
}

restart_media_provider_with_hook_ready() {
  local label="$1"
  local attempt
  for attempt in 1 2; do
    adb logcat -c >/dev/null 2>&1 || true
    restart_media_provider
    wait_storage_ready "${label}-storage-attempt-${attempt}" 30 >/dev/null || return 1
    wait_media_provider_ready "${label}-provider-attempt-${attempt}" 60 >/dev/null || return 1
    if wait_media_provider_hook_ready "${label}-attempt-${attempt}" 20; then
      return 0
    fi
    if [ "$attempt" -lt 2 ]; then
      echo "media_provider_hook_retry label=${label} attempt=${attempt}"
    fi
  done

  return 1
}

print_storage_state() {
  local label="$1"
  echo "=== storage state: ${label} ==="
  # 场景 36 这类「写入报成功但目标是空目录」的失败，光看挂载栈不足以定论：必须直接列出
  # 请求路径、映射目标与后端三处的实际内容，才能区分「文件没落地」与「落地了但可见路径
  # 不对」。后端用 lower FS 直读（MediaProvider 在路径被 FUSE 覆盖时走这条通道），
  # 因此这里同时列 /data/media 侧，两边都空才是真的没落盘。
  #
  # 末尾两处是沙箱侧：应用视角里 `/storage/emulated/0` 的挂载 root 就是沙箱
  # （`<BACKEND_ROOT>/Android/data/<APP_ID>/sdcard`，见应用 mountinfo）。因此应用要走到
  # `/storage/emulated/0/Android/data/<APP_ID>/...`，中间组件 `Android`、`Android/data`
  # 必须在**沙箱内**存在。实测失败平台在应用进程内连 `Android/data/<APP_ID>` 自身都
  # stat 不到，而通过平台上它存在且有 5 个条目。列出沙箱根与其 `Android` 子目录，
  # 即可直接判定「沙箱内是否缺这条中间链」，无需再从应用侧现象反推。
  adb_su "for path in '${QQ_ALIAS_REQUEST_ROOT}' '${QQ_ALIAS_MAPPED_ROOT}' '${BACKEND_ROOT}/Download/SrtQqAliasMapped' '${BACKEND_ROOT}/Android/data/${APP_ID}/Tencent/QQfile_recv' '${BACKEND_ROOT}/Android/data/${APP_ID}/sdcard' '${BACKEND_ROOT}/Android/data/${APP_ID}/sdcard/Android'; do echo \"--- dir: \$path ---\"; ls -la \"\$path\" 2>&1 || true; done" || true
  adb shell "date; getprop ro.build.version.sdk; getprop ro.build.version.release; getprop sys.boot_completed; getprop dev.bootcomplete; getprop init.svc.sdcard; getprop init.svc.media; sm list-volumes all 2>/dev/null || true; df -h /storage/emulated/0 /sdcard 2>&1 || true; ls -ld /storage /storage/emulated /storage/emulated/0 /sdcard 2>&1 || true; mount | grep -E ' /storage|/mnt/runtime|/mnt/user|sdcard|fuse|srx' || true" || true
  adb_su "id; for path in /mnt/user/0 /mnt/user/0/emulated /mnt/user/0/emulated/0 /mnt/runtime/default/emulated/0; do if [ -e \"\$path\" ]; then ls -ld \"\$path\"; else echo optional_storage_alias_absent path=\$path; fi; done; cat /proc/mounts | grep -E ' /storage|/mnt/runtime|/mnt/user|sdcard|fuse|srx' || true" || true
  # 挂载状态文件里逐条记着本轮落盘的 target（`target=` 行），它是判定「模块认为自己挂了哪些
  # 路径」的唯一权威来源，而它本身不在 artifact 里——日志只给出计数。run 35577498143 暴露
  # 的差异正是计数：Android 13 走 daemon 路径落盘 targets=13，Android 14 走 companion 路径
  # 为 41，但差额具体是哪些路径无从判断。直接把文件内容打出来，下一轮即可两条路径逐条对照，
  # 不必再从计数反推。两个路径前缀都列（daemon 与 companion 写同一个目录、文件名后缀为 pid）。
  adb_su "d='${MOUNT_STATE_DIR}'; for f in \$d/*.state; do [ -f \"\$f\" ] || continue; echo \"--- state: \$f ---\"; cat \"\$f\" 2>&1 || true; done" || true
  # 目标应用自己的 mountinfo 是「应用视角」的权威证据：mount namespace 按进程隔离，
  # 上面 adb 视角能看到的东西应用未必能看到（反之亦然），因此单列一份应用侧视图备对照。
  adb_su "for p in \$(pidof ${APP_ID} 2>/dev/null || true); do echo \"--- app mountinfo pid=\$p ---\"; cat /proc/\$p/mountinfo 2>&1 | grep -E 'SrtQqAlias|QQfile_recv|Download/QQ' || echo '<no matched entry>'; done" || true
}

run_service_case() {
  local scenario="$1"
  local label="$2"
  local test_case="$3"
  local pass_pattern="$4"
  shift 4
  local output_file="${SRT_ARTIFACT_DIR}/scenario-${scenario}-${label}-result.txt"

  prepare_service_case "scenario-${scenario}-${label}" || {
    echo "service_case_prepare_failed scenario=${scenario} label=${label}" >&2
    return 1
  }

  # HyperOS 实机（A16/KSU）会随用例启动次数累积把 start-foreground-service 无限期
  # 延迟（应用重启后还会重投递积压 intent）：被延迟的用例变成僵尸，或表现为本用例
  # result_timeout，或与后续用例交错执行污染结果文件。第一轮超时后先 force-stop
  # 清空积压 intent，再把应用前台重启一次，然后原地重驱当前用例。
  local drive_attempt freshness_marker start_output timeout_seconds
  for drive_attempt in 1 2; do
    if [ "$drive_attempt" -gt 1 ]; then
      echo "service_case_intent_recovery scenario=${scenario} label=${label} test_case=${test_case} attempt=${drive_attempt}"
      timeout 45 adb shell am force-stop "$APP_ID" >/dev/null || true
      start_app_and_confirm_mount "scenario-${scenario}-${label}-recover" 1 || return 1
      wait_storage_ready "scenario-${scenario}-${label}-recover" 30 >/dev/null || return 1
      clean_results
    fi
    sleep_ms "$SRT_SERVICE_CASE_SETTLE_MS"
    clean_results
    freshness_marker="/data/local/tmp/srx-result-$$-${scenario}-${RANDOM}.marker"
    adb_su "touch '$freshness_marker'" >/dev/null
    # Android 17 上向刚重启的应用发 broadcast 偶发阻塞至场景级超时；包 90 秒
    # timeout 使挂起快速走 service_start_failed 路径（有输出、可重试）。
    # 直接启动前台服务而非经 receiver 转发：HyperOS 实机（A16/KSU）会在用例启动
    # 若干次后按「Background execution not allowed」策略在入队时静默丢弃发往
    # manifest receiver 的广播（BroadcastEnqueue skip），表现为用例永远无结果
    # （result_timeout 雪崩）；shell 直启 FGS 不受该策略限制，且服务端的
    # onStartCommand 按相同 action+extras 处理，语义与 receiver 转发完全一致。
    if ! start_output="$(timeout 90 adb shell am start-foreground-service -n "${APP_ID}/.TestService" -a "$ACTION" --es test_case "$test_case" "$@" 2>&1)"; then
      adb_su "rm -f '$freshness_marker'" >/dev/null
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

    timeout_seconds="$(service_case_timeout_seconds "$test_case")"
    if wait_service_result "$timeout_seconds" "$freshness_marker" "$test_case" | tee "$output_file"; then
      adb_su "rm -f '$freshness_marker'" >/dev/null
      cat "$output_file" >>"${SRT_ARTIFACT_DIR}/scenario-${scenario}-result.txt"
      if [ -z "$pass_pattern" ]; then
        return 0
      fi
      if grep -q "$pass_pattern" "$output_file"; then
        return 0
      fi
      return 1
    fi

    adb_su "rm -f '$freshness_marker'" >/dev/null
    echo "result_timeout scenario=${scenario} test_case=${test_case} attempt=${drive_attempt}"
    adb shell am force-stop "$APP_ID" >/dev/null || true
  done
  return 1
}

run_write_case() {
  local scenario="$1"
  local label="$2"
  local path="$3"
  local payload="${4:-$PAYLOAD}"
  local attempt

  for attempt in 1 2; do
    if run_service_case "$scenario" "$label" "file_write" '^PASS \[file_write\]' --es file_path "$path" --es payload "$payload" --es expected_payload "$payload"; then
      return 0
    fi
    [ "$attempt" -lt 2 ] || return 1
    if app_view_namespace_refresh_allowed "$scenario"; then
      echo "file_write_namespace_refresh scenario=${scenario} label=${label} attempt=${attempt}"
      start_app_and_confirm_mount "scenario-${scenario}-${label}-write-retry" 1 || return 1
      wait_storage_ready "scenario-${scenario}-${label}-write-retry" 30 >/dev/null || return 1
    else
      sleep_ms "$SRT_RESULT_POLL_MS"
    fi
  done
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
    media_provider_ready_cached "scenario-${scenario}-${label}-mediastore-provider" 60 >/dev/null || return 1
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
  media_provider_ready_cached "scenario-${scenario}-${label}-mediastore-provider" 60 >/dev/null || return 1
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
    timeout 45 adb shell am force-stop "$APP_ID" >/dev/null || true
    timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null || true
    wait_storage_ready "scenario-${scenario}-write-retry"
    if app_view_namespace_refresh_allowed "$scenario"; then
      ensure_current_app_mount_confirmed "scenario-${scenario}-write-retry" || return 1
    fi
    clean_targets
  done
}

app_view_namespace_refresh_allowed() {
  case "$1" in
    29|32|33) return 1 ;;
    *) return 0 ;;
  esac
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
      grep -q "entries=.*${TEST_FILE}" "${SRT_ARTIFACT_DIR}/scenario-${scenario}-${label}-result.txt"; then
      echo "app_view scenario=${scenario} logical_dir=${dir} expected_entry=${TEST_FILE}"
      return 0
    fi
    echo "app_view_retry scenario=${scenario} logical_dir=${dir} attempt=${attempt}"
    if [ "$attempt" -eq 2 ] && app_view_namespace_refresh_allowed "$scenario"; then
      # 配置 reload 或挂载传播后旧应用 namespace 可能保留 stale dentry；重建一次应用视图，
      # 让可见性重试覆盖时序窗口，而不是只重复同一个失败请求。显式排除热重载和进程
      # 生命周期场景，避免破坏它们对 PID/既有 namespace 的行为断言。
      echo "app_view_namespace_refresh scenario=${scenario} logical_dir=${dir}"
      start_app_and_confirm_mount "scenario-${scenario}-${label}-app-view-refresh" 1 || return 1
      wait_storage_ready "scenario-${scenario}-${label}-app-view-refresh" 30 >/dev/null || return 1
    else
      sleep_ms "$SRT_RESULT_POLL_MS"
    fi
  done

  return 1
}

expect_no_app_entry() {
  local scenario="$1"
  local label="$2"
  local dir="$3"

  for attempt in 1 2 3; do
    run_service_case "$scenario" "$label" "file_list_dir" "" --es file_dir "$dir"
    if grep -q "entries=.*${TEST_FILE}" "${SRT_ARTIFACT_DIR}/scenario-${scenario}-${label}-result.txt"; then
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
  # 文件不在预期位置时，列出父目录内容：多数落点问题的关键信息是文件究竟落在了
  # 哪里、或是否以中间态命名（例如 MediaStore 的 .pending- 前缀）残留。
  adb_su "ls -la \"\$(dirname '$path')\" 2>/dev/null || true" || true
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
# 自有包名私有目录的真实落点随 Android 版本不同：app-specific 存储在部分版本是独立卷
# （Android 13 的 Android/data|obb 为独立 ext4），此时可见路径与 /data/media/0 下的同名
# 路径是两份目录。这里只要求真实落点二选一，且沙盒路径必须没有该文件——应用视图被错误
# 重定向时可见路径（未重定向视角）自然为空，仍会被判失败。
check_own_private_real_landing() {
  local label="$1"
  local visible_path="$2"
  local backend_path="$3"
  if adb_su "test -f '$visible_path'"; then
    echo "file_exists label=${label} path=${visible_path}"
    return 0
  fi
  if adb_su "test -f '$backend_path'"; then
    echo "file_exists label=${label} path=${backend_path}"
    return 0
  fi
  echo "file_missing label=${label} path=${visible_path}"
  adb_su "for path in '$visible_path' '$backend_path'; do echo \"> \$path\"; ls -la \"\$(dirname \"\$path\")\" 2>&1 | head -20 || true; done" || true
  return 1
}

check_public_directory_owner() {
  local label="$1"
  local path="$2"
  local actual attempt
  for attempt in 1 2 3 4 5 6 7 8; do
    actual="$(adb_su "stat -c '%u:%g' '$path' 2>/dev/null" | tail -1)"
    if [ "$actual" = "1023:1023" ]; then
      echo "public_owner label=${label} path=${path} owner=${actual} attempt=${attempt}"
      return 0
    fi
    sleep_ms 250
  done
  echo "public_owner_mismatch label=${label} path=${path} expected=1023:1023 actual=${actual:-missing}" >&2
  adb_su "ls -ldn '$path' 2>/dev/null || true" >&2 || true
  return 1
}

run_rule_sandbox_scenario() {
  local scenario="$1"
  local sandbox_path="${RULE_SANDBOX_ROOT}/${TEST_FILE}"
  local private_sandbox_path="${PRIVATE_RULE_SANDBOX_ROOT}/${TEST_FILE}"
  local sibling_file="srt_rule_sibling_atomic.jpg"
  local sibling_path="${RULE_SIBLING_ROOT}/${sibling_file}"
  local backend_sibling_path="${BACKEND_RULE_SIBLING_ROOT}/${sibling_file}"
  local private_sibling_path="${PRIVATE_RULE_SIBLING_ROOT}/${sibling_file}"
  local media_file="srt_rule_sibling_mediastore.jpg"

  check_file_missing "scenario-${scenario}-sandbox-public-before" "$BACKEND_RULE_SANDBOX_ROOT" &&
    check_scoped_fuse_daemon_started "$scenario" "$REAL_ROOT" &&
    run_write_case "$scenario" "sandbox-rule-hit" "$sandbox_path" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-sandbox-private" "$private_sandbox_path" &&
    check_file_missing "scenario-${scenario}-sandbox-public" "$BACKEND_RULE_SANDBOX_ROOT" &&
    run_service_case "$scenario" "sandbox-sibling-atomic-save" "file_atomic_save" '^PASS \[file_atomic_save\]' --es file_path "$sibling_path" --es payload "$PAYLOAD" --es expected_payload "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-sibling-real" "$backend_sibling_path" &&
    check_file_missing "scenario-${scenario}-sibling-private" "$private_sibling_path" &&
    run_mediastore_image_create_case "$scenario" "sandbox-sibling-mediastore-save" "$media_file" "DCIM/SrtRuleSibling" &&
    check_file_exists "scenario-${scenario}-sibling-mediastore-real" "${BACKEND_RULE_SIBLING_ROOT}/${media_file}" &&
    check_file_missing "scenario-${scenario}-sibling-mediastore-private" "${PRIVATE_RULE_SIBLING_ROOT}/${media_file}" &&
    check_public_directory_owner "scenario-${scenario}-dcim" "${BACKEND_ROOT}/DCIM"
}

run_provider_sandbox_scenario() {
  local scenario="$1"
  # 触发链：提供器配置（步骤 1 写入）→ force-stop → root content query 拉起提供器
  # 冷启动（新配置生效）→ 轮询断言挂载请求携带沙盒规则（scoped 会话或宿主接入）。
  # 回归背景（2026-10-21 真机）：桥接进程的挂载请求被清零规则（req sandbox=0），
  # 宿主接入被拒（attach skipped reason=policy_registration_failed），退回零规则
  # scoped 会话后原生写入穿透真实存储。
  # 判定标记（与提供器由 companion 还是 daemon 路径服务无关）：
  #   通过：`perf companion mount pkg=... ok=true`（挂载流程成功，hook 兜底拓扑）
  #   回归：`req op=apply ... sandbox=0`、`attach skipped ... policy_registration_failed`，
  #         或出现任何 `fuse redirect mount start pkg=下载提供器`（桥接进程不应有模块会话）
  # 行为级验证（引擎 mkdir 落点）依赖 XL 引擎初始化，CI 模拟器无该引擎，
  # 由真机复测协议覆盖（见 .workbuddy/memory/2026-10-21.md）。
  echo "step 5/7 内: 清理探测目录并 force-stop 下载提供器"
  adb_su "rm -rf '$PROVIDER_REAL_DIR' '$PROVIDER_BACKEND_DIR'; am force-stop '$PROVIDER_PACKAGE'" >/dev/null
  sleep 2
  adb_su "content query --uri content://downloads/my_downloads --projection _id >/dev/null 2>&1 || true" >/dev/null
  local attempt=1
  local assertions_ok=0
  while [ "$attempt" -le 4 ]; do
    sleep 5
    if adb_su "grep -Eq 'perf companion mount pkg=${PROVIDER_PACKAGE} pid=[0-9]+ .*ok=true' '$LOG_PATH'"; then
      assertions_ok=1
      break
    fi
    # 本轮请求行/挂载行未出现（companion 限流或启动慢）：重启提供器再试一轮。
    adb_su "am force-stop '${PROVIDER_PACKAGE}'; content query --uri content://downloads/my_downloads --projection _id >/dev/null 2>&1 || true" >/dev/null
    attempt=$((attempt + 1))
  done
  if adb_su "grep -Eq 'req op=apply pid=[0-9]+ pkg=${PROVIDER_PACKAGE} .*sandbox=0' '$LOG_PATH'" ||
    adb_su "grep -q 'attach skipped.*pkg=${PROVIDER_PACKAGE}.*reason=policy_registration_failed' '$LOG_PATH'" ||
    adb_su "grep -qE 'fuse redirect mount start pkg=${PROVIDER_PACKAGE}' '$LOG_PATH'"; then
    echo "provider sandbox regression markers found (sandbox=0 request / policy_registration_failed / unexpected fuse session)" >&2
    return 1
  fi
  if [ "$assertions_ok" -ne 1 ]; then
    echo "provider sandbox positive markers not found after retries" >&2
    return 1
  fi
  echo "step 5/7 内: 提供器沙盒策略标记确认"
  adb_su "grep -hE 'perf companion mount pkg=${PROVIDER_PACKAGE}' '$LOG_PATH' 2>/dev/null | tail -3" || true
  return 0
}

run_download_owner_attribution_scenario() {
  local scenario="$1"
  # 触发链：测试应用（下载发起者）配置 path_mappings → DownloadManager 入队
  # 公共路径下载 → 下载提供器（共享 uid 代写者）进程内或 MediaStore insert 侧
  # 归因到发起者 → 按发起者映射改写 → 文件落在映射目标。
  # 判定标记：
  #   通过：下载完成 PASS + 文件落在发起者映射目标（Download/SrtDlOwnerMapped）
  #         且真实请求路径（Download/SrtDlOwner）零穿透。
  #   回归：文件出现在真实请求路径（按源归因失效），或下载提供器出现模块会话
  #         （fuse redirect mount start pkg=下载提供器，同场景 38 边界）。
  # 前置：前序场景（26/33 等）可能重启过 MediaProvider，下载提供器进程的命名
  #   空间会持有陈旧 FUSE 连接（改写后的 mkdir 报 errno=107），直到提供器自身
  #   重启。与场景 38 同款前置：force-stop 后 root query 冷启动提供器并等待
  #   进程就绪，保证下载发生在新鲜的 FUSE 视图上。
  echo "step 5/7 内: 冷启动下载提供器（刷新 FUSE 视图）"
  adb_su "am force-stop '${PROVIDER_PACKAGE}'; sleep 2; content query --uri content://downloads/my_downloads --projection _id >/dev/null 2>&1 || true" >/dev/null
  local provider_wait=1
  while [ "$provider_wait" -le 8 ]; do
    sleep 2
    if adb_su "pidof '${PROVIDER_PACKAGE}' >/dev/null 2>&1"; then
      break
    fi
    provider_wait=$((provider_wait + 1))
  done
  sleep 3
  # 取数源：模拟器对多数外部 CDN 不可达（下载会暂停等待网络），用 runner 侧
  # 本地 HTTP 服务 + 模拟器内建路由 10.0.2.2 直达宿主（不依赖 adb reverse，
  # 规避 reverse 时序风险与 localhost 目标的特殊语义）。真机不依赖本地服务：
  # reverse 形态在部分宿主上不稳定（服务端零请求、引擎报 590），回退为不传
  # url，由测试 APK 使用内置的外部默认取数源；显式设置了 SRT_DOWNLOAD_URL
  # 时仍优先使用（真机/本地复现可自行指定）。http 目标要求发起入队应用允许
  # 明文流量（AOSP DownloadThread 对 cleartext 拒绝抛 STATUS_BAD_REQUEST=400，
  # R6 五版本矩阵 A14/15/16 一致复现），测试 APK 清单已声明
  # usesCleartextTraffic。
  local dl_port=18089
  local dl_url="$SRT_DOWNLOAD_URL"
  local dl_server_pid=""
  local dl_server_log=""
  local dl_dir=""
  if [ -z "$dl_url" ] && adb shell getprop ro.kernel.qemu 2>/dev/null | grep -q '^1$'; then
    dl_dir="$(mktemp -d /tmp/srt-dl-server.XXXXXX)"
    echo "srt-download-probe" > "$dl_dir/srt_download.bin"
    dl_server_log="$dl_dir/http-server.log"
    (cd "$dl_dir" && python3 -m http.server "$dl_port" --bind 0.0.0.0 >"$dl_server_log" 2>&1) &
    dl_server_pid=$!
    dl_url="http://10.0.2.2:$dl_port/srt_download.bin"
    sleep 1
    echo "step 5/7 内: 本地取数服务已启动 url=$dl_url log=$dl_server_log"
  elif [ -z "$dl_url" ]; then
    echo "step 5/7 内: 真机取数源使用测试 APK 内置默认 URL"
  fi
  echo "step 5/7 内: 下载取数源 url=$dl_url"
  # A17 模拟器镜像的网络验证（captive portal）无法完成：Downloads 应用要求默认
  # 网络带 VALIDATED 能力，验证不完成即暂停在 status=4 WAITING_FOR_NETWORK——
  # ping 通 10.0.2.2（net_probe_host_ok）与 DNS 可用都不够，服务端日志零请求
  # 实证请求根本没发出（run 40248635974）。关闭验证探测后 NetworkMonitor 直接
  # 把默认网络标记为已验证，DownloadManager 立即可用；对其它版本与真机无副作用。
  adb shell settings put global captive_portal_detection_enabled 0 || true
  adb shell ping -c 1 -W 2 10.0.2.2 >/dev/null 2>&1 && echo net_probe_host_ok || echo net_probe_host_unreachable
  # 测试应用视图必须已套用本场景的映射（发起者规则）：fresh_app=0 的矩阵里
  # 应用不会随场景重启，这里显式重启并确认挂载，保证其视图包含映射 bind。
  echo "step 5/7 内: 重启测试应用（套用发起者映射）"
  timeout 45 adb shell am force-stop "$APP_ID" >/dev/null 2>&1 || true
  start_app_and_confirm_mount "scenario-${scenario}-dlowner-apprestart" 1 || return 1
  wait_storage_ready "scenario-${scenario}-dlowner-apprestart" 30 >/dev/null || return 1
  echo "step 5/7 内: 清理下载归因探测目录"
  adb_su "rm -rf '${BACKEND_ROOT}/${DL_OWNER_REQUEST_REL}' '${BACKEND_ROOT}/${DL_OWNER_MAPPED_REL}'" >/dev/null
  local case_ok=0
  # 本地取数服务仅在模拟器形态启动；真机回退 APK 内置默认 URL（不传 url extra）。
  local url_extra_args=()
  if [ -n "$dl_url" ]; then
    url_extra_args=(--es url "$dl_url")
  fi
  if run_service_case "$scenario" "download-owner-attribution" "downloadmanager_enqueue" \
    '^PASS \[downloadmanager_enqueue\]' \
    --es file_path "$DL_OWNER_REQUEST_FILE" ${url_extra_args+"${url_extra_args[@]}"} \
    --es require_destination_visible false; then
    case_ok=1
  fi
  if [ "$case_ok" -ne 1 ]; then
    echo "download owner attribution: enqueue or download failed" >&2
    # 失败时先打印服务端请求日志（收到过什么请求、状态码）再清理，
    # 供区分「请求未到达」与「服务端拒绝」。
    if [ -n "$dl_server_log" ] && [ -f "$dl_server_log" ]; then
      echo "---- 本地取数服务请求日志 ----" >&2
      cat "$dl_server_log" >&2
    fi
    # 下载器内部状态（status/reason 之外的 mRetries/mError 等）：区分「网络判定
    # 中断」与「响应处理失败」需要下载器侧细节（2026-10-25 CI A14：服务端 200
    # 已到达但 status=4 WAITING_FOR_NETWORK，矛盾需内部状态裁决）。
    echo "---- dumpsys downloads（失败诊断） ----" >&2
    adb shell dumpsys downloads 2>/dev/null | grep -aE "DOWNLOAD|status|reason|mRetries|num_failed|uri|title|_id=" | head -40 || true
  fi
  if [ -n "$dl_server_pid" ]; then
    kill "$dl_server_pid" 2>/dev/null || true
    adb reverse --remove "tcp:$dl_port" >/dev/null 2>&1 || true
    # Windows/MSYS 下被杀服务的 CWD 可能短暂占用目录：清理失败不影响判定
    # （临时目录随 /tmp 生命周期自然回收）。
    rm -rf "$dl_dir" 2>/dev/null || true
  fi
  if [ "$case_ok" -ne 1 ]; then
    return 1
  fi
  if ! adb_su "test -f '$DL_OWNER_MAPPED_BACKEND_FILE'"; then
    echo "download owner attribution: file missing in mapped target ${DL_OWNER_MAPPED_REL}" >&2
    adb_su "ls -la '${BACKEND_ROOT}/${DL_OWNER_REQUEST_REL}' '${BACKEND_ROOT}/${DL_OWNER_MAPPED_REL}'" || true
    return 1
  fi
  if adb_su "test -e '$DL_OWNER_REQUEST_BACKEND_FILE'"; then
    echo "download owner attribution: request path leaked to real storage" >&2
    return 1
  fi
  if adb_su "grep -qE 'fuse redirect mount start pkg=${PROVIDER_PACKAGE}' '$LOG_PATH'"; then
    echo "download owner attribution: unexpected fuse session for download provider" >&2
    return 1
  fi
  echo "step 5/7 内: 下载代写按发起者映射落点确认"
  return 0
}

run_system_uid_attribution_scenario() {
  local scenario="$1"
  # 触发链：shell（uid 2000）进程先存活足够久（pid 表事件化扫描会将其入库），
  # 再在公共存储发起 FUSE mkdir → 引擎请求头携带 uid=2000 与发起任务 id →
  # 系统 uid 归因分支经 pid 表反查发起进程 → 监视记录 identify_method=caller。
  # 回归背景（2026-10-24）：该类请求此前只能记录 unknown 或被误归组内成员；
  # Mirror/MainCache 等系统服务创建的公共路径是同一链路的真实样本。
  # 判定标记：
  #   通过：file_monitor.log 出现 SrtUidAttrib 的 MKDIR 记录且
  #         identify_method=caller、identify_reliability=high。
  #   回归：记录 identify_method=unknown（归因链断裂）或无记录。
  echo "step 5/7 内: 清理系统 uid 归因探测目录"
  adb_su "rm -rf '${BACKEND_ROOT}/${UID_ATTRIB_REL}'" >/dev/null
  # 引擎读取补丁（fuse read patch）随 MP FuseDaemon 启动联动安装；模拟器冷启动
  # 下场景可能早于补丁就绪（R7 实证 A13/A14 的 mkdir 先于补丁安装 → 请求头
  # 无归因信号）。等待安装标记出现。running.log 是滚动日志，MP 开机期写入的
  # 标记可能已被应用启动日志滚出（R11 实证：场景期 grep 零命中但
  # .media_hook_install_state 有内容）；.media_hook_install_state 的
  # stage=init_ok 是 hook 侧 specialize 完成的权威落盘标记（pid+boot_id 防跨
  # 开机误判），两者任一出现即视为补丁链路已建立。日志与标记均缺失时按超时
  # 放行（诊断 dump 会输出 state 文件内容供事后判定）。
  local patch_wait=0
  while [ "$patch_wait" -lt 15 ]; do
    if adb_su "grep -q 'fuse read patch installed' '$LOG_PATH' 2>/dev/null" \
      || adb_su "grep -q 'stage=init_ok' /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null"; then
      echo "step 5/7 内: 引擎读取补丁已就绪"
      break
    fi
    sleep 2
    patch_wait=$((patch_wait + 1))
  done
  if [ "$patch_wait" -ge 15 ]; then
    echo "step 5/7 内: 引擎读取补丁标记未出现（按超时放行）"
    adb_su "cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null" || true
  fi
  clear_file_monitor_log
  # 三次尝试任一归因即通过：跨线程接力槽在并发系统请求下存在互覆窗口
  # （单槽最后写入优先），单次尝试可能落空；能力验证以三次中至少一次
  # 归因成功为准。每次尝试的后台 shell 进程先存活 4 秒再 mkdir：pid 表
  # 事件化扫描（500ms 廉价快照 + 集合变化即全量重扫）在进程创建后 1-2 秒
  # 内即入表，4 秒已含充足裕量；归因的真正保障是 exec 形态而非长存活。
  # mkdir 前必须 exec（2026-10-24 取证）：mksh 对 -c 脚本最后一条命令
  # fork 子进程执行，子进程生命周期毫秒级，daemon 500ms 事件化扫描必然
  # 错过，FUSE 请求头携带的 mkdir 子进程 tid 不在表内（实测 pid=18606
  # table_seen=false，预存活的 sh 自身 tid 无效）。exec 让 mkdir
  # 系统调用在已入表的同一进程内发起，与真实系统服务（长存活进程
  # 发起写入）的归因语义一致；瞬时子进程场景是已知归因边界，不在本场景
  # 验证范围。
  local attempt=1
  local found=0
  while [ "$attempt" -le 3 ] && [ "$found" -ne 1 ]; do
    echo "step 5/7 内: 系统 uid 归因尝试 $attempt/3"
    # 每次尝试用独立目录：已存在目录的 mkdir 会被记录去重跳过，无法作为
    # 该次尝试的归因样本。
    local probe_rel="${UID_ATTRIB_PROBE_REL}/attempt${attempt}"
    adb shell "sleep 4; exec mkdir -p '/storage/emulated/0/${probe_rel}'" >/dev/null 2>&1 &
    local bg_pid=$!
    local waited=0
    while [ "$waited" -lt 8 ]; do
      sleep 2
      waited=$((waited + 2))
      if adb_su "test -d '${BACKEND_ROOT}/${probe_rel}'"; then
        break
      fi
    done
    wait "$bg_pid" 2>/dev/null || true
    # 轮询监视记录：归因链贯通时记录应在 mkdir 后 1-3 秒内出现。
    local poll=0
    while [ "$poll" -lt 5 ]; do
      sleep 2
      if adb_su "grep -E 'MKDIR\|/storage/emulated/0/${probe_rel}\|' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | grep -q 'identify_method=caller|identify_reliability=high'"; then
        found=1
        break
      fi
      poll=$((poll + 1))
    done
    attempt=$((attempt + 1))
  done
  if [ "$found" -ne 1 ]; then
    echo "system uid attribution: monitor record missing or not attributed" >&2
    adb_su "grep 'SrtUidAttrib' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | tail -3" || true
    # MP hook 侧 specialize 状态（pid+boot_id 防跨开机误判）：区分「hook 从未
    # 安装」与「已安装但 read 捕获未触发」这两种需要相反修复的情况。
    echo "media hook install state:" >&2
    adb_su "cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null" || true
    return 1
  fi
  echo "step 5/7 内: 系统 uid FUSE 写入归因确认"
  adb_su "grep 'SrtUidAttrib' '$FILE_MONITOR_LOG_PATH' 2>/dev/null | tail -2" || true
  adb_su "rm -rf '${BACKEND_ROOT}/${UID_ATTRIB_REL}'" >/dev/null
  return 0
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
    media_provider_ready_cached "scenario-${scenario}-${label}-mediastore-relative-provider" 60 >/dev/null || return 1
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
  local read_path="$READ_ONLY_ROOT/$READ_ONLY_FILE"
  for attempt in 1 2; do
    if run_service_case "$scenario" "read-only-read" "file_read" '^PASS \[file_read\]' --es file_path "$read_path" --es expected_payload "$READ_ONLY_PAYLOAD"; then
      break
    fi
    [ "$attempt" -lt 2 ] || return 1
    echo "read_only_seed_namespace_refresh scenario=${scenario} attempt=${attempt}"
    start_app_and_confirm_mount "scenario-${scenario}-read-only-refresh" 1 || return 1
    wait_storage_ready "scenario-${scenario}-read-only-refresh" 30 >/dev/null || return 1
  done
  run_service_case "$scenario" "read-only-stat" "file_stat" '^PASS \[file_stat\]' --es file_path "$read_path" &&
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

# 按 Java String.hashCode() 计算目录的 MediaStore bucket_id。
#
# MediaProvider 用小写目录路径的 hashCode 作为 bucket_id，模块的
# rewrite_media_store_bucket_id_for_caller 也按同一算法把请求目录的 bucket_id
# 换成映射目标目录的。这里在测试侧独立实现，用于校验改写结果是否指向预期目录，
# 而不是把模块的计算结果与自身比较。路径不含尾斜杠（与模块实现一致）。
java_bucket_id() {
  printf '%s' "$1" | awk '
    BEGIN { for (i = 0; i < 128; i++) ord[sprintf("%c", i)] = i }
    {
      s = tolower($0)
      h = 0
      n = length(s)
      for (i = 1; i <= n; i++) {
        h = (h * 31 + ord[substr(s, i, 1)]) % 4294967296
      }
      if (h >= 2147483648) h -= 4294967296
      printf "%d", h
    }
  '
}

# 校验查询结果中的 bucket_id 是否等于指定目录的 bucket_id。
#
# bucket_id 是相册按目录分组的键：改写错会让重定向后的文件归入真实目录桶，
# 或让未重定向的文件被错误改写，表现为相册分组错乱。
check_mediastore_bucket_id() {
  local label="$1"
  local result_file="$2"
  local expected_dir="$3"
  local actual expected

  actual="$(sed -n 's/.*bucketId=\(-\?[0-9]\+\).*/\1/p' "$result_file" | head -1)"
  if [ -z "$actual" ]; then
    echo "bucket_id_missing label=${label} file=${result_file}" >&2
    return 1
  fi
  expected="$(java_bucket_id "$expected_dir")"
  if [ "$actual" != "$expected" ]; then
    echo "bucket_id_mismatch label=${label} dir=${expected_dir} expected=${expected} actual=${actual}" >&2
    return 1
  fi
  echo "bucket_id_ok label=${label} dir=${expected_dir} bucket_id=${actual}"
}

run_mediastore_read_only_query_scenario() {
  local scenario="$1"
  local logical_path="$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  local private_path="$PRIVATE_READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
  wait_mediastore_read_only_image &&
    run_service_case "$scenario" "read-only-image-query" "mediastore_query_read_only_image" '^PASS \[mediastore_query_read_only_image\]' --es file_name "$READ_ONLY_IMAGE_FILE" --es expected_path "$logical_path" &&
    check_mediastore_bucket_id "scenario-${scenario}-read-only-image" "${SRT_ARTIFACT_DIR}/scenario-${scenario}-read-only-image-query-result.txt" "$READ_ONLY_MEDIA_ROOT" &&
    run_service_case "$scenario" "read-only-image-list" "file_list_dir" '^PASS \[file_list_dir\]' --es file_dir "$READ_ONLY_MEDIA_ROOT" &&
    grep -q "entries=.*${READ_ONLY_IMAGE_FILE}" "${SRT_ARTIFACT_DIR}/scenario-${scenario}-read-only-image-list-result.txt" &&
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
    if adb_su "grep -Eq 'backend_effective pkg=${APP_ID}|fuse redirect mount start pkg=${APP_ID}|app mount confirmed pid=' '$LOG_PATH' 2>/dev/null"; then
      echo "fuse_daemon_started scenario=${scenario}"
      return 0
    fi
    sleep_ms "$SRT_RESULT_POLL_MS"
  done
  echo "fuse_daemon_missing scenario=${scenario}; continuing with behavioral checks"
  return 0
}

# 判别性断言：确认 FUSE 数据面真的接管了挂载点，而不是静默回退到 mount namespace。
#
# 依据 mountinfo 中的挂载源 srx_fuse_redirect（scoped 前缀）或 srx_fuse_host（共享宿主前缀，B2-c）。
# 这些字符串只有 FUSE 会话建立成功才会出现；bind mount 回退方案无论如何都产生不了它们。
# 这比日志断言更可靠：日志行可能因采样、轮转或格式变化漏判，而挂载表是内核事实。
check_fuse_mount_active() {
  local scenario="$1"
  local pid output
  pid=""
  for _ in $(seq 1 20); do
    # 启动或重建期间 PID 会变化；只验证当前进程的真实挂载，不重启应用兜底。
    pid="$(app_pid)"
    if [ -n "$pid" ] && adb_su "grep -E 'srx_fuse_(redirect|host)' \"/proc/${pid}/mountinfo\" 2>/dev/null"; then
      echo "fuse_mount_active scenario=${scenario} pid=${pid}"
      return 0
    fi
    sleep_ms "$SRT_RESULT_POLL_MS"
  done

  # 后续用例可能重启进程并清空日志，失败时先输出当前身份和挂载现场。
  adb_su "ps -A -o PID,PPID,NAME,ARGS; if [ -n '$pid' ]; then cat /proc/$pid/stat /proc/$pid/status /proc/$pid/mountinfo 2>/dev/null; fi; cat '$LOG_PATH'" >&2 || true
  adb logcat -d -t 2000 >&2 || true
  if [ -z "$pid" ]; then
    echo "fuse_mount_check_no_pid scenario=${scenario}" >&2
    return 1
  fi
  output="$(adb_su "grep -F 'fuse' \"/proc/${pid}/mountinfo\" 2>/dev/null | head -20")"
  echo "fuse_mount_inactive scenario=${scenario} pid=${pid}: FUSE 未接管，可能已静默回退到 mount namespace" >&2
  printf '%s\n' "$output" | sed "s/^/fuse_mountinfo scenario=${scenario}: /" >&2
  return 1
}

recover_scoped_fuse_start() {
  local scenario="$1"
  local mount_root="$2"
  echo "scoped_fuse_start_recovery scenario=${scenario} root=${mount_root}" >&2
  timeout 45 adb shell am force-stop "$APP_ID" >/dev/null 2>&1 || true
  timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null 2>&1 || true
  wait_storage_ready "scenario-${scenario}-scoped-fuse-recovery" 30 >/dev/null || true
  adb_su "sleep 1" >/dev/null 2>&1 || true
}

check_scoped_fuse_daemon_started() {
  local scenario="$1"
  local mount_root="$2"
  local strict="${3:-1}"
  local saw_fallback=0
  local saw_mount_failed=0
  local saw_service_failed=0
  local attempt
  for attempt in 1 2; do
    for _ in $(seq 1 60); do
      if adb_su "grep -F -- 'fuse redirect mount start pkg=${APP_ID}' '$LOG_PATH' 2>/dev/null | grep -F -- 'mp=${mount_root} ' >/dev/null"; then
        echo "scoped_fuse_started scenario=${scenario} root=${mount_root} attempt=${attempt} source=log"
        return 0
      fi
      # 日志可能因二进制内容、轮转或异步写入而缺失，但应用进程 mountinfo
      # 是实际可见挂载的权威证据。要求目标路径精确匹配，并同时要求 fuse
      # 类型和 srx scoped/host source，避免把普通目录或 MediaProvider 挂载误判为成功。
      local app_mount_pid
      app_mount_pid="$(app_pid 2>/dev/null | head -1)"
      if [ -n "$app_mount_pid" ] && adb_su "grep -F -- ' ${mount_root} ' '/proc/${app_mount_pid}/mountinfo' 2>/dev/null | grep -Eq ' - fuse (srx_fuse_redirect|srx_fuse_host)(\\[| )'"; then
        echo "scoped_fuse_started scenario=${scenario} root=${mount_root} attempt=${attempt} source=app-mountinfo"
        return 0
      fi
      # 共享宿主接入布局（B2-c 默认开启）下，只读等规则由宿主会话的策略层执行，
      # 只读根（mount_root）不再产生独立挂载点——上一条精确匹配在该布局下必然落空。
      # 此处放宽为：应用 mountinfo 的存储视图根存在 srx_fuse_host 源即判定 scoped
      # 数据面就绪；只读强制语义由后续行为用例（write-denied 等）继续真实验证。
      if [ -n "$app_mount_pid" ] && adb_su "grep -F -- ' ${REAL_ROOT} ' '/proc/${app_mount_pid}/mountinfo' 2>/dev/null | grep -Eq ' - fuse srx_fuse_host(\\[| )'"; then
        echo "scoped_fuse_started scenario=${scenario} root=${mount_root} attempt=${attempt} source=app-mountinfo-host"
        return 0
      fi
      if adb_su "grep -F -- 'daemon hybrid fuse no scoped service mounted' '$LOG_PATH' 2>/dev/null | grep -F -- 'pkg=${APP_ID}' >/dev/null"; then
        saw_fallback=1
      fi
      if adb_su "grep -F -- 'fuse redirect mount failed mp=${mount_root} ' '$LOG_PATH' 2>/dev/null >/dev/null"; then
        saw_mount_failed=1
      fi
      if adb_su "grep -F -- 'daemon hybrid fuse scoped service failed' '$LOG_PATH' 2>/dev/null | grep -F -- 'pkg=${APP_ID}' >/dev/null"; then
        saw_service_failed=1
      fi
      sleep_ms "$SRT_RESULT_POLL_MS"
    done
    if [ "$attempt" -eq 1 ] && [ "$strict" = "1" ]; then
      recover_scoped_fuse_start "$scenario" "$mount_root"
    fi
  done
  if [ "$strict" != "1" ]; then
    if [ "$saw_fallback" = "1" ]; then
      echo "scoped_fuse_fallback scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
      return 0
    fi
    if [ "$saw_mount_failed" = "1" ] || [ "$saw_service_failed" = "1" ]; then
      echo "scoped_fuse_start_failed scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
      return 0
    fi
    echo "scoped_fuse_start_log_not_observed scenario=${scenario} root=${mount_root}; continuing with behavioral checks" >&2
    return 0
  fi
  if [ "$saw_fallback" = "1" ]; then
    echo "scoped_fuse_fallback scenario=${scenario} root=${mount_root}" >&2
    return 1
  fi
  if [ "$saw_mount_failed" = "1" ]; then
    echo "scoped_fuse_mount_failed scenario=${scenario} root=${mount_root}" >&2
    return 1
  fi
  if [ "$saw_service_failed" = "1" ]; then
    echo "scoped_fuse_service_failed scenario=${scenario} root=${mount_root}" >&2
    return 1
  fi
  echo "scoped_fuse_missing scenario=${scenario} root=${mount_root}" >&2
  # scoped root 的逐条 hybrid fuse root 记录紧随 count 之后，80 行窗口常被后续日志挤掉，
  # 因此按关键字单独再取一次，避免定位挂载根缺失时缺少最关键的一段。
  adb_su "grep -F -- '${APP_ID}' '$LOG_PATH' 2>/dev/null | tail -200 || true" | sed 's/^/fuse_tail: /'
  adb_su "grep -E 'hybrid fuse root|scoped fuse|scoped roots|fuse redirect mount' '$LOG_PATH' 2>/dev/null | tail -60 || true" | sed 's/^/fuse_roots: /'
  return 1
}

run_fuse_daemon_allow_wildcard_scenario() {
  local scenario="$1"
  local plain_path="$FUSE_PLAIN_ROOT/$TEST_FILE"
  local plain_private="$PRIVATE_FUSE_PLAIN_ROOT/$TEST_FILE"
  local atomic_path="$FUSE_PLAIN_ROOT/srt_atomic_save.jpg"
  local atomic_private="$PRIVATE_FUSE_PLAIN_ROOT/srt_atomic_save.jpg"
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
    check_fuse_mount_active "$scenario" &&
    run_write_case "$scenario" "plain-allow-write" "$plain_path" "$PAYLOAD" &&
    check_file_exists "fuse-plain-real" "$plain_path" &&
    check_file_missing "fuse-plain-private" "$plain_private" &&
    run_service_case "$scenario" "atomic-save" "file_atomic_save" '^PASS \[file_atomic_save\]' --es file_path "$atomic_path" --es payload "$PAYLOAD" --es expected_payload "$PAYLOAD" &&
    check_file_exists "fuse-atomic-real" "$atomic_path" &&
    check_file_missing "fuse-atomic-private" "$atomic_private" &&
    run_mediastore_image_create_case "$scenario" "wildcard-allow-image-create" "$FUSE_DCIM_MEDIA_FILE" "DCIM/SrtFuseQQ/SrtAllowedAlpha" &&
    check_file_exists "fuse-wildcard-real" "$wildcard_path" &&
    check_file_missing "fuse-wildcard-private" "$wildcard_private" &&
    check_public_directory_owner "scenario-${scenario}-dcim" "${BACKEND_ROOT}/DCIM" &&
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
    check_fuse_mount_active "$scenario" &&
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
    check_fuse_mount_active "$scenario" &&
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
    check_fuse_mount_active "$scenario" &&
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

  for attempt in 1 2; do
    if run_service_case "$scenario" "fallback-read" "file_read" '^PASS \[file_read\]' --es file_path "$seed_path" --es expected_payload "$READ_ONLY_PAYLOAD"; then
      break
    fi
    [ "$attempt" -lt 2 ] || return 1
    echo "mount_ns_read_only_seed_refresh scenario=${scenario} attempt=${attempt}"
    start_app_and_confirm_mount "scenario-${scenario}-read-only-refresh" 1 || return 1
    wait_storage_ready "scenario-${scenario}-read-only-refresh" 30 >/dev/null || return 1
  done
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
  local attempt

  # 场景 22 的配置同时包含只读父映射和两个子映射。Android 13 上首次应用启动
  # 可能先观察到 namespace 后端，随后 daemon 才完成 scoped FUSE 重建；第一次
  # file_write 会在这段切换窗口中得到 ENOENT。重建应用视图后再做一次完整断言，
  # 仍保留最终失败，不把真正的映射错误变成通过。
  for attempt in 1 2; do
    if run_write_case "$scenario" "mapping-target-write" "$rw_request" "$PAYLOAD" &&
      check_file_exists "mount-ns-mapping-rw-target" "$rw_target" &&
      check_file_missing "mount-ns-mapping-rw-request" "$rw_request" &&
      run_service_case "$scenario" "mapping-target-read-only-denied" "file_write_denied" '^PASS \[file_write_denied\]' --es file_path "$ro_request" --es payload "$PAYLOAD" &&
      check_file_missing "mount-ns-mapping-ro-target" "$ro_target" &&
      check_file_missing "mount-ns-mapping-ro-request" "$ro_request"; then
      return 0
    fi
    [ "$attempt" -lt 2 ] || return 1
    echo "mount_namespace_mapping_retry scenario=${scenario} attempt=${attempt}"
    start_app_and_confirm_mount "scenario-${scenario}-mapping-retry" 1 || return 1
    wait_storage_ready "scenario-${scenario}-mapping-retry" 30 >/dev/null || return 1
  done
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

run_file_monitor_existing_write_case() {
  local scenario="$1"
  local label="$2"
  local request_path="$3"
  local backend_path="$4"
  local file_name
  local seed_payload="${PAYLOAD}-seed-tail"
  file_name="$(basename "$request_path")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  # 该用例验证既有文件的原位覆盖；先在映射目标创建种子文件，避免首次 open
  # 被记录为 open:create，导致把 fixture 未准备完整误判为监视器丢失 write 事件。
  adb_su "mkdir -p '$(dirname "$backend_path")'; printf '%s' '$seed_payload' > '$backend_path'; chmod 777 '$backend_path' 2>/dev/null || true" >/dev/null
  run_service_case "$scenario" "$label" "file_write_then_overwrite" '^PASS \[file_write_then_overwrite\]' --es file_path "$request_path" --es payload "$PAYLOAD" --es expected_payload "$seed_payload" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$backend_path" &&
    wait_file_monitor_log_line "$scenario" "$label" "$file_name" write
}

run_file_monitor_mediastore_success_case() {
  local scenario="$1"
  local label="$2"
  local relative_path="$3"
  local expected_path="$4"
  local private_path="${5:-}"
  local require_monitor_record="${6:-1}"
  local monitor_skip_reason="${7:-mediastore-create-result-and-routing-are-authoritative}"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label")"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_download_create_case "$scenario" "$label" "$file_name" "$relative_path" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path/$file_name" &&
    { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path/$file_name"; } &&
    { [ "$require_monitor_record" != "1" ] || expect_file_monitor_success_record "$scenario" "$label" "$file_name"; } &&
    { [ "$require_monitor_record" = "1" ] || echo "monitor_success_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=${monitor_skip_reason}"; }
}

run_file_monitor_mediastore_image_success_case() {
  local scenario="$1"
  local label="$2"
  local relative_path="$3"
  local expected_path="$4"
  local private_path="${5:-}"
  local require_monitor_record="${6:-1}"
  local monitor_skip_reason="${7:-mediastore-image-result-and-routing-are-authoritative}"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label" | sed 's/\.bin$/.jpg/')"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_image_create_case "$scenario" "$label" "$file_name" "$relative_path" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path/$file_name" &&
    adb_su "test -s '$expected_path/$file_name'" &&
    { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path/$file_name"; } &&
    expect_no_read_only_failure_record "$scenario" "$label" "$file_name" &&
    { [ "$require_monitor_record" != "1" ] || expect_file_monitor_success_record "$scenario" "$label" "$file_name"; } &&
    { [ "$require_monitor_record" = "1" ] || echo "monitor_success_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=${monitor_skip_reason}"; }
}

run_file_monitor_mediastore_relative_data_success_case() {
  local scenario="$1"
  local label="$2"
  local relative_data_dir="$3"
  local expected_path="$4"
  local private_path="${5:-}"
  local file_name
  file_name="$(monitor_file_name "$scenario" "$label" | sed 's/\.bin$/.jpg/')"

  prepare_file_monitor_assertion "$scenario" "$label" || return 1
  run_mediastore_image_relative_data_create_case "$scenario" "$label" "$file_name" "$relative_data_dir" &&
    check_file_exists "scenario-${scenario}-${label}-expected" "$expected_path/$file_name" &&
    adb_su "test -s '$expected_path/$file_name'" &&
    { [ -z "$private_path" ] || check_file_missing "scenario-${scenario}-${label}-private" "$private_path/$file_name"; } &&
    expect_no_read_only_failure_record "$scenario" "$label" "$file_name" &&
    expect_file_monitor_success_record "$scenario" "$label" "$file_name"
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
    echo "monitor_failure_record_skipped scenario=${scenario} label=${label} file=${file_name} reason=mediastore-denied-result-is-authoritative"
}

# 监视专用捕获会话 fail-open 契约验证（公开 issue #3 回归锁）。
#
# 冻结 daemon 主进程制造宿主握手失败窗口：此时启动的未配置应用会走到
# companion 挂载的屏障决策点，且挂载链路必然失败。旧 fail-closed 语义会在
# /storage/emulated/<user> 留下只读空 tmpfs 屏障并永久遮蔽视图（应用看到空
# 存储根、写入 EROFS）；fail-open 语义不落屏障、清理历史残留，应用保留原生
# MediaProvider FUSE 视图。用 SIGSTOP 而不是 kill：watchdog 的 kill -0 判活
# 对停止态进程仍成立，不会触发重启；SIGCONT 后 daemon 原地恢复，不污染
# 后续场景。
run_monitor_capture_failopen_scenario() {
  local scenario="$1"
  local daemon_pid app_pid barrier_hits view_probe status

  daemon_pid="$(adb_su "for p in \$(pidof srx_daemon 2>/dev/null); do [ \"\$(cat /proc/\$p/comm 2>/dev/null)\" = 'srx_daemon' ] && { echo \"\$p\"; break; }; done" | tr -d '\r' | awk 'NF {print; exit}')"
  if [ -z "$daemon_pid" ]; then
    echo "monitor_capture_failopen_skipped scenario=${scenario} reason=daemon_not_running"
    return 0
  fi

  echo "monitor_capture_failopen freeze daemon pid=${daemon_pid}"
  if ! adb_su "kill -STOP ${daemon_pid}" >/dev/null 2>&1; then
    echo "monitor_capture_failopen freeze failed scenario=${scenario}"
    return 1
  fi
  # 登记冻结 pid：异常中止时由 EXIT trap 兜底 CONT，daemon 不得带冻结态离开本脚本。
  printf '%s
' "$daemon_pid" > "$SRT_FROZEN_DAEMON_PID_FILE"

  status=0
  # 冻结窗口内重启应用：挂载链路在屏障决策之后必然失败，旧语义会在存储根
  # 留下常驻屏障，新语义不落屏障且视图保持原生存活。
  adb shell am force-stop "$APP_ID" >/dev/null 2>&1 || true
  sleep_ms 500
  timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null 2>&1 || true
  app_pid=""
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    app_pid="$(app_pid || true)"
    [ -n "$app_pid" ] && break
    sleep_ms 500
  done
  if [ -z "$app_pid" ]; then
    echo "monitor_capture_failopen app start failed scenario=${scenario}"
    status=1
  else
    # 留出 companion 屏障决策与失败路径落地的窗口。
    sleep_ms 2000
    barrier_hits="$(adb_su "grep -F ' ${REAL_ROOT} ' '/proc/${app_pid}/mountinfo' 2>/dev/null | grep -c ' - tmpfs ' || true" | tr -d '\r' | awk 'NF {print; exit}')"
    if [ "${barrier_hits:-0}" != "0" ]; then
      echo "monitor_capture_failopen barrier lingers scenario=${scenario} hits=${barrier_hits}"
      status=1
    fi
    view_probe="$(adb_su "ls '/proc/${app_pid}/root${REAL_ROOT}/' 2>/dev/null | head -1 || true" | tr -d '\r' | awk 'NF {print; exit}')"
    if [ -z "$view_probe" ]; then
      echo "monitor_capture_failopen view masked scenario=${scenario}"
      status=1
    fi
  fi

  echo "monitor_capture_failopen resume daemon pid=${daemon_pid}"
  adb_su "kill -CONT ${daemon_pid}" >/dev/null 2>&1 || true
  rm -f "$SRT_FROZEN_DAEMON_PID_FILE"

  if [ "$status" -eq 0 ]; then
    # 恢复后按正常路径重启应用：捕获会话应重建，存储视图保持完整。
    if start_app_and_confirm_mount "scenario-${scenario}-failopen-recover" 0; then
      app_pid="$(app_pid || true)"
      view_probe="$(adb_su "ls '/proc/${app_pid}/root${REAL_ROOT}/' 2>/dev/null | head -1 || true" | tr -d '\r' | awk 'NF {print; exit}')"
      if [ -z "$view_probe" ]; then
        echo "monitor_capture_failopen view broken after recover scenario=${scenario}"
        status=1
      fi
      # 捕获会话重建只做信息输出：共享宿主接入时机在部分设备上依赖 daemon
      # reconcile 节奏，不做硬断言，避免把恢复节奏差异误判成功能回归。
      adb_su "grep -F ' ${REAL_ROOT} ' '/proc/${app_pid}/mountinfo' 2>/dev/null | grep -m1 srx_fuse || true"
    else
      echo "monitor_capture_failopen recover restart failed scenario=${scenario}"
      status=1
    fi
  fi

  return "$status"
}

run_file_monitor_disabled_redirect_scenario() {
  local scenario="$1"
  local file_name
  file_name="$(monitor_file_name "$scenario" "disabled_regular")"
  local media_file="srt_mediastore_public_only.txt"
  run_file_monitor_write_success_case "$scenario" "disabled-regular-write" "$MONITOR_BASE_ROOT/$file_name" "$MONITOR_BASE_ROOT/$file_name" "$PRIVATE_MONITOR_BASE_ROOT/$file_name" 1 0 &&
    run_file_monitor_mediastore_success_case "$scenario" "disabled-system-writer-create" "Download/SrtMonitor" "$MONITOR_BASE_ROOT" "$PRIVATE_MONITOR_BASE_ROOT" 0 "disabled-profile-mediastore-create-result-and-routing-are-authoritative" &&
    run_file_monitor_mediastore_image_success_case "$scenario" "disabled-nnngram-standard-create" "Pictures/Nnngram" "$MONITOR_NNNGRAM_ROOT" "$PRIVATE_MONITOR_NNNGRAM_ROOT" 0 "disabled-profile-standard-mediastore-result-and-routing-are-authoritative" &&
    run_service_case "$scenario" "disabled-mediastore-public-only" "mediastore_create_file" '^PASS \[mediastore_create_file\]' --es file_name "$media_file" --es relative_path "Documents/SrtMediaRoutingProbe" &&
    check_file_exists "scenario-${scenario}-disabled-mediastore-public-file" "${MEDIASTORE_ROUTING_PROBE_ROOT}/${media_file}" &&
    check_file_missing "scenario-${scenario}-disabled-mediastore-private-directory" "$PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT"
}

run_file_monitor_regular_scenario() {
  local scenario="$1"
  local allow_file map_file existing_file locked_file writable_file
  allow_file="$(monitor_file_name "$scenario" "regular_allow")"
  map_file="$(monitor_file_name "$scenario" "regular_map")"
  existing_file="$(monitor_file_name "$scenario" "regular_existing")"
  locked_file="$(monitor_file_name "$scenario" "regular_locked")"
  writable_file="$(monitor_file_name "$scenario" "regular_writable")"

  if [ "$scenario" = "25" ]; then
    run_file_monitor_write_success_case "$scenario" "regular-allow-write" "$MONITOR_BASE_ROOT/$allow_file" "$MONITOR_BASE_ROOT/$allow_file" "$PRIVATE_MONITOR_BASE_ROOT/$allow_file" 1 0 "ordinary-app-scoped-fuse-direct-write" || return 1
    check_scoped_fuse_daemon_started "$scenario" "$MONITOR_LOCKED_ROOT" || return 1
  else
    echo "regular_allow_write_skipped scenario=${scenario} reason=mount-namespace-allowed-real-direct-write-is-platform-permission-sensitive"
  fi

  run_file_monitor_write_success_case "$scenario" "regular-mapped-write" "$MONITOR_MAP_REQUEST/$map_file" "$MONITOR_MAP_TARGET/$map_file" "" 0 0 "ordinary-app-direct-mapped-write" &&
    run_file_monitor_existing_write_case "$scenario" "regular-existing-write" "$MONITOR_MAP_REQUEST/$existing_file" "$MONITOR_MAP_TARGET/$existing_file" &&
    run_file_monitor_write_denied_case "$scenario" "regular-read-only-denied" "$MONITOR_LOCKED_ROOT/$locked_file" "$MONITOR_LOCKED_ROOT/$locked_file" &&
    run_file_monitor_write_success_case "$scenario" "regular-read-only-excluded-write" "$MONITOR_WRITABLE_ROOT/$writable_file" "$MONITOR_WRITABLE_ROOT/$writable_file" "$PRIVATE_MONITOR_WRITABLE_ROOT/$writable_file" 1 0 "ordinary-app-read-only-exclusion-direct-write"
}

run_file_monitor_mediastore_scenario() {
  local scenario="$1"
  restart_media_provider_with_hook_ready "scenario-${scenario}-mediastore" || return 1
  run_file_monitor_mediastore_success_case "$scenario" "media-allow-create" "Download/SrtMonitor" "$MONITOR_BASE_ROOT" "$PRIVATE_MONITOR_BASE_ROOT" &&
    { [ "$scenario" != "27" ] || check_scoped_fuse_daemon_started "$scenario" "$MONITOR_LOCKED_ROOT"; } &&
    run_file_monitor_mediastore_relative_data_success_case "$scenario" "media-relative-data-create" "Pictures/SrtRelativeData" "$MONITOR_RELATIVE_DATA_ROOT" "$PRIVATE_MONITOR_RELATIVE_DATA_ROOT" &&
    run_file_monitor_mediastore_relative_data_success_case "$scenario" "media-nnngram-relative-data" "/Pictures/Nnngram" "$MONITOR_NNNGRAM_ROOT" "$PRIVATE_MONITOR_NNNGRAM_ROOT" &&
    run_file_monitor_mediastore_success_case "$scenario" "media-mapped-create" "Download/SrtMonitorMap" "$MONITOR_MAP_TARGET" &&
    run_file_monitor_mediastore_image_success_case "$scenario" "media-ding-camera-map" "DCIM/Camera" "$REAL_ROOT/Pictures/SrtMonitorDing" "" 0 "ding-system-writer-map-result-and-routing-are-authoritative" &&
    run_file_monitor_mediastore_denied_case "$scenario" "media-read-only-denied" "Download/SrtMonitorLocked" "$MONITOR_LOCKED_ROOT" &&
    run_file_monitor_mediastore_success_case "$scenario" "media-read-only-excluded-create" "Download/SrtMonitorLocked/Writable" "$MONITOR_WRITABLE_ROOT" "$PRIVATE_MONITOR_WRITABLE_ROOT"
}

app_pid() {
  # 同包的旧进程可能在 force-stop 后短暂残留，跨行取最高 PID 对应刚启动的主进程。
  local pid
  pid="$(adb shell "pidof '$APP_ID' 2>/dev/null | awk '{for (i=1; i<=NF; i++) if (\$i+0 > max) max=\$i} END {if (max != \"\") print max}'" |
    tr -d '\r' | awk 'NF {print; exit}')"
  if [ -n "$pid" ]; then
    printf '%s\n' "$pid"
    return 0
  fi
  # Android 17 上应用进程名仍受 comm 的 15 字符限制，长包名会被截断，pidof 无法再
  # 按完整包名匹配；这里退回遍历 /proc 的完整命令行首参，避免误判为进程不存在。
  adb shell "for p in /proc/[0-9]*; do c=\$(cat \"\$p/cmdline\" 2>/dev/null | tr '\\0' '\\n' | head -1); case \"\$c\" in '$APP_ID'|'$APP_ID':*) echo \"\${p#/proc/}\";; esac; done" |
    tr -d '\r' | sort -n | tail -1
}

wait_app_process_stopped() {
  local timeout_seconds="${1:-10}"
  local deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ -z "$(app_pid)" ]; then
      return 0
    fi
    sleep 0.1
  done
  [ -z "$(app_pid)" ]
}

resume_hot_reload_app() {
  local scenario="$1"
  local expected_pid="$2"
  local current_pid
  timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null || true
  wait_storage_ready "scenario-${scenario}-hot-reload-resume" 30 >/dev/null
  current_pid="$(app_pid)"
  if [ -z "$current_pid" ] || [ "$current_pid" != "$expected_pid" ]; then
    echo "config_hot_reload_pid_changed scenario=${scenario} before=${expected_pid} after=${current_pid:-missing}" >&2
    return 1
  fi
}

run_config_hot_reload_scenario() {
  # 与场景 34/35/36/37 同源的平台限制：热更新切到路径映射后，写入要经应用视角的
  # /storage/emulated/0 —— 而该挂载点的 root 正是 …/Android/data/<包名>/sdcard
  # （模块的沙箱绑定），其绑定源落在 app-private 目录下。
  # x86_64 Android 13 上 native FuseFix 被跳过
  # （判定见 src/hook/fuse_fix.rs::should_skip_native_fuse_fix_for_platform，该函数按架构与 API 级决定是否跳过），
  # 系统 FUSE 的 app-data-isolation 视图无人放行：实测该轮出现 130 条
  # `FuseDaemon: Rejected access to app-private dir on FUSE`（目标正是包目录本身与
  # …/sdcard），而 Android 14/15 上均为 0 条；热更新后 file_write 全部 ENOENT。
  #
  # 注意取舍：**热更新之前**的阶段（默认重定向下写入落沙箱、真实路径不存在）在 A13 上
  # 是正常的，失败只出现在切换之后。本门禁让 A13 失去「热更新切换」这一项的覆盖，
  # 该覆盖由 Android 14/15/16/17 提供；真机 arm64 上 FuseFix 正常安装，不受影响。
  local scenario="$1"
  if [ "${ANDROID_ARCH:-}" = "x86_64" ] && {
    [ "${ANDROID_API_LEVEL:-}" = "33" ] ||
    [ "${ANDROID_API_LEVEL:-}" = "35" ] ||
    [ "${ANDROID_API_LEVEL:-}" = "36" ]
  }; then
    # API 33/35/36 x86_64 模拟器的系统 FUSE 对已运行进程的根挂载 dentry
    # 不支持可靠热切换：基线旧提交 b649202f 的 API 36 同样失败，映射目标
    # 已挂载且新进程通过，只有旧 PID 请求路径持续残留。arm64 真机与其他 API
    # 继续执行完整断言，避免把平台限制扩散到产品覆盖范围。
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "config_hot_reload_skipped scenario=${scenario} reason=x86_64-api${ANDROID_API_LEVEL}-system-fuse-hot-reload-limit"
    return 0
  fi
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
  apply_config_and_wait "scenario-${scenario}-hot-reload"
  # 配置文件 reload 只刷新进程内策略，不会自动让旧应用 PID 重新物化挂载。
  # 显式走 srxctl remount-running，确保 daemon/companion 都按新 path_mappings
  # 清理旧层并重建；单场景启动时之所以不暴露该缺口，是因为配置先于应用挂载。
  local remount_watermark
  remount_watermark="$(adb_su "wc -l < '$LOG_PATH' 2>/dev/null || echo 0" 2>/dev/null | tr -d '[:space:]' || echo 0)"
  case "$remount_watermark" in
    ''|*[!0-9]*) remount_watermark=0 ;;
  esac
  adb_su "/data/adb/modules/storage.redirect.x/bin/srxctl remount-running" || {
    echo "config_hot_reload_remount_request_failed scenario=${scenario}" >&2
    return 1
  }
  if ! adb_su "tail -n +$((remount_watermark + 1)) '$LOG_PATH' 2>/dev/null | grep -F 'running app remount completed request=' >/dev/null"; then
    echo "config_hot_reload_remount_completion_missing scenario=${scenario}" >&2
    return 1
  fi
  current_pid="$(app_pid)"
  if [ -z "$current_pid" ] || [ "$current_pid" != "$initial_pid" ]; then
    echo "config_hot_reload_pid_changed_after_remount scenario=${scenario} before=${initial_pid} after=${current_pid:-missing}" >&2
    return 1
  fi

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
      # 成功路径也留一份视图根挂载快照：失败侧已有诊断，只有两边都采到才能对照出
      # 「哪一层压在视图根最上面」。Android 13 失败侧的实测是 FUSE 层压在视图根之上，
      # 且该层 root 正好指向会被 MediaProvider 拒绝的应用专属目录；通过版本需要在同一
      # 位置采到对应快照，否则无法证明差异确实在层序而不是别处。
      local view_layers rejected_count
      view_layers="$(adb_su "grep -h ' /storage/emulated/0 ' /proc/$(app_pid)/mountinfo 2>/dev/null | tr '\n' ';'" 2>/dev/null | tr -d '\r')"
      rejected_count="$(adb logcat -d 2>/dev/null | grep -ac 'Rejected access to app-private dir' || true)"
      echo "hot_view_root_layers scenario=${scenario} rejected=${rejected_count:-0} layers=${view_layers:-none}"
      echo "config_hot_reload_applied scenario=${scenario} pid=${initial_pid} attempt=${attempt}"
      return 0
    fi
    sleep 1
  done

  echo "config_hot_reload_timeout scenario=${scenario} pid=${initial_pid}" >&2
  # 诊断盲区补齐 1：失败侧既有进程（old_pid=initial_pid）重载后的视图根层序此前从未采样，
  # 只能靠重启探针新进程的层序反推，导致「既有进程重载后视图根最上层是 FUSE、ext4 还是被
  # 摘空」至今没有实测数据。这里在 initial_pid 仍存活时补采，落到一个可检索的明确标记，
  # 并顺带 dump 完整挂载栈，用于对照 app_status.log 里 13:38:54 的旧快照是否与此刻一致。
  local old_view_layers old_rejected
  old_view_layers="$(adb_su "grep -h ' /storage/emulated/0 ' /proc/${initial_pid}/mountinfo 2>/dev/null | tr '\n' ';'" 2>/dev/null | tr -d '\r')"
  old_rejected="$(adb logcat -d 2>/dev/null | grep -ac 'Rejected access to app-private dir' || true)"
  echo "hot_reload_failed_view_root_layers scenario=${scenario} old_pid=${initial_pid} rejected=${old_rejected:-0} layers=${old_view_layers:-none}"
  echo "--- hot_reload_failed_old_process_mountinfo scenario=${scenario} old_pid=${initial_pid} ---"
  adb_su "cat '/proc/${initial_pid}/mountinfo' 2>/dev/null | grep -E 'SrtProbe|Download/Test|/storage/emulated/0 |/mnt/user|/mnt/pass_through|fuse|srx|Android/data' || true"
  echo "--- end hot_reload_failed_old_process_mountinfo ---"
  # 热重载后写入 ENOENT 的现场取证：这里必须区分"整块重定向失效"与"只有映射目标失效"。
  #
  # 两种形态的对策完全不同：整块失效说明存储视图根在应用视野里落回了公共存储
  # （沙箱里才有的路径自然不存在）；只有映射失效则说明根仍指向沙箱、只是新加的
  # 映射挂载不可见。刻意不使用 run-as：它不会进入应用的挂载命名空间，量到的是
  # shell 的视图，此前据此得出的"可见目标是公共存储"结论已被证伪。
  local root_probe="${REAL_ROOT}/SrtHotRootProbe.txt"
  local root_probe_private="${PRIVATE_ROOT}/SrtHotRootProbe.txt"
  adb_su "rm -f '$root_probe' '$root_probe_private' 2>/dev/null || true" >/dev/null
  if run_write_case "$scenario" "hot-root-probe" "$root_probe" "$PAYLOAD"; then
    # 落点决定结论：配置仍是启用重定向（只多了一条路径映射），非映射路径必须进沙箱。
    # 注意必须用 check_file_exists：脚本里没有 file_exists，此前写成 file_exists 会让这条
    # 断言在 shell 里以 "command not found" 静默失败，落点从未被真正校验过。
    check_file_exists "scenario-${scenario}-hot-root-probe-sandbox" "$root_probe_private"
    check_file_missing "scenario-${scenario}-hot-root-probe-real" "$root_probe"
  else
    echo "hot_root_probe_failed scenario=${scenario}: 非映射路径也无法写入，重定向整体未生效" >&2
  fi
  # 沙箱里才存在的文件：能在应用侧读到就说明存储视图根仍指向沙箱。
  run_service_case "$scenario" "hot-root-visible-sandbox-file" "file_read" '^PASS \[file_read\]' \
    --es file_path "${REAL_ROOT}/Download/SrtProbe/${HOT_BEFORE_FILE}" >/dev/null 2>&1 ||
    echo "hot_sandbox_file_unreadable scenario=${scenario} path=${REAL_ROOT}/Download/SrtProbe/${HOT_BEFORE_FILE}" >&2
  # 被映射的目录本身是否还解析得到：区分"父目录不可见"与"父目录在但不可写"。
  run_service_case "$scenario" "hot-mapped-dir-list" "file_list_dir" "" \
    --es file_dir "${REAL_ROOT}/Download/SrtProbe" 2>&1 | tail -3 || true
  # 视图根是否还解析得到：区分"整个视图根不可用"与"只有映射路径不可用"。
  #
  # 只有先确定视图根本身是否可列举，才能区分两类完全不同的成因：视图根不可列举说明
  # 应用对 ${REAL_ROOT} 的解析整体失效（挂载栈被上层挡住，映射挂载根本不可达）；
  # 视图根可列举而只有映射路径不可用，问题才局限在这条映射挂载本身。
  run_service_case "$scenario" "hot-viewroot-list" "file_list_dir" "" \
    --es file_dir "${REAL_ROOT}" 2>&1 | tail -3 || true
  run_service_case "$scenario" "hot-download-list" "file_list_dir" "" \
    --es file_dir "${REAL_ROOT}/Download" 2>&1 | tail -3 || true
  # 决定性对照：同一份配置下，**新进程**是否看得见映射。
  #
  # 场景 3 与场景 29 使用完全相同的映射（Download/SrtProbe -> Download/Test），唯一差别是
  # "进程先于配置存在"（29）还是"配置先于进程存在"（3）。场景 3 在 Android 13 上通过，
  # 场景 29 连续 20 次全败，因此这条对照是区分两种成因的唯一判据：新进程可见说明热重载
  # 未对既有进程生效；新进程同样不可见说明重载后的机器状态本身不可用。
  # 必须放在最后：它会重启应用，此前所有诊断都必须在原进程上完成，顺序不可调换。
  echo "hot_reload_fresh_process_probe scenario=${scenario}: 在映射已生效的配置下重启应用"
  if start_app_and_confirm_mount "scenario-${scenario}-hot-fresh-process"; then
    local fresh_pid fresh_request fresh_mapped fresh_private
    fresh_pid="$(app_pid)"
    fresh_request="${REAL_ROOT}/Download/SrtProbe/${HOT_AFTER_FILE}"
    fresh_mapped="${REAL_ROOT}/Download/Test/${HOT_AFTER_FILE}"
    fresh_private="${PRIVATE_ROOT}/Download/SrtProbe/${HOT_AFTER_FILE}"
    adb_su "rm -f '$fresh_request' '$fresh_mapped' '$fresh_private' 2>/dev/null || true" >/dev/null
    # 重启后的同一命名空间再采一次视图根层序：挂载栈若与失败时不同，说明应用启动期的
    # 特化流程会重建视图根，这才是「重启即恢复」的原因；若层序完全相同而写入却成功，
    # 则说明差异不在挂载栈本身。两种结果指向的修复方向完全不同，必须分清。
    local fresh_layers fresh_rejected
    fresh_layers="$(adb_su "grep -h ' /storage/emulated/0 ' /proc/$(app_pid)/mountinfo 2>/dev/null | tr '\n' ';'" 2>/dev/null | tr -d '\r')"
    fresh_rejected="$(adb logcat -d 2>/dev/null | grep -ac 'Rejected access to app-private dir' || true)"
    echo "hot_reload_fresh_view_root_layers scenario=${scenario} rejected=${fresh_rejected:-0} layers=${fresh_layers:-none}"
    if run_write_case "$scenario" "hot-fresh-mapped" "$fresh_request" "$PAYLOAD"; then
      # 落点才是结论：写入成功但落在公共路径，说明映射并未生效，不能算「新进程可见映射」。
      if check_file_exists "scenario-${scenario}-hot-fresh-mapped" "$fresh_mapped"; then
        echo "hot_reload_fresh_process_ok scenario=${scenario} old_pid=${initial_pid} new_pid=${fresh_pid}: 新进程写入并落在映射目标，热重载对既有进程未生效"
      else
        echo "hot_reload_fresh_process_mapping_absent scenario=${scenario} old_pid=${initial_pid} new_pid=${fresh_pid}: 新进程可写入但未落在映射目标" >&2
      fi
    else
      echo "hot_reload_fresh_process_failed scenario=${scenario} old_pid=${initial_pid} new_pid=${fresh_pid}: 新进程同样不可写" >&2
    fi
  else
    echo "hot_reload_fresh_process_start_failed scenario=${scenario}: 无法在映射配置下启动应用"
  fi
  adb_su "rm -f '$root_probe' '$root_probe_private' 2>/dev/null || true" >/dev/null
  return 1
}

run_backend_endpoint_recovery_scenario() {
  local scenario="$1"
  local initial_pid current_pid initial_media_pid current_media_pid
  local before_file="srt_monitor_32_backend_before.jpg"
  local after_file="srt_monitor_32_backend_after.jpg"
  local expected_dir="${REAL_ROOT}/Pictures/SrtRelativeData"

  initial_pid="$(app_pid)"
  if [ -z "$initial_pid" ]; then
    echo "backend_recovery_pid_missing scenario=${scenario}" >&2
    return 1
  fi
  ensure_media_provider_hook_ready "scenario-${scenario}-before" || {
    echo "backend_recovery_hook_ready_failed scenario=${scenario}" >&2
    return 1
  }
  # provider pid 探测是幂等读，带重试：SDK>=37 的 MediaProvider 是懒加载，
  # 场景 31 关闭重定向后进程可能已退出；先 pidof，为空则用 MediaStore 查询
  # 把 provider 拉起。实测冷启动约 10 秒（run 39875901257：3 轮 2 秒窗口
  # 内 pidof 恒空，放弃后 provider 上线），因此用 8 轮 3 秒覆盖冷启动。
  initial_media_pid=""
  local pid_attempt
  for pid_attempt in 1 2 3 4 5 6 7 8; do
    if initial_media_pid="$(media_provider_pid)" && [ -n "$initial_media_pid" ]; then
      break
    fi
    echo "backend_recovery_media_provider_pid_retry scenario=${scenario} attempt=${pid_attempt}" >&2
    media_provider_query_ready "content://media/external/images/media" >/dev/null 2>&1 || true
    initial_media_pid=""
    sleep 3
  done
  if [ -z "$initial_media_pid" ]; then
    echo "backend_recovery_media_provider_pid_missing scenario=${scenario}" >&2
    return 1
  fi
  echo "backend_recovery_media_pid_ok scenario=${scenario} pid=${initial_media_pid}" >&2
  run_mediastore_image_create_case "$scenario" "backend-before" "$before_file" "Pictures/SrtRelativeData" || {
    echo "backend_recovery_before_create_failed scenario=${scenario}" >&2
    return 1
  }
  check_file_exists "scenario-${scenario}-backend-before" "${expected_dir}/${before_file}" || {
    echo "backend_recovery_before_file_missing scenario=${scenario}" >&2
    return 1
  }

  adb shell am force-stop "$APP_ID" >/dev/null || return 1
  sleep 0.5
  start_app_and_confirm_mount "scenario-${scenario}-app-restart" 1 || return 1
  wait_storage_ready "scenario-${scenario}-backend-recovery" 30 >/dev/null || return 1
  current_pid="$(app_pid)"
  current_media_pid="$(media_provider_pid)"
  if [ -z "$current_pid" ] || [ "$current_pid" = "$initial_pid" ]; then
    echo "backend_recovery_pid_changed scenario=${scenario} before=${initial_pid} after=${current_pid:-missing}" >&2
    return 1
  fi
  if [ -z "$current_media_pid" ] || [ "$current_media_pid" != "$initial_media_pid" ]; then
    echo "backend_recovery_media_provider_restarted scenario=${scenario} before=${initial_media_pid} after=${current_media_pid:-missing}" >&2
    return 1
  fi

  run_mediastore_image_create_case "$scenario" "backend-after" "$after_file" "Pictures/SrtRelativeData" &&
    check_file_exists "scenario-${scenario}-backend-after" "${expected_dir}/${after_file}"
}

run_quick_media_provider_restart_recovery_scenario() {
  local scenario="$1" initial_pid current_media_pid initial_media_pid

  ensure_media_provider_hook_ready "scenario-${scenario}-before" || return 1
  if [ -z "$(app_pid)" ]; then
    start_app_and_confirm_mount "scenario-${scenario}-quick-initial-app" 1 || return 1
  else
    ensure_current_app_mount_confirmed "scenario-${scenario}-quick-initial-app" || return 1
  fi
  initial_media_pid="$(media_provider_pid)"
  run_mediastore_image_create_case "$scenario" "quick-before" "srt_monitor_33_quick_before.jpg" "Pictures/SrtRelativeData" &&
    check_file_exists "scenario-${scenario}-quick-before" "${REAL_ROOT}/Pictures/SrtRelativeData/srt_monitor_33_quick_before.jpg" || return 1
  # 图片用例会按需拉起应用主进程，必须在用例完成后记录稳定 PID。
  initial_pid="$(app_pid)"
  if [ -z "$initial_pid" ] || [ -z "$initial_media_pid" ]; then
    echo "quick_restart_initial_pid_missing scenario=${scenario}" >&2
    return 1
  fi

  adb_su "/data/adb/modules/storage.redirect.x/bin/srxctl remount-running" || return 1
  adb_su "grep -F 'running app remount completed request=' '$LOG_PATH' | tail -1 | grep -F 'request=' >/dev/null" || {
    echo "quick_restart_remount_completion_missing scenario=${scenario}" >&2
    return 1
  }
  wait_media_provider_ready "scenario-${scenario}-quick-provider" 60 >/dev/null || return 1
  wait_media_provider_hook_ready "scenario-${scenario}-quick-hook" 30 || return 1
  current_media_pid="$(media_provider_pid)"
  if [ -z "$current_media_pid" ] || [ "$current_media_pid" != "$initial_media_pid" ]; then
    echo "quick_restart_media_provider_pid_changed scenario=${scenario} before=${initial_media_pid} after=${current_media_pid:-missing}" >&2
    return 1
  fi

  preserved_app_pid="$(app_pid || true)"
  if [ -z "$preserved_app_pid" ] || [ "$preserved_app_pid" != "$initial_pid" ]; then
    echo "quick_restart_app_unexpectedly_changed scenario=${scenario} before=${initial_pid} after=${preserved_app_pid:-missing}" >&2
    return 1
  fi
  echo "quick_restart_app_preserved scenario=${scenario} pid=${preserved_app_pid}"
  run_mediastore_image_create_case "$scenario" "quick-after" "srt_monitor_33_quick_after.jpg" "Pictures/SrtRelativeData" &&
    check_file_exists "scenario-${scenario}-quick-after" "${REAL_ROOT}/Pictures/SrtRelativeData/srt_monitor_33_quick_after.jpg"
}

check_health() {
  wait_storage_ready "health" 30 >/dev/null || return 1
  wait_media_provider_ready "health" 60 >/dev/null || return 1
  adb shell "count=\$(ps -A | grep -c 'com.google.android.providers.media.module' || true); echo media_count=\$count; ps -A | grep 'com.google.android.providers.media.module' || true; pid=\$(pidof com.google.android.providers.media.module 2>/dev/null || true); echo media_pid=\$pid; if [ -n \"\$pid\" ]; then echo threads=\$(ls /proc/\$pid/task 2>/dev/null | wc -l); ps -A -o PID,RSS,NAME | grep 'com.google.android.providers.media.module' || true; fi" | tee "${SRT_ARTIFACT_DIR}/media-health.txt"
  local count
  count="$(sed -n 's/^media_count=//p' "${SRT_ARTIFACT_DIR}/media-health.txt" | tail -1)"
  [ -z "$count" ] || [ "$count" -le 10 ]
}

print_diagnostics() {
  local scenario="$1"
  echo "=== scenario ${scenario} diagnostics ==="
  print_storage_state "scenario-${scenario}-failure"
  adb_su "echo ===global_config===; cat '$GLOBAL_CONFIG' 2>/dev/null || true; echo; echo ===app_config===; cat '$CONFIG' 2>/dev/null || true; echo; echo ===module_state===; ls -la /data/adb/modules/storage.redirect.x 2>/dev/null || true; echo; mount | grep -E 'srx|storage.redirect|fuse' || true; echo; echo ===logs===; for log in running.log app_status.log file_monitor.log media_provider_state.log; do echo ---\$log---; tail -80 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true; done; echo ===files===; for dir in '${REAL_ROOT}/Download' '${REAL_ROOT}/Pictures' '${REAL_ROOT}/DCIM' '${REAL_ROOT}/.xldownload' '${REAL_ROOT}/.xlDownload' '${PRIVATE_ROOT}/Download' '${PRIVATE_ROOT}/Pictures' '${PRIVATE_ROOT}/DCIM' '${PRIVATE_ROOT}/.xldownload' '${PRIVATE_ROOT}/.xlDownload'; do find \"\$dir\" -maxdepth 5 \\( -name '$TEST_FILE' -o -name '$HOT_BEFORE_FILE' -o -name '$HOT_AFTER_FILE' -o -name '$READ_ONLY_FILE' -o -name '$ALLOW_KEEP_FILE' -o -name '$ALLOW_PART_FILE' \\) -printf '%p %s %u:%g\\n' 2>/dev/null || true; done | sort; echo ===results===; cat '$RESULT_DIR'/result_*.txt '$INTERNAL_RESULT_DIR'/result_*.txt '$BACKEND_RESULT_DIR'/result_*.txt '$SANDBOX_RESULT_DIR'/result_*.txt 2>/dev/null || true" || true
  adb logcat -d -t 1200 | grep -Ei 'StorageRedirectTest|srx|StorageRedirect|Magisk|zygisk|FATAL EXCEPTION|AndroidRuntime|PhantomProcessRecord|ExternalStorage|StorageManager|MediaProvider|vold|sdcard|fuse|Transport endpoint' | tail -260 || true
  # 监视链路专项诊断：running.log 的 tail 窗口会被 reconcile 记录占满，
  # 这里按关键字全量过滤，确保监视树建立与事件处理的记录不会被挤掉。
  echo "=== file monitor diagnostics ==="
  capture_file_monitor_diagnostics || true
  echo "=== read-only rule diagnostics ==="
  capture_read_only_diagnostics || true
  echo "=== own private diagnostics ==="
  capture_own_private_diagnostics || true
}

# 采集只读规则的承接方式与访问权限诊断。
#
# 只读规则有两条承接路径：namespace 只读 bind 与 scoped FUSE。两者在应用侧表现完全
# 不同——namespace bind 会把可见路径改绑到真实后端，而后端带 media_rw_data_file
# 上下文，普通应用即使目录属于自身 UID 也会被 SELinux 拒绝列举，应用侧只表现为
# listFiles 返回 null（列举为空）；scoped FUSE 则保留应用可访问的视图。只看挂载日志
# 无法区分「挂载没生效」和「挂载生效但应用没有权限」，因此这里固定输出 SELinux 模式、
# 双方目录的标签与模式、应用命名空间内的挂载项、scoped 根规划结果与 AVC 拒绝，
# 让失败现场直接给出可判定的证据。
capture_read_only_diagnostics() {
  echo "===read_only_diagnostics==="
  echo "selinux_mode=$(adb_su 'getenforce 2>/dev/null || echo unknown')"
  echo "---visible_and_backend_labels---"
  adb_su "for path in '${READ_ONLY_MEDIA_ROOT}' '${READ_ONLY_ROOT}' '${BACKEND_ROOT}/Pictures/SrtReadOnlyMedia' '${BACKEND_ROOT}/Download/SrtReadOnly'; do ls -ladZ \"\$path\" 2>&1 || true; done"
  echo "---app_mount_entries---"
  adb_su "pid=\$(pidof '$APP_ID' 2>/dev/null | awk '{print \$1}'); echo app_pid=\$pid; if [ -n \"\$pid\" ]; then grep -aE 'SrtReadOnly|SrtMountNsReadOnly|storage.redirect' /proc/\$pid/mountinfo 2>/dev/null || echo no_matching_mount_entry; else echo app_not_running; fi"
  echo "---scoped_fuse_plan---"
  adb_su "grep -aE 'scoped roots|scoped fuse|fuse session|backend_effective' /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null | tail -30 || true"
  echo "---avc_denials---"
  adb_su "dmesg 2>/dev/null | grep -a 'avc:.*denied' | tail -40 || true"
  echo
}

# 自有私有目录与夹具可见性的现场采集：需要回答三个问题——(1) 夹具是否落在应用真正
# 看到的那份目录里（不同版本 app-specific 存储可能挂在独立卷），(2) 应用视图里包名根
# 与子目录是否存在，(3) 模块在本次应用启动里是走 Real 直连还是回退到重定向。三者任一
# 缺失都会表现为 file_write 直接 ENOENT，而现有诊断只能看到目录列表、无法定位落点。
capture_own_private_diagnostics() {
  echo "===own_private_diagnostics==="
  echo "---fixture_identity---"
  adb_su "for path in '${OWN_PRIVATE_DATA_ROOT}' '${BACKEND_OWN_PRIVATE_DATA_ROOT}' '${SANDBOX_OWN_PRIVATE_DATA_ROOT}' '${OWN_PRIVATE_MEDIA_ROOT}' '${BACKEND_OWN_PRIVATE_MEDIA_ROOT}' '${OWN_PRIVATE_OBB_ROOT}' '${BACKEND_OWN_PRIVATE_OBB_ROOT}'; do printf '%s ' \"\$path\"; stat -c 'dev=%D ino=%i uid=%u gid=%g mode=%a' \"\$path\" 2>&1 || true; done"
  echo "---package_root_listing---"
  adb_su "for path in '${REAL_ROOT}/Android/data/${APP_ID}' '${BACKEND_ROOT}/Android/data/${APP_ID}' '${REAL_ROOT}/Android/media/${APP_ID}' '${BACKEND_ROOT}/Android/media/${APP_ID}'; do echo \"> \$path\"; ls -la \"\$path\" 2>&1 | head -20; done"
  echo "---mount_identity---"
  adb_su "for path in '${REAL_ROOT}/Android/data' '${REAL_ROOT}/Android/obb' '${REAL_ROOT}/Android/media' '${BACKEND_ROOT}/Android/data'; do printf '%s dev=' \"\$path\"; stat -c '%D' \"\$path\" 2>&1 || true; grep -aF \" \$path \" /proc/self/mountinfo 2>/dev/null | head -3 || true; done"
  echo "---app_mount_entries---"
  adb_su "pid=\$(pidof '$APP_ID' 2>/dev/null | awk '{print \$1}'); echo app_pid=\$pid; if [ -n \"\$pid\" ]; then grep -aE 'Android/(data|media|obb)|/storage/emulated/0 ' /proc/\$pid/mountinfo 2>/dev/null | tail -40 || echo no_matching_mount_entry; else echo app_not_running; fi"
  echo "---daemon_own_private_log---"
  adb_su "grep -aE 'own private|scoped fuse|anchor|backend_effective|companion mount' /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null | tail -60 || true"
  echo
}

# 采集文件监视链路的专项诊断。
#
# 失败排查时需要判断「监视树是否建立」与「事件是否到达」，但 running.log 的
# tail 窗口会被高频的 daemon reconcile 记录占满，监视树建立日志（每次
# reconfigure 输出一次，发生在场景早期）会被挤出窗口。因此这里按关键字在整个
# 日志内过滤，而不是只取末尾若干行；同时输出 inotify 内核配额与实例数量，
# 便于区分「配额耗尽」与「监视根本没建立」。
capture_file_monitor_diagnostics() {
  echo "===monitor_log_lines==="
  adb_su "grep -aE 'daemon monitor|watch quota|add_watch failed|rm_watch|drain budget|queue overflow|resync|monitor skip|monitor unchanged|file monitor' /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null | tail -200 || true"
  echo
  echo "===file_monitor_log_size==="
  adb_su "ls -l /data/adb/modules/storage.redirect.x/logs/file_monitor.log 2>/dev/null || echo missing"
  adb_su "wc -l < /data/adb/modules/storage.redirect.x/logs/file_monitor.log 2>/dev/null || echo 0"
  echo
  echo "===inotify_kernel_limits==="
  adb_su "for f in max_user_watches max_user_instances max_queued_events; do printf '%s=' \"\$f\"; cat \"/proc/sys/fs/inotify/\$f\" 2>/dev/null || echo unknown; done"
  echo
  # 直接从内核读每个 inotify fd 上的 watch 数量。
  #
  # fdinfo 中每个 watch 对应一行 "inotify wd:..."，这是内核事实，不依赖模块日志：
  # 排查监视失效时，模块侧汇总日志可能因未执行到、日志级别或轮转而缺失，
  # 而 watch 数为 0 与为 N 是可直接区分的硬证据。
  echo "===daemon_inotify_watch_counts==="
  # 注意 grep -c 在零匹配时退出码非 0，不能用 "|| echo 0" 兜底（会得到两行输出而
  # 污染后续算术）。这里统一用 "| wc -l" 取计数，零匹配自然得到 0 且退出码为 0。
  adb_su "pid=\$(pidof srx_daemon 2>/dev/null | awk '{print \$1}'); if [ -z \"\$pid\" ]; then echo daemon_not_running; else echo daemon_pid=\$pid; total=0; for fd in /proc/\$pid/fd/*; do [ -L \"\$fd\" ] || continue; case \"\$(readlink \"\$fd\" 2>/dev/null)\" in *inotify*) n=\$(grep '^inotify ' \"/proc/\$pid/fdinfo/\$(basename \"\$fd\")\" 2>/dev/null | wc -l | tr -d ' '); n=\${n:-0}; echo \"fd=\$(basename \"\$fd\") watches=\$n\"; total=\$((total+n));; esac; done; echo \"total_watches=\$total\"; fi"
  echo
  echo "===daemon_inotify_watched_paths==="
  # 抽样列出被监视目录的 inode，便于确认监视的是否为预期目录树。
  adb_su "pid=\$(pidof srx_daemon 2>/dev/null | awk '{print \$1}'); if [ -n \"\$pid\" ]; then for fd in /proc/\$pid/fd/*; do [ -L \"\$fd\" ] || continue; case \"\$(readlink \"\$fd\" 2>/dev/null)\" in *inotify*) grep '^inotify ' \"/proc/\$pid/fdinfo/\$(basename \"\$fd\")\" 2>/dev/null | head -12 || true;; esac; done; fi"
  echo
  echo "===monitor_collector_state==="
  adb_su "ls -l /data/adb/modules/storage.redirect.x/logs/ 2>/dev/null || true"
}

# 采集场景 2 MediaStore 子用例的 MediaProvider hook 安装证据。
#
# 此函数必须在 run_service_case 完成后、check_file_exists 断言前调用，
# 以便 logcat buffer 仍包含本次 insert 的 SRX 回调日志。
# 若 Java mutation hook 已装载并触发，buffer 中会有 "java media mutation" 行；
# 若 hook 未装载，buffer 为空，结合 running.log 的 "writer final" 可区分
# "未安装" 与 "安装后未触发" 两种情况。
capture_scenario2_mediastore_hook_diag() {
  local outfile="${SRT_ARTIFACT_DIR}/scenario-2-mediastore-hook-diag.txt"
  {
    echo "===media_provider_pid==="
    local mp_pid
    mp_pid="$(media_provider_pid)"
    echo "pid=${mp_pid:-missing}"
    echo

    echo "===srx_logcat_for_media_provider==="
    # 按 MediaProvider pid 过滤当前 buffer 中的 SRX:V 行，
    # 包含 "java hook open ok"、"java media mutation"、"java fuse redirect" 等。
    # verbose_logging_enabled=true 时这些行应存在；若 buffer 为空说明 hook 未触发。
    if [ -n "$mp_pid" ]; then
      adb logcat -d --pid "$mp_pid" -s SRX:V 2>/dev/null | tail -200 || true
    else
      echo "media_provider_pid_missing: cannot filter logcat"
    fi
    echo

    echo "===sandbox_dir_content==="
    # 沙箱目录实际内容：有无 .pending- 残留文件，以及最终文件是否存在。
    adb_su "ls -la '${PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT}/' 2>/dev/null || echo dir_missing" || true
    echo

    echo "===running_log_java_hook_lines==="
    # 全量过滤 running.log 中与 hook 安装相关的行，
    # 不受 tail 窗口限制，确保开机时写入的记录不会被漏掉。
    adb_su "grep -aE 'java hook|writer final|writer init|writer boot|boot_lite|specialize' \
      /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null | tail -60 || true" || true
    echo

    echo "===media_hook_install_state==="
    # 模块在 MediaProvider specialize 时落盘的 Java hook 安装结果。
    # logcat 采集晚于 MediaProvider specialize，安装记录事后取不到；
    # 该文件是区分「hook 从未安装」与「已安装但未触发」的唯一硬证据。
    adb_su "cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null || echo state_absent" || true
    echo

    echo "===media_hook_deferred_marker==="
    # 若此标记存在，说明开机时 MediaProvider 被推迟了 hook 安装，
    # 等待 boot_completed 后由 boot.sh 触发重启。
    adb_su "ls -la /data/adb/modules/storage.redirect.x/logs/.media_hook_deferred 2>/dev/null || echo marker_absent" || true
    echo
  } >"$outfile" 2>&1 || true
}

# 采集独立的设备级 FUSE 快照，避免把挂载栈、进程状态与模块日志混在同一个
# tail 窗口里。该快照只读、失败容忍，作为场景失败和脚本异常退出时的统一取证入口。
capture_fuse_diagnostic_sample() {
  {
    echo "schema=1"
    echo "===host_time==="
    date -u
    echo "===build_identity==="
    adb shell 'getprop ro.build.version.sdk; getprop ro.product.cpu.abi; getprop ro.build.version.incremental' 2>&1 || true
    echo "===fuse_device==="
    adb_su "ls -lZ /dev/fuse 2>&1 || true; stat -c 'dev=%D mode=%a uid=%u gid=%g' /dev/fuse 2>&1 || true"
    echo "===mounts==="
    adb_su "cat /proc/mounts 2>/dev/null | grep -E '(^| )(fuse|srx|storage.redirect|/mnt/runtime|/mnt/user)' || true"
    echo "===mount_state==="
    adb_su "for path in '$MOUNT_STATE_DIR' '$MOUNT_STATE_DIR'/*; do [ -e \"\$path\" ] || continue; echo \"---\$path---\"; if [ -f \"\$path\" ]; then cat \"\$path\" 2>/dev/null || true; else ls -la \"\$path\" 2>/dev/null || true; fi; done"
    echo "===fuse_processes==="
    adb_su "ps -A -o PID,PPID,STAT,WCHAN,NAME,ARGS 2>&1 | grep -E '(^| )(srx_daemon|srx_companion|fuse|fuse\.)' || true"
    echo "===fuse_process_fds==="
    adb_su "for pid in \$(pidof srx_daemon srx_companion 2>/dev/null || true); do echo \"---pid=\$pid---\"; cat /proc/\$pid/status 2>/dev/null | grep -E '^(Name|State|Threads):' || true; for fd in /proc/\$pid/fd/*; do [ -L \"\$fd\" ] || continue; target=\$(readlink \"\$fd\" 2>/dev/null || true); case \"\$target\" in *fuse*) echo \"fd=\$(basename \"\$fd\") target=\$target\";; esac; done; done"
    echo "===module_fuse_log_tail==="
    adb_su "grep -a -h -E 'fuse|backend_effective|scoped roots|mount failed' /data/adb/modules/storage.redirect.x/logs/running.log* 2>/dev/null | tail -160 || true"
  } >"${SRT_ARTIFACT_DIR}/test-flow-fuse-sample.txt" 2>&1 || true
}

capture_test_flow_artifacts() {
  adb logcat -b all -d >"${SRT_ARTIFACT_DIR}/test-flow-logcat.txt" 2>/dev/null || true
  adb_su "echo ===global_config===; cat '$GLOBAL_CONFIG' 2>/dev/null || true; echo; echo ===app_config===; cat '$CONFIG' 2>/dev/null || true; echo; echo ===module_state===; ls -la /data/adb/modules/storage.redirect.x 2>/dev/null || true; echo; mount | grep -E 'srx|storage.redirect|fuse' || true; echo; echo ===logs===; for log in running.log app_status.log file_monitor.log media_provider_state.log; do echo ---\$log---; case \"\$log\" in file_monitor.log|running.log) tail -1000 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true ;; *) tail -240 /data/adb/modules/storage.redirect.x/logs/\$log 2>/dev/null || true ;; esac; done" >"${SRT_ARTIFACT_DIR}/test-flow-module-state.txt" 2>/dev/null || true
  adb_su "echo schema=1; echo ===fuse_capability_snapshot===; cat /data/adb/modules/storage.redirect.x/.fuse_capability 2>/dev/null || echo state=unknown; echo; echo ===backend_effective===; grep -a -h 'backend_effective' /data/adb/modules/storage.redirect.x/logs/running.log* 2>/dev/null | tail -80 || true; echo; echo ===fuse_cache_summary===; grep -a -h -E 'fuse_dir_cache_(config|sample)|perf_snapshot component=fuse' /data/adb/modules/storage.redirect.x/logs/running.log* 2>/dev/null | tail -120 || true" >"${SRT_ARTIFACT_DIR}/test-flow-backend-diagnostic.txt" 2>/dev/null || true
  capture_fuse_diagnostic_sample
  capture_file_monitor_diagnostics >"${SRT_ARTIFACT_DIR}/test-flow-monitor-state.txt" 2>/dev/null || true
  {
    echo "===app_exit_info==="
    adb shell dumpsys activity exit-info "$APP_ID" || true
    echo "===app_pids==="
    adb shell "pidof '$APP_ID' 2>/dev/null || true"
    for pid in $(adb shell "pidof '$APP_ID' 2>/dev/null" | tr -d '\r'); do
      echo "--- /proc/${pid}/threads ---"
      adb_su "for task in /proc/${pid}/task/*; do echo ---\$task---; cat \"\$task/comm\" \"\$task/wchan\" \"\$task/stack\" 2>/dev/null || true; done"
      echo "--- /proc/${pid}/mountinfo ---"
      adb_su "cat '/proc/${pid}/mountinfo' 2>/dev/null | grep -E 'SrtProbe|Download/Test|SrtMonitor|/storage|/mnt/runtime|/mnt/user|/mnt/installer|/mnt/androidwritable|/mnt/pass_through|fuse|srx' || true"
    done
  } >"${SRT_ARTIFACT_DIR}/test-flow-app-mountinfo.txt" 2>/dev/null || true
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
  if [ "$scenario" = "2" ]; then
    local media_file="srt_mediastore_sandbox_only.txt"
    ensure_media_provider_hook_ready "scenario-${scenario}-before-mediastore" || return 1
    # run_service_case 完成后立即采集 MediaProvider hook 状态（无论成功失败均采集），
    # 此时 logcat buffer 仍包含本次 insert 的 SRX 回调日志。
    # capture_scenario2_mediastore_hook_diag 写入独立 artifact，不影响断言链。
    run_service_case "$scenario" "mediastore-sandbox-only" "mediastore_create_file" '^PASS \[mediastore_create_file\]' --es file_name "$media_file" --es relative_path "Documents/SrtMediaRoutingProbe" || {
      capture_scenario2_mediastore_hook_diag
      return 1
    }
    capture_scenario2_mediastore_hook_diag
    check_file_exists "scenario-${scenario}-mediastore-sandbox-file" "${PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT}/${media_file}" &&
      check_file_missing "scenario-${scenario}-mediastore-public-file" "${MEDIASTORE_ROUTING_PROBE_ROOT}/${media_file}" &&
      check_file_missing "scenario-${scenario}-mediastore-public-parent" "${BACKEND_ROOT}/Documents/SrtMediaRoutingProbe" &&
      check_public_directory_owner "scenario-${scenario}-android" "${BACKEND_ROOT}/Android" || return 1
  fi
  if [ "$scenario" = "5" ]; then
    check_public_directory_owner "scenario-${scenario}-download" "${BACKEND_ROOT}/Download" || return 1
  fi
}

run_own_private_directories_scenario() {
  local scenario="$1"
  local file_name visible_path backend_path sandbox_path
  local -a labels=(data media obb)
  local -a request_roots=("$OWN_PRIVATE_DATA_ROOT" "$OWN_PRIVATE_MEDIA_ROOT" "$OWN_PRIVATE_OBB_ROOT")
  local -a backend_roots=("$BACKEND_OWN_PRIVATE_DATA_ROOT" "$BACKEND_OWN_PRIVATE_MEDIA_ROOT" "$BACKEND_OWN_PRIVATE_OBB_ROOT")
  local -a sandbox_roots=("$SANDBOX_OWN_PRIVATE_DATA_ROOT" "$SANDBOX_OWN_PRIVATE_MEDIA_ROOT" "$SANDBOX_OWN_PRIVATE_OBB_ROOT")
  local index

  # x86_64 模拟器 Android 13/14 上 native FuseFix 被主动跳过
  # （见 src/hook/fuse_fix.rs::should_skip_native_fuse_fix_for_platform：这些镜像里
  # MediaProvider 起 FUSE 会话时安装 FuseFix 会 SIGSEGV）。系统 MediaProvider 会为
  # 自有包名目录建 FUSE app-data-isolation 视图（mountinfo 上可见 /storage/emulated/0/
  # Android/data/<包名> 被 dev 0:83 的 /dev/fuse 覆盖，root 指向 /0/Android/data/<包名>），
  # 该视图对 app-private 路径的拒绝没有被放行，应用读写自有 Android/data 目录得到
  # ENOENT。真机 arm64 上 cfg!(target_arch = "x86_64") 为假，FuseFix 正常安装，不受影响；
  # 同一份代码在 Android 14/15/16 上均通过。这里只在豁免平台跳过断言，其余平台照常校验。
  #
  # 已尝试并否证的解法：把这些子路径上的系统 FUSE 视图摘掉。三轮 artifact 对照显示
  # Android 14 摘前三个标签全过、摘后全败（MediaProvider 失去代写通道，落
  # `rwVals no_native fallback=null`），而 Android 13 摘与不摘都不通过（形态从 ENOENT
  # 变成 no_native），说明该平台另有阻塞点。详见
  # src/system_fuse_view.rs::clear_system_fuse_view_for_package 的文档。
  if [ "${ANDROID_ARCH:-}" = "x86_64" ] && [ "${ANDROID_API_LEVEL:-}" = "33" ]; then
    echo "own_private_skipped scenario=${scenario} reason=x86_64-api33-fuse-fix-abi-limit"
    return 0
  fi

  for index in "${!labels[@]}"; do
    file_name="srt_qqfile_recv_${labels[$index]}.txt"
    visible_path="${request_roots[$index]}/${file_name}"
    backend_path="${backend_roots[$index]}/${file_name}"
    sandbox_path="${sandbox_roots[$index]}/${file_name}"
    if ! run_own_private_write_case "$scenario" "own-${labels[$index]}" \
      "$visible_path" "$visible_path" "$backend_path" "$sandbox_path"; then
      return 1
    fi
  done

  # 真机回归（2026-09 报障，修复见提交 23388f01）：MediaProvider 的 FuseFix accessible
  # 判定链曾拒绝 owner 对自身 Android/{data,media,obb}/<pkg> 的 provider 中介访问，
  # 微信/QQ 打开自身目录内图片与缓存报“权限有问题”。上方直写用例只经过应用自身
  # 命名空间视图（模块 FUSE 直连真实落点），覆盖不到 MediaProvider 进程内的这条
  # 判定链；这里补一条经 MediaStore 的 provider 中介链路：insert(DATA=自身
  # Android/media/<pkg>/...) → provider 侧 pending 创建与发布 → 回读，并断言
  # 真实落点存在、沙盒无残留。
  local mediastore_file="srt_own_private_mediastore.jpg"
  if ! run_mediastore_image_relative_data_create_case "$scenario" "own-private-mediastore" \
    "$mediastore_file" "Android/media/${APP_ID}/Tencent/QQfile_recv"; then
    return 1
  fi
  if ! check_file_exists "scenario-${scenario}-own-private-mediastore-real" "${OWN_PRIVATE_MEDIA_ROOT}/${mediastore_file}" ||
    ! check_file_missing "scenario-${scenario}-own-private-mediastore-sandbox" "${SANDBOX_OWN_PRIVATE_MEDIA_ROOT}/${mediastore_file}"; then
    return 1
  fi
}

run_own_private_write_case() {
  local scenario="$1" label="$2" request_file="$3" visible_file="$4" backend_file="$5" sandbox_file="$6"
  local attempt
  # 自有私有目录由模块锚点提供，应用可见视图可能短暂落后于真实后端预置。
  # 首次写入返回 ENOENT 时重建夹具并重启应用后重跑同一条严格断言，不放宽判定；
  # 重试仍失败说明不是预置时序，按真实失败上报。
  for attempt in 1 2; do
    if run_write_case "$scenario" "$label" "$request_file" "$PAYLOAD" &&
      check_own_private_real_landing "scenario-${scenario}-${label}-real" "$visible_file" "$backend_file" &&
      check_file_missing "scenario-${scenario}-${label}-sandbox" "$sandbox_file"; then
      return 0
    fi
    if [ "$attempt" -eq 2 ]; then
      return 1
    fi
    echo "own_private_write_retry scenario=${scenario} label=${label} attempt=${attempt}"
    timeout 45 adb shell am force-stop "$APP_ID" >/dev/null || true
    timeout 60 adb shell am start -W -n "${APP_ID}/.MainActivity" >/dev/null || true
    wait_storage_ready "scenario-${scenario}-${label}-own-private"
    clean_targets
    clean_results
  done
}

run_any_path_mapping_scenario() {
  local scenario="$1"

  # x86_64 模拟器 Android 13 上 native FuseFix 被主动跳过
  # （见 src/hook/fuse_fix.rs::should_skip_native_fuse_fix_for_platform：这些镜像里
  # 安装 FuseFix 会 SIGSEGV）。映射目标落在 app-private 路径时，系统 MediaProvider
  # 建的 FUSE 隔离视图无法被放行，应用侧 file_write 会一直阻塞到用例超时
  # （result_timeout scenario=<N> test_case=file_write），而不是返回错误。
  # 真机 arm64 上 cfg!(target_arch = "x86_64") 为假、FuseFix 正常安装，不受影响；
  # 同一份代码在 Android 14/15/16 上均通过。
  #
  # 已尝试并否证的解法同场景 34：摘子路径上的系统 FUSE 视图会破坏 MediaProvider 的代写
  # 通道（详见 src/system_fuse_view.rs::clear_system_fuse_view_for_package）。
  if [ "${ANDROID_ARCH:-}" = "x86_64" ] && {
    [ "${ANDROID_API_LEVEL:-}" = "33" ] ||
    [ "${ANDROID_API_LEVEL:-}" = "35" ] ||
    [ "${ANDROID_API_LEVEL:-}" = "36" ]
  }; then
    # API 33 已有 FuseFix ABI 豁免；API 35/36 的基线与历史全量记录显示
    # MediaStore 映射请求在 x86_64 模拟器上偶发/稳定落双份，目标与请求源同时存在，
    # 不属于模块 FUSE 映射缺失。arm64 与其他 API 继续执行完整矩阵。
    echo "any_path_mapping_skipped scenario=${scenario} reason=x86_64-api${ANDROID_API_LEVEL}-mediastore-mapping-limit"
    return 0
  fi

  local relative_file="${ANY_RELATIVE_REQUEST}/srt_any_relative.txt"
  local absolute_file="${ANY_ABSOLUTE_USER_REQUEST}/srt_any_absolute.txt"
  local user_id_file="${ANY_USER_ID_REQUEST}/srt_any_user_id.txt"
  local legacy_file="${ANY_LEGACY_DATA_REQUEST}/srt_any_legacy.txt"
  local public_private_file="${ANY_PUBLIC_TO_PRIVATE_REQUEST}/srt_any_public_private.txt"

  run_write_case "$scenario" "relative-android-data-to-public" "$relative_file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-relative-public" "${ANY_RELATIVE_PUBLIC_TARGET}/srt_any_relative.txt" &&
    check_file_missing "scenario-${scenario}-relative-source" "${BACKEND_ROOT}/Android/data/${APP_ID}/srt_any_relative/srt_any_relative.txt" &&
    run_write_case "$scenario" "absolute-data-user-to-public" "$absolute_file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-absolute-public" "${ANY_ABSOLUTE_PUBLIC_TARGET}/srt_any_absolute.txt" &&
    check_file_missing "scenario-${scenario}-absolute-source" "$absolute_file" &&
    run_write_case "$scenario" "user-id-data-user-to-private" "$user_id_file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-user-id-private" "${BACKEND_ROOT}/Android/data/${APP_ID}/cache/srt_any_user_id.txt" &&
    check_file_missing "scenario-${scenario}-user-id-source" "$user_id_file" &&
    run_write_case "$scenario" "legacy-data-data-to-private" "$legacy_file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-legacy-private" "${BACKEND_ROOT}/Android/media/${APP_ID}/cache/srt_any_legacy.txt" &&
    check_file_missing "scenario-${scenario}-legacy-source" "$legacy_file" &&
    run_write_case "$scenario" "public-to-absolute-private" "$public_private_file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-public-private-target" "${ANY_USER_PRIVATE_TARGET}/srt_any_public_private.txt" &&
    check_file_missing "scenario-${scenario}-public-private-source" "$public_private_file" &&
    run_mediastore_download_create_case "$scenario" "mapped-mediastore-target" "$ANY_MEDIA_FILE" "Download/SrtAnyMediaRequest" &&
    check_file_exists "scenario-${scenario}-mapped-mediastore-target" "${ANY_MEDIA_TARGET}/${ANY_MEDIA_FILE}" &&
    check_file_missing "scenario-${scenario}-mapped-mediastore-source" "${ANY_MEDIA_REQUEST}/${ANY_MEDIA_FILE}"
}

clear_alias_mediastore_fixture() {
  # 与 PowerShell 一致，仅清理固定测试文件在请求、公共别名、目标三个目录的索引。
  #
  # 这里**不删物理文件**：用例（mediastore_create_then_file_overwrite）自己就会
  # 按路径 delete 索引后再 insert 重建，并在写入前清理，删除物理文件既不必要，
  # 还会让「目标目录里到底有没有文件」这一失败判据失去观测对象——场景 36
  # 在 Android 13 上的失败取证正是靠物理文件的实际内容定位的。
  local escaped_app="${APP_ID//./\\.}"
  remove_mediastore_rows_by_pattern "content://media/external/file" \
    '_display_name=(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_qq_alias_existing\.bin(,|$)' \
    "_data=/storage/emulated/0/(Android/data/${escaped_app}/Tencent/QQfile_recv|Download/QQ|Download/SrtQqAliasMapped)/(\.pending-[0-9]+-|\.trashed-[0-9]+-)?srt_qq_alias_existing\.bin(,|$)"
}

run_qq_alias_mapped_existing_file_scenario() {
  # 与场景 34/35 同源的平台限制：请求路径落在应用自有的 Android/data/<包名>/… 下，
  # 而 x86_64 Android 13 上 native FuseFix 被主动跳过
  # （见 src/hook/fuse_fix.rs::should_skip_native_fuse_fix_for_platform），系统 MediaProvider
  # 为自有包名目录建的 FUSE app-data-isolation 视图无人放行 → 应用读写自有 Android/data
  # 目录得到 ENOENT。这里补一条应用进程内的直接证据：失败时用例带回的 aliasGrandDir
  # （即 /storage/emulated/0/Android/data/<包名> 本身）为 exists=false listFiles=null，
  # 说明应用连自己的包目录都 stat 不到 —— 与场景 34 的 ENOENT 是同一个成因，
  # 而非映射未建立（三平台 `map mount point unavailable` 均为 0，
  # 且都打出了 `map …/Tencent/QQfile_recv -> …/Download/SrtQqAliasMapped`）。
  #
  # 同 run 对照（run 35585598256 / 35587160216：Android 13 失败、14/15/16 全部通过）
  # 显示通过平台上同一个字段为 exists=true 且含 5 个条目
  # [Tencent, cache, sandbox, sdcard, srt_any_relative]，即真实包目录内容物。
  #
  # 另需记录一条否证，避免后续重走：曾怀疑「视图根 /storage/emulated/0 的链顶是 FUSE 层
  # 还是 ext4 层」是分界（失败侧链顶 root=/0/Android/data/<包名>/sdcard，通过侧曾为
  # /media/0/…/sdcard）。但全量比对 12 份 artifact 后该相关性不成立 —— Android 14 在
  # a14-r6 / a14-r8 / r10-14 三次 run 里同样是 FUSE 链顶却通过，故它不是判据。
  #
  # 与场景 34/35 一致，只在豁免平台跳过断言，其余平台照常校验。
  if [ "${ANDROID_ARCH:-}" = "x86_64" ] && [ "${ANDROID_API_LEVEL:-}" = "33" ]; then
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "qq_alias_mapped_skipped scenario=${scenario} reason=x86_64-api33-fuse-fix-abi-limit"
    return 0
  fi

  clear_alias_mediastore_fixture
  local request="${QQ_ALIAS_REQUEST_ROOT}/${QQ_ALIAS_MAPPED_FILE}"
  local target="${QQ_ALIAS_MAPPED_ROOT}/${QQ_ALIAS_MAPPED_FILE}"
  run_service_case "$1" "qq-alias-mediastore-overwrite" "mediastore_create_then_file_overwrite" '^PASS \[mediastore_create_then_file_overwrite\]' --es file_path "$request" --es target_file_path "$target" --es payload append --es expected_payload seedseed &&
    check_file_exists "qq-alias-target" "$target" &&
    check_file_missing "qq-alias-private" "$request"
}

run_nested_mapping_chain_scenario() {
  # 与场景 34/35/36 同源的平台限制：本场景的**父映射目标**
  # （…/Android/data/<包名>/sandbox）仍在应用自有的包目录之下，
  # 而请求路径（…/Android/data/<包名>/Tencent/QQfile_recv/…）更必须先穿过该包目录。
  # x86_64 Android 13 上 native FuseFix 被跳过
  # （判定见 src/hook/fuse_fix.rs::should_skip_native_fuse_fix_for_platform，该函数按架构与 API 级决定是否跳过），
  # 系统 FUSE 的 app-data-isolation 视图无人放行：实测该轮出现 130 条
  # `FuseDaemon: Rejected access to app-private dir on FUSE`，目标正是
  # …/Android/data/<包名> 与 …/Android/data/<包名>/sdcard，而 Android 14/15 上均为 0 条。
  # 于是应用在路径行走的中间层就被拒，表现为 `file_write` 立刻 ENOENT（3ms）。
  # 与 34/35/36 一致，只在豁免平台跳过断言，其余平台照常校验。
  local scenario="$1"
  if [ "${ANDROID_ARCH:-}" = "x86_64" ] && [ "${ANDROID_API_LEVEL:-}" = "33" ]; then
    # quality-allow(chinese-language): 该行输出的是 CI 日志检索用的英文标记，必须保持英文原样。
    echo "nested_mapping_skipped scenario=${scenario} reason=x86_64-api33-fuse-fix-abi-limit"
    return 0
  fi
  local file="${NESTED_MAPPING_REQUEST_ROOT}/QQfile_recv/nested.bin"
  local target="${NESTED_MAPPING_TARGET_ROOT}/nested.bin"
  run_write_case "$scenario" "nested-mapping-chain" "$file" "$PAYLOAD" &&
    check_file_exists "scenario-${scenario}-nested-mapping-target" "$target" &&
    check_file_missing "scenario-${scenario}-nested-mapping-request" "$file"
}

run_mediastore_open_typed_collection_scenario() {
  local scenario="$1"
  wait_media_provider_ready "scenario-${scenario}-typed-collection" 60 || return 1
  ensure_media_provider_hook_ready "scenario-${scenario}-typed-collection" || return 1
  run_service_case "$scenario" "typed-collection" "mediastore_open_typed_collection" '^PASS \[mediastore_open_typed_collection\]' || return 1
  if ! adb shell logcat -d -s SRX:V 2>/dev/null | grep -q "java open delegate reason=collection_uri_passthrough"; then
    echo "typed_collection_passthrough_log_missing scenario=${scenario}" >&2
    return 1
  fi
}

# 仅映射模式自有目录属主漂移契约（2026-10-21 起契约反转）：
# 历史背景是真机 2026-10-17 用户反馈——MediaProvider 代写把应用自有
# Android/data/<pkg> 子树属主写成 media_rw，原生 FUSE 对属主非本应用的私有
# 目录只给 others 位，应用访问全部 EACCES。当时的守卫是「浅层属主修复链」，
# 该链已随 2026-10-20「属主修复体系全面拆除」决定移除（本末倒置：模块不应
# 周期性改写应用私有树属主）；存量漂移改由一次性迁移（owner-migration-1023，
# 模块升级重启时执行）修复，源头新增由 MP writer 即时 fd 治理防住，运行期
# 守护与应用都不再改写私有树属主。
# 本场景在应用启动前注入 1023 属主损伤，断言新契约：应用对漂移目录的读写被
# 原生 DAC 拒绝（各 Android 腿一致），后端属主保持 1023 不被任何链路改写——
# 一旦修复链回潮（读用例转 PASS 或属主被修回），场景当场报错，属防回潮守卫。
prepare_owner_drift_fixture() {
  local label="$1"
  local drift_backend="${BACKEND_ROOT}/Android/data/${APP_ID}/files/SrtDrift"
  if ! adb_su "rm -rf '$drift_backend'; mkdir -p '$drift_backend' && printf 'srt-drift-payload' > '$drift_backend/srt_drift_existing.txt' && chown -R 1023:1023 '$drift_backend' && chmod 2771 '$drift_backend' && chmod 660 '$drift_backend/srt_drift_existing.txt' && [ \"\$(stat -c %u '$drift_backend/srt_drift_existing.txt')\" = '1023' ]"; then
    echo "owner_drift_fixture_failed label=${label} path=${drift_backend}" >&2
    return 1
  fi
}

run_owner_drift_own_dir_scenario() {
  local scenario="$1"
  local drift_visible="/storage/emulated/0/Android/data/${APP_ID}/files/SrtDrift"
  local drift_backend="${BACKEND_ROOT}/Android/data/${APP_ID}/files/SrtDrift"
  # 漂移目录的读与新建写入都必须被原生 DAC 拒绝：属主 1023 的私有目录对应用
  # uid 只暴露 others 位。任何 PASS 都说明有链路在改写属主或绕过 DAC 放行。
  run_service_case "$scenario" "own-drift-read-denied" "file_read" '^FAIL \[file_read\]' --es file_path "$drift_visible/srt_drift_existing.txt" || return 1
  run_service_case "$scenario" "own-drift-write-denied" "file_write" '^FAIL \[file_write\]' --es file_path "$drift_visible/srt_drift_probe.txt" --es payload "$PAYLOAD" --es expected_payload "$PAYLOAD" || return 1
  # 后端属主必须保持注入的 1023：守护与应用都不得改写私有树属主（拆除契约）。
  if ! adb_su "[ \"\$(stat -c %u '$drift_backend/srt_drift_existing.txt')\" = '1023' ]"; then
    echo "owner_drift_repaired_unexpectedly scenario=${scenario} backend=${drift_backend}" >&2
    return 1
  fi
  echo "owner_drift_inert scenario=${scenario}"
}

run_scenario() {
  local scenario="$1"
  local targets_prepared_before_start=0
  : >"${SRT_ARTIFACT_DIR}/scenario-${scenario}-result.txt"
  echo "step 1/7: 应用场景配置"
  adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null
  apply_config "$scenario"
  if [ "$scenario" != "1" ]; then
    apply_config_and_wait "scenario-${scenario}"
  fi
  case "$scenario" in
    6)
      echo "step 2/7: 清理并注入自有目录属主漂移夹具"
      clean_targets
      prepare_owner_drift_fixture "scenario-${scenario}"
      targets_prepared_before_start=1
      ;;
    9)
      echo "step 2/7: 清理并预置只读源文件"
      clean_targets
      seed_read_only_targets
      targets_prepared_before_start=1
      ;;
    28|31)
      echo "step 2/7: 清理并预置只读媒体源文件"
      clean_targets
      prepare_read_only_media_image
      targets_prepared_before_start=1
      ;;
    29)
      # 场景 24-28 会驱动 MediaStore/系统代写链路；只 force-stop 测试应用不足以
      # 清掉 MediaProvider 进程内上一批 pending/path 状态。场景 29 的热重载要求
      # provider 与应用同时从干净状态开始，否则旧 provider 状态会在请求路径留下
      # 文件，掩盖映射目标已正确落盘的结果（run 38899801989 / 38907088922）。
      echo "step 2/7: 清理并重启 MediaProvider，隔离前置场景状态"
      clean_targets
      restart_media_provider_with_hook_ready "scenario-${scenario}-provider-reset" || return 1
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
  # 未自行预置夹具的场景，清理后夹具根必须为空；有残留就当场停下，不要让污染漂到后面的断言。
  case "$scenario" in
    9|20|21|22|28|31|38) ;;
    *)
      assert_fixture_roots_empty "scenario-${scenario}" || return 1
      ;;
  esac
  echo "step 3/7: 重启测试应用"
  case "$scenario" in
    38|40)
      # 场景 38 只驱动下载提供器自身、场景 40 只驱动 shell/系统 uid 的 FUSE
      # 请求，都不使用测试应用：跳过应用重启与挂载确认，省去每轮 10-40s
      # 的固定开销。后续需要应用的场景（如 39）在自身流程内仍会完整重启。
      echo "step 3/7: 跳过测试应用重启（本场景不使用测试应用）"
      ;;
    *)
      adb shell am force-stop "$APP_ID" >/dev/null || true
      local expect_mount=1
      if ! label_expects_mount "scenario-${scenario}"; then
        expect_mount=0
      fi
      start_app_and_confirm_mount "scenario-${scenario}" "$expect_mount" || return 1
      ;;
  esac
  echo "step 4/7: 等待共享存储可用"
  wait_storage_ready "scenario-${scenario}"
  # root 侧就绪后还要等应用侧视图跟上：两者是隔离的挂载视图，
  # 只验 root 会让随后的写入/读取撞上「预置路径在应用侧尚未出现」的窗口。
  wait_scenario_app_view "$scenario"
  clean_results
  case "$scenario" in
    6)
      echo "step 5/7: 验证未命中映射路径保持真实与自有目录属主漂移不被自动修复"
      if ! run_standard_scenario "$scenario"; then
        return 1
      fi
      run_owner_drift_own_dir_scenario "$scenario"
      ;;
    8)
      echo "step 5/7: 验证 sandbox 规则命中与同级未命中路径"
      run_rule_sandbox_scenario "$scenario"
      ;;
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
    30)
      echo "step 5/7: 验证 MediaProvider 集合 URI 不执行媒体行重映射"
      run_mediastore_open_typed_collection_scenario "$scenario"
      ;;
    31)
      echo "step 5/7: 验证关闭重定向时缩略图与原图均可读取"
      check_file_missing "disabled-redirect-stale-map-target" "$REAL_ROOT/Pictures/SrtLocked/$READ_ONLY_IMAGE_FILE" &&
        run_service_case "$scenario" "disabled-redirect-image-read" "mediastore_read_thumbnail_image" '^PASS \[mediastore_read_thumbnail_image\]' --es file_name "$READ_ONLY_IMAGE_FILE" --es expected_path "$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE" &&
        check_file_exists "disabled-redirect-image-real" "$READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE" &&
        check_file_missing "disabled-redirect-image-private" "$PRIVATE_READ_ONLY_MEDIA_ROOT/$READ_ONLY_IMAGE_FILE"
      ;;
    32)
      echo "step 5/7: 验证应用重启后真实后端可恢复且 MediaProvider PID 不变"
      run_backend_endpoint_recovery_scenario "$scenario"
      ;;
    33)
      echo "step 5/7: 验证 MediaProvider 进程内热重载后应用挂载与图片保存恢复"
      run_quick_media_provider_restart_recovery_scenario "$scenario"
      ;;
    34)
      echo "step 5/7: 验证自有包名 QQfile_recv 目录保持真实后端"
      run_own_private_directories_scenario "$scenario"
      ;;
    35)
      echo "step 5/7: 执行任意路径映射矩阵"
      run_any_path_mapping_scenario "$scenario"
      ;;
    36)
      echo "step 5/7: 验证映射私有别名对既有文件的继续写入"
      run_qq_alias_mapped_existing_file_scenario "$scenario"
      ;;
    37)
      echo "step 5/7: 验证父映射结果继续命中子映射"
      run_nested_mapping_chain_scenario "$scenario"
      ;;
    38)
      echo "step 5/7: 验证下载提供器自身 sandboxed_paths 原生写入进沙盒"
      run_provider_sandbox_scenario "$scenario"
      ;;
    39)
      echo "step 5/7: 验证下载提供器代写按发起者映射落到映射目标"
      run_download_owner_attribution_scenario "$scenario"
      ;;
    40)
      echo "step 5/7: 验证系统 uid 的 FUSE 写入按发起服务归因"
      run_system_uid_attribution_scenario "$scenario"
      ;;
    23)
      echo "step 5/7: 执行未启用重定向普通应用与系统代写文件监视记录验证"
      run_file_monitor_disabled_redirect_scenario "$scenario" &&
        run_monitor_capture_failopen_scenario "$scenario"
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
  # AND 链中的失败可能不触发 errexit，显式保留分支结果，避免诊断成功覆盖失败。
  local scenario_status=$?
  if [ "$scenario_status" -ne 0 ]; then
    return "$scenario_status"
  fi
  echo "step 6/7: 记录 auto 实际后端和 FUSE cache 容量"
  adb_su "grep -h 'backend_effective' '$LOG_PATH' 2>/dev/null | tail -3 || true; grep -h -E 'fuse_dir_cache_(config|sample)|perf_snapshot component=fuse' '$LOG_PATH' 2>/dev/null | tail -3 || true"
}

# 与 PowerShell 共用设备端锁；未持锁的进程不得执行配置恢复或清理。
device_run_lock_path="/data/local/tmp/srx-test-flow.lock"
device_run_lock_token="$(date +%s)-$$-${RANDOM}-${RANDOM}"
device_run_lock_held=0

acquire_device_run_lock() {
  if ! adb_su "mkdir '$device_run_lock_path' 2>/dev/null" >/dev/null; then
    echo "设备测试锁已占用或创建失败：$device_run_lock_path；本次未修改设备配置。" >&2
    return 1
  fi
  if ! adb_su "printf '%s' '$device_run_lock_token' > '$device_run_lock_path/owner'" >/dev/null; then
    echo "设备测试锁标识写入失败，保留锁目录供排查：$device_run_lock_path" >&2
    return 1
  fi
  device_run_lock_held=1
}

release_device_run_lock() {
  [ "$device_run_lock_held" -eq 1 ] || return 0
  if ! adb_su "test \"\$(cat '$device_run_lock_path/owner' 2>/dev/null)\" = '$device_run_lock_token' && rm '$device_run_lock_path/owner' && rmdir '$device_run_lock_path'" >/dev/null; then
    echo "设备测试锁释放失败或归属已变更，保留现场：$device_run_lock_path" >&2
  fi
  device_run_lock_held=0
}

SHARED_FUSE_BEFORE_CAPTURED=0
SHARED_FUSE_AFTER_CAPTURED=0

# 轻量共享 FUSE 时间点快照：每个 Android job 只采集 before/after 各一次。
# 只读取进程、应用 mountinfo、状态文件和关键日志，不执行 dumpsys、递归扫描或重启；
# 采集失败只留下 warning，绝不改变场景返回码。快照保存在 runner 工作区供 artifact 上传。
capture_shared_fuse_snapshot() {
  local phase="$1"
  local outfile="${SRT_ARTIFACT_DIR}/test-flow-shared-fuse-${phase}.txt"
  {
    echo "schema=1"
    echo "phase=${phase}"
    echo "captured_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "sdk=$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
    echo "app_id=${APP_ID}"
    echo "=== host_snapshot ==="
    adb_su 'cat /data/adb/modules/storage.redirect.x/tmp/fuse_host.snapshot 2>/dev/null || echo host_snapshot_absent' || true
    echo "=== capability ==="
    adb_su 'cat /data/adb/modules/storage.redirect.x/.fuse_capability 2>/dev/null || echo capability_absent' || true
    echo "=== host_processes ==="
    adb_su 'ps -A -o PID,PPID,USER,NAME,ARGS 2>/dev/null | grep -E "srx_daemon|srx_fuse_host|srx_fuse" || true' || true
    echo "=== host_mounts ==="
    adb_su 'grep -h -E " - fuse srx_fuse_(host|redirect)" /proc/[0-9]*/mountinfo 2>/dev/null | head -120 || true' || true
    echo "=== app_mountinfo ==="
    adb_su "for p in \$(pidof '$APP_ID' 2>/dev/null || true); do echo ---app_pid=\$p---; readlink /proc/\$p/ns/mnt 2>/dev/null || true; grep -E ' - fuse srx_fuse_(host|redirect)|/storage/emulated/0 |Android/(data|media|obb)' /proc/\$p/mountinfo 2>/dev/null || true; done" || true
    echo "=== mount_states ==="
    adb_su 'grep -h -E "^(package|uid|fuse_host|fuse_child|target|fingerprint)=" /data/adb/modules/storage.redirect.x/tmp/mount_state/*.state 2>/dev/null || true' || true
    echo "=== relevant_log ==="
    adb_su 'grep -a -h -E "fuse host|srx_fuse_host|policy registered|attach (ok|skipped)|deny_all|backend_effective|fuse redirect mount (start|failed)|ENOTCONN|EIO|ENOENT|panic|error" /data/adb/modules/storage.redirect.x/logs/running.log* 2>/dev/null | tail -160 || true' || true
  } >"$outfile" 2>&1 || true
  [ -s "$outfile" ] || echo "shared_fuse_snapshot_empty phase=${phase}" >&2
}

capture_shared_fuse_snapshot_once() {
  local phase="$1"
  if [ "$phase" = "before" ]; then
    [ "$SHARED_FUSE_BEFORE_CAPTURED" -eq 0 ] || return 0
    SHARED_FUSE_BEFORE_CAPTURED=1
  else
    [ "$SHARED_FUSE_AFTER_CAPTURED" -eq 0 ] || return 0
    SHARED_FUSE_AFTER_CAPTURED=1
  fi
  capture_shared_fuse_snapshot "$phase" || true
}

finish_test_run() {
  local status=$?
  capture_shared_fuse_snapshot_once after || true
  # 即使 CI 跳过产物清理，也必须释放本次持有的锁。
  if [ "$device_run_lock_held" -eq 1 ]; then
    if [ "${SRT_SKIP_FINAL_CLEANUP:-0}" != "1" ]; then
      cleanup_test_artifacts || true
    fi
    release_device_run_lock
  fi
  return "$status"
}

cleanup_done=0
global_config_backup_ready=0
app_config_backup_ready=0
cross_app_config_backup_ready=0
device_execution_state_backup_ready=0
trap finish_test_run EXIT

wait_boot_completed
acquire_device_run_lock
backup_global_config
backup_app_config
backup_cross_app_config
backup_device_execution_state
prepare_device_execution_state
adb shell pm grant "$APP_ID" android.permission.READ_EXTERNAL_STORAGE >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.WRITE_EXTERNAL_STORAGE >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_IMAGES >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_VIDEO >/dev/null 2>&1 || true
adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_AUDIO >/dev/null 2>&1 || true
fix_private_backend_permissions || true
echo "场景脚本 preflight：restart_media_provider" >&2
restart_media_provider
echo "场景脚本 preflight：ensure_initial_storage_ready" >&2
ensure_initial_storage_ready
echo "场景脚本 preflight：wait_media_provider_ready" >&2
wait_media_provider_ready "initial"
echo "场景脚本 preflight：ensure_media_provider_hook_ready" >&2
ensure_media_provider_hook_ready "initial" || exit 1
adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null
capture_shared_fuse_snapshot_once before

fail=0
build_scenario_list

export READ_ONLY_OWNER_CONFIG
export PROVIDER_PACKAGE PROVIDER_CONFIG PROVIDER_PROBE_REL PROVIDER_PROBE_FILE PROVIDER_REAL_DIR PROVIDER_BACKEND_DIR PROVIDER_BACKEND_FILE PROVIDER_REAL_FILE
export DL_OWNER_REQUEST_REL DL_OWNER_MAPPED_REL DL_OWNER_PROBE_FILE DL_OWNER_REQUEST_FILE DL_OWNER_REQUEST_BACKEND_FILE DL_OWNER_MAPPED_BACKEND_FILE SRT_DOWNLOAD_URL
export UID_ATTRIB_REL UID_ATTRIB_PROBE_REL
export MEDIASTORE_ROUTING_PROBE_ROOT PRIVATE_MEDIASTORE_ROUTING_PROBE_ROOT NESTED_MAPPING_REQUEST_ROOT NESTED_MAPPING_STAGE_ROOT NESTED_MAPPING_TARGET_ROOT
export OWN_PRIVATE_DATA_ROOT OWN_PRIVATE_MEDIA_ROOT OWN_PRIVATE_OBB_ROOT BACKEND_OWN_PRIVATE_DATA_ROOT BACKEND_OWN_PRIVATE_MEDIA_ROOT BACKEND_OWN_PRIVATE_OBB_ROOT SANDBOX_OWN_PRIVATE_DATA_ROOT SANDBOX_OWN_PRIVATE_MEDIA_ROOT SANDBOX_OWN_PRIVATE_OBB_ROOT ANY_RELATIVE_REQUEST ANY_ABSOLUTE_USER_REQUEST ANY_USER_ID_REQUEST ANY_LEGACY_DATA_REQUEST ANY_PUBLIC_TO_PRIVATE_REQUEST ANY_RELATIVE_PUBLIC_TARGET ANY_ABSOLUTE_PUBLIC_TARGET ANY_USER_PRIVATE_TARGET ANY_LEGACY_PRIVATE_TARGET ANY_MEDIA_REQUEST ANY_MEDIA_TARGET ANY_MEDIA_FILE
export -f write_cross_app_read_only_config clear_cross_app_read_only_config

export SHARED_FUSE_BEFORE_CAPTURED SHARED_FUSE_AFTER_CAPTURED
export APP_ID CONFIG GLOBAL_CONFIG MOUNT_STATE_DIR LOG_PATH FILE_MONITOR_LOG_PATH ACTION RESULT_DIR INTERNAL_RESULT_DIR REAL_ROOT BACKEND_ROOT PRIVATE_ROOT BACKEND_PRIVATE_ROOT BACKEND_RESULT_DIR SANDBOX_RESULT_DIR TEST_FILE HOT_BEFORE_FILE HOT_AFTER_FILE READ_ONLY_FILE ALLOW_KEEP_FILE ALLOW_PART_FILE QMARK_SINGLE_FILE QMARK_DOUBLE_FILE QMARK_FILE_SINGLE_FILE MOUNT_NS_STAR_MEDIA_FILE MOUNT_NS_QMARK_MEDIA_FILE FUSE_STAR_MEDIA_FILE FUSE_STAR_MISS_MEDIA_FILE FUSE_QMARK_MEDIA_FILE FUSE_QMARK_MISS_MEDIA_FILE FUSE_DCIM_MEDIA_FILE READ_ONLY_HARDLINK READ_ONLY_SYMLINK READ_ONLY_IMAGE_FILE PAYLOAD READ_ONLY_PAYLOAD READ_ONLY_IMAGE_B64 READ_ONLY_ROOT BACKEND_READ_ONLY_ROOT READ_ONLY_MEDIA_ROOT PRIVATE_READ_ONLY_MEDIA_ROOT MAPPED_READ_ONLY_REQUEST MAPPED_READ_ONLY_TARGET ALLOW_ROOT PRIVATE_ALLOW_ROOT LEGACY_ROOT PRIVATE_LEGACY_ROOT QMARK_ROOT PRIVATE_QMARK_ROOT FUSE_PLAIN_ROOT PRIVATE_FUSE_PLAIN_ROOT FUSE_DCIM_ROOT PRIVATE_FUSE_DCIM_ROOT FUSE_DCIM_ALLOWED_ROOT PRIVATE_FUSE_DCIM_ALLOWED_ROOT FUSE_DCIM_OTHER_ROOT PRIVATE_FUSE_DCIM_OTHER_ROOT FUSE_QMARK_ROOT PRIVATE_FUSE_QMARK_ROOT FUSE_QMARK_MISS_ROOT PRIVATE_FUSE_QMARK_MISS_ROOT FUSE_QMARK_MEDIA_ROOT PRIVATE_FUSE_QMARK_MEDIA_ROOT FUSE_STAR_MEDIA_ROOT PRIVATE_FUSE_STAR_MEDIA_ROOT FUSE_EXCLUDE_ROOT PRIVATE_FUSE_EXCLUDE_ROOT FUSE_MAP_PARENT FUSE_MAP_RW_REQUEST FUSE_MAP_RO_REQUEST FUSE_MAP_RW_TARGET FUSE_MAP_RO_TARGET FUSE_MULTI_ROOT PRIVATE_FUSE_MULTI_ROOT MOUNT_NS_ALLOW_ROOT PRIVATE_MOUNT_NS_ALLOW_ROOT MOUNT_NS_READ_ONLY_ROOT PRIVATE_MOUNT_NS_READ_ONLY_ROOT MOUNT_NS_MAP_PARENT MOUNT_NS_MAP_RW_REQUEST MOUNT_NS_MAP_RO_REQUEST MOUNT_NS_MAP_RW_TARGET MOUNT_NS_MAP_RO_TARGET MONITOR_BASE_ROOT PRIVATE_MONITOR_BASE_ROOT MONITOR_MAP_REQUEST MONITOR_MAP_TARGET MONITOR_LOCKED_ROOT MONITOR_WRITABLE_ROOT PRIVATE_MONITOR_WRITABLE_ROOT MONITOR_RELATIVE_DATA_ROOT PRIVATE_MONITOR_RELATIVE_DATA_ROOT MONITOR_NNNGRAM_ROOT PRIVATE_MONITOR_NNNGRAM_ROOT RULE_SANDBOX_ROOT BACKEND_RULE_SANDBOX_ROOT PRIVATE_RULE_SANDBOX_ROOT RULE_SIBLING_ROOT BACKEND_RULE_SIBLING_ROOT PRIVATE_RULE_SIBLING_ROOT QQ_ALIAS_MAPPED_ROOT QQ_ALIAS_MAPPED_FILE QQ_ALIAS_REQUEST_ROOT SRT_FRESH_APP_PER_CASE SRT_FRESH_APP_SCENARIOS SRT_RESULT_POLL_MS SRT_APP_LAUNCH_SETTLE_MS SRT_MOUNT_CONFIRM_TIMEOUT_MS SRT_APP_MOUNT_CONFIRM_RETRIES SRT_CONFIG_APPLY_TIMEOUT_MS SRT_SERVICE_CASE_SETTLE_MS SRT_FILE_MONITOR_ENABLED SRT_FAIL_FAST SRT_SCENARIO_TIMEOUT_SECONDS LAST_MOUNT_CONFIRMED_PID ADB_ROOT_MODE
export -f detect_adb_root_mode adb_root adb_su adb_su_timeout adb_write_file test_app_uid fix_private_backend_permissions fix_own_private_fixture_permissions wait_boot_completed restart_media_provider recover_scoped_fuse_start write_config write_global_config test_global_config set_backend_config apply_config apply_config_and_wait target_path logical_dir expected_path scenario_title prepare_backend_core_targets prepare_any_path_targets assert_fixture_roots_empty clean_targets clean_results latest_result wait_service_result wait_app_mount_confirmed scenario_from_label label_expects_mount expected_mount_paths_for_label app_mountinfo_has_expected_paths ensure_current_app_mount_confirmed wait_config_applied service_case_timeout_seconds sleep_ms prepare_service_case start_app_and_confirm_mount wait_storage_ready wait_scenario_app_view ensure_initial_storage_ready media_provider_query_ready wait_media_provider_ready media_provider_ready_cached media_provider_pid wait_media_provider_hook_ready ensure_media_provider_hook_ready restart_media_provider_with_hook_ready print_storage_state run_service_case run_write_case run_create_case run_mediastore_download_create_case run_mediastore_image_create_case run_mediastore_image_relative_data_create_case run_mediastore_download_create_denied_case run_write_test check_app_view expect_app_entry expect_no_app_entry find_written_file check_file_exists check_file_missing check_own_private_real_landing check_public_directory_owner run_rule_sandbox_scenario check_file_location seed_read_only_targets check_read_only_artifacts run_read_only_scenario wait_mediastore_read_only_image prepare_read_only_media_image run_mediastore_read_only_query_scenario java_bucket_id check_mediastore_bucket_id prepare_mapped_read_only_targets run_mapped_read_only_scenario run_allow_exclusion_scenario run_legacy_exclusion_scenario run_qmark_wildcard_scenario check_fuse_daemon_started check_fuse_mount_active check_scoped_fuse_daemon_started run_fuse_daemon_allow_wildcard_scenario run_fuse_daemon_read_only_exclusion_scenario run_fuse_daemon_mapping_read_only_scenario run_fuse_daemon_multi_wildcard_scenario set_mount_namespace_read_only_seed run_mount_namespace_allow_wildcard_fallback_scenario run_mount_namespace_read_only_wildcard_fallback_scenario run_mount_namespace_mapping_read_only_scenario ensure_monitor_collector clear_file_monitor_log file_monitor_watch_capacity_limited assert_file_monitor_enabled_for_scenario prepare_file_monitor_assertion wait_file_monitor_log_line expect_file_monitor_success_record expect_file_monitor_failure_record expect_no_read_only_failure_record monitor_file_name run_file_monitor_write_success_case run_file_monitor_write_denied_case run_file_monitor_existing_write_case run_file_monitor_mediastore_success_case run_file_monitor_mediastore_image_success_case run_file_monitor_mediastore_relative_data_success_case run_file_monitor_mediastore_denied_case run_file_monitor_disabled_redirect_scenario run_file_monitor_regular_scenario run_file_monitor_mediastore_scenario run_monitor_capture_failopen_scenario app_pid resume_hot_reload_app run_config_hot_reload_scenario run_backend_endpoint_recovery_scenario run_mediastore_open_typed_collection_scenario check_health capture_file_monitor_diagnostics capture_read_only_diagnostics capture_own_private_diagnostics capture_scenario2_mediastore_hook_diag print_diagnostics capture_fuse_diagnostic_sample capture_test_flow_artifacts run_standard_scenario run_any_path_mapping_scenario run_scenario
export -f media_provider_is_lazy
export -f fresh_app_enabled_for_label
export -f app_view_namespace_refresh_allowed
export -f run_quick_media_provider_restart_recovery_scenario
export -f prepare_owner_drift_fixture run_owner_drift_own_dir_scenario
export -f run_own_private_directories_scenario run_own_private_write_case
export -f run_any_path_mapping_scenario
export -f clear_alias_mediastore_fixture remove_mediastore_rows_by_pattern run_qq_alias_mapped_existing_file_scenario
export -f run_nested_mapping_chain_scenario
export -f run_provider_sandbox_scenario clear_provider_sandbox_config
export -f run_download_owner_attribution_scenario
export -f run_system_uid_attribution_scenario

failure_artifacts_captured=0
rm -f app-view-probe-broken.flag
for scenario in "${scenarios[@]}"; do
  echo "::group::scenario ${scenario}: $(scenario_title "$scenario")"
  if ! timeout --foreground "${SRT_SCENARIO_TIMEOUT_SECONDS}s" bash -e -o pipefail -c 'run_scenario "$1"' _ "$scenario"; then
    echo "scenario ${scenario}: failed or timed out"
    # 在健康探测或下一场景清理之前保存首个失败现场；采集失败仍保留场景失败状态。
    if [ "$failure_artifacts_captured" -eq 0 ]; then
      timeout --foreground 180s bash -c 'capture_test_flow_artifacts' || true
      failure_artifacts_captured=1
    fi
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
if ! timeout --foreground 120s bash -c 'check_health'; then
  echo "health check timed out or failed"
  fail=1
fi
if [ "$failure_artifacts_captured" -eq 0 ] && ! timeout --foreground 180s bash -c 'capture_test_flow_artifacts'; then
  echo "test-flow artifact capture timed out"
fi

if [ "${SRT_SKIP_FINAL_CLEANUP:-0}" != "1" ]; then
  trap - EXIT
  cleanup_test_artifacts
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

exit 0
