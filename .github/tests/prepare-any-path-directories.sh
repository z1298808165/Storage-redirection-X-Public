#!/system/bin/sh
# 映射源不能作为公共目录预置；这里只接收本轮需要创建的真实目标或专用夹具目录。
for path do
  mkdir -p "$path" || exit $?
  if ! test -d "$path"; then
    echo "fixture_not_directory path=$path" >&2
    exit 1
  fi
  chmod -R 777 "$path" 2>/dev/null || :
done
