#!/usr/bin/env bash
set -euo pipefail

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

PREFLIGHT_DIAGNOSTICS="test-flow-preflight.txt"

capture_preflight_diagnostics() {
  {
    echo "=== host_time ==="
    date -u
    echo "=== adb_devices ==="
    adb devices -l 2>&1 || true
    echo "=== boot_properties ==="
    adb shell 'getprop sys.boot_completed; getprop dev.bootcomplete; getprop init.svc.bootanim' 2>&1 || true
    echo "=== storage_state ==="
    adb shell 'sm list-volumes all 2>&1; ls -ld /storage /storage/emulated /storage/emulated/0 /sdcard 2>&1; df -h /storage/emulated/0 /sdcard 2>&1' 2>&1 || true
  } >"$PREFLIGHT_DIAGNOSTICS"
}

wait_for_adb_ready() {
  local deadline=$((SECONDS + 300))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ "$(adb get-state 2>/dev/null || true)" = "device" ] &&
      [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = "1" ]; then
      return 0
    fi
    if adb devices 2>/dev/null | grep -q $'\toffline$'; then
      adb reconnect offline >/dev/null 2>&1 || true
      adb kill-server >/dev/null 2>&1 || true
      adb start-server >/dev/null 2>&1 || true
    fi
    sleep 3
  done
  return 1
}

wait_for_emulated_storage() {
  local timeout_seconds="${1:-120}"
  local deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb shell "sm list-volumes all 2>/dev/null | grep -q 'emulated;0 mounted' && test -d /storage/emulated/0 && ls -ld /storage/emulated/0 >/dev/null 2>&1" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

prepare_device_health() {
  if ! wait_for_adb_ready; then
    capture_preflight_diagnostics
    echo "Android 设备未进入可用状态" >&2
    return 1
  fi
  if wait_for_emulated_storage 120; then
    return 0
  fi

  echo "共享存储未挂载，执行一次模拟器重启恢复" >&2
  adb reboot >/dev/null 2>&1 || true
  if ! wait_for_adb_ready || ! wait_for_emulated_storage 180; then
    capture_preflight_diagnostics
    echo "模拟器重启后共享存储仍不可用" >&2
    return 1
  fi
}

prepare_device_health

TEST_APP_APK="$(find tests/storage-redirect-test/app/build/outputs/apk/debug -maxdepth 1 -name '*-debug.apk' -print -quit)"
if [ -z "$TEST_APP_APK" ]; then
  echo "No test app debug APK found under tests/storage-redirect-test/app/build/outputs/apk/debug." >&2
  find tests/storage-redirect-test/app/build/outputs -maxdepth 4 -type f -name '*.apk' -print 2>/dev/null || true
  exit 1
fi

if [ "${ANDROID_API_LEVEL:-}" = "34" ] && [ -n "${PERSIST_SRX_FUSE_PROBE:-}" ]; then
  adb shell setprop persist.debug.srx.fuse_probe "$PERSIST_SRX_FUSE_PROBE" >/dev/null 2>&1 || true
  adb shell setprop debug.srx.fuse_probe "$PERSIST_SRX_FUSE_PROBE" >/dev/null 2>&1 || true
  probe_value="$(adb shell getprop debug.srx.fuse_probe 2>/dev/null | tr -d '\r' || true)"
  persist_probe_value="$(adb shell getprop persist.debug.srx.fuse_probe 2>/dev/null | tr -d '\r' || true)"
  echo "fuse_probe_property=$probe_value persist_fuse_probe_property=$persist_probe_value"
  export SRT_SCENARIOS="1"
fi

MODULE_ZIP="build/test-flow/assets/storage.redirect.x-v${VERSION}-${MODULE_ABI}.zip" \
  APP_APK="$TEST_APP_APK" \
  bash .github/tests/install-storage-redirect-module.sh

adb shell appops set me.fakerqu.test.storageredirect MANAGE_EXTERNAL_STORAGE allow || true
export SRT_SKIP_FINAL_CLEANUP=1
export SRT_FAIL_FAST="${SRT_FAIL_FAST:-1}"
export SRT_SCENARIO_TIMEOUT_SECONDS="${SRT_SCENARIO_TIMEOUT_SECONDS:-300}"
bash .github/tests/run-storage-redirect-scenarios.sh
