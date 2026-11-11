#!/usr/bin/env bash
set -euo pipefail

export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL="*"

# 测试流产物统一落盘目录（AGENTS.md「临时文件管理」规范）：temp/test-flow/<启动时间戳>/。
# 本脚本先建目录并注入 SRT_ARTIFACT_DIR，后续场景脚本与诊断脚本共用同一目录，
# 同一天多轮测试互不覆盖；CI 的 artifact 上传按 temp/test-flow/ 整目录收集。
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRT_ARTIFACT_DIR="${SRT_ARTIFACT_DIR:-${repo_root}/temp/test-flow/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$SRT_ARTIFACT_DIR"
export SRT_ARTIFACT_DIR

PREFLIGHT_DIAGNOSTICS="${SRT_ARTIFACT_DIR}/test-flow-preflight.txt"

# 诊断只用于取证，任何一条 adb 在设备半死时都会无限阻塞——这正是「还没开跑就
# 卡住」的成因之一，所以每条都套短超时：诊断绝不能把自己变成挂起源。
capture_preflight_diagnostics() {
  {
    echo "=== host_time ==="
    date -u
    echo "=== adb_devices ==="
    timeout 20 adb devices -l 2>&1 || true
    echo "=== boot_properties ==="
    timeout 20 adb shell 'getprop sys.boot_completed; getprop dev.bootcomplete; getprop init.svc.bootanim' 2>&1 || true
    echo "=== storage_state ==="
    timeout 30 adb shell 'sm list-volumes all 2>&1; ls -ld /storage /storage/emulated /storage/emulated/0 /sdcard 2>&1; df -h /storage/emulated/0 /sdcard 2>&1' 2>&1 || true
  } >"$PREFLIGHT_DIAGNOSTICS"
  # 同时打到 step 日志：A17 分片**有意不上传**诊断资产（ci.yml 的
  # `matrix.android.version != 17`，见提交 c8d8271d「保留 Android17 … 不上传诊断资产」），
  # 只落文件的话那一版失败时这份现场根本看不到，而那正是最需要它的一版。
  # 本函数只在失败/超时路径被调用，不会污染正常轮次的日志。
  cat "$PREFLIGHT_DIAGNOSTICS" >&2 || true
}

# 基础设置阶段总预算（秒，可覆盖）：从脚本启动到场景循环开始前的全部准备工作
# ——设备健康检查、模块安装、包服务恢复。
#
# 统一限制 KernelSU 打补丁、重启与服务恢复的等待，避免准备阶段无限阻塞。
#
# 这一段是「还没开跑就卡住」的唯一有界防线：场景看门狗只覆盖场景脚本本体，
# 模块安装与 adb 调用在设备半死时会无限阻塞（run 40272962460 五版本停在
# Run 步骤 33 分钟、取消请求不生效）。job/step 级超时虽能兜底，但要等十几分钟到
# 一两个小时，且被硬杀时拿不到 preflight 快照；本预算让基础设置卡住时 15 分钟内
# 显式失败并留下诊断，进入快速重跑。
#
# 「15 分钟」是**判定目标**，不是硬墙：判定点之后还要打现场快照，最坏再加一次
# adb 调用上限（SETUP_ADB_TIMEOUT_SECONDS，20 秒）与快照本身的固定开销（~70 秒），
# 所以真实上界约 16.5 分钟。这段尾巴仍在 step 的 3 分钟收尾余量之内。
#
# 改这个值必须同步 parse_scenario_scope.py 的 SETUP_BUDGET_MINUTES：步骤级预算
# 按「基础设置 + 看门狗 + 余量」推导，两侧不一致会让 step 抢先于看门狗触发。
SETUP_BUDGET_SECONDS="${SETUP_BUDGET_SECONDS:-900}"
# 只接受整数秒：写成 `15m` 之类会让后面的算术展开直接报
# "arithmetic expression" 这种看不出根因的错误，这里显式拒绝并给出可读信息。
case "$SETUP_BUDGET_SECONDS" in
  ''|*[!0-9]*)
    echo "SETUP_BUDGET_SECONDS 必须是非负整数秒（当前：${SETUP_BUDGET_SECONDS}）" >&2
    exit 1
    ;;
esac
setup_started=$SECONDS

setup_remaining_seconds() {
  local left=$((SETUP_BUDGET_SECONDS - (SECONDS - setup_started)))
  [ "$left" -gt 0 ] || left=0
  printf '%s' "$left"
}

# 把各等待函数自己的固定上限压到基础设置预算内。
# 不这样做的话，预算只是「在阶段边界和收口处检查」，而 wait_for_adb_ready(300s)、
# wait_for_emulated_storage(120/180s) 这些自带上限的等待仍能在预算用尽后继续跑，
# 最坏把「15 分钟内判定」拖到 20 分钟。取两者较小值（含 0：预算已耗尽时立即
# 让等待返回失败，而不是再等一整个默认上限），保证预算真正权威。
# 边界会按轮询间隔上浮（wait_for_adb_ready 步长 3s、wait_for_emulated_storage 2s），
# 实测 4 秒预算 → 7 秒返回，多出的正好是一个轮询间隔，可忽略。
setup_bounded_seconds() {
  local default="$1" remaining
  remaining="$(setup_remaining_seconds)"
  if [ "$remaining" -lt "$default" ]; then
    printf '%s' "$remaining"
  else
    printf '%s' "$default"
  fi
}

# 设备等待失败时给出准确归因：基础设置预算把等待裁剪到 0 秒，与「设备真的不可用」
# 是两回事，报错信息必须能区分——否则排查会照着设备方向走错路（实测：预算归零时
# 原本只报「Android 设备未进入可用状态」）。这类情况在真实轮次里够得着：模块安装
# 吃掉大部分预算后，后面的 wait_for_adb_ready 会被削短。
report_device_wait_failure() {
  if [ "$(setup_remaining_seconds)" -le 0 ]; then
    echo "基础设置超时：${1}期间已用尽 ${SETUP_BUDGET_SECONDS}s 预算" >&2
  else
    echo "${2}" >&2
  fi
  # 结论先打、现场后打：让人先看到是哪一类失败，再往下读快照细节。
  capture_preflight_diagnostics || true
}

# 在基础设置预算内执行一个阶段：超出预算以 timeout 退出码（124/137）按失败处理并
# 显式标注阶段名，而不是等 step/job 级超时硬杀。
run_setup_phase() {
  local phase="$1"
  shift
  local remaining
  remaining="$(setup_remaining_seconds)"
  if [ "$remaining" -le 0 ]; then
    echo "基础设置超时：进入「${phase}」前已用尽 ${SETUP_BUDGET_SECONDS}s 预算" >&2
    capture_preflight_diagnostics || true
    exit 1
  fi
  echo "::group::基础设置 ${phase}（剩余 ${remaining}s / 预算 ${SETUP_BUDGET_SECONDS}s）"
  local code=0
  # 不加 --foreground：让 timeout 把该阶段放进独立进程组，超时时整组终止，
  # 连阶段内部拉起的 adb 子进程一起收掉。只杀直接子进程时，残留 adb 会继续
  # 持有 stdout 写端，GitHub runner 判定 step 未结束，job 反而静默挂到超时
  # （run 40286639350 实证：11 分 44 秒零输出）。
  timeout --kill-after=30s "${remaining}s" "$@" || code=$?
  echo "::endgroup::"
  # timeout 超时先发 TERM，命令乖乖退出时回 124；命令忽略 TERM 时 --kill-after
  # 升级为 SIGKILL，退出码变成 137（实测确认）。两者都属「基础设置超时」，必须
  # 分别识别——只认 124 会把「卡死」误报成「阶段失败」，丢掉本预算的诊断价值。
  # 137 也可能来自 OOM kill，那种情况按失败处理同样正确。
  if [ "$code" -eq 124 ] || [ "$code" -eq 137 ]; then
    echo "基础设置超时：「${phase}」超出 ${SETUP_BUDGET_SECONDS}s 预算（timeout 退出码 ${code}），按失败处理以便快速重跑" >&2
    capture_preflight_diagnostics || true
    exit 1
  fi
  return "$code"
}

# 与 setup_bounded_seconds 同义，但下限保到 1 秒：`timeout 0` 在 GNU coreutils 里
# 表示「**不设超时**」，直接用剩余预算会把「预算已耗尽」变成「无限等待」，正好反了。
setup_bounded_timeout() {
  local left
  left="$(setup_bounded_seconds "$1")"
  [ "$left" -ge 1 ] || left=1
  printf '%s' "$left"
}

# 基础设置阶段内的 adb 调用一律套超时。理由是「还没开跑就卡住」并不只发生在
# 阶段内部：设备半死时 `adb` **客户端本身**就可能无限阻塞（get-state / shell /
# reconnect 皆然），而下面的等待函数只在**两轮之间**检查 deadline——只要单次
# 调用被挂住，15 分钟预算就永远走不到判定点。套上超时后，预算才是真的权威。
# 20 秒对健康设备绰绰有余（正常毫秒级返回），卡死时最多多等一个上限。
SETUP_ADB_TIMEOUT_SECONDS="${SETUP_ADB_TIMEOUT_SECONDS:-20}"
adb_bounded() {
  timeout "${SETUP_ADB_TIMEOUT_SECONDS}s" adb "$@"
}

wait_for_adb_ready() {
  local deadline=$((SECONDS + $(setup_bounded_seconds 300)))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if [ "$(adb_bounded get-state 2>/dev/null || true)" = "device" ] &&
      [ "$(adb_bounded shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)" = "1" ]; then
      return 0
    fi
    if adb_bounded devices 2>/dev/null | grep -q $'\toffline$'; then
      adb_bounded reconnect offline >/dev/null 2>&1 || true
      adb_bounded kill-server >/dev/null 2>&1 || true
      adb_bounded start-server >/dev/null 2>&1 || true
    fi
    sleep 3
  done
  return 1
}

wait_for_emulated_storage() {
  local timeout_seconds
  timeout_seconds="$(setup_bounded_seconds "${1:-120}")"
  local deadline=$((SECONDS + timeout_seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if adb_bounded shell "sm list-volumes all 2>/dev/null | grep -q 'emulated;0 mounted' && test -d /storage/emulated/0 && ls -ld /storage/emulated/0 >/dev/null 2>&1" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

prepare_device_health() {
  if ! wait_for_adb_ready; then
    report_device_wait_failure "等待设备就绪" "Android 设备未进入可用状态"
    return 1
  fi
  if wait_for_emulated_storage 120; then
    return 0
  fi

  echo "共享存储未挂载，执行一次模拟器重启恢复" >&2
  adb_bounded reboot >/dev/null 2>&1 || true
  if ! wait_for_adb_ready || ! wait_for_emulated_storage 180; then
    report_device_wait_failure "模拟器重启恢复" "模拟器重启后共享存储仍不可用"
    return 1
  fi
}

# Android 17（API 37）系统镜像的 mapper.ranchu gralloc 未实现 DMA 颜色缓冲读回，
# 而当前 emulator 的 gfxstream 后端仍会广播 ReadColorBufferDMA 特性。SystemUI 的
# RegionSampling 与 system_server 的 TaskSnapshotPersister 都会触发读回，随后命中
# `Assertion failed: !rcEnc->featureInfo()->hasReadColorBufferDma` 并连带重启 framework。
# 该缺陷与 GPU 模式无关；API 37 测试流必须同时关闭两个入口，并在模拟器/模块重启
# 后重新应用，因为 task snapshot 开关是 system_server 进程内状态。
android17_disable_graphics_readback() {
  local phase="${1:-设备启动后}"
  local sdk
  sdk="$(timeout 15s adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)"
  case "$sdk" in
    '' | *[!0-9]*) return 0 ;;
  esac
  if [ "$sdk" -ne 37 ]; then
    return 0
  fi

  echo "android17: ${phase}关闭 task snapshots 与 SystemUI 图形读回入口"

  # android17-release IWindowManager.aidl：135 为 isTaskSnapshotSupported，137 为
  # setTaskSnapshotEnabled。模块重启后的 framework 可能在短窗口内再次重启，因此
  # 每轮都绑定 system_server PID，稳定观察后再复核全部状态；PID 变化即从头重放。
  local state_file="${SRT_ARTIFACT_DIR}/test-flow-graphics-state.txt"
  if [ "$phase" = "初次启动后" ]; then
    : >"$state_file"
  fi

  # 300 秒窗口同时受基础设置预算裁剪：这一段跑在预算之内，若预算已接近耗尽，
  # 再给满 300 秒会把「15 分钟内判定」拖成 20 分钟，并吃掉留给 step 的收尾余量。
  local deadline=$((SECONDS + $(setup_bounded_seconds 300))) attempt=0
  local server_pid stable_server_pid snapshot_disable_output snapshot_state stable_snapshot_state
  local disable_output disabled_packages stable_disabled_packages system_ui_pid
  while [ "$SECONDS" -lt "$deadline" ]; do
    attempt=$((attempt + 1))
    server_pid="$(timeout 10s adb shell pidof system_server 2>/dev/null | tr -d '\r' || true)"
    if [ -z "$server_pid" ]; then
      adb_bounded reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    snapshot_disable_output="$(timeout 20s adb shell service call window 137 i32 0 2>&1 || true)"
    snapshot_state="$(timeout 20s adb shell service call window 135 2>&1 || true)"
    {
      printf '=== phase=%s attempt=%s ===\n' "$phase" "$attempt"
      printf 'system_server_pid=%s\n' "$server_pid"
      printf 'setTaskSnapshotEnabled: %s\n' "$snapshot_disable_output"
      printf 'isTaskSnapshotSupported: %s\n' "$snapshot_state"
    } >>"$state_file"
    if ! grep -Eq 'Parcel\([[:space:]]*00000000[[:space:]]+00000000' <<<"$snapshot_state"; then
      adb_bounded reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    disable_output="already_disabled"
    if ! grep -qx 'package:com.android.systemui' <<<"$disabled_packages"; then
      disable_output="$(timeout 20s adb shell "pm disable-user --user 0 com.android.systemui" 2>&1 || true)"
      disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    fi
    {
      printf 'pm_disable_user=%s\n' "$disable_output"
      printf 'disabled_packages=%s\n' "$disabled_packages"
    } >>"$state_file"
    if ! grep -qx 'package:com.android.systemui' <<<"$disabled_packages"; then
      adb_bounded reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    if ! timeout 15s adb shell "service list 2>/dev/null | grep -q 'activity'" >/dev/null 2>&1 ||
      ! timeout 15s adb shell "cmd package list packages >/dev/null 2>&1" >/dev/null 2>&1 ||
      ! wait_for_emulated_storage 20; then
      adb_bounded reconnect >/dev/null 2>&1 || true
      sleep 2
      continue
    fi

    sleep 8
    stable_server_pid="$(timeout 10s adb shell pidof system_server 2>/dev/null | tr -d '\r' || true)"
    stable_snapshot_state="$(timeout 20s adb shell service call window 135 2>&1 || true)"
    stable_disabled_packages="$(timeout 20s adb shell "cmd package list packages -d --user 0" 2>&1 | tr -d '\r' || true)"
    system_ui_pid="$(timeout 10s adb shell pidof com.android.systemui 2>/dev/null | tr -d '\r' || true)"
    {
      printf 'stable_system_server_pid=%s\n' "$stable_server_pid"
      printf 'stable_isTaskSnapshotSupported=%s\n' "$stable_snapshot_state"
      printf 'stable_system_ui_pid=%s\n' "$system_ui_pid"
    } >>"$state_file"
    if [ "$stable_server_pid" = "$server_pid" ] &&
      grep -Eq 'Parcel\([[:space:]]*00000000[[:space:]]+00000000' <<<"$stable_snapshot_state" &&
      grep -qx 'package:com.android.systemui' <<<"$stable_disabled_packages"; then
      # SystemUI 是 persistent 系统进程，system_server 会在 kill 后立即重新拉起；
      # package disabled 与 task snapshot=false 才是有效门禁，PID 只保留作诊断。
      echo "android17: ${phase}图形读回入口已关闭，system_server=${server_pid} systemui=${system_ui_pid:-absent}"
      return 0
    fi
  done

  capture_preflight_diagnostics
  echo "android17: ${phase}图形读回入口在稳定窗口内未保持关闭" >&2
  return 1
}

prepare_device_health
# A17 图形读回保护开关：默认 1 保持既有行为。该缺陷来自 emulator 的 gfxstream 与
# mapper.ranchu gralloc，但系统镜像可能已被 Google 修复；
# 置 0 可跳过保护实测最新镜像是否已不再触发 ReadColorBufferDMA 崩溃。
SRT_A17_GRAPHICS_GUARD="${SRT_A17_GRAPHICS_GUARD:-1}"
if [ "$SRT_A17_GRAPHICS_GUARD" = "1" ]; then
  android17_disable_graphics_readback "初次启动后"
else
  echo "android17: SRT_A17_GRAPHICS_GUARD=0，跳过图形读回保护（实验）"
fi

# CI 模拟器只保留 FuseFix 属性诊断，不安装 native FuseFix hook。
export SRT_FUSE_FIX_ENABLED=false

TEST_APP_APK="$(find tests/storage-redirect-test/app/build/outputs/apk/debug -maxdepth 1 -name '*-debug.apk' -print -quit)"
if [ -z "$TEST_APP_APK" ]; then
  echo "No test app debug APK found under tests/storage-redirect-test/app/build/outputs/apk/debug." >&2
  find tests/storage-redirect-test/app/build/outputs -maxdepth 4 -type f -name '*.apk' -print 2>/dev/null || true
  exit 1
fi

# 用基础设置预算包住：这一步内部有多轮 adb install / framework 重启等待，设备
# 半死时会静默无限阻塞，是「还没开跑就卡住」最集中的位置。
if ! run_setup_phase "模块安装" env \
  MODULE_ZIP="build/test-flow/assets/storage.redirect.x-v${VERSION}-${MODULE_ABI}.zip" \
  APP_APK="$TEST_APP_APK" \
  bash .github/tests/install-storage-redirect-module.sh; then
  capture_preflight_diagnostics || true
  echo "模块安装失败，按失败处理" >&2
  exit 1
fi

# 模块安装会重启 framework；在任何 package 枚举前立即重放图形保护，缩短
# SystemUI/task snapshot 再次触发 ReadColorBufferDMA 崩溃的窗口。
if [ "$SRT_A17_GRAPHICS_GUARD" = "1" ]; then
  android17_disable_graphics_readback "模块重启后"
else
  echo "android17: SRT_A17_GRAPHICS_GUARD=0，模块重启后同样跳过图形读回保护（实验）"
fi

# Android 17 可能限制普通应用的 PackageManager 可见性，记录 root 与普通 shell 的枚举差异。
# 三条枚举各带上限 60 秒，且都被剩余预算裁剪（下限 1 秒）：预算已耗尽时它们
# 只剩 1 秒，不会在收口判定前再吃掉 3 分钟、把 step 的收尾余量挤没。
{
  echo "=== api_level ==="
  echo "${ANDROID_API_LEVEL:-unknown}"
  echo "=== shell_packages ==="
  timeout "$(setup_bounded_timeout 60)" adb shell "pm list packages -f -U --user 0 2>/dev/null" 2>&1 || true
  echo "=== root_packages ==="
  timeout "$(setup_bounded_timeout 60)" adb shell "su 0 sh -c 'pm list packages -f -U --user 0 2>/dev/null'" 2>&1 || true
  echo "=== package_service ==="
  timeout "$(setup_bounded_timeout 60)" adb shell "cmd package list packages -f -U --user 0 2>/dev/null" 2>&1 || true
} > "${SRT_ARTIFACT_DIR}/test-flow-app-list.txt"

# 模块重启后 system_server/package 服务可能还在恢复，进入场景脚本前再次等待。
if ! wait_for_adb_ready; then
  report_device_wait_failure "模块重启后等待设备恢复" "模块重启后 Android 设备未恢复到可用状态"
  exit 1
fi
# 这段等待也要落在基础设置预算内：它跑在场景循环之前，若不裁剪，预算耗尽后还能
# 再跑满 120 秒，把「15 分钟内判定」拖到 17 分钟，并吃掉留给 step 的收尾余量。
package_service_deadline=$((SECONDS + $(setup_bounded_seconds "${PACKAGE_SERVICE_TIMEOUT_SECONDS:-120}")))
while [ "$SECONDS" -lt "$package_service_deadline" ]; do
  if adb_bounded shell "cmd package list packages >/dev/null 2>&1" >/dev/null 2>&1; then
    break
  fi
  adb_bounded reconnect >/dev/null 2>&1 || true
  sleep 2
done

adb_bounded shell appops set me.fakerqu.test.storageredirect MANAGE_EXTERNAL_STORAGE allow || true
export SRT_SKIP_FINAL_CLEANUP=1
export SRT_FAIL_FAST="${SRT_FAIL_FAST:-1}"
export SRT_SCENARIO_TIMEOUT_SECONDS="${SRT_SCENARIO_TIMEOUT_SECONDS:-300}"
# 场景取景（本地/手动回退解析）：CI 已由 prepare 的 outputs 把范围解析成
# SRT_SCENARIOS 直接传入，本块仅在本地运行或未走 prepare 时生效——此时
# 手动触发传入的场景编号优先，其次看提交信息里的「单场景 29」（可逗号列举）。
#
# 验证单点修复时通过这两个入口收窄范围，避免临时改写 workflow。
# srx-scenario-scope:begin
scenario_scope="${SRT_SCENARIOS_OVERRIDE:-${SRT_SCENARIOS:-}}"
if [ -z "$scenario_scope" ]; then
  scenario_message="${SRT_COMMIT_MESSAGE:-}"
  if [[ "$scenario_message" =~ 单场景[：:]?[[:space:]]*([0-9]+([,，][0-9]+)*) ]]; then
    scenario_scope="${BASH_REMATCH[1]}"
  fi
fi
if [ -n "$scenario_scope" ]; then
  export SRT_SCENARIOS="${scenario_scope//，/,}"
  # 全量时 CI 传的是 `all`，它不是「受限范围」；打 limited 会误导判据核对。
  case "$SRT_SCENARIOS" in
    all|ALL) echo "scenario_scope=full value=all" ;;
    *) echo "scenario_scope=limited value=${SRT_SCENARIOS}" ;;
  esac
fi
# srx-scenario-scope:end
# 场景脚本的失败退出码直接透传给 job（此前 224 无任何标记），这里显式打印便于定位。
# 全局看门狗（按场景数自适应）：模拟器在场景失败后可能进入异常状态，脚本内
# 后续 adb 调用会无限阻塞（run 40269845250 窄跑卡死 32 分钟实证；terminate
# 修复只覆盖脚本退出之后）。预算＝15 分钟基础 + 每场景 90 秒：单场景约
# 16.5 分钟封顶，全量 40 场景 75 分钟；超时以 124 退出并显式标注，job 快速
# 失败进入重跑，而不是无限占用 runner。
#
# 四层预算必须严格嵌套：job > step > (基础设置 + 本看门狗) > 单场景 300s。
# step 与 job 由 parse_scenario_scope.py 按同一场景数自适应产出：step＝基础设置
# 15 + 本看门狗 15+1.5n + 收尾余量 3 = 33+1.5n 分钟，job 再加 15 分钟开销。
# 此前 job 级写死 45/50 分钟比 40 场景的 75 分钟看门狗还小，导致看门狗永不
# 触发、永远被 job 硬杀，`scenario_exit == 124` 分支里的模拟器清场与残留进程
# 快照全跑不到。
#
# 场景数必须按清单取，不能数 SRT_SCENARIOS 的字面 token：全量时该变量的值是
# `all`（空值走脚本内默认清单），按 token 数只能算出 1，看门狗缩到 990s，
# 全量 40 场景必被腰斩——run 40324023063 的 A14 跑到场景 29 就被 124 杀掉，
# A17 只是刚好赶上，掩盖了这个预算错误。
scenario_total="$(grep -c '\"id\"' "$(dirname "$0")/storage-redirect-scenarios.json" 2>/dev/null || echo 0)"
[ "$scenario_total" -ge 1 ] 2>/dev/null || scenario_total=40
case "${SRT_SCENARIOS:-}" in
  ''|all|ALL) scenario_count="$scenario_total" ;;
  *) scenario_count="$(awk -F',' 'BEGIN{n=1} END{print (NF>0)?NF:1}' <<<"${SRT_SCENARIOS}")" ;;
esac
[ "$scenario_count" -ge 1 ] 2>/dev/null || scenario_count="$scenario_total"

# 基础设置阶段收口：到这里为止都是「还没开跑」的准备工作。预算用尽就直接失败，
# 不能进了场景循环再等 step/job 级超时——那会把 15 分钟就能判定的失败拖成
# 十几分钟到一两个小时的干等，而且被硬杀时拿不到 preflight 快照。
setup_elapsed=$((SECONDS - setup_started))
echo "setup_elapsed_seconds=${setup_elapsed} setup_budget_seconds=${SETUP_BUDGET_SECONDS} scenario_count=${scenario_count}"
if [ "$(setup_remaining_seconds)" -le 0 ]; then
  capture_preflight_diagnostics || true
  echo "基础设置超时：场景循环开始前已用尽 ${SETUP_BUDGET_SECONDS}s 预算（实际 ${setup_elapsed}s）" >&2
  exit 1
fi

scenario_watchdog=$((900 + scenario_count * 90))
scenario_exit=0
# --kill-after：场景脚本在被 TERM 后仍可能因为继承的 adb 子进程不退而挂住，
# 那样 timeout 自己会无限等待、看门狗形同失效（只能等 step 级兜底，且拿不到下面的
# 清场与残留快照）。60 秒后升级 SIGKILL，保证看门狗自身一定有界。
timeout --kill-after=60s "${scenario_watchdog}s" bash .github/tests/run-storage-redirect-scenarios.sh || scenario_exit=$?
# 124＝被 TERM 终止，137＝由 --kill-after 升级 SIGKILL（实测确认）。两者都算看门狗触发。
if [ "$scenario_exit" -eq 124 ] || [ "$scenario_exit" -eq 137 ]; then
  echo "场景脚本执行超时（${scenario_watchdog}s 看门狗触发，timeout 退出码 ${scenario_exit}，场景范围=${SRT_SCENARIOS:-全量}），按失败处理以便快速重跑" >&2
fi
# 只在看门狗超时时主动终止模拟器：那时设备侧可能已无响应，action 收尾的
# `adb emu kill` 会无限挂起（run 40257794067/40261786446 实证）。
# 正常失败路径**不能**提前 kill：模拟器先死会让 action 收尾的 emu kill 拿到
# Connection refused，收尾不干净（run 40286639350：07:45:48 结束 Terminate
# Emulator 后静默到 17 分钟步骤超时）。正常失败交给 action 自己收尾即可，
# 配合场景脚本里取数服务的 exec 修复，step 能在秒级结束。
if [ "$scenario_exit" -eq 124 ] || [ "$scenario_exit" -eq 137 ]; then
  timeout 60 adb emu kill >/dev/null 2>&1 || true
fi
if [ "$scenario_exit" -ne 0 ]; then
  echo "场景脚本退出码：${scenario_exit}" >&2
  # 失败时快照宿主侧的残留进程：GitHub runner 只在 step 的所有 stdout 写端
  # 关闭后才判定结束，任何继承管道又不退出的子进程都会让 job 静默挂到步骤
  # 超时（run 40286639350 实证 11 分 44 秒零输出）。这段输出用于下一轮直接
  # 判定「谁还活着」，不必再靠推测。
  echo "---- 宿主残留进程快照（失败诊断） ----" >&2
  ps -eo pid,ppid,stat,etime,cmd 2>/dev/null |
    grep -aE "emulator|qemu|http\.server|adb " |
    grep -av "grep -aE" | head -20 >&2 || true
  exit "$scenario_exit"
fi
