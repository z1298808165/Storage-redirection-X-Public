is_skipped_package() {
  package_name="$1"
  # 排除媒体与模块自身，避免和系统代写热更新策略冲突。
  case "$package_name" in
    ""|\
    com.storage.redirect.x|\
    com.android.providers.media|\
    com.android.providers.media.module|\
    com.google.android.providers.media.module)
      return 0
      ;;
  esac
  return 1
}

is_effective_package_config() {
  package_name="$1"
  config_file="$APPS_CONFIG_DIR/$package_name.json"
  if [ ! -f "$config_file" ]; then
    return 1
  fi
  if ! grep -q '"users"[[:space:]]*:' "$config_file" 2>/dev/null; then
    return 1
  fi
  # 仅将存在 enabled=true 的配置视为生效配置。
  if grep -Eq '"enabled"[[:space:]]*:[[:space:]]*true' "$config_file" 2>/dev/null; then
    return 0
  fi
  return 1
}

is_package_query_ready() {
  cmd package list packages 2>/dev/null | grep -q '^package:'
}

is_auto_enable_new_apps_enabled() {
  [ -f "$CONFIG_DIR/global.json" ] || return 1
  grep -Eq '"auto_enable_redirect_for_new_apps"[[:space:]]*:[[:space:]]*true' "$CONFIG_DIR/global.json" 2>/dev/null
}

get_current_boot_id() {
  [ -r /proc/sys/kernel/random/boot_id ] || return 0
  cat /proc/sys/kernel/random/boot_id 2>/dev/null | head -n 1
}

get_uptime_seconds() {
  awk '{ print int($1) }' /proc/uptime 2>/dev/null
}

is_package_event_receiver_ready() {
  ready_file="${PACKAGE_EVENT_RECEIVER_READY_FILE:-$LOGS_DIR/.package_event_receiver_ready}"
  [ -s "$ready_file" ] || return 1

  current_boot_id=$(get_current_boot_id)
  [ -n "$current_boot_id" ] || return 1
  ready_boot_id=$(awk -F'|' '
    $1 == "srx-package-receiver-ready-v1" { print $3; exit }
  ' "$ready_file" 2>/dev/null)
  [ "$ready_boot_id" = "$current_boot_id" ] || return 1

  ready_pid=$(awk -F'|' '
    $1 == "srx-package-receiver-ready-v1" { print $4; exit }
  ' "$ready_file" 2>/dev/null)
  echo "$ready_pid" | grep -Eq '^[0-9][0-9]*$' || return 1
  [ -d "/proc/$ready_pid" ] || return 1

  ready_cmdline=$(tr '\000' ' ' < "/proc/$ready_pid/cmdline" 2>/dev/null | head -n 1)
  case "$ready_cmdline" in
    *system_server*) return 0 ;;
  esac
  return 1
}

is_device_interactive_for_package_poll() {
  power_state=$(dumpsys power 2>/dev/null)
  if [ -n "$power_state" ]; then
    echo "$power_state" | grep -Eq 'mWakefulness=Awake|mInteractive=true|Display Power: state=ON' || return 1
  fi

  deep_idle_state=$(cmd deviceidle get deep 2>/dev/null | tr -d '\r' | head -n 1)
  case "$deep_idle_state" in
    IDLE|IDLE_PENDING|SENSING|LOCATING|IDLE_MAINTENANCE)
      return 1
      ;;
  esac
  return 0
}

is_auto_new_apps_package_poll_eligible() {
  is_auto_enable_new_apps_enabled || return 1
  is_package_event_receiver_ready && return 1

  uptime_seconds=$(get_uptime_seconds)
  if echo "$uptime_seconds" | grep -Eq '^[0-9][0-9]*$'; then
    if [ "$uptime_seconds" -lt "${AUTO_NEW_APPS_PACKAGE_POLL_GRACE_SECONDS:-15}" ]; then
      return 1
    fi
  fi

  return 0
}

get_auto_enable_new_apps_template_id() {
  [ -f "$CONFIG_DIR/global.json" ] || return 0
  sed -n 's/.*"auto_enable_new_apps_template_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$CONFIG_DIR/global.json" 2>/dev/null |
    head -n 1 |
    grep -E '^[A-Za-z0-9_.-]{1,80}$'
}

list_user_ids() {
  cmd user list 2>/dev/null |
    sed -n 's/.*{\([0-9][0-9]*\):.*/\1/p' |
    sort -u
}

list_user_installed_packages() {
  user_id="$1"
  [ -n "$user_id" ] || return 0
  cmd package list packages --user "$user_id" -3 2>/dev/null |
    sed -n 's/^package://p' |
    grep -E '^[A-Za-z0-9_.-]+$' |
    sort -u
}

is_safe_package_name() {
  package_name="$1"
  [ -n "$package_name" ] || return 1
  echo "$package_name" | grep -Eq '^[A-Za-z0-9_.-]{1,255}$'
}

is_valid_user_id() {
  user_id="$1"
  echo "$user_id" | grep -Eq '^[0-9][0-9]*$'
}

sanitize_install_signature() {
  sed 's/|/_/g; s/[[:cntrl:]]/_/g'
}

get_package_install_signature() {
  package_name="$1"
  is_safe_package_name "$package_name" || {
    echo "unknown"
    return 0
  }

  first_install=$(dumpsys package "$package_name" 2>/dev/null |
    sed -n 's/^[[:space:]]*firstInstallTime=//p' |
    head -n 1 |
    sanitize_install_signature)
  if [ -n "$first_install" ]; then
    echo "first=$first_install"
    return 0
  fi

  apk_path=$(cmd package path "$package_name" 2>/dev/null |
    sed -n 's/^package://p' |
    head -n 1)
  if [ -n "$apk_path" ] && [ -e "$apk_path" ]; then
    apk_sig=$(stat -c '%Y:%s' "$apk_path" 2>/dev/null | sanitize_install_signature)
    if [ -n "$apk_sig" ]; then
      echo "apk=$apk_sig"
      return 0
    fi
  fi

  echo "unknown"
}

is_user_package_installed() {
  user_id="$1"
  package_name="$2"
  is_valid_user_id "$user_id" || return 1
  is_safe_package_name "$package_name" || return 1
  cmd package list packages --user "$user_id" "$package_name" 2>/dev/null |
    grep -qx "package:$package_name"
}

is_user_third_party_package_installed() {
  user_id="$1"
  package_name="$2"
  is_valid_user_id "$user_id" || return 1
  is_safe_package_name "$package_name" || return 1
  cmd package list packages --user "$user_id" -3 "$package_name" 2>/dev/null |
    grep -qx "package:$package_name"
}

build_auto_new_apps_baseline() {
  output_file="$1"
  : > "$output_file"
  user_ids=$(list_user_ids)
  if [ -z "$user_ids" ]; then
    user_ids=0
  fi
  for user_id in $user_ids; do
    list_user_installed_packages "$user_id" |
      while IFS= read -r package_name; do
        [ -n "$package_name" ] || continue
        signature=$(get_package_install_signature "$package_name")
        echo "$user_id|$package_name|$signature" >> "$output_file"
      done
  done
  sort -u -o "$output_file" "$output_file" 2>/dev/null
}

build_auto_new_apps_package_snapshot() {
  output_file="$1"
  : > "$output_file"
  user_ids=$(list_user_ids)
  if [ -z "$user_ids" ]; then
    user_ids=0
  fi
  for user_id in $user_ids; do
    list_user_installed_packages "$user_id" |
      while IFS= read -r package_name; do
        [ -n "$package_name" ] || continue
        echo "$user_id|$package_name" >> "$output_file"
      done
  done
  sort -u -o "$output_file" "$output_file" 2>/dev/null
}

refresh_auto_new_apps_baseline() {
  if ! is_package_query_ready; then
    log -p w -t Boot "skip new app baseline refresh: package query unavailable"
    return 1
  fi

  mkdir -p "$CONFIG_DIR"
  tmp_file="$AUTO_NEW_APPS_BASELINE_FILE.tmp"
  build_auto_new_apps_baseline "$tmp_file"
  mv "$tmp_file" "$AUTO_NEW_APPS_BASELINE_FILE" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
  return 0
}

poll_auto_new_apps_from_package_list() {
  is_auto_enable_new_apps_enabled || return 0
  is_package_event_receiver_ready && return 0
  if ! is_package_query_ready; then
    log -p w -t Boot "skip new app package poll: package query unavailable"
    return 0
  fi

  if [ ! -s "$AUTO_NEW_APPS_BASELINE_FILE" ]; then
    refresh_auto_new_apps_baseline
    return 0
  fi

  current_file="$LOGS_DIR/.auto_new_apps.current.$$"
  changed_file="$LOGS_DIR/.auto_new_apps.poll_changed.$$"
  : > "$changed_file"

  if ! build_auto_new_apps_package_snapshot "$current_file"; then
    rm -f "$current_file" "$changed_file"
    return 0
  fi

  current_count=$(wc -l < "$current_file" 2>/dev/null | awk '{print $1 + 0}')
  if [ "$current_count" -le 0 ]; then
    log -p w -t Boot "skip new app package poll: empty package snapshot"
    rm -f "$current_file" "$changed_file"
    return 0
  fi

  uid_map_for_new_app_poll_refreshed=0
  while IFS='|' read -r user_id package_name; do
    [ -n "$user_id" ] || continue
    [ -n "$package_name" ] || continue
    is_valid_user_id "$user_id" || continue
    is_safe_package_name "$package_name" || continue
    if auto_new_apps_baseline_has_package "$user_id" "$package_name"; then
      continue
    fi

    if [ "${uid_map_for_new_app_poll_refreshed:-0}" -eq 0 ]; then
      refresh_uid_map force
      uid_map_for_new_app_poll_refreshed=1
    fi
    log -p i -t Boot "new app detected by package poll: user=$user_id pkg=$package_name"
    handle_auto_new_app_added_for_user "$user_id" "$package_name" "$changed_file"
  done < "$current_file"

  rm -f "$current_file"
  log_hot_reload_packages_from_file "$changed_file" "new-app-package-poll"
  rm -f "$changed_file"
}

auto_new_apps_baseline_has_entry() {
  user_id="$1"
  package_name="$2"
  signature="$3"
  [ -f "$AUTO_NEW_APPS_BASELINE_FILE" ] || return 1
  awk -F'|' -v user_id="$user_id" -v package_name="$package_name" -v signature="$signature" '
    $1 == user_id && $2 == package_name && $3 == signature { found = 1; exit }
    END { if (found) exit 0; exit 1 }
  ' "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
}

auto_new_apps_baseline_has_package() {
  user_id="$1"
  package_name="$2"
  [ -f "$AUTO_NEW_APPS_BASELINE_FILE" ] || return 1
  awk -F'|' -v user_id="$user_id" -v package_name="$package_name" '
    $1 == user_id && $2 == package_name { found = 1; exit }
    END { if (found) exit 0; exit 1 }
  ' "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
}

remove_auto_new_apps_baseline_entry() {
  user_id="$1"
  package_name="$2"
  is_valid_user_id "$user_id" || return 1
  is_safe_package_name "$package_name" || return 1
  mkdir -p "$CONFIG_DIR"
  [ -f "$AUTO_NEW_APPS_BASELINE_FILE" ] || : > "$AUTO_NEW_APPS_BASELINE_FILE"
  tmp_file="$AUTO_NEW_APPS_BASELINE_FILE.tmp"
  awk -F'|' -v user_id="$user_id" -v package_name="$package_name" '
    !($1 == user_id && $2 == package_name)
  ' "$AUTO_NEW_APPS_BASELINE_FILE" > "$tmp_file" 2>/dev/null || {
    rm -f "$tmp_file"
    return 1
  }
  mv "$tmp_file" "$AUTO_NEW_APPS_BASELINE_FILE" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
}

update_auto_new_apps_baseline_entry() {
  user_id="$1"
  package_name="$2"
  is_valid_user_id "$user_id" || return 1
  is_safe_package_name "$package_name" || return 1
  remove_auto_new_apps_baseline_entry "$user_id" "$package_name" || return 1
  if ! is_user_third_party_package_installed "$user_id" "$package_name"; then
    return 0
  fi
  signature=$(get_package_install_signature "$package_name")
  echo "$user_id|$package_name|$signature" >> "$AUTO_NEW_APPS_BASELINE_FILE"
  sort -u -o "$AUTO_NEW_APPS_BASELINE_FILE" "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
  chmod 644 "$AUTO_NEW_APPS_BASELINE_FILE" 2>/dev/null
}

extract_template_user_profile() {
  template_id="$1"
  user_id="$2"
  templates_file="$CONFIG_DIR/templates.json"
  [ -n "$template_id" ] || return 1
  [ -n "$user_id" ] || return 1
  [ -f "$templates_file" ] || return 1

  awk -v target_id="$template_id" -v target_user="$user_id" '
    function ch(pos) { return substr(json, pos, 1) }
    function skip_ws(pos) {
      while (pos <= len && ch(pos) ~ /[ \t\r\n]/) pos++
      return pos
    }
    function read_string(pos,    out, c, esc) {
      out = ""
      pos++
      esc = 0
      while (pos <= len) {
        c = ch(pos)
        if (esc) {
          out = out c
          esc = 0
        } else if (c == "\\") {
          esc = 1
        } else if (c == "\"") {
          string_value = out
          return pos + 1
        } else {
          out = out c
        }
        pos++
      }
      string_value = ""
      return 0
    }
    function skip_string(pos,    c, esc) {
      pos++
      esc = 0
      while (pos <= len) {
        c = ch(pos)
        if (esc) esc = 0
        else if (c == "\\") esc = 1
        else if (c == "\"") return pos + 1
        pos++
      }
      return 0
    }
    function skip_block(pos, open_ch, close_ch,    depth, c) {
      depth = 0
      while (pos <= len) {
        c = ch(pos)
        if (c == "\"") {
          pos = skip_string(pos)
          if (!pos) return 0
          continue
        }
        if (c == open_ch) depth++
        else if (c == close_ch) {
          depth--
          if (depth == 0) return pos + 1
        }
        pos++
      }
      return 0
    }
    function skip_value(pos,    c) {
      pos = skip_ws(pos)
      c = ch(pos)
      if (c == "{") return skip_block(pos, "{", "}")
      if (c == "[") return skip_block(pos, "[", "]")
      if (c == "\"") return skip_string(pos)
      while (pos <= len && ch(pos) !~ /[,}\]]/) pos++
      return pos
    }
    function object_value_start(obj_pos, key,    end, pos, key_end, value_start) {
      obj_pos = skip_ws(obj_pos)
      if (ch(obj_pos) != "{") return 0
      end = skip_value(obj_pos)
      pos = skip_ws(obj_pos + 1)
      while (pos > 0 && pos < end && ch(pos) != "}") {
        if (ch(pos) != "\"") return 0
        key_end = read_string(pos)
        if (!key_end) return 0
        pos = skip_ws(key_end)
        if (ch(pos) != ":") return 0
        value_start = skip_ws(pos + 1)
        if (string_value == key) return value_start
        pos = skip_value(value_start)
        pos = skip_ws(pos)
        if (ch(pos) == ",") pos = skip_ws(pos + 1)
      }
      return 0
    }
    function first_object_value_start(obj_pos,    end, pos, key_end, value_start) {
      obj_pos = skip_ws(obj_pos)
      if (ch(obj_pos) != "{") return 0
      end = skip_value(obj_pos)
      pos = skip_ws(obj_pos + 1)
      while (pos > 0 && pos < end && ch(pos) != "}") {
        if (ch(pos) != "\"") return 0
        key_end = read_string(pos)
        if (!key_end) return 0
        pos = skip_ws(key_end)
        if (ch(pos) != ":") return 0
        value_start = skip_ws(pos + 1)
        if (ch(value_start) == "{") return value_start
        pos = skip_value(value_start)
        pos = skip_ws(pos)
        if (ch(pos) == ",") pos = skip_ws(pos + 1)
      }
      return 0
    }
    function array_next_value(arr_pos, current_pos,    pos) {
      if (current_pos == 0) {
        pos = skip_ws(arr_pos + 1)
      } else {
        pos = skip_value(current_pos)
        pos = skip_ws(pos)
        if (ch(pos) == ",") pos = skip_ws(pos + 1)
      }
      if (ch(pos) == "]") return 0
      return pos
    }
    {
      json = json $0 "\n"
    }
    END {
      len = length(json)
      root = skip_ws(1)
      templates = object_value_start(root, "templates")
      if (!templates || ch(templates) != "[") exit 1
      item = 0
      while ((item = array_next_value(templates, item)) > 0) {
        if (ch(item) != "{") exit 1
        id_start = object_value_start(item, "id")
        if (!id_start || ch(id_start) != "\"") continue
        id_end = read_string(id_start)
        if (!id_end || string_value != target_id) continue
        config = object_value_start(item, "config")
        users = object_value_start(config, "users")
        if (!users || ch(users) != "{") exit 1
        profile = object_value_start(users, target_user)
        if (!profile) profile = first_object_value_start(users)
        if (!profile || ch(profile) != "{") exit 1
        profile_end = skip_value(profile)
        if (!profile_end) exit 1
        print substr(json, profile, profile_end - profile)
        exit 0
      }
      exit 1
    }
  ' "$templates_file"
}

write_user_profile_app_config() {
  package_name="$1"
  user_id="$2"
  profile_file="$3"
  config_file="$APPS_CONFIG_DIR/$package_name.json"
  tmp_file="$config_file.tmp"

  [ -n "$package_name" ] || return 1
  [ -n "$user_id" ] || return 1
  [ -s "$profile_file" ] || return 1
  is_skipped_package "$package_name" && return 1

  if [ -f "$config_file" ]; then
    if grep -Eq "\"$user_id\"[[:space:]]*:[[:space:]]*\{" "$config_file" 2>/dev/null; then
      return 1
    fi

    awk -v user_id="$user_id" -v profile_file="$profile_file" '
      BEGIN {
        profile = ""
        while ((getline line < profile_file) > 0) {
          profile = profile line "\n"
        }
        close(profile_file)
        sub(/\n$/, "", profile)
        if (profile == "") exit 1
      }
      {
        lines[NR] = $0
      }
      END {
        users_close = 0
        for (i = NR - 1; i >= 1; i--) {
          if (lines[i] ~ /^[[:space:]]*}[[:space:]]*$/ && lines[i + 1] ~ /^}[[:space:]]*$/) {
            users_close = i
            break
          }
        }
        if (users_close <= 2) {
          exit 1
        }
        for (i = 1; i <= NR; i++) {
          if (i == users_close - 1 && lines[i] !~ /,[[:space:]]*$/) {
            print lines[i] ","
            continue
          }
          if (i == users_close) {
            print "    \"" user_id "\": " profile
          }
          print lines[i]
        }
      }
    ' "$config_file" > "$tmp_file" || {
      rm -f "$tmp_file"
      log -p w -t Boot "skip new app auto-enable: existing config cannot append user=$user_id pkg=$package_name"
      return 1
    }

    mv "$tmp_file" "$config_file" || {
      rm -f "$tmp_file"
      return 1
    }
    chmod 644 "$config_file" 2>/dev/null
    return 0
  fi

  {
    echo "{"
    echo "  \"users\": {"
    printf '    "%s": ' "$user_id"
    cat "$profile_file"
    echo ""
    echo "  }"
    echo "}"
  } > "$tmp_file" || {
    rm -f "$tmp_file"
    return 1
  }

  mv "$tmp_file" "$config_file" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$config_file" 2>/dev/null
  return 0
}

write_template_enabled_app_config() {
  package_name="$1"
  user_id="$2"
  template_id=$(get_auto_enable_new_apps_template_id)
  [ -n "$template_id" ] || return 1

  profile_file="$LOGS_DIR/.auto_new_app_template_profile.$$"
  if ! extract_template_user_profile "$template_id" "$user_id" > "$profile_file"; then
    rm -f "$profile_file"
    log -p w -t Boot "auto-enable template unavailable, fallback default: template=$template_id user=$user_id pkg=$package_name"
    return 1
  fi

  if write_user_profile_app_config "$package_name" "$user_id" "$profile_file"; then
    rm -f "$profile_file"
    log -p i -t Boot "auto enable redirect with template: template=$template_id user=$user_id pkg=$package_name"
    return 0
  fi

  rm -f "$profile_file"
  return 1
}

write_default_enabled_app_config() {
  package_name="$1"
  user_id="$2"
  config_file="$APPS_CONFIG_DIR/$package_name.json"
  tmp_file="$config_file.tmp"

  [ -n "$package_name" ] || return 1
  [ -n "$user_id" ] || return 1
  is_skipped_package "$package_name" && return 1
  if [ -f "$config_file" ]; then
    if grep -Eq "\"$user_id\"[[:space:]]*:[[:space:]]*\{" "$config_file" 2>/dev/null; then
      return 1
    fi

    awk -v user_id="$user_id" '
      {
        lines[NR] = $0
      }
      END {
        users_close = 0
        for (i = NR - 1; i >= 1; i--) {
          if (lines[i] ~ /^[[:space:]]*}[[:space:]]*$/ && lines[i + 1] ~ /^}[[:space:]]*$/) {
            users_close = i
            break
          }
        }
        if (users_close <= 2) {
          exit 1
        }
        for (i = 1; i <= NR; i++) {
          if (i == users_close - 1 && lines[i] !~ /,[[:space:]]*$/) {
            print lines[i] ","
            continue
          }
          if (i == users_close) {
            print "    \"" user_id "\": {"
            print "      \"enabled\": true"
            print "    }"
          }
          print lines[i]
        }
      }
    ' "$config_file" > "$tmp_file" || {
      rm -f "$tmp_file"
      log -p w -t Boot "skip new app auto-enable: existing config cannot append user=$user_id pkg=$package_name"
      return 1
    }

    mv "$tmp_file" "$config_file" || {
      rm -f "$tmp_file"
      return 1
    }
    chmod 644 "$config_file" 2>/dev/null
    return 0
  fi

  cat > "$tmp_file" <<EOF
{
  "users": {
    "$user_id": {
      "enabled": true
    }
  }
}
EOF
  mv "$tmp_file" "$config_file" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$config_file" 2>/dev/null
  return 0
}

write_auto_enabled_app_config() {
  package_name="$1"
  user_id="$2"
  write_template_enabled_app_config "$package_name" "$user_id" && return 0
  write_default_enabled_app_config "$package_name" "$user_id"
}

sync_auto_new_apps_startup_state() {
  if ! is_package_query_ready; then
    log -p w -t Boot "skip new app startup sync: package query unavailable"
    return 0
  fi

  if ! is_auto_enable_new_apps_enabled || [ ! -s "$AUTO_NEW_APPS_BASELINE_FILE" ]; then
    refresh_auto_new_apps_baseline
  fi
}

resolve_package_event_user_ids() {
  event_user_id="$1"
  package_name="$2"
  if is_valid_user_id "$event_user_id"; then
    echo "$event_user_id"
    return 0
  fi

  user_ids=$(list_user_ids)
  if [ -z "$user_ids" ]; then
    user_ids=0
  fi
  for user_id in $user_ids; do
    if is_user_package_installed "$user_id" "$package_name"; then
      echo "$user_id"
    fi
  done
}

handle_auto_new_app_added_for_user() {
  user_id="$1"
  package_name="$2"
  changed_file="$3"

  is_valid_user_id "$user_id" || return 0
  is_safe_package_name "$package_name" || return 0
  is_skipped_package "$package_name" && return 0

  if ! is_user_third_party_package_installed "$user_id" "$package_name"; then
    log -p i -t Boot "skip new app auto-enable: not third-party user package user=$user_id pkg=$package_name"
    update_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
    return 0
  fi

  signature=$(get_package_install_signature "$package_name")
  if auto_new_apps_baseline_has_entry "$user_id" "$package_name" "$signature"; then
    log -p i -t Boot "skip new app auto-enable: baseline already has user=$user_id pkg=$package_name"
    return 0
  fi

  if write_auto_enabled_app_config "$package_name" "$user_id"; then
    echo "$package_name" >> "$changed_file"
    log -p i -t Boot "auto enable redirect for new app: user=$user_id pkg=$package_name"
  else
    log -p i -t Boot "skip new app auto-enable: config already exists or cannot write user=$user_id pkg=$package_name"
  fi

  update_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
}

handle_package_added_event() {
  event_user_id="$1"
  package_name="$2"
  replacing="$3"

  # 等待 PMS 状态稳定；PackageEventReceiver 也会延迟发送重复事件，
  # 即使只读取到一行，此等待也能保证直接消费文件时的稳定性。
  sleep 2
  refresh_uid_map force

  changed_file="$LOGS_DIR/.new_app_config_packages.tmp"
  : > "$changed_file"

  # 卸载流程会把配置置回 disabled 并留标记（文件保留以免丢用户自定义规则）。
  # 重装时先恢复这类配置，否则 auto-enable 会因「profile 已存在」跳过写入，
  # 开关永远停在关闭态——这正是「卸载重装后不自动开启」的根因。
  if restore_uninstall_disabled_config "$package_name"; then
    echo "$package_name" >> "$changed_file"
  fi

  user_ids=$(resolve_package_event_user_ids "$event_user_id" "$package_name")
  if [ -z "$user_ids" ]; then
    log -p w -t Boot "skip package added event: no user resolved pkg=$package_name"
    rm -f "$changed_file"
    return 0
  fi

  for user_id in $user_ids; do
    if [ "$replacing" = "1" ]; then
      update_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
      continue
    fi

    if ! is_auto_enable_new_apps_enabled; then
      update_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
      continue
    fi

    if [ ! -s "$AUTO_NEW_APPS_BASELINE_FILE" ]; then
      refresh_auto_new_apps_baseline || {
        log -p w -t Boot "skip new app auto-enable: baseline unavailable"
        continue
      }
      remove_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
    fi

    handle_auto_new_app_added_for_user "$user_id" "$package_name" "$changed_file"
  done

  log_hot_reload_packages_from_file "$changed_file" "new-app-auto-enable"
  rm -f "$changed_file"
}

handle_package_replaced_event() {
  event_user_id="$1"
  package_name="$2"
  sleep 2
  refresh_uid_map force
  user_ids=$(resolve_package_event_user_ids "$event_user_id" "$package_name")
  for user_id in $user_ids; do
    update_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
  done
}

handle_package_removed_event() {
  event_user_id="$1"
  package_name="$2"
  replacing="$3"
  [ "$replacing" = "1" ] && return 0
  refresh_uid_map force

  user_ids=$(resolve_package_event_user_ids "$event_user_id" "$package_name")
  if [ -z "$user_ids" ] && is_valid_user_id "$event_user_id"; then
    user_ids="$event_user_id"
  fi
  for user_id in $user_ids; do
    remove_auto_new_apps_baseline_entry "$user_id" "$package_name" >/dev/null 2>&1 || true
  done
  sync_uninstalled_app_configs
}

handle_package_event_line() {
  event_line="$1"
  case "$event_line" in
    srx-package-event-v1"|"*) ;;
    *) return 0 ;;
  esac

  IFS='|' read -r event_version event_action event_user_id event_uid event_replacing event_package_name event_time <<EOF
$event_line
EOF
  [ "$event_version" = "srx-package-event-v1" ] || return 0
  is_safe_package_name "$event_package_name" || return 0

  case "$event_action" in
    added)
      handle_package_added_event "$event_user_id" "$event_package_name" "$event_replacing"
      ;;
    replaced)
      handle_package_replaced_event "$event_user_id" "$event_package_name"
      ;;
    removed|fully_removed)
      handle_package_removed_event "$event_user_id" "$event_package_name" "$event_replacing"
      ;;
  esac
}

process_package_event_log_delta() {
  ensure_log_file "$PACKAGE_EVENT_LOG_FILE"
  size=$(get_file_size "$PACKAGE_EVENT_LOG_FILE")
  offset=$(cat "$PACKAGE_EVENT_OFFSET_FILE" 2>/dev/null)
  offset=${offset:-}
  if ! echo "$offset" | grep -Eq '^[0-9][0-9]*$'; then
    echo "$size" > "$PACKAGE_EVENT_OFFSET_FILE"
    chmod 644 "$PACKAGE_EVENT_OFFSET_FILE" 2>/dev/null
    return 0
  fi
  if [ "$offset" -gt "$size" ]; then
    offset=0
  fi
  if [ "$size" -le "$offset" ]; then
    return 0
  fi

  start_byte=$((offset + 1))
  delta_file="$LOGS_DIR/.package_events.delta.$$"
  if ! tail -c +"$start_byte" "$PACKAGE_EVENT_LOG_FILE" > "$delta_file" 2>/dev/null; then
    dd if="$PACKAGE_EVENT_LOG_FILE" bs=1 skip="$offset" 2>/dev/null > "$delta_file"
  fi
  while IFS= read -r event_line; do
    handle_package_event_line "$event_line"
  done < "$delta_file"
  rm -f "$delta_file"

  echo "$size" > "$PACKAGE_EVENT_OFFSET_FILE"
  chmod 644 "$PACKAGE_EVENT_OFFSET_FILE" 2>/dev/null
  rotate_log_file_if_needed "$PACKAGE_EVENT_LOG_FILE" "$MAX_PACKAGE_EVENT_LOG_BYTES" "$LOG_ROTATE_BACKUPS"
}

get_package_install_state() {
  package_name="$1"
  [ -n "$package_name" ] || {
    echo "unknown"
    return 0
  }
  cmd package path "$package_name" >/dev/null 2>&1 && {
    echo "installed"
    return 0
  }

  user_ids=$(cmd user list 2>/dev/null | sed -n 's/.*{\([0-9][0-9]*\):.*/\1/p' | sort -u)
  if [ -z "$user_ids" ]; then
    echo "unknown"
    return 0
  fi

  for user_id in $user_ids; do
    package_lines=$(cmd package list packages --user "$user_id" "$package_name" 2>/dev/null)
    query_status=$?
    if [ "$query_status" -ne 0 ]; then
      echo "unknown"
      return 0
    fi
    echo "$package_lines" | grep -qx "package:$package_name" && {
      echo "installed"
      return 0
    }
  done

  echo "missing"
}

disable_package_config() {
  package_name="$1"
  config_file="$APPS_CONFIG_DIR/$package_name.json"
  tmp_file="$config_file.tmp"
  [ -f "$config_file" ] || return 1
  grep -Eq '"enabled"[[:space:]]*:[[:space:]]*true' "$config_file" 2>/dev/null || return 1

  sed 's/"enabled"[[:space:]]*:[[:space:]]*true/"enabled": false/g' "$config_file" > "$tmp_file" 2>/dev/null || {
    rm -f "$tmp_file"
    return 1
  }
  mv "$tmp_file" "$config_file" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$config_file" 2>/dev/null
  # 卸载禁用标记：配置文件保留（用户自定义规则不丢），但重装时需要据此区分
  # 「卸载流程关的」与「用户手工关的」——前者在 added 事件时恢复 enabled，
  # 后者保持 disabled 不受打扰。只有卸载流程会产生这个标记（唯一调用点）。
  : > "$config_file.uninstall_disabled"
  return 0
}

restore_uninstall_disabled_config() {
  package_name="$1"
  config_file="$APPS_CONFIG_DIR/$package_name.json"
  marker_file="$config_file.uninstall_disabled"
  [ -f "$marker_file" ] || return 1
  rm -f "$marker_file"
  [ -f "$config_file" ] || return 1
  grep -Eq '"enabled"[[:space:]]*:[[:space:]]*false' "$config_file" 2>/dev/null || return 0

  tmp_file="$config_file.tmp"
  sed 's/"enabled"[[:space:]]*:[[:space:]]*false/"enabled": true/g' "$config_file" > "$tmp_file" 2>/dev/null || {
    rm -f "$tmp_file"
    return 1
  }
  mv "$tmp_file" "$config_file" || {
    rm -f "$tmp_file"
    return 1
  }
  chmod 644 "$config_file" 2>/dev/null
  log -p i -t Boot "restored config disabled by uninstall: pkg=$package_name"
  return 0
}

sync_uninstalled_app_configs() {
  [ -d "$APPS_CONFIG_DIR" ] || return 0
  if ! is_package_query_ready; then
    log -p w -t Boot "skip uninstalled config sync: package query unavailable"
    return 0
  fi

  changed_file="$LOGS_DIR/.uninstalled_config_packages.tmp"
  : > "$changed_file"
  refresh_uid_map force

  for config_file in "$APPS_CONFIG_DIR"/*.json; do
    [ -f "$config_file" ] || continue
    package_name=$(basename "$config_file" .json)
    [ -n "$package_name" ] || continue
    is_skipped_package "$package_name" && continue
    is_effective_package_config "$package_name" || continue

    install_state=$(get_package_install_state "$package_name")
    if [ "$install_state" = "installed" ] || [ "$install_state" = "unknown" ]; then
      [ "$install_state" = "unknown" ] && log -p w -t Boot "skip uninstalled config sync for unknown package state: pkg=$package_name"
      continue
    fi

    if disable_package_config "$package_name"; then
      echo "$package_name" >> "$changed_file"
      log -p i -t Boot "disable config for uninstalled package: pkg=$package_name"
    fi
  done

  log_hot_reload_packages_from_file "$changed_file" "uninstalled-sync"
  rm -f "$changed_file"
}

build_config_state_file() {
  output_file="$1"
  : > "$output_file"
  if [ ! -d "$APPS_CONFIG_DIR" ]; then
    return 0
  fi

  for config_file in "$APPS_CONFIG_DIR"/*.json; do
    [ -f "$config_file" ] || continue
    package_name=$(basename "$config_file" .json)
    [ -n "$package_name" ] || continue
    mtime=$(stat -c '%Y' "$config_file" 2>/dev/null)
    size=$(stat -c '%s' "$config_file" 2>/dev/null)
    mtime=${mtime:-0}
    size=${size:-0}
    if is_effective_package_config "$package_name"; then
      effective=1
    else
      effective=0
    fi
    # 状态行格式：包名|mtime|size|是否生效(0/1)
    echo "$package_name|$mtime|$size|$effective" >> "$output_file"
  done

  sort -o "$output_file" "$output_file" 2>/dev/null
}

get_state_line() {
  state_file="$1"
  package_name="$2"
  awk -F'|' -v target="$package_name" '$1 == target { print; exit }' "$state_file" 2>/dev/null
}

get_state_effective() {
  state_line="$1"
  if [ -z "$state_line" ]; then
    echo 0
    return 0
  fi
  echo "$state_line" | awk -F'|' '{print $4 + 0}'
}

get_state_signature() {
  state_line="$1"
  if [ -z "$state_line" ]; then
    echo ""
    return 0
  fi
  echo "$state_line" | awk -F'|' '{print $2 "|" $3}'
}

should_queue_hot_reload_by_state() {
  old_line="$1"
  new_line="$2"
  old_effective=$(get_state_effective "$old_line")
  new_effective=$(get_state_effective "$new_line")

  # 生效配置仍为生效配置时，仅在内容变化后排队热重载。
  if [ "$old_effective" -eq 1 ] && [ "$new_effective" -eq 1 ]; then
    old_sig=$(get_state_signature "$old_line")
    new_sig=$(get_state_signature "$new_line")
    [ "$old_sig" != "$new_sig" ]
    return $?
  fi

  # 生效状态发生切换（启用/禁用/删除）时排队热重载。
  if [ "$old_effective" -eq 1 ] || [ "$new_effective" -eq 1 ]; then
    return 0
  fi
  # 非生效配置变更不触发热重载，避免无意义协调运行进程。
  return 1
}

get_uid_by_package() {
  package_name="$1"
  awk -F':' -v target="$package_name" '$1 == target { print $2; exit }' "$SYSTEM_WRITER_UIDS_FILE" 2>/dev/null
}

get_packages_by_uid() {
  target_uid="$1"
  awk -F':' -v uid="$target_uid" '$2 == uid { print $1 }' "$SYSTEM_WRITER_UIDS_FILE" 2>/dev/null
}

log_hot_reload_packages_from_file() {
  package_file="$1"
  reason="$2"
  [ -s "$package_file" ] || return 0
  sort -u -o "$package_file" "$package_file" 2>/dev/null
  while IFS= read -r package_name; do
    [ -n "$package_name" ] || continue
    log -p i -t Boot "config hot-reload queued: pkg=$package_name reason=$reason"
  done < "$package_file"
}

handle_config_changes() {
  changed_packages="$1"
  old_state_file="$2"
  new_state_file="$3"
  reload_list_file="$LOGS_DIR/.config_reload_packages.tmp"
  : > "$reload_list_file"

  # UID 映射对账已上移到事件批入口（见 start_config_event_collector）：
  # 每批事件无条件 refresh，避免重装窗口的坏清单在 changed 为空时无法自愈。

  for package_name in $changed_packages; do
    is_skipped_package "$package_name" && continue

    old_line=$(get_state_line "$old_state_file" "$package_name")
    new_line=$(get_state_line "$new_state_file" "$package_name")
    should_reload_source=0
    if should_queue_hot_reload_by_state "$old_line" "$new_line"; then
      should_reload_source=1
      echo "$package_name" >> "$reload_list_file"
    fi

    uid=$(get_uid_by_package "$package_name")
    if [ -z "$uid" ]; then
      continue
    fi

    # 共享 UID 只扩展到有配置记录的包，避免误伤同 UID 的无关系统组件。
    for shared_package in $(get_packages_by_uid "$uid"); do
      [ "$shared_package" = "$package_name" ] && continue
      is_skipped_package "$shared_package" && continue

      shared_old_line=$(get_state_line "$old_state_file" "$shared_package")
      shared_new_line=$(get_state_line "$new_state_file" "$shared_package")
      if [ -z "$shared_old_line" ] && [ -z "$shared_new_line" ]; then
        continue
      fi

      # 同 UID 配置保持生效时，来源包生效变更也会触发热重载，避免进程状态不一致。
      if should_queue_hot_reload_by_state "$shared_old_line" "$shared_new_line"; then
        echo "$shared_package" >> "$reload_list_file"
        continue
      fi

      if [ "$should_reload_source" -eq 1 ]; then
        shared_old_effective=$(get_state_effective "$shared_old_line")
        shared_new_effective=$(get_state_effective "$shared_new_line")
        if [ "$shared_old_effective" -eq 1 ] || [ "$shared_new_effective" -eq 1 ]; then
          echo "$shared_package" >> "$reload_list_file"
        fi
      fi
    done
  done

  if [ ! -s "$reload_list_file" ]; then
    rm -f "$reload_list_file"
    return 0
  fi

  log_hot_reload_packages_from_file "$reload_list_file" "config-change"
  rm -f "$reload_list_file"
}

# 包事件慢路径：只在「长周期自愈复查」或「receiver 不可用」时执行。
#
# 此前这段工作放在 5 秒一轮的主循环里无条件执行，等于长期每秒 fork/exec 十几次
# （awk/grep/tr/sed/kill…）并 1 Hz 唤醒 CPU，是整机空闲期最重的一处常驻开销。
# 现在只在 inotifyd 超时（就绪时默认 60 秒一次）或 receiver 缺失时才会走到这里。
package_event_slow_path() {
  # 非 force 的 UID 映射对账自带 60 秒节流：保证「完全没有文件事件」时清单也能在
  # 一个节拍内自愈，不依赖安装/配置事件。
  refresh_uid_map
  if ! is_auto_new_apps_package_poll_eligible; then
    return 0
  fi
  if [ "${AUTO_NEW_APPS_PACKAGE_POLL_REQUIRE_INTERACTIVE:-0}" = "1" ] &&
    ! is_device_interactive_for_package_poll; then
    return 0
  fi
  log -p i -t Boot "run new app package poll fallback: default receiver not ready"
  poll_auto_new_apps_from_package_list
}

# 评估本轮等待预算：receiver 就绪（正常情形）时只做长周期自愈；未就绪时才需要
# 兜底轮询。兜底轮询的成本是每次 `cmd package list` 一次 binder 往返（system_server
# 侧要枚举全部包），2 秒一次的代价远高于「新装应用晚几秒被自动开启」，因此
# 未就绪时取 10 秒：仍保证开机/装应用后一个节拍内自动开启。
# 同时记录绝对截止时刻（uptime）：LOGS_DIR 里 running/media/debug 等诊断日志的
# 写入也会唤醒监听，若每次唤醒都把 read 超时重新计满，持续的诊断采样会让自愈
# 周期永远不到。截止时刻只在真正超时后重新评估，不被无关事件重置。
compute_package_event_wait_budget() {
  if is_package_event_receiver_ready; then
    event_wait_budget="${PACKAGE_EVENT_IDLE_WAIT_SECONDS:-1800}"
  else
    event_wait_budget="${PACKAGE_EVENT_FALLBACK_WAIT_SECONDS:-10}"
  fi
  deadline_now=$(get_uptime_seconds)
  case "$deadline_now" in
    ''|*[!0-9]*) event_wait_deadline=0 ;;
    *) event_wait_deadline=$((deadline_now + event_wait_budget)) ;;
  esac
}

start_package_event_collector() {
  # 上一次 boot 或异常退出可能留下失效 FIFO，文件名带 pid 不会自愈，
  # 不清理会无限累积；这里只删管道类型，避免误删同名普通文件。
  for stale_notify_fifo in "$MODDIR"/.package_event_notify.*; do
    [ -p "$stale_notify_fifo" ] || continue
    rm -f "$stale_notify_fifo"
  done
  (
    mkdir -p "$APPS_CONFIG_DIR"
    ensure_log_file "$PACKAGE_EVENT_LOG_FILE"
    sync_auto_new_apps_startup_state
    sync_uninstalled_app_configs

    # 事件驱动：整个生命周期只建一次 FIFO，主循环阻塞在 `read -t` 上，由 inotifyd
    # 的目录事件唤醒。此前是「每 5 秒重建 FIFO + 重启一次 inotifyd + 1 秒粒度
    # read 循环」，稳态下约 3 次 fork/exec 每秒且恒定 1 Hz 唤醒；现在空闲期只剩
    # 30 分钟一次的自愈复查（见 package_event_slow_path）——它只兜「receiver 静默
    # 死亡」「uid 映射清单长期无人刷新」这类没有事件可依赖的退化，不需要更高频率：
    # 安装/卸载本身走包事件日志，配置改动走 inotifyd，正常路径一次都不依赖它。
    #
    # FIFO 不放在 LOGS_DIR 内，否则目录监听会被自身创建/删除事件反复唤醒；只在启动
    # 时建一次，也让这类自激事件从「每 5 秒一轮」变成「整个生命周期一次」。
    notification_fifo="$MODDIR/.package_event_notify.$$"
    rm -f "$notification_fifo"
    # 必须自己同时持有读写两端再启动监听进程：裸输入重定向的 open(O_RDONLY) 在对端
    # 缺失时会无限期阻塞（wait_for_partner），而 `read -t` 的超时管不到 open。一旦
    # inotifyd 未能在本上下文启动，收集器就永久卡住，新装应用自动开启、卸载清理、
    # 基线更新与热重载全部静默失效。自持两端后 open 永不阻塞。
    watcher_ready=0
    if mkfifo "$notification_fifo" 2>/dev/null; then
      if exec 3<>"$notification_fifo" 2>/dev/null; then
        watcher_ready=1
      fi
    fi
    if [ "$watcher_ready" -ne 1 ]; then
      log -p w -t Boot "package event watcher unavailable; legacy polling path"
      rm -f "$notification_fifo"
      # 退化路径（极旧系统/无 mkfifo）：保留原有的低频轮询，功能不降级。
      while true; do
        package_event_slow_path
        process_package_event_log_delta
        sleep "${PACKAGE_EVENT_COLLECTOR_SLEEP_SECONDS:-2}"
      done
    fi

    while true; do
      # w=写入关闭 c=内容修改 m=新建 d=删除 n=移入
      inotifyd - "$LOGS_DIR":wcmdn >&3 2>/dev/null &
      watcher_pid="$!"

      # 等待预算与绝对截止时刻：见 compute_package_event_wait_budget。
      compute_package_event_wait_budget

      while true; do
        # 每轮 read 的超时从绝对截止时刻倒推剩余时间：无关日志事件只消耗剩余
        # 时间，不重置它，持续的诊断写入不会把慢路径自愈饿死。uptime 不可读的
        # 极端环境退回「每次事件后重新计满」的旧行为（功能不受影响）。
        event_wait_seconds="$event_wait_budget"
        if [ "$event_wait_deadline" -gt 0 ]; then
          deadline_now=$(get_uptime_seconds)
          case "$deadline_now" in
            ''|*[!0-9]*) ;;
            *)
              event_wait_seconds=$((event_wait_deadline - deadline_now))
              [ "$event_wait_seconds" -lt 1 ] && event_wait_seconds=1
              ;;
          esac
        fi

        event_line=""
        if IFS= read -t "$event_wait_seconds" -r event_line <&3; then
          case "$event_line" in
            *"package_events.log"*|*".package_event_receiver_ready"*) ;;
            # 目录里还有 running/media/debug 等诊断日志：它们的事件不触发业务处理，
            # 否则诊断采样会把收集器重新变成热循环。截止时刻已到时仍执行慢路径，
            # 避免持续高频无关事件让 read 永远不等到超时返回。
            *)
              if [ "$event_wait_deadline" -gt 0 ]; then
                deadline_now=$(get_uptime_seconds)
                case "$deadline_now" in
                  ''|*[!0-9]*) ;;
                  *)
                    if [ "$deadline_now" -ge "$event_wait_deadline" ]; then
                      package_event_slow_path
                      process_package_event_log_delta
                      compute_package_event_wait_budget
                    fi
                    ;;
                esac
              fi
              continue
              ;;
          esac
          # 去抖：合并 1 秒内的后续事件一起处理。轮数必须封顶——去抖窗口若被
          # 持续到达的事件无限延长，等待中的事件处理会被无限推迟（与超时被重置
          # 是同一类饿死问题）；封顶之后超出窗口的事件按 offset 在下一批补读。
          debounce_rounds=0
          while [ "$debounce_rounds" -lt 16 ] && IFS= read -t 1 -r _ <&3; do
            debounce_rounds=$((debounce_rounds + 1))
          done
          refresh_uid_map
          process_package_event_log_delta
          continue
        fi
        # 超时：到达绝对截止时刻后的自愈复查，并重新评估预算与截止时刻。这里的
        # 周期是「没有相关事件时的兜底」，事件到达时会立刻被上面的分支处理，且
        # 不重置截止时刻——只有真正等满一个周期才续期。
        package_event_slow_path
        process_package_event_log_delta
        compute_package_event_wait_budget
        if ! kill -0 "$watcher_pid" 2>/dev/null; then
          # 监听进程已退出：跳出内层重启它。退出期间的包事件不会丢——消费侧按
          # offset 读日志文件本身，重启后一次补读即可。
          break
        fi
      done

      kill "$watcher_pid" 2>/dev/null || true
      wait "$watcher_pid" 2>/dev/null || true
      log -p w -t Boot "package event watcher exited; restarting after backoff"
      sleep 5
    done
  ) &
  echo "$!" > "$PACKAGE_EVENT_COLLECTOR_PID_FILE"
  chmod 644 "$PACKAGE_EVENT_COLLECTOR_PID_FILE"
}

start_config_event_collector() {
  (
    tmp_old_state="$LOGS_DIR/.config_apps_state.old"
    tmp_new_state="$LOGS_DIR/.config_apps_state.new"
    # 启动时建立状态基线，避免把旧配置当作新增事件处理。
    build_config_state_file "$CONFIG_STATE_FILE"
    mkdir -p "$APPS_CONFIG_DIR"

    # inotifyd 监听配置目录，事件通过管道传给处理循环。
    # w=写入关闭 c=内容修改 m=新建 d=删除 n=移入
    while true; do
      inotifyd - "$APPS_CONFIG_DIR":wcmdn "$CONFIG_DIR":wcmdn 2>/dev/null |
      while IFS= read -r event_line; do
        # 仅处理 .json 文件事件
        case "$event_line" in
          *".json") ;;
          *) continue ;;
        esac

        # 去抖：读取 1 秒内的后续事件一起处理
        while read -t 1 -r _; do :; done

        if command -v sync_monitor_collector >/dev/null 2>&1; then
          sync_monitor_collector
        fi
        if command -v sync_debug_collectors >/dev/null 2>&1; then
          # 收敛放在子 shell 里执行：采集器启动会碰 logcat/dumpsys 等外部命令，
          # 任何异常退出都不能连带结束这个事件循环——它现在是采集器状态的唯一
          # 驱动源（周期看门狗已按规范移除），循环一死配置事件就再无人处理。
          ( sync_debug_collectors ) || log_collector_status "sync failed rc=$?"
        fi

        case "$event_line" in
          *"global.json")
            continue
            ;;
        esac

        if [ -f "$CONFIG_STATE_FILE" ]; then
          cp "$CONFIG_STATE_FILE" "$tmp_old_state"
        else
          : > "$tmp_old_state"
        fi

        build_config_state_file "$tmp_new_state"
        # 每批配置事件都强制对账 UID 映射，与包事件 receiver 是否就绪解耦：
        # 应用重装（卸载→安装）窗口内的任何一次 refresh 都会把「缺少该包」的
        # 清单落盘，而 uid 反查（MediaProvider 侧改写 MediaStore 落点）只认清单。
        # 若重装后的配置内容与旧状态一致（changed 为空），原先仅在变更处理里
        # 才触发的刷新不会执行，坏清单会一直残留到下一次真实配置变化。
        # 事件批入口无条件对账，保证「写配置即修正清单」；真机上 receiver 正常
        # 时包事件通道（added/replaced 后 2 秒对账）仍作为即时通道保留。
        refresh_uid_map force
        changed_packages=$(awk -F'|' '
          NR == FNR { old[$1] = $0; next }
          {
            new[$1] = $0;
            if (!( $1 in old ) || old[$1] != $0) {
              print $1;
            }
          }
          END {
            for (pkg in old) {
              if (!(pkg in new)) {
                print pkg;
              }
            }
          }
        ' "$tmp_old_state" "$tmp_new_state" | sort -u)
        if [ -n "$changed_packages" ]; then
          handle_config_changes "$changed_packages" "$tmp_old_state" "$tmp_new_state"
        fi
        mv "$tmp_new_state" "$CONFIG_STATE_FILE"
      done
      # inotifyd 异常退出时等待后重启
      sleep 5
    done
  ) &
  echo "$!" > "$CONFIG_EVENT_COLLECTOR_PID_FILE"
  chmod 644 "$CONFIG_EVENT_COLLECTOR_PID_FILE"
}
