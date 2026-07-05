#!/usr/bin/env bash
set -euo pipefail

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

adb wait-for-device
adb shell 'while [ "$(getprop sys.boot_completed)" != "1" ]; do sleep 2; done'

TEST_APP_APK="$(find tests/storage-redirect-test/app/build/outputs/apk/debug -maxdepth 1 -name '*-debug.apk' -print -quit)"
if [ -z "$TEST_APP_APK" ]; then
  echo "No test app debug APK found under tests/storage-redirect-test/app/build/outputs/apk/debug." >&2
  find tests/storage-redirect-test/app/build/outputs -maxdepth 4 -type f -name '*.apk' -print 2>/dev/null || true
  exit 1
fi

MODULE_ZIP="build/test-flow/assets/storage.redirect.x-v${VERSION}-${MODULE_ABI}.zip" \
  APP_APK="$TEST_APP_APK" \
  bash .github/tests/install-storage-redirect-module.sh

adb install -r "$TEST_APP_APK"
adb shell appops set me.fakerqu.test.storageredirect MANAGE_EXTERNAL_STORAGE allow || true
bash .github/tests/run-storage-redirect-scenarios.sh
