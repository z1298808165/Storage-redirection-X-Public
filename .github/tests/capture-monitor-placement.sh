#!/system/bin/sh
app=$1 file=$2 request=$3 target=$4 uri=$5
for path in "$request" "$target"; do
  for base in /data/media/0 "/data/media/0/Android/data/$app/sdcard"; do
    printf '\ndirectory=%s/%s\n' "$base" "$path"
    ls -lai "$base/$path" 2>&1
  done
  [ "$request" != "$target" ] || break
done
printf '\ncaller_pid='; pidof "$app"
if [ -n "$uri" ]; then
  printf '\nuri=%s\n' "$uri"
  content query --user 0 --uri "$uri" --projection _id:_display_name:relative_path:_data:is_pending:_size 2>&1
  echo "query_exit=$?"
fi
printf '\nmonitor:\n'
grep -F -- "$file" /data/adb/modules/storage.redirect.x/logs/file_monitor.log
printf '\npublication:\n'
logcat -d -v threadtime -s SRX:V MediaProvider:V | awk -v file="$file" -v uri="$uri" -v request="$request" -v target="$target" '
  index($0, file) || (uri != "" && index($0, uri)) || index($0, request) || index($0, target)
'
