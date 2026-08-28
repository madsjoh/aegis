runner_builder_error() {
  printf '%s\n' "$1" >&2
  printf 'Run `%s` to configure or repair the builder.\n' "$AEGIS_BUILDER_SETUP_COMMAND" >&2
  return 1
}

runner_verify_builder_health() {
  local builder_directory="$1"
  local private_key="$builder_directory/id_ed25519"
  local known_hosts_file="${AEGIS_BUILDER_KNOWN_HOSTS_FILE:-}"
  local remove_known_hosts=false
  local ssh_options=()

  if [ -z "$known_hosts_file" ]; then
    known_hosts_file="$(mktemp)"
    remove_known_hosts=true
  fi
  printf '[%s]:%s %s\n' "$AEGIS_BUILDER_HOST" "$AEGIS_BUILDER_PORT" "$AEGIS_BUILDER_HOST_KEY" > "$known_hosts_file"
  ssh_options=(
    -i "$private_key"
    -o BatchMode=yes
    -o IdentitiesOnly=yes
    -o StrictHostKeyChecking=yes
    -o UserKnownHostsFile="$known_hosts_file"
    -o ConnectTimeout=2
    -p "$AEGIS_BUILDER_PORT"
  )

  if ! ssh "${ssh_options[@]}" "builder@$AEGIS_BUILDER_HOST" true; then
    [ "$remove_known_hosts" != true ] || rm --force "$known_hosts_file"
    runner_builder_error "Aegis builder SSH authentication failed."
    return
  fi
  if ! ssh "${ssh_options[@]}" "builder@$AEGIS_BUILDER_HOST" nix store ping --store daemon; then
    [ "$remove_known_hosts" != true ] || rm --force "$known_hosts_file"
    runner_builder_error "The Aegis builder remote Nix daemon is unhealthy."
    return
  fi

  [ "$remove_known_hosts" != true ] || rm --force "$known_hosts_file"
}

runner_validate_builder_registration() {
  local configuration_directory="$1"
  local private_key="$2"
  local host_key_blob="$3"
  local actual_value=""
  local include_count=0
  local machine_host_key=""
  local machine_jobs=""
  local machine_key=""
  local machine_mandatory=""
  local machine_speed=""
  local machine_supported=""
  local machine_system=""
  local machine_uri=""
  local setting_count=0
  local total_setting_count=0
  local trailing_field=""

  include_count="$(builder_active_include_count "$configuration_directory/nix.conf")"
  if [ "$include_count" -ne 1 ]; then
    runner_builder_error "nix.conf must contain exactly one active !include /etc/nix/aegis.conf line."
    return
  fi

  total_setting_count="$(awk '
    /^[[:space:]]*($|#)/ { next }
    { count++ }
    END { print count + 0 }
  ' "$configuration_directory/aegis.conf")"
  if [ "$total_setting_count" -ne 4 ]; then
    runner_builder_error "aegis.conf may contain only the four required active Aegis settings."
    return
  fi

  while IFS='|' read -r setting expected_value; do
    setting_count="$(awk -F '=' -v key="$setting" '
      /^[[:space:]]*#/ { next }
      {
        name = $1
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
        if (name == key) count++
      }
      END { print count + 0 }
    ' "$configuration_directory/aegis.conf")"
    actual_value="$(awk -F '=' -v key="$setting" '
      /^[[:space:]]*#/ { next }
      {
        name = $1
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
        if (name == key) {
          value = substr($0, index($0, "=") + 1)
          sub(/[[:space:]]+#.*$/, "", value)
          gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
          print value
        }
      }
    ' "$configuration_directory/aegis.conf")"
    if [ "$setting_count" -ne 1 ] || [ "$actual_value" != "$expected_value" ]; then
      runner_builder_error "aegis.conf must define exactly one active $setting setting with the required value."
      return
    fi
  done <<'EOF'
builders|@/etc/nix/aegis-machines
builders-use-substitutes|true
extra-substituters|https://cache.nixos.org https://microvm.cachix.org
extra-trusted-public-keys|cache.nixos.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0= microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=
EOF

  read -r machine_uri machine_system machine_key machine_jobs machine_speed machine_supported machine_mandatory machine_host_key trailing_field \
    < "$configuration_directory/aegis-machines" || true
  if [ "$machine_uri" != "ssh-ng://builder@$AEGIS_BUILDER_HOST:$AEGIS_BUILDER_PORT" ] \
    || [ "$machine_system" != aarch64-linux ] \
    || [ "$machine_key" != "$private_key" ] \
    || [ "$machine_jobs" != 1 ] \
    || [ "$machine_speed" != 1 ] \
    || [ "$machine_supported" != - ] \
    || [ "$machine_mandatory" != - ] \
    || [ "$machine_host_key" != "$host_key_blob" ] \
    || [ -n "$trailing_field" ] \
    || [ "$(wc --lines < "$configuration_directory/aegis-machines")" -ne 1 ]; then
    runner_builder_error "aegis-machines does not contain the required host, port, system, private key path, and pinned key."
    return
  fi
}

runner_prepare_builder() {
  local builder_directory="${XDG_DATA_HOME:-$HOME/.local/share}/aegis/builder"
  local private_key="$builder_directory/id_ed25519"
  local public_key_file="$builder_directory/id_ed25519.pub"
  local host_key_file="$builder_directory/host-key.pub"
  local configuration_directory="${AEGIS_BUILDER_CONFIGURATION_DIRECTORY:-/etc/nix}"
  local builder_output=""
  local host_key_blob=""

  if [ ! -f "$private_key" ] \
    || [ ! -f "$public_key_file" ] \
    || [ ! -f "$host_key_file" ] \
    || [ ! -f "$configuration_directory/aegis-machines" ] \
    || [ ! -f "$configuration_directory/aegis.conf" ]; then
    runner_builder_error "The Aegis builder has not been configured."
    return
  fi

  AEGIS_BUILDER_PUBLIC_KEY_FILE="$public_key_file"
  AEGIS_BUILDER_HOST_KEY="$(<"$host_key_file")"
  builder_load_configuration || return
  host_key_blob="$(printf '%s\n' "$AEGIS_BUILDER_HOST_KEY" | cut -d ' ' -f 2)"
  if [ -z "$host_key_blob" ]; then
    runner_builder_error "The configured Aegis host key is invalid."
    return
  fi
  export AEGIS_BUILDER_CONTEXT AEGIS_BUILDER_CONTEXT_IDENTITY AEGIS_BUILDER_HOST_KEY AEGIS_BUILDER_PUBLIC_KEY_FILE AEGIS_BUILDER_VERSION

  runner_validate_builder_registration "$configuration_directory" "$private_key" "$host_key_blob" || return

  if ! builder_output="$(ensure_builder 2>&1)"; then
    if [ -n "$builder_output" ]; then
      printf '%s\n' "$builder_output" >&2
    fi
    runner_builder_error "The Aegis builder is unavailable."
    return
  fi

  runner_verify_builder_health "$builder_directory"
}
