#!/usr/bin/env bash
set -euo pipefail

VERSION="$1"
VERSION_CODE="$2"
SO_FILE="$3"
DAEMON_FILE="$4"
OUTPUT_PATH="$5"
MODULE_ABI="${6:-arm64-v8a}"

case "$OUTPUT_PATH" in
  /*) OUTPUT_ABS_PATH="$OUTPUT_PATH" ;;
  [A-Za-z]:*) OUTPUT_ABS_PATH="$OUTPUT_PATH" ;;
  *) OUTPUT_ABS_PATH="$PWD/$OUTPUT_PATH" ;;
esac

build_module_dir() {
  local module_dir="$1"
  rm -rf "$module_dir"
  mkdir -p "$module_dir"

  cp -r assets/zygisk_module/* "$module_dir/"
  rm -f "$module_dir/action.sh"
  cp LICENSE "$module_dir/LICENSE"
  cp COPYING "$module_dir/COPYING"

  mkdir -p "$module_dir/zygisk"
  case "$MODULE_ABI" in
    arm64-v8a|x86_64) ;;
    *)
      echo "Unsupported module ABI: $MODULE_ABI" >&2
      exit 1
      ;;
  esac
  cp "$SO_FILE" "$module_dir/zygisk/${MODULE_ABI}.so"
  mkdir -p "$module_dir/bin"
  if [[ -d assets/zygisk_module/bin ]]; then
    chmod 755 "$module_dir/bin"/* 2>/dev/null || true
  fi
  cp "$DAEMON_FILE" "$module_dir/bin/srx_daemon"
  chmod 755 "$module_dir/bin/srx_daemon"
  [[ -f "$module_dir/bin/srxctl" ]] && chmod 755 "$module_dir/bin/srxctl"
  [[ -f "$module_dir/bin/list_apps.dex" ]] && chmod 644 "$module_dir/bin/list_apps.dex"

  printf 'id=storage.redirect.x\nname=Storage Redirect X\nversion=v%s\nversionCode=%s\nauthor=Kindness-Kismet\ndescription=Storage Redirect X Core Module\nwebui=1\n' \
    "$VERSION" "$VERSION_CODE" > "$module_dir/module.prop"

  (
    cd "$module_dir"
    find . -type f \( -name '*.sh' -o -name '*.prop' -o -name '*.rule' -o -path './META-INF/*' -o -path './bin/srxctl' \) \
      -exec perl -0pi -e 's/\r\n?/\n/g' {} \;
  )
}

if [[ "$OUTPUT_PATH" == *.zip ]]; then
  MODULE_DIR=$(mktemp -d)
  trap 'rm -rf "$MODULE_DIR"' EXIT
  build_module_dir "$MODULE_DIR"
  mkdir -p "$(dirname "$OUTPUT_ABS_PATH")"
  (
    cd "$MODULE_DIR"
    zip -0 -r "$OUTPUT_ABS_PATH" .
  )
else
  build_module_dir "$OUTPUT_PATH"
fi
