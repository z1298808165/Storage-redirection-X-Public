#!/system/bin/sh
set -eu

fixture="${1:?missing fixture executable}"
library="${2:?missing module library}"
permissions="${3:?missing permission helper}"
module=/data/adb/modules/storage.redirect.x
app=me.fakerqu.test.storageredirect
lock=/data/local/tmp/srx-test-flow.lock
token="zn-$$-$(date +%s)"
mkdir "$lock" 2>/dev/null || { echo 'ZN fixture: device test lock held' >&2; exit 1; }
if ! printf '%s' "$token" > "$lock/owner"; then rmdir "$lock"; exit 1; fi
scratch=''
reload_pid=''
backed_up=0
restore() {
  result=$?
  restored=1
  trap - EXIT INT TERM HUP
  set +e
  if [ -n "$reload_pid" ]; then kill "$reload_pid" 2>/dev/null; wait "$reload_pid" 2>/dev/null; fi
  if [ "$backed_up" -eq 1 ]; then
    cp "$scratch/global.json" "$module/config/global.json" || restored=0
    if [ -f "$scratch/app.json" ]; then cp "$scratch/app.json" "$config" || restored=0; else rm -f "$config" || restored=0; fi
    if [ -f "$scratch/uid-map" ]; then cp "$scratch/uid-map" "$module/config/system_writer_uids.list" || restored=0; fi
    rm -f "$module/.runtime_disabled" || result=1
    rm -rf "$source_dir" "$target_dir" "$sandbox_dir" || result=1
  fi
  if [ "$restored" -eq 0 ]; then
    echo "FAIL [zn_restore] configuration backup retained at $scratch" >&2
    result=1
  elif [ -n "$scratch" ]; then
    rm -rf "$scratch" || result=1
  fi
  if [ "$(cat "$lock/owner" 2>/dev/null)" = "$token" ]; then
    rm "$lock/owner" && rmdir "$lock" || result=1
  else
    result=1
  fi
  exit "$result"
}
trap restore EXIT
trap 'exit 130' INT TERM HUP
# 权限准备先于任何配置修改与测试。
sh "$permissions" "$app"
uid="$(cmd package list packages -U "$app" | sed -n 's/.* uid://p')"
case "$uid" in ''|*[!0-9]*) echo 'ZN fixture: test application UID unavailable' >&2; exit 1 ;; esac
test ! -e "$module/.runtime_disabled"
scratch="$(mktemp -d /data/local/tmp/srx-zn-runtime.XXXXXX)"
config="$module/config/apps/$app.json"
cp "$module/config/global.json" "$scratch/global.json"
if [ -f "$config" ]; then cp "$config" "$scratch/app.json"; fi
source_name="SrxZnFixtureSource-${scratch##*.}"
target_name="SrxZnFixtureTarget-${scratch##*.}"
source_dir="/data/media/0/Download/$source_name"
target_dir="/data/media/0/Download/$target_name"
sandbox_dir="/data/media/0/Android/data/$app/sdcard/Download/$source_name"
test ! -e "$source_dir" && test ! -e "$target_dir" && test ! -e "$sandbox_dir"
backed_up=1
chmod 755 "$fixture"
passed=0
failed=0
shift 3
for tests in "$@"; do
  chmod 755 "$tests"
  if "$tests" --test-threads=1; then passed=$((passed + 1)); else failed=$((failed + 1)); fi
done
mkdir -p "$source_dir" "$target_dir"
chmod 777 "$source_dir" "$target_dir"
printf '%s\n' '{"file_monitor_enabled":false,"verbose_logging_enabled":true}' > "$module/config/global.json"
rm -f "$config"
run_case() {
  name="$1" expected="$2" mode="$3" package="$4" path="$5" evidence="$6"
  result=0
  if [ -n "$path" ]; then
    "$fixture" "$library" "$package" "$uid" "$mode" "$path" || result=$?
  else
    "$fixture" "$library" "$package" "$uid" "$mode" || result=$?
  fi
  if [ "$result" -eq "$expected" ] && { [ -z "$evidence" ] || [ "$(cat "$evidence" 2>/dev/null)" = srx-zn-fixture ]; }; then
    echo "PASS [zn_$name] expected_exit=$expected actual_exit=$result"
    passed=$((passed + 1))
  else
    echo "FAIL [zn_$name] expected_exit=$expected actual_exit=$result evidence=$evidence" >&2
    "$module/bin/srx_daemon" package-query P 0 "$package" >&2 || true
    grep -F "$package:" "$module/config/system_writer_uids.list" >&2 || true
    failed=$((failed + 1))
  fi
}
run_case passthrough 0 private "$app" '' ''
if [ -f "$module/config/system_writer_uids.list" ]; then
  cp "$module/config/system_writer_uids.list" "$scratch/uid-map"
  awk -F: -v app="$app" '$1 != app' "$scratch/uid-map" > "$scratch/uid-map-without-app"
  cp "$scratch/uid-map-without-app" "$module/config/system_writer_uids.list"
  run_case uncached_identity 0 private "$app" '' ''
  cp "$scratch/uid-map" "$module/config/system_writer_uids.list"
  rm "$scratch/uid-map"
else
  run_case uncached_identity 0 private "$app" '' ''
fi
printf '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/%s":"Download/%s"}}}}\n' "$source_name" "$target_name" > "$config"
run_case mapping 0 private "$app" "/storage/emulated/0/Download/$source_name/result.txt" "$target_dir/result.txt"
if [ -e "$source_dir/result.txt" ]; then echo 'FAIL [zn_mapping_source] unexpected physical source' >&2; failed=$((failed + 1)); fi
run_case shared_namespace 126 shared "$app" '' ''
run_case unknown_package 126 private com.srx.fixture.not_installed '' ''
run_case disconnected 126 disconnected "$app" '' ''
printf '%s\n' '{"users":{"0":{"enabled":true,"mapping_mode_only":true}}}' > "$config"
run_case empty_mapping 0 private "$app" '' ''
printf '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"read_only_paths":["Download/%s"]}}}\n' "$source_name" > "$config"
run_case read_only 122 private "$app" "/storage/emulated/0/Download/$source_name/denied.txt" ''
if [ -e "$source_dir/denied.txt" ]; then echo 'FAIL [zn_read_only_physical] denied write reached storage' >&2; failed=$((failed + 1)); fi
mkdir -p "$sandbox_dir"
chmod 777 "$sandbox_dir"
printf '%s\n' '{"users":{"0":{"enabled":true,"mapping_mode_only":false}}}' > "$config"
run_case isolation 0 private "$app" "/storage/emulated/0/Download/$source_name/isolated.txt" "$sandbox_dir/isolated.txt"
if [ -e "$source_dir/isolated.txt" ]; then echo 'FAIL [zn_isolation_source] unexpected physical source' >&2; failed=$((failed + 1)); fi
cat > "$config" <<'JSON'
{"users":{"0":{"enabled":false}}}
JSON
run_case disabled_profile 0 private "$app" '' ''
printf '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/%s":"Download/%s"}}}}\n' "$source_name" "$target_name" > "$config"
SRX_ZN_FIXTURE_DELAY_MS=10000 "$fixture" "$library" "$app" "$uid" private "/storage/emulated/0/Download/$source_name/reload.txt" > "$scratch/reload.log" 2>&1 &
reload_pid=$!
ready=0
for attempt in 1 2 3 4 5; do
  if grep -q '^ZN_FIXTURE_READY$' "$scratch/reload.log"; then ready=1; break; fi
  sleep 1
done
printf '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/%s":"Download/%s/Reloaded"}}}}\n' "$source_name" "$target_name" > "$config"
reload_result=0
wait "$reload_pid" || reload_result=$?
reload_pid=''
cat "$scratch/reload.log"
if [ "$ready" -eq 1 ] && [ "$reload_result" -eq 0 ] && [ "$(cat "$target_dir/Reloaded/reload.txt" 2>/dev/null)" = srx-zn-fixture ] && [ ! -e "$target_dir/reload.txt" ]; then
  echo 'PASS [zn_reload] running native process received updated mapping'; passed=$((passed + 1))
else
  echo "FAIL [zn_reload] ready=$ready exit=$reload_result" >&2; failed=$((failed + 1))
fi
rm -f "$config"
printf '%s\n' '{"file_monitor_enabled":true,"verbose_logging_enabled":true}' > "$module/config/global.json"
monitor_name="monitor-$$.txt"
run_case monitor_only 0 private "$app" "/storage/emulated/0/Download/$source_name/$monitor_name" "$source_dir/$monitor_name"
recorded=0
for attempt in 1 2 3; do
  if grep -F "$monitor_name" "$module/logs/file_monitor.log" | grep -Fq "$app"; then recorded=1; break; fi
  sleep 1
done
if [ "$recorded" -eq 1 ]; then echo 'PASS [zn_monitor_record] application identity recorded'; passed=$((passed + 1)); else echo 'FAIL [zn_monitor_record] application record missing' >&2; failed=$((failed + 1)); fi
# 运行时开关会触发 daemon 拆除视图，放在其它用例之后避免异步清理污染重载判据。
printf '%s\n' '{"file_monitor_enabled":false,"verbose_logging_enabled":true}' > "$module/config/global.json"
printf '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/%s":"Download/%s"}}}}\n' "$source_name" "$target_name" > "$config"
run_case runtime_disabled 0 disabled "$app" '' ''
echo "ZN_FIXTURE_SUMMARY passed=$passed failed=$failed"
test "$failed" -eq 0
