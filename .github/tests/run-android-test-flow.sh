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

# Android 17（API 37）系统镜像的 mapper.ranchu gralloc 未实现 DMA 颜色缓冲读回，
# 而当前 emulator 的 gfxstream 后端仍会广播 ReadColorBufferDMA 特性。SystemUI 的
# RegionSampling 与 system_server 的 TaskSnapshotPersister 都会触发读回，随后命中
# `Assertion failed: !rcEnc->featureInfo()->hasReadColorBufferDma` 并连带重启 framework。
# 该缺陷与 GPU 模式无关；API 37 测试流必须同时关闭两个入口，并在 rootAVD/模块重启
# 后重新应用，因为 task snapshot 开关是 system_server 进程内状态。
android17_disable_graphics_readback() {
  local phase="${1:-设备启动后}"
  local sdk
  sdk="$(timeout 15s adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  case "$sdk" in
    '' | *[!0-9]*) return 0 ;;
  esac
  if [ "$sdk" -ne 37 ]; then
    return 0
  fi

  echo "android17: ${phase}关闭 task snapshots 与 SystemUI 图形读回入口"

  # android17-release IWindowManager.aidl：135 为 isTaskSnapshotSupported，137 为
  # setTaskSnapshotEnabled。模块重启后的 framework 可能在短窗口内再次重启，因此
  # 每轮都绑定 system_server PID，稳定观察后再复核全部状态；PID 变化即从头重放。
  local state_file="test-flow-graphics-state.txt"
  if [ "$phase" = "初次启动后" ]; then
    : >"$state_file"
  fi

  local deadline=$((SECONDS + 300)) attempt=0
  local server_pid stable_server_pid snapshot_disable_output snapshot_state stable_snapshot_state
  local disable_output disabled_packages stable_disabled_packages system_ui_pid
  while [ "$SECONDS" -lt "$deadline" ]; do
    attempt=$((attempt + 1))
    server_pid="$(timeout 10s adb shell pidof system_server 2>/dev/null | tr -d '\r' || true)"
    if [ -z "$server_pid" ]; then
      adb reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    snapshot_disable_output="$(timeout 20s adb shell service call window 137 i32 0 2>&1 || true)"
    snapshot_state="$(timeout 20s adb shell service call window 135 2>&1 || true)"
    {
      printf '=== phase=%s attempt=%s ===\n' "$phase" "$attempt"
      printf 'system_server_pid=%s\n' "$server_pid"
      printf 'setTaskSnapshotEnabled: %s\n' "$snapshot_disable_output"
      printf 'isTaskSnapshotSupported: %s\n' "$snapshot_state"
    } >>"$state_file"
    if ! grep -Eq 'Parcel\([[:space:]]*00000000[[:space:]]+00000000' <<<"$snapshot_state"; then
      adb reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    disable_output="already_disabled"
    if ! grep -qx 'package:com.android.systemui' <<<"$disabled_packages"; then
      disable_output="$(timeout 20s adb shell "pm disable-user --user 0 com.android.systemui" 2>&1 || true)"
      disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    fi
    {
      printf 'pm_disable_user=%s\n' "$disable_output"
      printf 'disabled_packages=%s\n' "$disabled_packages"
    } >>"$state_file"
    if ! grep -qx 'package:com.android.systemui' <<<"$disabled_packages"; then
      adb reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    if ! timeout 15s adb shell "service list 2>/dev/null | grep -q 'activity'" >/dev/null 2>&1 ||
      ! timeout 15s adb shell "cmd package list packages >/dev/null 2>&1" >/dev/null 2>&1 ||
      ! wait_for_emulated_storage 20; then
      adb reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    sleep 8
    stable_server_pid="$(timeout 10s adb shell pidof system_server 2>/dev/null | tr -d '\r' || true)"
    stable_snapshot_state="$(timeout 20s adb shell service call window 135 2>&1 || true)"
    stable_disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    system_ui_pid="$(timeout 10s adb shell pidof com.android.systemui 2>/dev/null | tr -d '\r' || true)"
    {
      printf 'stable_system_server_pid=%s\n' "$stable_server_pid"
      printf 'stable_isTaskSnapshotSupported=%s\n' "$stable_snapshot_state"
      printf 'stable_system_ui_pid=%s\n' "$system_ui_pid"
    } >>"$state_file"
    if [ "$stable_server_pid" = "$server_pid" ] &&
      grep -Eq 'Parcel\([[:space:]]*00000000[[:space:]]+00000000' <<<"$stable_snapshot_state" &&
      grep -qx 'package:com.android.systemui' <<<"$stable_disabled_packages"; then
      # SystemUI 是 persistent 系统进程，system_server 会在 kill 后立即重新拉起；
      # package disabled 与 task snapshot=false 才是有效门禁，PID 只保留作诊断。
      echo "android17: ${phase}图形读回入口已关闭，system_server=${server_pid} systemui=${system_ui_pid:-absent}"
      return 0
    fi
  done

  capture_preflight_diagnostics
  echo "android17: ${phase}图形读回入口在稳定窗口内未保持关闭" >&2
  return 1
}

prepare_device_health
android17_disable_graphics_readback "初次启动后"

# CI 模拟器只保留 FuseFix 属性诊断，不安装 native FuseFix hook。
export SRT_FUSE_FIX_ENABLED=false

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

# 模块安装会重启 framework；在任何 package 枚举前立即重放图形保护，缩短
# SystemUI/task snapshot 再次触发 ReadColorBufferDMA 崩溃的窗口。
android17_disable_graphics_readback "模块重启后"

# Android 17 可能限制普通应用的 PackageManager 可见性，记录 root 与普通 shell 的枚举差异。
{
  echo "=== api_level ==="
  echo "${ANDROID_API_LEVEL:-unknown}"
  echo "=== shell_packages ==="
  adb shell "pm list packages -f -U --user 0 2>/dev/null" 2>&1 || true
  echo "=== root_packages ==="
  adb shell "su 0 sh -c 'pm list packages -f -U --user 0 2>/dev/null'" 2>&1 || true
  echo "=== package_service ==="
  adb shell "cmd package list packages -f -U --user 0 2>/dev/null" 2>&1 || true
} > test-flow-app-list.txt

# 模块重启后 system_server/package 服务可能还在恢复，进入场景脚本前再次等待。
if ! wait_for_adb_ready; then
  capture_preflight_diagnostics
  echo "模块重启后 Android 设备未恢复到可用状态" >&2
  exit 1
fi
package_service_deadline=$((SECONDS + ${PACKAGE_SERVICE_TIMEOUT_SECONDS:-120}))
while [ "$SECONDS" -lt "$package_service_deadline" ]; do
  if adb shell "cmd package list packages >/dev/null 2>&1" >/dev/null 2>&1; then
    break
  fi
  adb reconnect >/dev/null 2>&1 || true
  sleep 2
done

adb shell appops set me.fakerqu.test.storageredirect MANAGE_EXTERNAL_STORAGE allow || true
export SRT_SKIP_FINAL_CLEANUP=1
export SRT_FAIL_FAST="${SRT_FAIL_FAST:-1}"
export SRT_SCENARIO_TIMEOUT_SECONDS="${SRT_SCENARIO_TIMEOUT_SECONDS:-300}"
# 场景脚本的失败退出码直接透传给 job（此前 224 无任何标记），这里显式打印便于定位。
scenario_exit=0
bash .github/tests/run-storage-redirect-scenarios.sh || scenario_exit=$?
if [ "$scenario_exit" -ne 0 ]; then
  echo "场景脚本退出码：${scenario_exit}" >&2
  exit "$scenario_exit"
fi
