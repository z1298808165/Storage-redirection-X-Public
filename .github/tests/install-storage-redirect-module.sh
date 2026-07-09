#!/usr/bin/env bash
set -eu

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
ROOT_AVD_DIR="$RUNNER_TEMP/rootAVD"
rm -rf "$ROOT_AVD_DIR"
mkdir -p "$ROOT_AVD_DIR"
mkdir -p "$ROOT_AVD_DIR/Apps"
cp .github/vendor/rootAVD/rootAVD.sh "$ROOT_AVD_DIR/rootAVD.sh"
cp .github/vendor/rootAVD/rootAVD.bat "$ROOT_AVD_DIR/rootAVD.bat"
chmod +x "$ROOT_AVD_DIR/rootAVD.sh"

MAGISK_URL="${MAGISK_URL:-https://github.com/topjohnwu/Magisk/releases/download/v29.0/Magisk-v29.0.apk}"
if [ -n "${MAGISK_JSON:-}" ]; then
  MAGISK_URL="$(python3 - <<PY
import json
import urllib.request

with urllib.request.urlopen("$MAGISK_JSON", timeout=30) as response:
    print(json.load(response)["magisk"]["link"])
PY
)"
fi
curl -fL --retry 5 --retry-delay 5 --retry-all-errors "$MAGISK_URL" -o "$ROOT_AVD_DIR/Magisk.zip"

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
  local ramdisk_args=()
  EMULATOR_LOG="${RUNNER_TEMP:-/tmp}/rooted-emulator.log"

  if [ -n "${PATCHED_RAMDISK:-}" ] && [ -f "$PATCHED_RAMDISK" ]; then
    ramdisk_args=(-ramdisk "$PATCHED_RAMDISK")
  fi

  nohup "$ANDROID_HOME/emulator/emulator" -port "$emulator_port" -avd "$avd_name" "${ramdisk_args[@]}" -no-window -gpu swiftshader_indirect -no-snapshot-load -no-snapshot-save -noaudio -no-boot-anim >"$EMULATOR_LOG" 2>&1 &
  sleep 5
  if [ -f "$EMULATOR_LOG" ]; then
    tail -80 "$EMULATOR_LOG" || true
  fi
}

adb_root() {
  local command="PATH=/debug_ramdisk:/sbin:/data/adb/magisk:\$PATH; $1"
  local quoted
  quoted="$(printf '%s' "$command" | sed "s/'/'\\\\''/g")"
  adb shell "su 0 sh -c '$quoted'" || adb shell "su -c '$quoted'"
}

adb_su() {
  local command="PATH=/debug_ramdisk:/sbin:/data/adb/magisk:\$PATH; $1"
  adb_root "$1" || adb shell magisk su -c "$command" || adb shell /system/bin/magisk su -c "$command" || adb shell /debug_ramdisk/magisk su -c "$command"
}

adb_write_file() {
  local path="$1"
  local content="$2"
  local encoded
  encoded="$(printf '%s' "$content" | base64 | tr -d '\n')"
  adb_root "printf '%s' '$encoded' | base64 -d > '$path'"
}

adb_magisk() {
  local args="$1"
  adb_root "magisk_bin=''; for bin in /data/adb/magisk/magisk /debug_ramdisk/magisk /sbin/magisk /system/bin/magisk magisk; do if [ -x \"\$bin\" ]; then magisk_bin=\"\$bin\"; break; fi; found=\$(command -v \"\$bin\" 2>/dev/null || true); if [ -n \"\$found\" ]; then magisk_bin=\"\$found\"; break; fi; done; [ -n \"\$magisk_bin\" ] && \"\$magisk_bin\" $args"
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

  echo "Timed out waiting for root shell."
  adb shell getprop | grep -Ei 'magisk|boot|zygisk' || true
  adb shell which su || true
  adb shell which magisk || true
  adb shell ls -la /debug_ramdisk || true
  adb shell ls -la /sbin || true
  return 1
}

run_rootavd_patch() {
  local attempts="${ROOTAVD_PATCH_ATTEMPTS:-2}"
  local timeout_seconds="${ROOTAVD_PATCH_TIMEOUT_SECONDS:-600}"
  local attempt

  for attempt in $(seq 1 "$attempts"); do
    echo "Running rootAVD patch attempt $attempt/$attempts..."
    if timeout --foreground "${timeout_seconds}s" env ROOTAVD_NONINTERACTIVE=1 ROOTAVD_MAGISK_CHOICE=1 "$ROOT_AVD_DIR/rootAVD.sh" "$RAMDISK_REL"; then
      return 0
    fi
    echo "rootAVD patch attempt $attempt failed or timed out."
    adb devices -l || true
    adb kill-server >/dev/null 2>&1 || true
    if [ "$attempt" -lt "$attempts" ]; then
      wait_for_boot 180 || true
    fi
  done

  echo "rootAVD failed to patch the emulator ramdisk after $attempts attempt(s)."
  return 1
}

grant_magisk_shell() {
  adb_magisk "--sqlite \"REPLACE INTO settings (key,value) VALUES('root_access',3);\"" >/dev/null 2>&1 || true
  adb_magisk "--sqlite \"REPLACE INTO policies (uid,policy,until,logging,notification) VALUES(2000,2,0,1,0);\"" >/dev/null 2>&1 || true
}

assert_installed_module_files() {
  local module_dir="$1"
  local module_abi="${MODULE_ABI:-x86_64}"
  local check_script='module_dir="$1"; module_abi="$2"; for file in module.prop post-fs-data.sh service.sh sepolicy.rule LICENSE COPYING bin/srx_daemon zygisk/$module_abi.so; do if [ ! -s "$module_dir/$file" ]; then echo "Installed module file is empty or missing: $module_dir/$file"; ls -la "$module_dir"; exit 1; fi; done'
  adb_root "sh -c '$(printf '%s' "$check_script" | sed "s/'/'\\''/g")' sh '$module_dir' '$module_abi'"
}

install_storage_redirect_module() {
  adb push "$MODULE_ZIP" /data/local/tmp/storage-redirect-x.zip

  if adb_root 'magisk --install-module /data/local/tmp/storage-redirect-x.zip'; then
    assert_installed_module_files /data/adb/modules_update/storage.redirect.x
    adb_root 'rm -f /data/local/tmp/storage-redirect-x.zip' >/dev/null 2>&1 || true
    return
  fi

  echo "Magisk module install failed."
  adb_root 'id; command -v magisk || true; magisk -V || true; ls -la /data/adb; ls -la /data/adb/magisk; ls -la /data/adb/modules; ls -la /data/adb/modules_update' || true
  adb logcat -d -t 300 | grep -Ei 'magisk|zygisk|avc: denied|storage.redirect' || true
  exit 1
}

install_test_app_before_module_boot() {
  if [ ! -f "$APP_APK" ]; then
    echo "No test APK found at $APP_APK."
    exit 1
  fi

  adb install -r "$APP_APK"
  adb shell pm grant "$APP_ID" android.permission.READ_EXTERNAL_STORAGE >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.WRITE_EXTERNAL_STORAGE >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_IMAGES >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_VIDEO >/dev/null 2>&1 || true
  adb shell pm grant "$APP_ID" android.permission.READ_MEDIA_AUDIO >/dev/null 2>&1 || true
  adb shell appops set "$APP_ID" MANAGE_EXTERNAL_STORAGE allow >/dev/null 2>&1 || true
}

seed_storage_redirect_test_environment() {
  local global_config_content='{"file_monitor_enabled":false,"fuse_fix_enabled":true,"fuse_daemon_redirect_enabled":false,"verbose_logging_enabled":true,"auto_enable_redirect_for_new_apps":false,"auto_enable_new_apps_template_id":"","app_config_auto_save":true}'

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
  local timeout_seconds="${VERIFY_MODULE_TIMEOUT_SECONDS:-120}"
  local deadline=$((SECONDS + timeout_seconds))

  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); test -d \"\$module_dir\" && test ! -e \"\$module_dir/disable\" && test -d \"\$module_dir/config/apps\" && test -d \"\$logs_dir\" && { [ -z \"\$boot_id\" ] || [ \"\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true)\" = \"\$boot_id\" ] || test -f \"\$logs_dir/boot_\${boot_id}.marker\"; } && { [ -n \"\$daemon_pid\" ] && kill -0 \"\$daemon_pid\" 2>/dev/null || pidof srx_daemon >/dev/null 2>&1; }" >/dev/null 2>&1; then
      adb_su "module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; echo module_state=ready; cat \"\$module_dir/module.prop\"; echo boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_ok=\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true); echo daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); ps -A | grep srx_daemon || true; ls -la \"\$logs_dir\""
      return 0
    fi
    sleep 2
  done

  echo "Storage Redirect X module did not report the expected boot and daemon state."
  adb_su "id; module_dir=/data/adb/modules/storage.redirect.x; logs_dir=\"\$module_dir/logs\"; echo boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); echo boot_ok=\$(cat \"\$module_dir/.boot_ok\" 2>/dev/null || true); echo boot_pending=\$(cat \"\$module_dir/.boot_pending\" 2>/dev/null || true); echo daemon_pid=\$(cat \"\$logs_dir/.srx_daemon.pid\" 2>/dev/null || true); ps -A | grep -E 'srx_daemon|zygisk' || true; ls -la /data/adb/modules; ls -la \"\$module_dir\"; ls -la \"\$logs_dir\" 2>/dev/null || true; mount | grep -E 'srx|storage.redirect|zygisk|fuse' || true; cat /proc/mounts | grep -E 'srx|storage.redirect|zygisk|fuse' || true" || true
  adb logcat -d -t 500 | grep -Ei 'magisk|zygisk|storage.redirect|srx|avc: denied|linker|fatal' || true
  return 1
}

install_test_app_before_module_boot

if ! run_rootavd_patch; then
  exit 1
fi

AVD_DIR="${HOME}/.android/avd/${AVD_NAME:-test}.avd"
PATCHED_RAMDISK="$ANDROID_HOME/$RAMDISK_REL"
if [ -d "$AVD_DIR" ] && [ -f "$PATCHED_RAMDISK" ]; then
  echo "Copying patched ramdisk into $AVD_DIR"
  cp "$PATCHED_RAMDISK" "$AVD_DIR/ramdisk.img"
fi

wait_for_emulator_shutdown 90
adb kill-server >/dev/null 2>&1 || true
start_emulator
wait_for_boot 300

echo "Waiting for Magisk to initialize..."

magisk_ready_attempts="${MAGISK_READY_ATTEMPTS:-3}"
for i in $(seq 1 "$magisk_ready_attempts"); do
  echo "Attempt $i/$magisk_ready_attempts: Checking Magisk root availability..."
  grant_magisk_shell
  if adb_magisk '-V' >/dev/null 2>&1 && adb_root 'id' >/dev/null 2>&1; then
    echo "Magisk root is available."
    break
  fi
  if [ "$i" -eq "$magisk_ready_attempts" ]; then
    echo "Magisk root is not available after rootAVD."
    adb shell getprop | grep -i magisk || true
    adb shell which su || true
    adb shell which magisk || true
    adb_root 'id; ls -la /data/adb; ls -la /data/adb/magisk; find /data/adb -maxdepth 3 \( -name magisk -o -name magisk64 -o -name su \); ls -la /debug_ramdisk; find /debug_ramdisk -maxdepth 3 \( -name magisk -o -name su \)' || true
    adb shell ls -la /debug_ramdisk || true
    adb shell ls -la /dev | grep -i magisk || true
    adb shell find / -maxdepth 3 -name su -o -name magisk 2>/dev/null || true
    adb logcat -d -t 300 | grep -Ei 'magisk|magiskinit|init-ld|avc: denied|init:' || true
    adb shell ls -la /system/bin/su || true
    adb shell ls -la /system/bin/magisk || true
    adb shell ls -la /sbin || true
    exit 1
  fi
  echo "Magisk not ready yet, waiting 10s..."
  sleep 10
done

adb_magisk "--sqlite \"REPLACE INTO settings (key,value) VALUES('zygisk',1);\""
install_storage_redirect_module
seed_storage_redirect_test_environment
adb reboot
wait_for_boot 420
wait_for_root_shell 120
assert_installed_module_files /data/adb/modules/storage.redirect.x

if adb_magisk "--sqlite \"SELECT value FROM settings WHERE key='zygisk';\"" | grep -q 1; then
  echo "Zygisk setting is enabled."
else
  echo "Magisk CLI zygisk query was not available after reboot; verifying module load state instead."
fi

verify_storage_redirect_module_loaded
