#!/usr/bin/env bash
# 场景 1：未启用应用配置，验证默认真实路径写入
# 预期：探针文件物理落点在 /storage/emulated/0/Download/SrtProbe/
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

SCENARIO=1
TITLE="未启用应用配置，验证默认真实路径写入"

apply_config() {
  adb_su "rm -f '$CONFIG'" >/dev/null
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
