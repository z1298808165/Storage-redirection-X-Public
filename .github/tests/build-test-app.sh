#!/usr/bin/env bash
# 拉取测试仓源码并编译测试 APK，产物落到 test_app/app/build/outputs/apk/debug/
# 约束：需 JDK 与 Android SDK；测试仓地址由 TEST_REPO 覆盖
set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

TEST_REPO="${TEST_REPO:-https://github.com/Kindness-Kismet/StorageRedirectTest.git}"
TEST_REF="${TEST_REF:-main}"
TEST_APP_DIR="${REPO_ROOT}/test_app"

rm -rf "$TEST_APP_DIR"
git clone --depth 1 --branch "$TEST_REF" "$TEST_REPO" "$TEST_APP_DIR"

cd "$TEST_APP_DIR"
chmod +x ./gradlew
./gradlew --no-daemon --stacktrace :app:assembleDebug

APK="${TEST_APP_DIR}/app/build/outputs/apk/debug/app-debug.apk"
if [ ! -f "$APK" ]; then
  echo "测试 APK 构建失败：未找到 $APK"
  exit 1
fi
echo "测试 APK 构建成功：$APK"
