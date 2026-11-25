#!/system/bin/sh
# 两个宿主运行器共用设备端检查。stat 默认不跟随末级符号链接。
check=$1
shift
for path do
  if kind=$(LC_ALL=C stat -c %F "$path" 2>&1); then
    if [ "$check" = scan ]; then
      if [ "$kind" = directory ]; then
        find "$path" -type f -printf '%p size=%s owner=%u:%g mtime=%T@\n' || exit 1
      else
        printf '%s type=%s\n' "$path" "$kind"
      fi
    elif [ "$kind" = directory ]; then
      # 目录项消失后，stat 仍可能读到 FUSE 缓存的目录属性，因此还要核对父目录中的条目。
      if ! error=$(LC_ALL=C ls -A "$path" 2>&1 >/dev/null); then
        if [ "$error" = "ls: $path: No such file or directory" ]; then
          parent=${path%/*}
          names=$(LC_ALL=C ls -1A "$parent" 2>&1) || { echo "fixture_lookup_failed path=$parent error=$names" >&2; exit 1; }
          if ! printf '%s\n' "$names" | grep -Fxq "${path##*/}"; then continue; fi
        fi
        echo "fixture_lookup_failed path=$path error=$error" >&2
        exit 1
      fi
      echo "$check path=$path" >&2
      exit 1
    else
      echo "$check path=$path" >&2
      exit 1
    fi
  else
    case "$kind" in
      *"$path"*": No such file or directory") ;;
      *) echo "fixture_lookup_failed path=$path error=$kind" >&2; exit 1 ;;
    esac
  fi
done
