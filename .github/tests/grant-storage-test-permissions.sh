#!/system/bin/sh
set -eu

app="${1:?缺少测试应用包名}"
case "$app" in
  *[!a-zA-Z0-9_.]*|'') echo "无效测试应用包名：$app" >&2; exit 1 ;;
esac
sdk="$(getprop ro.build.version.sdk)"
case "$sdk" in
  *[!0-9]*|'') echo "无法读取 Android SDK 版本" >&2; exit 1 ;;
esac
requested="$(dumpsys package "$app")"
permissions="READ_EXTERNAL_STORAGE WRITE_EXTERNAL_STORAGE ACCESS_MEDIA_LOCATION"
operations="READ_EXTERNAL_STORAGE WRITE_EXTERNAL_STORAGE READ_MEDIA_IMAGES READ_MEDIA_VIDEO READ_MEDIA_AUDIO WRITE_MEDIA_IMAGES WRITE_MEDIA_VIDEO WRITE_MEDIA_AUDIO MANAGE_EXTERNAL_STORAGE MANAGE_MEDIA"
if [ "$sdk" -ge 33 ]; then
  permissions="$permissions READ_MEDIA_IMAGES READ_MEDIA_VIDEO READ_MEDIA_AUDIO POST_NOTIFICATIONS"
fi
if [ "$sdk" -ge 34 ]; then
  permissions="$permissions READ_MEDIA_VISUAL_USER_SELECTED"
  operations="$operations READ_MEDIA_VISUAL_USER_SELECTED"
fi
if [ "$sdk" -ge 37 ]; then
  permissions="$permissions ACCESS_LOCAL_NETWORK"
fi

granted=""
for permission in $permissions; do
  # 仅授予 APK 已声明的权限，系统特权权限不通过 pm grant 强行补入。
  if printf '%s\n' "$requested" | grep -Eq "^[[:space:]]+android[.]permission[.]$permission$"; then
    pm grant --user 0 "$app" "android.permission.$permission"
    granted="$granted $permission"
    if [ "$permission" = ACCESS_MEDIA_LOCATION ]; then
      operations="$operations ACCESS_MEDIA_LOCATION"
    fi
  fi
done
effective="$(dumpsys package "$app")"
for permission in $granted; do
  if ! printf '%s\n' "$effective" | grep -Fq "android.permission.$permission: granted=true"; then
    echo "测试权限未生效：android.permission.$permission" >&2
    exit 1
  fi
done
for operation in $operations; do
  cmd appops set --uid "$app" "$operation" allow
  cmd appops set "$app" "$operation" allow
  effective="$(cmd appops get "$app" "$operation")"
  if ! printf '%s\n' "$effective" | grep -Eq ': allow([;[:space:]]|$)' ||
    printf '%s\n' "$effective" | grep -Eq ': (ignore|deny|errored)([;[:space:]]|$)'; then
    echo "测试 AppOps 未生效：$operation $effective" >&2
    exit 1
  fi
done
printf '测试存储权限已就绪 package=%s sdk=%s\n' "$app" "$sdk"
