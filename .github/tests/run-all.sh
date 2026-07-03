#!/usr/bin/env bash
# 编排器：顺序执行 5 个场景脚本，末尾做 media provider 健康检查
# 每个场景独立进程运行，单场景失败不影响其余场景继续
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "${SCRIPT_DIR}/common.sh"

fail=0

wait_boot_completed
grant_test_permissions
adb_su ": > '$LOG_PATH' 2>/dev/null || true" >/dev/null

for scenario in 1 2 3 4 5; do
  echo "=== 执行场景 ${scenario} ==="
  if ! timeout --foreground 300s bash "${SCRIPT_DIR}/scenario-${scenario}.sh"; then
    echo "场景 ${scenario}: 失败或超时"
    fail=1
  fi
done

echo "=== media provider 健康检查（关联 issue #40/#43）==="
if ! check_health; then
  echo "健康检查失败：media provider 进程数异常"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "全部场景通过"
