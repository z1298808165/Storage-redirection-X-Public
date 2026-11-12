#!/system/bin/sh
set -eu

state=/data/local/tmp/srx-test-flow.lock/download-network

reconnect_network() {
  if [ "$mobile_data" = 1 ]; then
    svc data disable
    svc data enable
  fi
  if [ "$wifi_on" = 1 ]; then
    svc wifi disable
    svc wifi enable
  fi
}

case "${1:-}" in
  prepare)
    [ "$(getprop ro.kernel.qemu)" = 1 ] || exit 0
    [ "$(getprop ro.build.version.sdk)" -ge 37 ] || exit 0
    if [ -e "$state" ]; then
      echo "download network backup already exists" >&2
      exit 1
    fi
    portal_mode=$(settings get global captive_portal_mode)
    mobile_data=$(settings get global mobile_data)
    wifi_on=$(settings get global wifi_on)
    (umask 077; printf '%s\n' "$portal_mode" "$mobile_data" "$wifi_on" > "$state")
    # NetworkMonitor 在网络重连后读取此设置；回环下载也受 JobScheduler 的 VALIDATED 约束。
    settings put global captive_portal_mode 0
    reconnect_network
    for attempt in $(seq 1 30); do
      if dumpsys connectivity | grep '^  NetworkAgentInfo' | grep -Eq 'Capabilities: ([^]]*&)?VALIDATED(&|[[:space:]])'; then
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
