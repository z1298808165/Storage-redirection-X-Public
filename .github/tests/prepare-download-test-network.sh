#!/system/bin/sh
set -eu

state=/data/local/tmp/srx-test-flow.lock/download-network

default_network_validated() {
  networks=$(dumpsys connectivity) || return 1
  default_id=$(printf '%s\n' "$networks" | sed -n 's/^Active default network: \([0-9][0-9]*\)\r*$/\1/p')
  [ -n "$default_id" ] || return 1
  printf '%s\n' "$networks" | grep -F "  NetworkAgentInfo{network{$default_id}" |
    grep -Eq 'Capabilities: ([^] ]*&)?VALIDATED(&|[][:space:]])'
}

reconnect_network() {
  if [ "$mobile_data" = 1 ]; then
    svc data disable
  fi
  if [ "$wifi_on" = 1 ]; then
    svc wifi disable
  fi
  # 等旧网络真正断开再重连，避免异步开关合并后沿用旧的验证状态。
  disconnected=0
  for attempt in $(seq 1 15); do
    if networks=$(dumpsys connectivity) && ! printf '%s\n' "$networks" | grep '^  NetworkAgentInfo' | grep -Eq 'Transports: (CELLULAR|WIFI)'; then
      disconnected=1
      break
    fi
    sleep 1
  done
  if [ "$mobile_data" = 1 ]; then
    svc data enable
  fi
  if [ "$wifi_on" = 1 ]; then
    svc wifi enable
  fi
  if [ "$disconnected" != 1 ]; then
    echo "下载测试网络在 15 秒内未断开" >&2
    return 1
  fi
}

case "${1:-}" in
  prepare)
    [ "$(getprop ro.kernel.qemu)" = 1 ] || exit 0
    [ "$(getprop ro.build.version.sdk)" -ge 35 ] || exit 0
    if [ -e "$state" ]; then
      echo "download network backup already exists" >&2
      exit 1
    fi
    if default_network_validated; then
      echo "download_network_validated"
      exit 0
    fi
    portal_mode=$(settings get global captive_portal_mode)
    mobile_data=$(settings get global mobile_data)
    wifi_on=$(settings get global wifi_on)
    (umask 077; printf '%s\n' "$portal_mode" "$mobile_data" "$wifi_on" > "$state")
    # NetworkMonitor 在网络重连后读取此设置；回环下载也受 JobScheduler 的 VALIDATED 约束。
    settings put global captive_portal_mode 0
    reconnect_network
    for attempt in $(seq 1 30); do
      if default_network_validated; then
        echo "download_network_validated"
        exit 0
      fi
      sleep 1
    done
    echo "download network did not become VALIDATED within 30 seconds" >&2
    exit 1
    ;;
  restore)
    [ -f "$state" ] || exit 0
    portal_mode=$(sed -n '1p' "$state")
    mobile_data=$(sed -n '2p' "$state")
    wifi_on=$(sed -n '3p' "$state")
    if [ "$portal_mode" = null ]; then
      settings delete global captive_portal_mode >/dev/null
    else
      settings put global captive_portal_mode "$portal_mode"
    fi
    reconnect_network
    rm "$state"
    echo "download_network_restored"
    ;;
  *) exit 2 ;;
esac
