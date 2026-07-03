#!/usr/bin/env bash
# 场景 3：启用路径映射，验证 SrtProbe 写入真实 Test
# 预期：物理落点在 真实 Download/Test/；映射后的真实目录对应用不可见
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

SCENARIO=3
TITLE="启用路径映射，验证 SrtProbe 写入真实 Test"

apply_config() {
  write_config '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}'
}

check_app_view() {
  expect_app_entry "$SCENARIO" "app-view" "${REAL_ROOT}/Download/SrtProbe"
  expect_no_app_entry "$SCENARIO" "app-mapped-real-view" "${REAL_ROOT}/Download/Test"
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
  check_file_location "$SCENARIO" "${REAL_ROOT}/Download/Test/"

  echo "scenario ${SCENARIO} passed"
  echo "::endgroup::"
}

if ! main; then
  print_diagnostics "$SCENARIO"
  exit 1
fi
