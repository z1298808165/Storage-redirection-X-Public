#!/usr/bin/env bash
set -euo pipefail

emulator_pid=""
cleanup() {
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
}
trap cleanup EXIT

read -r -a options <<< "$EMULATOR_OPTIONS"
if [ "$DISABLE_HW_ACCEL" = true ] || { [ "$DISABLE_HW_ACCEL" = auto ] && { [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; }; }; then
  options+=(-accel off)
fi
emulator_log="$RUNNER_TEMP/avdlite-boot.log"
"$ANDROID_HOME/emulator/emulator" -port "$EMULATOR_PORT" -avd "$AVD_NAME" "${options[@]}" > "$emulator_log" 2>&1 &
emulator_pid=$!
deadline=$((SECONDS + BOOT_TIMEOUT))
while :; do
  if ! kill -0 "$emulator_pid" 2>/dev/null || [ "$SECONDS" -ge "$deadline" ]; then
    tail -100 "$emulator_log"
    echo "AvdLite 未在预算内完成开机。" >&2
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
