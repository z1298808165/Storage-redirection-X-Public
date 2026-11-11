#!/usr/bin/env bash
set -eu

# CI 的 A15-A17 统一由 KernelSU-AVD 注入 KernelSU LKM。
# KernelSU 不带 Zygisk，须先安装 Zygisk Next，再安装 SRX；两者在同一次重启生效。

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

MODULE_ZIP="${MODULE_ZIP:-$(find build/test-flow -maxdepth 2 -name '*x86_64*.zip' -print -quit 2>/dev/null || true)}"
if [ -z "$MODULE_ZIP" ]; then
  MODULE_ZIP="$(find core -maxdepth 1 -name '*x86_64.zip' -print -quit 2>/dev/null || true)"
fi
if [ -z "$MODULE_ZIP" ]; then
  echo "No Storage Redirect X x86_64 module zip was found."
  exit 1
fi

APP_ID="${APP_ID:-me.fakerqu.test.storageredirect}"
APP_APK="${APP_APK:-$(find tests/storage-redirect-test/app/build/outputs/apk/debug -maxdepth 1 -name '*-debug.apk' -print -quit 2>/dev/null || true)}"

# CI/Release 传入已校验的最新稳定版缓存；以下 URL 仅供未传缓存的独立调用使用。
KERNELSU_APK_URL="${KERNELSU_APK_URL:-https://github.com/tiann/KernelSU/releases/download/v3.3.1/KernelSU_v3.3.1_32680-release.apk}"
ZYGISK_NEXT_URL="${ZYGISK_NEXT_URL:-https://github.com/LSPosed/ZygiskNext/releases/download/v1.5.3/Zygisk-Next-1.5.3-880-22a1bf5-release.zip}"

KSUAVD_DIR="${RUNNER_TEMP:-/tmp}/ksuAVD"
rm -rf "$KSUAVD_DIR"
mkdir -p "$KSUAVD_DIR"
cp .github/vendor/ksuAVD/ksuAVD.sh "$KSUAVD_DIR/ksuAVD.sh"
chmod +x "$KSUAVD_DIR/ksuAVD.sh"

download_asset() {
  local url="$1"
  local target="$2"
  local label="$3"
  echo "下载 ${label}：$url"
  curl -fL --retry 5 --retry-delay 5 --retry-all-errors "$url" -o "$target"
}

if [ -n "${KERNELSU_APK_CACHE:-}" ]; then
  if [ ! -f "$KERNELSU_APK_CACHE" ]; then
    echo "Cached KernelSU APK not found: $KERNELSU_APK_CACHE" >&2
    exit 1
  fi
  cp "$KERNELSU_APK_CACHE" "$KSUAVD_DIR/KernelSU.apk"
else
  download_asset "$KERNELSU_APK_URL" "$KSUAVD_DIR/KernelSU.apk" "KernelSU 管理器 APK"
fi
if [ -n "${ZYGISK_NEXT_CACHE_ZIP:-}" ]; then
  if [ ! -f "$ZYGISK_NEXT_CACHE_ZIP" ]; then
    echo "Cached Zygisk Next archive not found: $ZYGISK_NEXT_CACHE_ZIP" >&2
    exit 1
  fi
  cp "$ZYGISK_NEXT_CACHE_ZIP" "$KSUAVD_DIR/ZygiskNext.zip"
else
  download_asset "$ZYGISK_NEXT_URL" "$KSUAVD_DIR/ZygiskNext.zip" "Zygisk Next 模块"
fi

RAMDISK_REL="system-images/android-${ANDROID_API_LEVEL}/${ANDROID_TARGET}/${ANDROID_ARCH}/ramdisk.img"
RAMDISK="$ANDROID_HOME/$RAMDISK_REL"
if [ ! -f "$RAMDISK" ]; then
  echo "No ramdisk.img found at expected Android SDK system image path."
  exit 1
fi

wait_for_boot() {
  local timeout_seconds="${1:-300}"
  local deadline=$((SECONDS + timeout_seconds))
  local boot_completed=""

  while [ "$SECONDS" -lt "$deadline" ]; do
    # 返回 2 区分进程提前退出与普通启动超时，供调用方决定是否重试。
    if [ -n "${EMULATOR_PID:-}" ] && ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "模拟器进程已退出（pid=$EMULATOR_PID），不再等待开机。"
      if [ -n "${EMULATOR_LOG:-}" ] && [ -f "$EMULATOR_LOG" ]; then
        echo "=== emulator log tail ==="
        tail -80 "$EMULATOR_LOG" || true
      fi
      return 2
    fi
    timeout 10s adb wait-for-device >/dev/null 2>&1 || true
    boot_completed="$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)"
    if [ "$boot_completed" = "1" ]; then
      return 0
    fi
    if adb devices | grep -q 'offline'; then
      adb kill-server >/dev/null 2>&1 || true
    fi
    sleep 2
  done

  echo "Timed out waiting for emulator boot."
  adb devices -l || true
  if [ -n "${EMULATOR_LOG:-}" ] && [ -f "$EMULATOR_LOG" ]; then
    echo "=== emulator log tail ==="
    tail -200 "$EMULATOR_LOG" || true
  fi
  return 1
}

wait_for_emulator_shutdown() {
  local timeout_seconds="${1:-60}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    if ! adb devices | grep -q '^emulator-'; then
      return 0
    fi
    adb emu kill >/dev/null 2>&1 || true
    sleep 2
  done

  echo "Timed out waiting for previous emulator shutdown."
  adb devices -l || true
  return 1
}

EMULATOR_PID=""

start_emulator() {
  local avd_name="${AVD_NAME:-test}"
  local emulator_port="${EMULATOR_PORT:-5554}"
  local gpu_mode="${EMULATOR_GPU_MODE:-swiftshader_indirect}"
  local ramdisk_args=()
  EMULATOR_LOG="${RUNNER_TEMP:-/tmp}/ksu-rooted-emulator.log"

  if [ -n "${PATCHED_RAMDISK:-}" ] && [ -f "$PATCHED_RAMDISK" ]; then
    ramdisk_args=(-ramdisk "$PATCHED_RAMDISK")
  fi

  nohup "$ANDROID_HOME/emulator/emulator" -port "$emulator_port" -avd "$avd_name" "${ramdisk_args[@]}" -no-window -gpu "$gpu_mode" -no-snapshot-load -no-snapshot-save -noaudio -no-boot-anim >"$EMULATOR_LOG" 2>&1 &
  # 供 wait_for_boot 检查本次启动的进程是否存活。
  EMULATOR_PID=$!
  sleep 5
  if [ -f "$EMULATOR_LOG" ]; then
    tail -80 "$EMULATOR_LOG" || true
  fi
}

# 仅在模拟器进程提前退出（返回 2）时重试一次，普通启动超时直接失败。
start_emulator_and_boot() {
  local attempt rc
  for attempt in 1 2; do
    start_emulator
    rc=0
    wait_for_boot 300 || rc=$?
    if [ "$rc" -eq 0 ]; then
      return 0
    fi
    if [ "$rc" -ne 2 ] || [ "$attempt" -eq 2 ]; then
      return 1
    fi
    echo "模拟器启动后立即退出，重试一次（第 ${attempt}/2 次）。"
    wait_for_emulator_shutdown 30 || true
    adb kill-server >/dev/null 2>&1 || true
  done
  return 1
}

wait_for_reboot() {
  local timeout_seconds="${1:-420}"
  local previous_boot_id deadline current_boot_id stable_attempts restarts=0
  previous_boot_id="$(timeout 10s adb shell cat /proc/sys/kernel/random/boot_id 2>/dev/null | tr -d '\r' || true)"
  if [ -z "$previous_boot_id" ]; then
    echo "无法读取重启前的 boot_id，拒绝把旧开机状态当成重启完成。" >&2
    return 1
  fi

  adb reboot >/dev/null 2>&1 || true
  deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ -n "$EMULATOR_PID" ] && ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      if [ "$restarts" -ge 1 ]; then
        echo "重启期间模拟器进程再次退出，放弃设备启动恢复。" >&2
        return 1
      fi
      echo "重启期间模拟器进程退出，重新启动已打补丁的 AVD。" >&2
      wait_for_emulator_shutdown 30 || true
      adb kill-server >/dev/null 2>&1 || true
      if ! start_emulator_and_boot; then
        return 1
      fi
      restarts=$((restarts + 1))
      continue
    fi

    current_boot_id="$(timeout 10s adb shell cat /proc/sys/kernel/random/boot_id 2>/dev/null | tr -d '\r' || true)"
    if [ -n "$current_boot_id" ] && [ "$current_boot_id" != "$previous_boot_id" ] &&
      [ "$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = "1" ]; then
      stable_attempts=0
      while [ "$stable_attempts" -lt 5 ]; do
        sleep 1
        if { [ -n "$EMULATOR_PID" ] && ! kill -0 "$EMULATOR_PID" 2>/dev/null; } ||
          [ "$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" != "1" ] ||
          [ "$(timeout 10s adb shell cat /proc/sys/kernel/random/boot_id 2>/dev/null | tr -d '\r' || true)" != "$current_boot_id" ]; then
          break
        fi
        stable_attempts=$((stable_attempts + 1))
      done
      if [ "$stable_attempts" -eq 5 ]; then
        echo "模拟器重启完成且连接稳定：boot_id=$current_boot_id"
        return 0
      fi
      continue
    fi
    sleep 2
  done

  echo "重启后未在 ${timeout_seconds}s 内观察到新的、稳定的 boot_id。" >&2
  return 1
}

# 先固定可用的 su 形式，避免将远程命令失败误当成 su 语法失败而重复执行。
ROOT_SU_FORM=""
detect_root_su_form() {
  if [ -n "$ROOT_SU_FORM" ]; then
    return 0
  fi
  if adb shell 'su 0 sh -c id' >/dev/null 2>&1; then
    ROOT_SU_FORM="su 0 sh -c"
    return 0
  fi
  if adb shell 'su -c id' >/dev/null 2>&1; then
    ROOT_SU_FORM="su -c"
    return 0
  fi
  echo "No usable KernelSU root shell found." >&2
  return 1
}

adb_root() {
  local command="PATH=/data/adb/ksu/bin:/data/adb/ksu:/data/local/tmp:\$PATH; $1"
  local encoded runner
  detect_root_su_form
  encoded="$(printf '%s' "$command" | base64 | tr -d '\n')"
  runner="printf '%s' '$encoded' | base64 -d | sh"
  adb shell "$ROOT_SU_FORM '$runner'"
}

adb_su() {
  local -
  set -o pipefail
  adb_root "$1" | tr -d '\r'
}

adb_write_file() {
  local path="$1"
  local content="$2"
  local encoded
  encoded="$(printf '%s' "$content" | base64 | tr -d '\n')"
  adb_root "printf '%s' '$encoded' | base64 -d > '$path'"
}

adb_ksud() {
  local args="$1"
  adb_root "for bin in /data/adb/ksud /data/adb/ksu/bin/ksud /data/local/tmp/ksud ksud; do if [ -x \"\$bin\" ]; then \"\$bin\" $args; exit \$?; fi; found=\$(command -v \"\$bin\" 2>/dev/null || true); if [ -n \"\$found\" ]; then \"\$found\" $args; exit \$?; fi; done; echo ksud_not_found >&2; exit 127"
}

# Zygisk Next 安装器使用 /data/adb/ksud，始终从本轮 APK 提取，避免复用旧版。
ensure_ksud() {
  unzip -p "$KSUAVD_DIR/KernelSU.apk" lib/x86_64/libksud.so > "$KSUAVD_DIR/ksud"
  test -s "$KSUAVD_DIR/ksud"
  adb push "$KSUAVD_DIR/ksud" /data/local/tmp/ksud
  adb_root 'cp -f /data/local/tmp/ksud /data/adb/ksud && chmod 755 /data/adb/ksud && /data/adb/ksud --version'
}

wait_for_root_shell() {
  local timeout_seconds="${1:-120}"
  local deadline=$((SECONDS + timeout_seconds))
  local restarts=0

  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ -n "$EMULATOR_PID" ] && ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      if [ "$restarts" -ge 1 ]; then
        echo "模拟器进程再次退出，KernelSU root shell 不可用。" >&2
        return 1
      fi
      echo "等待 KernelSU root shell 时模拟器退出，重新启动已打补丁的 AVD。" >&2
      wait_for_emulator_shutdown 30 || true
      adb kill-server >/dev/null 2>&1 || true
      if ! start_emulator_and_boot; then
        return 1
      fi
      restarts=$((restarts + 1))
      continue
    fi
    if adb_root 'id' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for KernelSU root shell."
  return 1
}

dump_root_diagnostics() {
  local target="${1:-test-flow-ksu-root.txt}"
  {
    echo "=== boot_properties ==="
    adb shell 'getprop ro.build.version.sdk; getprop ro.build.version.release; uname -r; getprop sys.boot_completed' 2>&1 || true
    echo "=== su_probe ==="
    adb shell 'command -v su || true; ls -la /data/adb/ksu 2>/dev/null || echo ksu_dir_absent' 2>&1 || true
    echo "=== su_forms ==="
    adb shell 'su -c id' 2>&1 | head -3 || true
    adb shell 'su 0 sh -c id' 2>&1 | head -3 || true
    echo "=== root_shell ==="
    adb_root 'id; echo adb_dir; ls -la /data/adb 2>/dev/null || echo adb_dir_absent; echo ksu_bin; ls -la /data/adb/ksu/bin 2>/dev/null || echo ksu_bin_absent; echo ksud_lookup; command -v ksud || echo ksud_not_in_path; echo ksud_version; for bin in /data/adb/ksu/bin/ksud ksud /data/local/tmp/ksud; do [ -x "$bin" ] && { "$bin" -V 2>&1 || true; break; }; done' 2>&1 || true
    echo "=== modules ==="
    adb_root 'ls -la /data/adb/modules 2>/dev/null || echo modules_absent; ls -la /data/adb/modules_update 2>/dev/null || echo modules_update_absent' 2>&1 || true
    echo "=== selinux_state ==="
    adb_root 'getenforce; cat /sys/fs/selinux/enforce 2>/dev/null || echo enforce_unreadable; ls /sys/fs/selinux 2>/dev/null | head -20 || echo selinuxfs_absent' 2>&1 || true
    echo "=== ksud_sepolicy_check ==="
    adb_root 'for bin in /data/adb/ksud /data/adb/ksu/bin/ksud ksud; do [ -x "$bin" ] && { echo "ksud=$bin"; "$bin" sepolicy check "allow ksu_file ksu_file file setattr" 2>&1; echo "check_exit=$?"; break; }; done || echo ksud_missing' 2>&1 || true
    echo "=== kernel_ksu_log ==="
    adb_root 'dmesg 2>/dev/null | grep -iE "kernelsu|ksu|sepolicy|avc|lsm|symbol" | tail -60 || echo dmesg_unavailable' 2>&1 || true
    echo "=== crash_logcat ==="
    adb logcat -d -b crash -t 200 2>/dev/null || true
    echo "=== logcat ==="
    adb logcat -d -t 300 2>/dev/null | grep -Ei 'kernelsu|ksud|zygisk|zygisksu|avc: denied|storage.redirect|srx' || true
  } >"$target" 2>&1 || true
}

run_ksuavd_patch() {
  local attempts="${KSUAVD_PATCH_ATTEMPTS:-2}"
  local timeout_seconds="${KSUAVD_PATCH_TIMEOUT_SECONDS:-900}"
  local attempt

  for attempt in $(seq 1 "$attempts"); do
    echo "Running KernelSU-AVD patch attempt $attempt/$attempts..."
    if timeout --foreground "${timeout_seconds}s" bash "$KSUAVD_DIR/ksuAVD.sh" "$RAMDISK_REL"; then
      return 0
    fi
    echo "KernelSU-AVD patch attempt $attempt failed or timed out."
    adb devices -l || true
    adb kill-server >/dev/null 2>&1 || true
    if [ "$attempt" -lt "$attempts" ]; then
      wait_for_boot 180 || true
    fi
  done

  echo "KernelSU-AVD failed to patch the emulator ramdisk after $attempts attempt(s)."
  adb shell 'uname -r; getprop ro.build.version.sdk' 2>&1 || true
  return 1
}

assert_installed_module_files() {
  local module_dir="$1"
  local module_abi="${MODULE_ABI:-x86_64}"
  local check_script='module_dir="$1"; module_abi="$2"; for file in module.prop post-fs-data.sh service.sh sepolicy.rule LICENSE COPYING bin/srx_daemon zygisk/$module_abi.so; do if [ ! -s "$module_dir/$file" ]; then echo "Installed module file is empty or missing: $module_dir/$file"; ls -la "$module_dir"; exit 1; fi; done'
  adb_root "sh -c '$(printf '%s' "$check_script" | sed "s/'/'\\''/g")' sh '$module_dir' '$module_abi'"
}

install_modules_with_ksud() {
  adb push "$KSUAVD_DIR/ZygiskNext.zip" /data/local/tmp/zygisk-next.zip
  adb push "$MODULE_ZIP" /data/local/tmp/storage-redirect-x.zip
  adb_root 'sync'

  # Zygisk 实现必须先注册，SRX 的 zygisk/*.so 才有加载方；两者在同一次重启里生效。
  local archive attempt
  for archive in zygisk-next storage-redirect-x; do
    for attempt in 1 2; do
      if adb_ksud "module install /data/local/tmp/$archive.zip"; then
        break
      fi
      echo "KernelSU module $archive install attempt ${attempt}/2 failed."
      if [ "$attempt" -eq 2 ]; then
        dump_root_diagnostics
        return 1
      fi
      sleep 3
    done
  done

  assert_installed_module_files /data/adb/modules_update/storage.redirect.x
  adb_root 'module_dir=/data/adb/modules_update/zygisksu; for file in module.prop lib64/libzygisk.so; do if [ ! -s "$module_dir/$file" ]; then echo "Installed Zygisk Next file is empty or missing: $module_dir/$file" >&2; exit 1; fi; done'
  adb_root 'rm -f /data/local/tmp/zygisk-next.zip /data/local/tmp/storage-redirect-x.zip' >/dev/null 2>&1 || true
}

seed_storage_redirect_test_environment() {
  local global_config_content='{"file_monitor_enabled":false,"fuse_fix_enabled":true,"storage_backend_mode":"auto","verbose_logging_enabled":true,"auto_enable_redirect_for_new_apps":false,"auto_enable_new_apps_template_id":"","app_config_auto_save":true}'

  for module_dir in /data/adb/modules_update/storage.redirect.x /data/adb/modules/storage.redirect.x; do
    if adb_root "[ -d '$module_dir' ]"; then
      adb_root "mkdir -p '$module_dir/config/apps'"
      adb_write_file "$module_dir/config/global.json" "$global_config_content"
      adb_root "chmod 644 '$module_dir/config/global.json'"
      adb_root "rm -f '$module_dir/config/apps/${APP_ID}.json'"
    fi
  done
}

# ran 表示启动标记属于本轮 boot_id，missed 表示不匹配，unknown 表示查询无有效结果。
module_boot_marker_state() {
  local out
  out="$(adb_su 'module_dir=/data/adb/modules/storage.redirect.x; boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null); boot_ok=$(cat "$module_dir/.boot_ok" 2>/dev/null); boot_pending=$(cat "$module_dir/.boot_pending" 2>/dev/null); if [ -n "$boot_id" ] && { [ "$boot_ok" = "$boot_id" ] || [ "$boot_pending" = "$boot_id" ]; }; then echo ran; else echo missed; fi' 2>/dev/null | tr -d '\r' | awk 'NF {print; exit}')"
  case "$out" in
    ran | missed) printf '%s' "$out" ;;
    *) printf 'unknown' ;;
  esac
}

verify_storage_redirect_module_loaded() {
  local timeout_seconds="${VERIFY_MODULE_TIMEOUT_SECONDS:-300}"
  local deadline=$((SECONDS + timeout_seconds))
  local reason="等待 ${timeout_seconds}s 内未上报"

  # 本轮未执行 post-fs-data 时直接进入干净重启，避免无效轮询。
  if [ "$(module_boot_marker_state)" = "missed" ]; then
    reason="本次开机未执行 post-fs-data（.boot_ok/.boot_pending 均不是本轮 boot_id）"
  else
    while [ "$SECONDS" -lt "$deadline" ]; do
      if adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); test -d \"\$module_dir\" && test ! -e \"\$module_dir/disable\" && test -d \"\$module_dir/config/apps\" && test -d \"\$logs_dir\" && { [ -z \"\$boot_id\" ] || [ \"\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true)\" = \"\$boot_id\" ] || test -f \"\$logs_dir/boot_\${boot_id}.marker\"; } && { [ -n \"\$daemon_pid\" ] && kill -0 \"\$daemon_pid\" 2>/dev/null || pidof srx_daemon >/dev/null 2>&1; }" >/dev/null 2>&1; then
        adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; echo module_state=ready; cat \"\$module_dir/module.prop\"; echo boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_ok=\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true); echo daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); ps -A | grep srx_daemon || true; ls -la \"\$logs_dir\""
        return 0
      fi
      sleep 2
    done
  fi

  echo "Storage Redirect X module did not report the expected boot and daemon state.（$reason）"
  dump_root_diagnostics
  adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; echo boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_ok=\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true); echo boot_pending=\$(cat \"\$module_dir/.boot_pending\" 2>/dev/null || true); echo daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); ps -A | grep -E 'srx_daemon|zygisk' || true; ls -la /data/adb/modules; ls -la \"\$module_dir\"; ls -la \"\$logs_dir\" 2>/dev/null || true; mount | grep -E 'srx|storage.redirect|zygisk|fuse' || true; cat /proc/mounts | grep -E 'srx|storage.redirect|zygisk|fuse' || true" || true
  adb logcat -d -t 500 | grep -Ei 'kernelsu|ksud|zygisk|zygisksu|storage.redirect|srx|avc: denied|linker|fatal' || true
  return 1
}

verify_storage_redirect_module_loaded_with_reboot_retry() {
  if verify_storage_redirect_module_loaded; then
    return 0
  fi

  echo "Storage Redirect X module was installed but did not start after the first reboot; retrying one clean boot."
  wait_for_reboot 420
  wait_for_root_shell 180
  assert_installed_module_files /data/adb/modules/storage.redirect.x

  verify_storage_redirect_module_loaded
}

# 注入状态用于门禁；模块目录、进程与映射仅供诊断。
verify_zygisk_injection_chain() {
  local target="test-flow-ksu-zygisk.txt"
  local status
  status="$(adb_root '/data/adb/modules/zygisksu/bin/zygiskd status' | tr -d '\r')"
  {
    echo "=== boot_id ==="
    adb_root 'cat /proc/sys/kernel/random/boot_id 2>/dev/null || true'
    echo "=== zygisksu_module_dir ==="
    adb_root 'ls -la /data/adb/modules/zygisksu 2>/dev/null || echo zygisksu_module_absent'
    echo "=== zygiskd_process ==="
    adb_root 'ps -A | grep -E "zygiskd|zygote" || echo no_zygisk_process'
    echo "=== zygiskd_status ==="
    printf '%s\n' "$status"
    echo "=== maps_with_zygisksu ==="
    adb_root 'timeout 20 sh -c "grep -l zygisksu /proc/[0-9]*/maps 2>/dev/null | head -20" || echo maps_probe_failed'
    echo "=== maps_with_srx_zygisk ==="
    adb_root 'timeout 20 sh -c "grep -l storage.redirect.x/zygisk /proc/[0-9]*/maps 2>/dev/null | head -20" || echo maps_probe_failed'
    echo "=== module_logs ==="
    adb_root 'tail -30 /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null || echo running_log_absent'
  } >"$target" 2>&1 || true

  if grep -q 'zygisksu_module_absent' "$target"; then
    echo "Zygisk Next 未安装到 /data/adb/modules/zygisksu，应用注入链不可用。" >&2
    return 1
  fi
  if ! printf '%s\n' "$status" | grep -Eq '(^|[[:space:]])inject_state:[[:space:]]*1([[:space:]]|$)'; then
    echo "Zygisk Next 未报告活动的注入状态（诊断记录已写入 $target）。" >&2
    return 1
  fi
  if grep -q 'maps_probe_failed' "$target"; then
    echo "未能取到 zygisk 映射证据（诊断记录已写入 $target）。" >&2
  fi
  return 0
}

media_provider_pid() {
  adb_su 'for package in com.android.providers.media.module com.google.android.providers.media.module com.android.providers.media android.process.media; do pidof "$package" 2>/dev/null || true; done' |
    tr -d '\r' |
    awk 'NF { print $1; exit }'
}

restart_media_provider_process() {
  local package
  for package in com.google.android.providers.media.module com.android.providers.media.module com.android.providers.media; do
    adb shell am force-stop "$package" >/dev/null 2>&1 || true
  done
  adb_su "pkill -f com.google.android.providers.media.module 2>/dev/null || true; pkill -f com.android.providers.media.module 2>/dev/null || true" >/dev/null 2>&1 || true
  sleep 2
}

wait_media_provider_hook_ready() {
  local label="$1"
  local timeout_seconds="${2:-60}"
  local deadline pid boot_id install_state
  deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    adb shell content query --uri content://media/external_primary/file --projection _id --where '_id=-1' >/dev/null 2>&1 || true
    pid="$(media_provider_pid)"
    if [ -n "$pid" ]; then
      boot_id="$(adb shell cat /proc/sys/kernel/random/boot_id 2>/dev/null | tr -d '\r' || true)"
      install_state="$(adb_su 'cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null || true' | tr -d '\r')"
      if [ -n "$boot_id" ] && grep -Fq "stage=init_ok pid=${pid} boot_id=${boot_id} " <<<"$install_state"; then
        echo "media_provider_hook_ready label=${label} pid=${pid} boot_id=${boot_id}"
        return 0
      fi
    fi
    sleep 2
  done

  pid="$(media_provider_pid)"
  echo "MediaProvider hook did not become ready: label=${label} pid=${pid:-missing}" >&2
  adb_su "boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_id=\$boot_id; echo install_state; cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null || echo state_absent; echo deferred_marker; ls -la /data/adb/modules/storage.redirect.x/logs/.media_hook_deferred 2>/dev/null || echo marker_absent; echo media_processes; ps -A | grep -E 'providers.media|android.process.media' || true; if [ -n '${pid:-}' ]; then echo module_maps; grep -E 'storage.redirect.x/zygisk|libsrx_core' '/proc/${pid}/maps' 2>/dev/null || echo module_map_absent; fi; echo module_zygisk_files; ls -la /data/adb/modules/storage.redirect.x/zygisk/ 2>/dev/null || echo zygisk_dir_absent; echo module_mapped_processes; timeout 15 sh -c \"grep -l 'storage.redirect.x/zygisk' /proc/[0-9]*/maps 2>/dev/null | head -20\" || echo maps_probe_failed; echo media_provider_state_log; tail -20 /data/adb/modules/storage.redirect.x/logs/media_provider_state.log 2>/dev/null || echo media_provider_state_absent; echo module_running_log_tail; tail -30 /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null || echo running_log_absent" || true
  adb logcat -d -t 500 | grep -Ei 'kernelsu|ksud|zygisk|zygisksu|storage.redirect|srx|avc: denied|linker|fatal' || true
  return 1
}

verify_media_provider_hook_with_reboot_retry() {
  if wait_media_provider_hook_ready "module-boot" 60; then
    return 0
  fi

  echo "MediaProvider hook absent after module boot; restarting the MediaProvider process and re-checking."
  restart_media_provider_process
  if wait_media_provider_hook_ready "module-restart" 60; then
    return 0
  fi

  echo "MediaProvider hook was absent after module boot; retrying one clean device boot."
  wait_for_reboot 420
  wait_for_root_shell 180
  assert_installed_module_files /data/adb/modules/storage.redirect.x
  VERIFY_MODULE_TIMEOUT_SECONDS=120 verify_storage_redirect_module_loaded

  wait_media_provider_hook_ready "module-clean-boot" 120
}

defer_media_provider_hook_check_for_lazy_provider() {
  case "${ANDROID_API_LEVEL:-}" in
    37|37.*|3[8-9]|3[8-9].*)
      echo "Android ${ANDROID_API_LEVEL} MediaProvider 采用惰性启动，延后 hook readiness 到测试流首次访问。"
      return 0
      ;;
  esac
  verify_media_provider_hook_with_reboot_retry
}

install_test_app_before_module_boot() {
  if [ ! -f "$APP_APK" ]; then
    echo "No test APK found at $APP_APK."
    exit 1
  fi

  local deadline=$((SECONDS + ${PACKAGE_SERVICE_TIMEOUT_SECONDS:-120}))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb shell "cmd package list packages >/dev/null 2>&1" >/dev/null 2>&1; then
      break
    fi
    adb reconnect >/dev/null 2>&1 || true
    sleep 2
  done

  local attempts="${APP_INSTALL_ATTEMPTS:-5}"
  local attempt
  local installed=0
  for attempt in $(seq 1 "$attempts"); do
    if adb install -r "$APP_APK"; then
      # 以 PackageManager 可查询到包名作为安装成功的依据。
      if adb shell "cmd package list packages 2>/dev/null | grep -qx 'package:$APP_ID'"; then
        installed=1
        break
      fi
    fi
    echo "测试 APK 安装后包名不可见，重试安装：${attempt}/${attempts}" >&2
    if [ "$attempt" -eq "$attempts" ]; then
      echo "测试 APK 安装失败，PackageManager 可能仍未就绪。" >&2
      adb shell "cmd package list packages 2>&1" || true
      exit 1
    fi
    adb reconnect >/dev/null 2>&1 || true
    sleep 5
  done
  if [ "$installed" -ne 1 ]; then
    echo "测试 APK 安装失败：包名 $APP_ID 未出现在 package 服务中。" >&2
    adb shell "cmd package list packages 2>&1" || true
    exit 1
  fi
  adb shell pm grant "$APP_ID" android.permission.READ_EXTERNAL_STORAGE >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.WRITE_EXTERNAL_STORAGE >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_IMAGES >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_VIDEO >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_AUDIO >/dev/null 2>&1 || true
  adb shell appops set "$APP_ID" MANAGE_EXTERNAL_STORAGE allow >/dev/null 2>&1 || true
}

ensure_test_app_available() {
  if adb shell "cmd package list packages 2>/dev/null | grep -qx 'package:$APP_ID'"; then
    return 0
  fi
  echo "测试应用在模块重启后不可见，重新安装" >&2
  install_test_app_before_module_boot
  if adb shell "cmd package list packages 2>/dev/null | grep -qx 'package:$APP_ID'"; then
    return 0
  fi
  echo "测试应用在重新安装后仍不可用" >&2
  adb shell "cmd package list packages 2>&1" || true
  return 1
}

install_test_app_before_module_boot

# ksuAVD 会把打好的 ramdisk 覆盖回系统镜像并留下 .backup；重跑同一 job 时先还原，
# 避免对已打补丁的 ramdisk 二次注入。
if [ -f "$RAMDISK.backup" ]; then
  echo "Restoring stock ramdisk from $RAMDISK.backup"
  cp -f "$RAMDISK.backup" "$RAMDISK"
fi

if ! run_ksuavd_patch; then
  dump_root_diagnostics
  exit 1
fi

AVD_DIR="${HOME}/.android/avd/${AVD_NAME:-test}.avd"
PATCHED_RAMDISK="$RAMDISK"
if [ -d "$AVD_DIR" ] && [ -f "$PATCHED_RAMDISK" ]; then
  echo "Copying KernelSU patched ramdisk into $AVD_DIR"
  cp "$PATCHED_RAMDISK" "$AVD_DIR/ramdisk.img"
fi

wait_for_emulator_shutdown 90
adb kill-server >/dev/null 2>&1 || true
if ! start_emulator_and_boot; then
  echo "模拟器启动后未能完成开机（日志见上方 emulator log tail），按失败处理。" >&2
  exit 1
fi

echo "Waiting for KernelSU to initialize..."

ksu_ready_attempts="${KSU_READY_ATTEMPTS:-3}"
for i in $(seq 1 "$ksu_ready_attempts"); do
  echo "Attempt $i/$ksu_ready_attempts: Checking KernelSU root availability..."
  if wait_for_root_shell 120; then
    echo "KernelSU root is available."
    break
  fi
  if [ "$i" -eq "$ksu_ready_attempts" ]; then
    echo "KernelSU root is not available after KernelSU-AVD patch."
    dump_root_diagnostics
    exit 1
  fi
  echo "KernelSU not ready yet, waiting 10s..."
  sleep 10
done

# 在安装模块前保存 root 身份、ksud 位置与 /data/adb 结构。
dump_root_diagnostics
ensure_ksud
install_modules_with_ksud
seed_storage_redirect_test_environment
if [ -n "${PERSIST_SRX_FUSE_PROBE:-}" ]; then
  adb_root "setprop persist.debug.srx.fuse_probe '$PERSIST_SRX_FUSE_PROBE'"
  adb_root "printf '%s\\n' '$PERSIST_SRX_FUSE_PROBE' > /data/adb/modules/storage.redirect.x/.fuse_probe"
  echo "持久 Fuse 探针属性已在模块重启前写入：$PERSIST_SRX_FUSE_PROBE"
fi
wait_for_reboot 420
wait_for_root_shell 180

# 仅显式设置 SRT_KSU_SELINUX_PERMISSIVE=1 时关闭 SELinux enforcement，用于隔离策略故障。
if [ "${SRT_KSU_SELINUX_PERMISSIVE:-0}" = "1" ]; then
  echo "诊断：把 SELinux 置为 permissive（仅用于验证 sepolicy 是否为唯一阻塞项）"
  adb_root 'setenforce 0; getenforce'
fi

assert_installed_module_files /data/adb/modules/storage.redirect.x

verify_storage_redirect_module_loaded_with_reboot_retry
defer_media_provider_hook_check_for_lazy_provider
verify_zygisk_injection_chain
ensure_test_app_available
adb shell "am start -n $APP_ID/.MainActivity" >/dev/null 2>&1 || true
sleep 3
adb shell appops set "$APP_ID" MANAGE_EXTERNAL_STORAGE allow >/dev/null 2>&1 || true
