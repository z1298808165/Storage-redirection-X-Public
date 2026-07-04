#!/system/bin/sh
# Storage Redirect X - Module Actions

MODDIR="/data/adb/modules/storage.redirect.x"
LOGDIR="$MODDIR/logs"
STATS_FILE="$MODDIR/stats"

resolve_action_primary_arch() {
  arch=$(getprop ro.product.cpu.abi 2>/dev/null)
  if [ -z "$arch" ]; then
    arch=$(getprop ro.product.cpu.abilist64 2>/dev/null | awk -F',' '{print $1}')
  fi
  if [ -z "$arch" ]; then
    arch=$(getprop ro.product.cpu.abilist 2>/dev/null | awk -F',' '{print $1}')
  fi

  case "$arch" in
    arm64-v8a|aarch64)
      echo "arm64-v8a"
      ;;
    x86_64|x86-64)
      echo "x86_64"
      ;;
    *)
      echo ""
      ;;
  esac
}

resolve_action_logd_bin() {
  primary_arch=$(resolve_action_primary_arch)
  if [ -z "$primary_arch" ]; then
    return 1
  fi
  logd_bin="$MODDIR/bin/$primary_arch/srx_logd"
  [ -x "$logd_bin" ] || return 1
  echo "$logd_bin"
}

send_logd_control() {
  command="$1"
  logd_bin=$(resolve_action_logd_bin) || return 1
  "$logd_bin" control "$command" >/dev/null 2>&1
}

clear_logs_via_logd() {
  send_logd_control "clear-all"
}

flush_logs_via_logd() {
  send_logd_control "flush-all"
}

case "$1" in
  "Reload Redirect")
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Reload redirect config"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    CONFIG_DIR="$MODDIR/config/apps"
    if [ -d "$CONFIG_DIR" ]; then
      killed_count=0
      app_list=""

      for config_file in "$CONFIG_DIR"/*.json; do
        if [ -f "$config_file" ]; then
          package=$(basename "$config_file" .json)
          pid=$(pidof "$package" 2>/dev/null)
          if [ -n "$pid" ]; then
            kill -9 $pid 2>/dev/null
            killed_count=$((killed_count + 1))
            app_list="$app_list  ok $package\n"
          fi
        fi
      done

      if [ $killed_count -eq 0 ]; then
        echo "No redirected app running"
      else
        echo "Restarted apps: count=$killed_count"
        echo ""
        printf "$app_list"
      fi
    else
      echo "error: missing config dir path=$CONFIG_DIR"
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Reload done"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Press any key to close..."
    read -n 1
    ;;

  "View Logs")
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Storage Redirect X running log"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    if [ -f "$LOGDIR/running.log" ]; then
      flush_logs_via_logd >/dev/null 2>&1
      tail -n 100 "$LOGDIR/running.log"
    else
      echo "error: missing log file path=$LOGDIR/running.log"
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Press any key to close..."
    read -n 1
    ;;

  "Clear Logs")
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Clear logs"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    if [ -d "$LOGDIR" ]; then
      if clear_logs_via_logd; then
        echo "Cleared log files: count=4"
        echo "Stats reset: ok"
      else
        echo "error: clear via srx_logd failed"
      fi
    else
      echo "error: missing logs dir path=$LOGDIR"
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Press any key to close..."
    read -n 1
    ;;

  *)
    echo "error: unknown action=$1"
    exit 1
    ;;
esac

exit 0
