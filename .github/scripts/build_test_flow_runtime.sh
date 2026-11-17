#!/usr/bin/env bash
set -euo pipefail

: "${VERSION:?缺少 VERSION}"
: "${VERSION_CODE:?缺少 VERSION_CODE}"
: "${TARGET_TRIPLE:?缺少 TARGET_TRIPLE}"
: "${MODULE_ABI:?缺少 MODULE_ABI}"

if [[ "${SRX_NO_PATH_METADATA_REPAIR:-}" == "1" ]]; then
  export RUSTFLAGS="${RUSTFLAGS:-} --cfg srx_no_path_metadata_repair"
fi

mkdir -p build/test-flow/module-bin build/test-flow/assets
bash .github/scripts/build_native_tests.sh "$TARGET_TRIPLE" build/test-flow/native
cargo build --target "$TARGET_TRIPLE" --release --locked
CARGO_TARGET_DIR_RESOLVED="$(cargo metadata --format-version 1 --no-deps --locked | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
cp "${CARGO_TARGET_DIR_RESOLVED}/${TARGET_TRIPLE}/release/libsrx_core.so" build/test-flow/module-bin/libsrx_core.so
cp "${CARGO_TARGET_DIR_RESOLVED}/${TARGET_TRIPLE}/release/srx_daemon" build/test-flow/module-bin/srx_daemon
bash .github/scripts/package_module.sh \
  "$VERSION" "$VERSION_CODE" \
  build/test-flow/module-bin/libsrx_core.so \
  build/test-flow/module-bin/srx_daemon \
  "build/test-flow/assets/storage.redirect.x-v${VERSION}-${MODULE_ABI}.zip" \
  "$MODULE_ABI"
./gradlew --no-daemon --console=plain --stacktrace \
  :storageRedirectTestApp:testDebugUnitTest \
  :storageRedirectTestMediaFileApi:testDebugUnitTest \
  :storageRedirectTestApp:assembleDebug
