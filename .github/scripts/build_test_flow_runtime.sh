#!/usr/bin/env bash
set -euo pipefail

: "${VERSION:?缺少 VERSION}"
: "${VERSION_CODE:?缺少 VERSION_CODE}"
: "${TARGET_TRIPLE:?缺少 TARGET_TRIPLE}"
: "${MODULE_ABI:?缺少 MODULE_ABI}"

mkdir -p build/test-flow/module-bin build/test-flow/assets
cargo test --target "$TARGET_TRIPLE" --no-run
cargo build --target "$TARGET_TRIPLE" --release
cp "target/${TARGET_TRIPLE}/release/libsrx_core.so" build/test-flow/module-bin/libsrx_core.so
cp "target/${TARGET_TRIPLE}/release/srx_daemon" build/test-flow/module-bin/srx_daemon
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
