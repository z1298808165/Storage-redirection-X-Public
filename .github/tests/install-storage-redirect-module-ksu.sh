#!/usr/bin/env bash
set -eu

# KernelSU 版模块安装流程：用 KernelSU-AVD 给模拟器打 KernelSU，再用 ksud 安装
# Zygisk Next 与 Storage Redirect X 模块。
#
# 与 Magisk 版（install-storage-redirect-module.sh）的关键差异：
#   1. root 来自 KernelSU LKM，ramdisk 由 ksuAVD.sh 通过 ksud boot-patch 注入；
#   2. KernelSU 不自带 Zygisk，SRX 的应用进程注入依赖独立的 Zygisk 实现
#      （Zygisk Next，module id=zygisksu），必须先于 SRX 安装并在同一次重启生效；
#   3. 模块安装器是 ksud，安装后同样落到 /data/adb/modules_update，重启后合并。
#
# 该脚本只由 KernelSU 实验 workflow（ci-kernelsu-a17.yml）调用，不参与正式测试矩阵。

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

# KernelSU 与 Zygisk 资产固定版本，保证可复现；需要试新版本时用环境变量覆盖。
#
# 默认用上游官方 KernelSU。KernelSU-Next v3.4.0（ksud 3.4.0 / 33294，uapi 4）也实测通过，
# 但未发现官方版的能力缺失：官方 v3.3.0（32601，uapi 2）在 run 36793549753 上未开
# permissive 也跑通了场景 2，此前那次 `Unable to apply SELinux patches` 属间歇现象。
# 换版本时用 KERNELSU_APK_URL 覆盖，并重新跑完整对照再下结论。
KERNELSU_APK_URL="${KERNELSU_APK_URL:-https://github.com/tiann/KernelSU/releases/download/v3.3.0/KernelSU_v3.3.0_32601-release.apk}"
ZYGISK_NEXT_URL="${ZYGISK_NEXT_URL:-https://github.com/LSPosed/ZygiskNext/releases/download/v1.5.0/Zygisk-Next-1.5.0-843-5217106-release.zip}"

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

download_asset "$KERNELSU_APK_URL" "$KSUAVD_DIR/KernelSU.apk" "KernelSU 管理器 APK"
download_asset "$ZYGISK_NEXT_URL" "$KSUAVD_DIR/ZygiskNext.zip" "Zygisk Next 模块"

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
  sleep 5
  if [ -f "$EMULATOR_LOG" ]; then
    tail -80 "$EMULATOR_LOG" || true
  fi
}

# KernelSU 的 su 不接受 `su -c`（实测报 `su: invalid uid/gid '-c'`），只有
# `su 0 sh -c` 能拿到 root。这里先探测一次可用形态并固定下来：不能在每次调用里
# 用 `||` 退回另一种写法——命令执行成功但内部失败时会误触发兜底，把真实退出码
# 换成兜底命令的退出码。远程命令用 base64 传递，避免多层引号转义出错。
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

# 实测 KernelSU v3.3 的 LKM 模式在 AVD 上并不落盘 /data/adb/ksu，设备上没有现成
# 的 ksud；而 Zygisk Next 的 customize.sh 又硬编码调用 /data/adb/ksud（缺它就直接
# `Failed to install module script`）。这里保证该路径一定有 ksud：优先复用设备上
# 已有的 ksud，没有就从同一个 KernelSU APK 解出与模拟器架构匹配的那一份，版本与
# 已注入的 LKM 同源。
ensure_ksud() {
  if adb_root '[ -x /data/adb/ksud ]' >/dev/null 2>&1; then
    echo "ksud 已在 /data/adb/ksud。"
    adb_root '/data/adb/ksud --version 2>&1 || true'
    return 0
  fi

  local source_bin
  source_bin="$(adb_root 'for bin in /data/adb/ksu/bin/ksud /data/local/tmp/ksud; do [ -x "$bin" ] && { echo "$bin"; break; }; done; command -v ksud 2>/dev/null || true' | tr -d '\r' | grep -E '^/' | head -1 || true)"
  if [ -z "$source_bin" ]; then
    echo "设备上没有 ksud，从 KernelSU APK 解出后推送。"
    python3 - "$KSUAVD_DIR/KernelSU.apk" "$KSUAVD_DIR/ksud" <<'PY'
import sys
import zipfile

apk_path, out_path = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(apk_path) as apk:
    names = [n for n in apk.namelist() if n.endswith("/libksud.so")]
    preferred = [n for n in names if "x86_64" in n] or names
    if not preferred:
        raise SystemExit("KernelSU APK 中没有 libksud.so")
    with open(out_path, "wb") as target:
        target.write(apk.read(preferred[0]))
print("ksud_source=%s" % preferred[0])
PY
    adb push "$KSUAVD_DIR/ksud" /data/local/tmp/ksud
    source_bin="/data/local/tmp/ksud"
  fi

  echo "把 ksud 从 $source_bin 补到 /data/adb/ksud"
  adb_root "chmod 755 '$source_bin'; cp -f '$source_bin' /data/adb/ksud; chmod 755 /data/adb/ksud; /data/adb/ksud --version 2>&1 || true"
}

wait_for_root_shell() {
  local timeout_seconds="${1:-120}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
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

  # Zygisk 实现必须先注册，SRX 的 zygisk/*.so 才有加载方；两者在同一次重启里生效。
  if ! adb_ksud "module install /data/local/tmp/zygisk-next.zip"; then
    echo "Zygisk Next module install failed."
    dump_root_diagnostics
    exit 1
  fi

  if ! adb_ksud "module install /data/local/tmp/storage-redirect-x.zip"; then
    echo "KernelSU module install failed."
    dump_root_diagnostics
    exit 1
  fi

  assert_installed_module_files /data/adb/modules_update/storage.redirect.x
  adb_root "ls -la /data/adb/modules_update; ls -la /data/adb/modules_update/zygisksu 2>/dev/null || echo zygisksu_absent"
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

verify_storage_redirect_module_loaded() {
  local timeout_seconds="${VERIFY_MODULE_TIMEOUT_SECONDS:-300}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); test -d \"\$module_dir\" && test ! -e \"\$module_dir/disable\" && test -d \"\$module_dir/config/apps\" && test -d \"\$logs_dir\" && { [ -z \"\$boot_id\" ] || [ \"\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true)\" = \"\$boot_id\" ] || test -f \"\$logs_dir/boot_\${boot_id}.marker\"; } && { [ -n \"\$daemon_pid\" ] && kill -0 \"\$daemon_pid\" 2>/dev/null || pidof srx_daemon >/dev/null 2>&1; }" >/dev/null 2>&1; then
      adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; echo module_state=ready; cat \"\$module_dir/module.prop\"; echo boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_ok=\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true); echo daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); ps -A | grep srx_daemon || true; ls -la \"\$logs_dir\""
      return 0
    fi
    sleep 2
  done

  echo "Storage Redirect X module did not report the expected boot and daemon state."
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
  adb reboot
  wait_for_boot 420
  wait_for_root_shell 180
  assert_installed_module_files /data/adb/modules/storage.redirect.x

  verify_storage_redirect_module_loaded
}

# KernelSU 不带 Zygisk，注入链是「Zygisk Next → SRX zygisk/*.so」。这里把链上每一环
# 都留下取证记录：模块目录、zygiskd 进程、以及真正把 so 映射进进程的内存证据。
verify_zygisk_injection_chain() {
  local target="test-flow-ksu-zygisk.txt"
  {
    echo "=== boot_id ==="
    adb_root 'cat /proc/sys/kernel/random/boot_id 2>/dev/null || true'
    echo "=== zygisksu_module_dir ==="
    adb_root 'ls -la /data/adb/modules/zygisksu 2>/dev/null || echo zygisksu_module_absent'
    echo "=== zygiskd_process ==="
    adb_root 'ps -A | grep -E "zygiskd|zygote" || echo no_zygisk_process'
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
  adb_su "boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_id=\$boot_id; echo install_state; cat /data/adb/modules/storage.redirect.x/logs/.media_hook_install_state 2>/dev/null || echo state_absent; echo media_processes; ps -A | grep -E 'providers.media|android.process.media' || true; if [ -n '${pid:-}' ]; then echo module_maps; grep -E 'storage.redirect.x/zygisk|libsrx_core' '/proc/${pid}/maps' 2>/dev/null || echo module_map_absent; fi" || true
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
  adb reboot
  wait_for_boot 420
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
      # Android 17 上 adb install 可能返回成功，但包名并未进入 PackageManager，
      # 后续场景会因 app uid/pid 缺失而失败，因此以包名校验结果作为安装成功依据。
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
start_emulator
wait_for_boot 300

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

# 取证：KernelSU 在 AVD 上的落地形态尚在摸索（实测 /data/adb/ksu 未生成），
# 这里把 root 身份、ksud 位置与 /data/adb 结构完整落盘，失败时可直接定位。
dump_root_diagnostics
ensure_ksud
install_modules_with_ksud
seed_storage_redirect_test_environment
if [ -n "${PERSIST_SRX_FUSE_PROBE:-}" ]; then
  adb_root "setprop persist.debug.srx.fuse_probe '$PERSIST_SRX_FUSE_PROBE'"
  adb_root "printf '%s\\n' '$PERSIST_SRX_FUSE_PROBE' > /data/adb/modules/storage.redirect.x/.fuse_probe"
  echo "持久 Fuse 探针属性已在模块重启前写入：$PERSIST_SRX_FUSE_PROBE"
fi
adb reboot
wait_for_boot 420
wait_for_root_shell 180

# 诊断开关（仅实验通道）：KernelSU 的 LKM 在 AVD 上应用 sepolicy 失败
# （模块安装时打印 `Unable to apply SELinux patches`），随后系统进程写
# `.media_hook_install_state` 会被 avc denied 拦住，MediaProvider hook 装不上。
# 这里把 SELinux 置为 permissive 只是为了验证「sepolicy 补丁缺失」是不是唯一
# 阻塞项，不作为正式链路的行为——默认关闭，需显式打开。
if [ "${SRT_KSU_SELINUX_PERMISSIVE:-0}" = "1" ]; then
  echo "诊断：把 SELinux 置为 permissive（仅用于验证 sepolicy 是否为唯一阻塞项）"
  adb_root 'setenforce 0; getenforce'
fi

assert_installed_module_files /data/adb/modules/storage.redirect.x

verify_storage_redirect_module_loaded_with_reboot_retry
verify_zygisk_injection_chain
defer_media_provider_hook_check_for_lazy_provider
ensure_test_app_available
adb shell "am start -n $APP_ID/.MainActivity" >/dev/null 2>&1 || true
sleep 3
adb shell appops set "$APP_ID" MANAGE_EXTERNAL_STORAGE allow >/dev/null 2>&1 || true
