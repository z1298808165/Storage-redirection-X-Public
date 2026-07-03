#!/usr/bin/env bash
# 在已启动的模拟器上：装测试 APK、rootAVD 刷 Magisk、装 SRX 模块、开 Zygisk 并重启验证
# 约束：模块 zip 由本仓源码构建，路径 build/zygisk/*x86_64.zip
set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

MODULE_ID="${MODULE_ID:-storage.redirect.x}"
MODULE_DIR="/data/adb/modules/${MODULE_ID}"
APP_ID="${APP_ID:-me.fakerqu.test.storageredirect}"
APP_APK="${APP_APK:-${REPO_ROOT}/test_app/app/build/outputs/apk/debug/app-debug.apk}"

MODULE_ZIP="$(find "${REPO_ROOT}/build/zygisk" -maxdepth 1 -name '*x86_64.zip' -print -quit 2>/dev/null || true)"
if [ -z "$MODULE_ZIP" ]; then
  echo "未找到 x86_64 模块 zip，请先执行 python scripts/build.py build-zygisk --abi x86_64"
  exit 1
fi

ROOT_AVD_DIR="${RUNNER_TEMP:-/tmp}/rootAVD"
rm -rf "$ROOT_AVD_DIR"
mkdir -p "$ROOT_AVD_DIR/Apps"
cp "${REPO_ROOT}/.github/vendor/rootAVD/rootAVD.sh" "$ROOT_AVD_DIR/rootAVD.sh"
cp "${REPO_ROOT}/.github/vendor/rootAVD/rootAVD.bat" "$ROOT_AVD_DIR/rootAVD.bat"
chmod +x "$ROOT_AVD_DIR/rootAVD.sh"

MAGISK_JSON="${MAGISK_JSON:-https://raw.githubusercontent.com/topjohnwu/magisk-files/master/stable.json}"
MAGISK_URL="$(python3 - <<PY
import json, urllib.request
with urllib.request.urlopen("$MAGISK_JSON", timeout=30) as r:
    print(json.load(r)["magisk"]["link"])
PY
)"
curl -fsSL "$MAGISK_URL" -o "$ROOT_AVD_DIR/Magisk.zip"

RAMDISK_REL="system-images/android-${ANDROID_API_LEVEL}/${ANDROID_TARGET}/${ANDROID_ARCH}/ramdisk.img"
RAMDISK="$ANDROID_HOME/$RAMDISK_REL"
if [ ! -f "$RAMDISK" ]; then
  echo "未找到 ramdisk.img: $RAMDISK"
  exit 1
fi

wait_for_boot() {
  local timeout_seconds="${1:-300}"
  local deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ "$(timeout 10s adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = "1" ]; then
      return 0
    fi
    sleep 2
  done
  echo "等待模拟器启动超时。"
  adb devices -l || true
  [ -n "${EMULATOR_LOG:-}" ] && [ -f "$EMULATOR_LOG" ] && tail -200 "$EMULATOR_LOG" || true
  return 1
}

wait_for_emulator_shutdown() {
  local deadline=$((SECONDS + ${1:-60}))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if ! adb devices | grep -q '^emulator-'; then
      return 0
    fi
    adb emu kill >/dev/null 2>&1 || true
    sleep 2
  done
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
  [ -f "$EMULATOR_LOG" ] && tail -80 "$EMULATOR_LOG" || true
}

adb_root() {
  local command="PATH=/debug_ramdisk:/sbin:/data/adb/magisk:\$PATH; $1"
  local quoted
  quoted="$(printf '%s' "$command" | sed "s/'/'\\\\''/g")"
  adb shell "su 0 sh -c '$quoted'" || adb shell "su -c '$quoted'"
}

adb_magisk() {
  adb_root "magisk_bin=''; for bin in /data/adb/magisk/magisk /debug_ramdisk/magisk /sbin/magisk /system/bin/magisk magisk; do if [ -x \"\$bin\" ]; then magisk_bin=\"\$bin\"; break; fi; found=\$(command -v \"\$bin\" 2>/dev/null || true); if [ -n \"\$found\" ]; then magisk_bin=\"\$found\"; break; fi; done; [ -n \"\$magisk_bin\" ] && \"\$magisk_bin\" $1"
}

grant_magisk_shell() {
  adb_magisk "--sqlite \"REPLACE INTO settings (key,value) VALUES('root_access',3);\"" >/dev/null 2>&1 || true
  adb_magisk "--sqlite \"REPLACE INTO policies (uid,policy,until,logging,notification) VALUES(2000,2,0,1,0);\"" >/dev/null 2>&1 || true
}

install_module() {
  adb push "$MODULE_ZIP" /data/local/tmp/storage-redirect-x.zip
  if adb_root 'magisk --install-module /data/local/tmp/storage-redirect-x.zip'; then
    adb_root 'rm -f /data/local/tmp/storage-redirect-x.zip' >/dev/null 2>&1 || true
    return
  fi
  echo "Magisk 模块安装失败。"
  adb_root 'id; magisk -V || true; ls -la /data/adb/modules' || true
  adb logcat -d -t 300 | grep -Ei 'magisk|zygisk|avc: denied|storage.redirect' || true
  exit 1
}

seed_config() {
  local content='{"users":{"0":{"enabled":true}}}'
  for dir in /data/adb/modules_update/${MODULE_ID} ${MODULE_DIR}; do
    if adb_root "[ -d '$dir' ]"; then
      adb_root "mkdir -p '$dir/config/apps'"
      printf '%s' "$content" | adb_root "cat > '$dir/config/apps/${APP_ID}.json'"
      adb_root "chmod 644 '$dir/config/apps/${APP_ID}.json'"
    fi
  done
}

verify_loaded() {
  adb_root "test -d ${MODULE_DIR} && test ! -e ${MODULE_DIR}/disable"
  adb_root "grep -q ' /dev/srx_config ' /proc/mounts"
  adb_root "ls -la ${MODULE_DIR}/logs; ls -la /dev/srx_config"
}

# 安装测试 APK
if [ ! -f "$APP_APK" ]; then
  echo "未找到测试 APK: $APP_APK"
  exit 1
fi
adb install -r "$APP_APK"

# rootAVD 刷入 Magisk
if ! ROOTAVD_NONINTERACTIVE=1 ROOTAVD_MAGISK_CHOICE=1 "$ROOT_AVD_DIR/rootAVD.sh" "$RAMDISK_REL"; then
  echo "rootAVD 刷 ramdisk 失败。"
  exit 1
fi

AVD_DIR="${HOME}/.android/avd/${AVD_NAME:-test}.avd"
PATCHED_RAMDISK="$ANDROID_HOME/$RAMDISK_REL"
if [ -d "$AVD_DIR" ] && [ -f "$PATCHED_RAMDISK" ]; then
  cp "$PATCHED_RAMDISK" "$AVD_DIR/ramdisk.img"
fi

wait_for_emulator_shutdown 90
adb kill-server >/dev/null 2>&1 || true
start_emulator
wait_for_boot 300

echo "等待 Magisk 初始化..."
attempts="${MAGISK_READY_ATTEMPTS:-3}"
for i in $(seq 1 "$attempts"); do
  echo "尝试 $i/$attempts: 检查 Magisk root..."
  grant_magisk_shell
  if adb_magisk '-V' >/dev/null 2>&1 && adb_root 'id' >/dev/null 2>&1; then
    echo "Magisk root 可用。"
    break
  fi
  if [ "$i" -eq "$attempts" ]; then
    echo "rootAVD 后 Magisk root 仍不可用。"
    adb logcat -d -t 300 | grep -Ei 'magisk|magiskinit|avc: denied|init:' || true
    exit 1
  fi
  sleep 10
done

adb_magisk "--sqlite \"REPLACE INTO settings (key,value) VALUES('zygisk',1);\""
install_module
seed_config
adb reboot
wait_for_boot 300

if ! adb_magisk "--sqlite \"SELECT value FROM settings WHERE key='zygisk';\"" | grep -q 1; then
  echo "重启后 Zygisk 未启用。"
  exit 1
fi

verify_loaded
echo "SRX 模块安装并加载成功。"
