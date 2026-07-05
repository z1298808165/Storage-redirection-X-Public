#!/usr/bin/env bash
set -euo pipefail

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$REPO_ROOT"

RUN_DEVICE_SCENARIOS="${RUN_DEVICE_SCENARIOS:-1}"
INSTALL_MODULE="${SRX_TEST_INSTALL_MODULE:-1}"
TARGET_TRIPLE="${SRX_TEST_TARGET_TRIPLE:-aarch64-linux-android}"
MODULE_ABI="${SRX_TEST_MODULE_ABI:-arm64-v8a}"
BUILD_DIR="${SRX_TEST_BUILD_DIR:-build/test-flow}"
TEST_APP_APK_DIR="tests/storage-redirect-test/app/build/outputs/apk/debug"

find_test_app_apk() {
  find "$TEST_APP_APK_DIR" -maxdepth 1 -name '*-debug.apk' -print -quit 2>/dev/null || true
}

python_cmd=()
if [ -n "${PYTHON:-}" ]; then
  python_cmd=("$PYTHON")
elif command -v python3 >/dev/null 2>&1 && python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  python_cmd=(python3)
elif command -v py >/dev/null 2>&1 && py -3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  python_cmd=(py -3)
elif command -v python >/dev/null 2>&1 && python -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
  python_cmd=(python)
else
  echo "Unable to find a usable Python 3 interpreter for test-flow verification." >&2
  exit 1
fi

VERSION_DATA="$("${python_cmd[@]}" .github/scripts/resolve_build_version.py --include-dirty --format github)"
VERSION="$(printf '%s\n' "$VERSION_DATA" | awk -F= '$1 == "version" { print $2 }')"
VERSION_CODE="$(printf '%s\n' "$VERSION_DATA" | awk -F= '$1 == "version_code" { print $2 }')"

if [ -z "$VERSION" ] || [ -z "$VERSION_CODE" ]; then
  echo "Unable to resolve test-flow version." >&2
  exit 1
fi

mkdir -p "$BUILD_DIR/module-bin" "$BUILD_DIR/assets"

echo "==> Build Rust test binaries for $TARGET_TRIPLE"
cargo test --target "$TARGET_TRIPLE" --no-run

echo "==> Build SRX module binaries for $TARGET_TRIPLE"
cargo build --target "$TARGET_TRIPLE" --release
cp "target/${TARGET_TRIPLE}/release/libsrx_core.so" "$BUILD_DIR/module-bin/libsrx_core.so"
cp "target/${TARGET_TRIPLE}/release/srx_daemon" "$BUILD_DIR/module-bin/srx_daemon"

echo "==> Package test-flow module zip"
MODULE_ZIP="$BUILD_DIR/assets/storage.redirect.x-v${VERSION}-${MODULE_ABI}.zip"
bash .github/scripts/package_module.sh \
  "$VERSION" "$VERSION_CODE" \
  "$BUILD_DIR/module-bin/libsrx_core.so" \
  "$BUILD_DIR/module-bin/srx_daemon" \
  "$MODULE_ZIP" \
  "$MODULE_ABI"

echo "==> Run Android unit tests and build test APK"
./gradlew --no-daemon --console=plain --stacktrace \
  :app:testDebugUnitTest \
  :storageRedirectTestApp:testDebugUnitTest \
  :storageRedirectTestMediaFileApi:testDebugUnitTest \
  :storageRedirectTestApp:assembleDebug

TEST_APP_APK="$(find_test_app_apk)"
if [ -z "$TEST_APP_APK" ]; then
  echo "Unable to find test app debug APK under $TEST_APP_APK_DIR." >&2
  exit 1
fi

if [ "$RUN_DEVICE_SCENARIOS" = "0" ]; then
  echo "RUN_DEVICE_SCENARIOS=0: device scenario suite skipped."
  exit 0
fi

echo "==> Verify connected device and installed module state"
adb wait-for-device
adb shell 'while [ "$(getprop sys.boot_completed)" != "1" ]; do sleep 2; done'

adb_su() {
  local command="$1"
  local quoted
  quoted="$(printf '%s' "$command" | sed "s/'/'\\''/g")"
  adb shell "su 0 sh -c '$quoted'" || adb shell "su -c '$quoted'"
}

if [ "$INSTALL_MODULE" != "0" ]; then
  echo "==> Install freshly built module zip and reboot test device"
  remote_zip="/data/local/tmp/storage.redirect.x-test-flow.zip"
  adb push "$MODULE_ZIP" "$remote_zip"
  adb_su "rm -rf /data/adb/modules_update/storage.redirect.x"
  if adb_su "if [ -x /data/adb/ksu/bin/ksud ]; then /data/adb/ksu/bin/ksud module install '$remote_zip'; elif command -v ksud >/dev/null 2>&1; then ksud module install '$remote_zip'; else exit 127; fi"; then
    true
  else
    adb_su "magisk --install-module '$remote_zip'"
  fi
  adb_su "rm -f '$remote_zip'" || true
  adb reboot
  adb wait-for-device
  adb shell 'while [ "$(getprop sys.boot_completed)" != "1" ]; do sleep 2; done'
fi

adb_su "id; test -d /data/adb/modules/storage.redirect.x; test ! -e /data/adb/modules/storage.redirect.x/disable; for file in module.prop post-fs-data.sh service.sh sepolicy.rule LICENSE COPYING bin/srx_daemon zygisk/${MODULE_ABI}.so; do test -s /data/adb/modules/storage.redirect.x/\$file || exit 1; done; cat /data/adb/modules/storage.redirect.x/module.prop" >/dev/null

echo "==> Install test APK"
adb install -r "$TEST_APP_APK"

echo "==> Run device scenario suite"
MODULE_ZIP="$MODULE_ZIP" APP_APK="$TEST_APP_APK" \
  bash .github/tests/run-storage-redirect-scenarios.sh
