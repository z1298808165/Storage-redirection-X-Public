#!/usr/bin/env bash
# 场景 5：放行真实 Download，验证保持原路径写入
# 预期：物理落点在 真实 Download/SrtProbe/（放行后不重定向）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

SCENARIO=5
TITLE="放行真实 Download，验证保持原路径写入"

apply_config() {
  write_config '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download"]}}}'
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
  check_file_location "$SCENARIO" "${REAL_ROOT}/Download/SrtProbe/"

  echo "scenario ${SCENARIO} passed"
  echo "::endgroup::"
}

if ! main; then
  print_diagnostics "$SCENARIO"
  exit 1
fi
