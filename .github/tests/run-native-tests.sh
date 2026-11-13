#!/usr/bin/env bash
set -euo pipefail

artifacts="${1:?missing native artifacts directory}"
abi="${2:?missing module ABI}"
case "$abi" in arm64-v8a|x86_64) ;; *) echo "Unsupported ABI: $abi" >&2; exit 1 ;; esac
remote="$(adb shell mktemp -d /data/local/tmp/srx-native-tests.XXXXXX | tr -d '\r')"
[[ "$remote" =~ ^/data/local/tmp/srx-native-tests\.[A-Za-z0-9]+$ ]]
cleanup() {
  local result=$?
  trap - EXIT
  if ! adb shell rm -rf "$remote"; then
    echo 'FAIL [native_cleanup] device artifacts remain' >&2
    result=1
  fi
  exit "$result"
}
trap cleanup EXIT
adb push "$artifacts/." "$remote/"
adb push .github/tests/run-zn-runtime-fixture.sh .github/tests/grant-storage-test-permissions.sh "$remote/"
adb shell "su 0 sh '$remote/run-zn-runtime-fixture.sh' '$remote/zn-runtime-fixture' '/data/adb/modules/storage.redirect.x/zygisk/$abi.so' '$remote/grant-storage-test-permissions.sh' '$remote/srx_core-tests' '$remote/srx_daemon-tests'"
