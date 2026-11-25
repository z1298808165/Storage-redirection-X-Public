#!/system/bin/sh
# 同包的工作资料进程也会出现在 pidof 中；主进程必须同时匹配用户 0 和完整包名。
ps -A -o UID,PID,NAME | awk -v package="$package" '
  $1 ~ /^[0-9]+$/ && $1 + 0 < 100000 && $2 ~ /^[1-9][0-9]*$/ && $3 == package {
    pid = $2
    count++
  }
  END { if (count == 1) print pid }
'
