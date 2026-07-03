#!/usr/bin/env bash
# 场景 2：启用重定向，验证写入应用私有空间
# 预期：探针文件物理落点在 私有 sdcard 下 Download/SrtProbe/
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

SCENARIO=2
TITLE="启用重定向，验证写入应用私有空间"

apply_config() {
  write_config '{"users":{"0":{"enabled":true}}}'
}

check_app_view() {
  expect_app_entry "$SCENARIO" "app-view" "${REAL_ROOT}/Download/SrtProbe"
}

main() {
  echo "::group::scenario ${SCENARIO}: ${TITLE}"
  wait_boot_completed
  grant_test_permissions
  apply_config
  prepare_scenario "$SCENARIO"

  echo "step: 从应用进程写入文件"
  run_write_test "$SCENARIO"

  echo "step: 校验应用视角可见文件"
  check_app_view "$SCENARIO"

  echo "step: 校验 root 视角物理落点"
  check_file_location "$SCENARIO" "${PRIVATE_ROOT}/Download/SrtProbe/"

  echo "scenario ${SCENARIO} passed"
  echo "::endgroup::"
}

if ! main; then
  print_diagnostics "$SCENARIO"
  exit 1
fi
