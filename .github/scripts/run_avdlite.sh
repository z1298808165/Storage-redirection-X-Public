#!/usr/bin/env bash
set -euo pipefail

export ANDROID_SERIAL="emulator-$EMULATOR_PORT"
# ADB 扫描的是控制台相邻的传输端口。启动服务前校正本轮的扫描范围。
if [[ -n "${ADB_LOCAL_TRANSPORT_MAX_PORT:-}" ]] && (( ADB_LOCAL_TRANSPORT_MAX_PORT < EMULATOR_PORT + 1 )); then
  export ADB_LOCAL_TRANSPORT_MAX_PORT=$((EMULATOR_PORT + 1))
fi
timeout 15s adb start-server
emulator_pid=""
cleanup() {
  local status=$?
  if [ "$status" -eq 0 ] && ! timeout 30s adb -s "$ANDROID_SERIAL" shell sync; then
    echo "关闭模拟器前无法将测试清理结果写入磁盘。" >&2
    status=1
  fi
  timeout 15s adb emu kill >/dev/null 2>&1 || true
  if [ -n "$emulator_pid" ] && jobs -pr | grep -qx "$emulator_pid"; then
    kill "$emulator_pid" >/dev/null 2>&1 || true
    for attempt in 1 2 3 4 5; do
      jobs -pr | grep -qx "$emulator_pid" || break
      sleep 1
    done
    if jobs -pr | grep -qx "$emulator_pid"; then
      kill -KILL "$emulator_pid" >/dev/null 2>&1 || true
    fi
    wait "$emulator_pid" 2>/dev/null || true
  fi
  return "$status"
}
trap 'cleanup || exit $?' EXIT

read -r -a options <<< "$EMULATOR_OPTIONS"
if [ "$DISABLE_HW_ACCEL" = true ] || { [ "$DISABLE_HW_ACCEL" = auto ] && { [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; }; }; then
  options+=(-accel off)
fi
if command -v cygpath >/dev/null 2>&1; then
  for key in ANDROID_AVD_HOME ANDROID_USER_HOME ANDROID_EMULATOR_HOME; do
    if [[ -n "${!key:-}" ]]; then
      path=$(cygpath -am "${!key}")
      export "$key=$path"
    fi
  done
fi
emulator_log="$RUNNER_TEMP/avdlite-boot.log"
"$ANDROID_HOME/emulator/emulator" -port "$EMULATOR_PORT" -avd "$AVD_NAME" "${options[@]}" > "$emulator_log" 2>&1 &
emulator_pid=$!
deadline=$((SECONDS + BOOT_TIMEOUT))
while :; do
  if ! kill -0 "$emulator_pid" 2>/dev/null || [ "$SECONDS" -ge "$deadline" ]; then
    tail -100 "$emulator_log"
    timeout 10s adb devices -l >&2 || true
    if ! kill -0 "$emulator_pid" 2>/dev/null; then
      echo "AvdLite 进程提前退出。" >&2
    else
      echo "AvdLite 等待 $ANDROID_SERIAL 的 ADB 开机属性超时。" >&2
    fi
    exit 1
  fi
  if command -v cygpath >/dev/null 2>&1 && grep -qm1 'Unexpected error while creating: .*emu-last-feature-flags.protobuf.lock' "$emulator_log"; then
    tail -c 4096 "$emulator_log" >&2
    echo "模拟器无法创建 Android 用户目录锁文件。" >&2
    exit 1
  fi
  if [ "$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = 1 ]; then
    break
  fi
  sleep 2
done

page_size="$(timeout 15s adb shell getconf PAGE_SIZE | tr -d '\r')"
if [ "$page_size" != 4096 ]; then
  echo "AvdLite 页大小错误：$page_size，期望 4096。" >&2
  exit 1
fi
adb shell input keyevent 82
if [ "$DISABLE_ANIMATIONS" = true ]; then
  for setting in window_animation_scale transition_animation_scale animator_duration_scale; do
    adb shell settings put global "$setting" 0.0
  done
fi
bash .github/tests/run-android-test-flow.sh
