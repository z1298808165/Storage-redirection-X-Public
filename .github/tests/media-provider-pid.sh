#!/system/bin/sh
dumpsys activity providers | awk '
  $1 == "*" && $2 ~ /^ContentProviderRecord[{]/ {
    owner = ($3 == "u0")
    pid = ""
  }
  owner && $1 ~ /^proc=/ {
    pid = ""
    if ($1 ~ /^proc=ProcessRecord[{]/ && $2 ~ /^[1-9][0-9]*:/) {
      split($2, process, ":")
      pid = process[1]
    }
  }
  owner && pid && $1 ~ /^authority=/ {
    names = $1
    sub(/^authority=/, "", names)
    if ((";" names ";") ~ /;media;/) {
      if (selected && selected != pid) ambiguous = 1
      selected = pid
    }
  }
  END { if (selected && !ambiguous) print selected }
'
