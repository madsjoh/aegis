builder_setup_error() {
  printf 'aegis builder setup: %s\n' "$1" >&2
  return 1
}

builder_setup_as_root() {
  sudo "$@"
}

builder_setup_scan_host_key() {
  local scanned_key=""
  local attempts=0

  while [ "$attempts" -lt 30 ]; do
    scanned_key="$(ssh-keyscan -t ed25519 -p "$AEGIS_BUILDER_PORT" "$AEGIS_BUILDER_HOST" 2>/dev/null || true)"
    if [ -n "$scanned_key" ]; then
      break
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  if [ -z "$scanned_key" ]; then
    builder_setup_error "SSH did not provide an Ed25519 host key."
    return
  fi

  printf '%s\n' "$scanned_key" | cut -d ' ' -f 2- | grep '^ssh-ed25519 ' | head --lines=1
}

builder_setup_generate_credentials() {
  local key_directory="$1"
  local private_key="$key_directory/id_ed25519"

  mkdir --parents "$key_directory"
  chmod 0700 "$key_directory"
  if [ ! -f "$private_key" ] || [ ! -f "$private_key.pub" ]; then
    if [ -e "$private_key" ] || [ -e "$private_key.pub" ]; then
      builder_setup_error "Builder credentials are incomplete in $key_directory."
      return
    fi
    ssh-keygen -q -t ed25519 -N '' -f "$private_key"
  fi
  chmod 0600 "$private_key"
  chmod 0644 "$private_key.pub"
}

builder_setup_active_include_count() {
  builder_active_include_count "$1"
}

builder_setup_has_active_include() {
  [ "$(builder_setup_active_include_count "$1")" -gt 0 ]
}

builder_setup_install_file() {
  local source_file="$1"
  local destination_file="$2"
  local mode="$3"
  local temporary_file="${destination_file}.aegis-new"

  if ! builder_setup_as_root install -m "$mode" "$source_file" "$temporary_file"; then
    builder_setup_as_root rm -f "$temporary_file" || true
    return 1
  fi
  if ! builder_setup_as_root mv -f "$temporary_file" "$destination_file"; then
    builder_setup_as_root rm -f "$temporary_file" || true
    return 1
  fi
}

builder_setup_backup_file() {
  local source_file="$1"
  local backup_file="$2"

  if [ -e "$source_file" ]; then
    builder_setup_as_root cat "$source_file" > "$backup_file" || return
    if ! builder_setup_as_root stat -f '%Lp' "$source_file" > "$backup_file.mode" 2>/dev/null; then
      builder_setup_as_root stat --format='%a' "$source_file" > "$backup_file.mode" || return
    fi
    printf '%s\n' present > "$backup_file.state"
  else
    printf '%s\n' absent > "$backup_file.state"
  fi
}

builder_setup_restore_file() {
  local destination_file="$1"
  local backup_file="$2"

  if [ "$(<"$backup_file.state")" = present ]; then
    builder_setup_as_root install -m "$(<"$backup_file.mode")" "$backup_file" "$destination_file"
  else
    builder_setup_as_root rm -f "$destination_file"
  fi
}

builder_setup_backup_user_file() {
  local source_file="$1"
  local backup_file="$2"

  if [ -e "$source_file" ]; then
    cp -p "$source_file" "$backup_file" || return
    printf '%s\n' present > "$backup_file.state"
  else
    printf '%s\n' absent > "$backup_file.state"
  fi
}

builder_setup_restore_user_file() {
  local destination_file="$1"
  local backup_file="$2"

  if [ "$(<"$backup_file.state")" = present ]; then
    cp -p "$backup_file" "$destination_file.aegis-new" || return
    mv -f "$destination_file.aegis-new" "$destination_file"
  else
    rm -f "$destination_file" "$destination_file.aegis-new"
  fi
}

builder_setup_install_user_file() {
  local source_file="$1"
  local destination_file="$2"

  install -m 0644 "$source_file" "$destination_file.aegis-new" || {
    rm -f "$destination_file.aegis-new"
    return 1
  }
  mv -f "$destination_file.aegis-new" "$destination_file"
}

builder_setup_rollback() {
  local configuration_directory="$1"
  local staging_directory="$2"
  local include_added="$3"
  local host_key_file="${4:-}"

  builder_setup_restore_file "$configuration_directory/aegis-machines" "$staging_directory/backup-aegis-machines" || true
  builder_setup_restore_file "$configuration_directory/aegis.conf" "$staging_directory/backup-aegis.conf" || true
  if [ "$include_added" = true ]; then
    builder_setup_restore_file "$configuration_directory/nix.conf" "$staging_directory/backup-nix.conf" || true
  fi
  if [ -n "$host_key_file" ] && [ -f "$staging_directory/backup-host-key.state" ]; then
    builder_setup_restore_user_file "$host_key_file" "$staging_directory/backup-host-key" || true
  fi
  builder_setup_as_root launchctl kickstart -k system/org.nixos.nix-daemon >/dev/null 2>&1 || true
}

builder_setup_validate_host_key() {
  local host_key_blob="$1"
  local staging_directory="$2"
  local encoded_key="$staging_directory/host-key.base64"
  local decoded_key="$staging_directory/host-key.decoded"
  local public_key="$staging_directory/host-key.pub"

  printf '%s' "$host_key_blob" > "$encoded_key"
  if ! base64 --decode < "$encoded_key" > "$decoded_key" 2>/dev/null \
    && ! base64 -D < "$encoded_key" > "$decoded_key" 2>/dev/null; then
    builder_setup_error "The builder host key is not valid base64."
    return
  fi
  if [ ! -s "$decoded_key" ]; then
    builder_setup_error "The builder host key decodes to an empty value."
    return
  fi
  printf 'ssh-ed25519 %s\n' "$host_key_blob" > "$public_key"
  if ! ssh-keygen -l -f "$public_key" >/dev/null 2>&1; then
    builder_setup_error "The builder host key is not a valid Ed25519 public key."
  fi
}

builder_setup_wait_for_daemon() {
  local attempts=0

  while [ "$attempts" -lt 30 ]; do
    if nix --store daemon store ping >/dev/null 2>&1; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  builder_setup_error "The Nix daemon did not become ready after reload."
}

builder_setup_verify_remote_build() {
  local configuration_directory="$1"
  local nonce=""
  local output_path=""
  local output_value=""

  nonce="$(date +%s)-$$-$RANDOM"
  output_path="$(nix --store daemon build \
    --builders "@$configuration_directory/aegis-machines" \
    --impure \
    --max-jobs 0 \
    --no-link \
    --print-out-paths \
    --expr "derivation { name = \"aegis-builder-test-$nonce\"; system = \"aarch64-linux\"; builder = \"/bin/sh\"; args = [ \"-c\" \"mkdir -p \\\$out && printf verified > \\\$out/value\" ]; }" \
  )" || return
  output_value="$(nix --store daemon store cat "$output_path/value")" || return
  if [ "$output_value" != verified ]; then
    builder_setup_error "The remote builder produced an unexpected verification result."
  fi
}

builder_setup_transaction() (
  local configuration_directory="$1"
  local private_key="$2"
  local host_key_blob="$3"
  local staging_directory="$4"
  local host_key_file="$5"
  local include_added=false
  local installation_begun=false
  local transaction_committed=false

  set -o errexit

  builder_setup_transaction_cleanup() {
    local status="$?"

    trap - EXIT INT TERM
    if [ "$installation_begun" = true ] && [ "$transaction_committed" != true ]; then
      builder_setup_rollback "$configuration_directory" "$staging_directory" "$include_added" "$host_key_file"
    fi
    builder_setup_as_root rm -f \
      "$configuration_directory/aegis-machines.aegis-new" \
      "$configuration_directory/aegis.conf.aegis-new" \
      "$host_key_file.aegis-new" || true
    rm --force --recursive "$staging_directory"
    exit "$status"
  }

  trap builder_setup_transaction_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  printf '%s\n' \
    "ssh-ng://builder@$AEGIS_BUILDER_HOST:$AEGIS_BUILDER_PORT aarch64-linux $private_key 1 1 - - $host_key_blob" \
    > "$staging_directory/aegis-machines"
  printf '%s\n' \
    'builders = @/etc/nix/aegis-machines' \
    'builders-use-substitutes = true' \
    'extra-substituters = https://cache.nixos.org https://microvm.cachix.org' \
    'extra-trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0= microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=' \
    > "$staging_directory/aegis.conf"

  builder_setup_as_root mkdir -p "$configuration_directory"
  builder_setup_backup_file "$configuration_directory/aegis-machines" "$staging_directory/backup-aegis-machines"
  builder_setup_backup_file "$configuration_directory/aegis.conf" "$staging_directory/backup-aegis.conf"
  builder_setup_backup_user_file "$host_key_file" "$staging_directory/backup-host-key"
  if ! builder_setup_has_active_include "$configuration_directory/nix.conf"; then
    builder_setup_backup_file "$configuration_directory/nix.conf" "$staging_directory/backup-nix.conf"
    printf '%s\n' '!include /etc/nix/aegis.conf' > "$staging_directory/include"
    installation_begun=true
    include_added=true
    builder_setup_as_root tee -a "$configuration_directory/nix.conf" < "$staging_directory/include" >/dev/null
  fi

  installation_begun=true
  builder_setup_install_file "$staging_directory/aegis-machines" "$configuration_directory/aegis-machines" 0644
  builder_setup_install_file "$staging_directory/aegis.conf" "$configuration_directory/aegis.conf" 0644
  builder_setup_as_root launchctl kickstart -k system/org.nixos.nix-daemon
  builder_setup_wait_for_daemon
  builder_setup_verify_remote_build "$configuration_directory"
  builder_setup_install_user_file "$staging_directory/host-key.pub" "$host_key_file"
  transaction_committed=true
)

builder_setup() {
  local configuration_directory="${AEGIS_BUILDER_CONFIGURATION_DIRECTORY:-/etc/nix}"
  local data_directory="${XDG_DATA_HOME:-$HOME/.local/share}/aegis/builder"
  local private_key="$data_directory/id_ed25519"
  local host_key_file="$data_directory/host-key.pub"
  local host_key_blob=""
  local scanned_host_key=""
  local staging_directory=""
  local include_count=0

  if [ "$(uname -s)" != Darwin ] && [ -z "${AEGIS_BUILDER_CONFIGURATION_DIRECTORY:-}" ]; then
    builder_setup_error "Builder setup supports macOS only."
    return
  fi

  builder_load_configuration || return
  include_count="$(builder_setup_active_include_count "$configuration_directory/nix.conf")"
  if [ "$include_count" -gt 1 ]; then
    builder_setup_error "nix.conf contains more than one active Aegis include. Remove duplicates before running setup."
    return
  fi
  if { [ ! -f "$private_key" ] || [ ! -f "$private_key.pub" ]; } && builder_container_exists; then
    builder_setup_error "Builder credentials are missing while the Aegis builder container exists. Remove and recreate the Aegis builder container before running setup again."
    return
  fi
  builder_setup_generate_credentials "$data_directory" || return
  AEGIS_BUILDER_PUBLIC_KEY="$(<"$private_key.pub")"
  AEGIS_BUILDER_PUBLIC_KEY_FILE="$private_key.pub"

  if [ -f "$host_key_file" ]; then
    AEGIS_BUILDER_HOST_KEY="$(<"$host_key_file")"
  fi
  builder_start || return
  scanned_host_key="$(builder_setup_scan_host_key)" || return
  if [ -n "${AEGIS_BUILDER_HOST_KEY:-}" ] && [ "$scanned_host_key" != "$AEGIS_BUILDER_HOST_KEY" ]; then
    builder_setup_error "SSH host key mismatch."
    return
  fi
  AEGIS_BUILDER_HOST_KEY="$scanned_host_key"
  builder_verify_host_key || return
  host_key_blob="$(printf '%s\n' "$AEGIS_BUILDER_HOST_KEY" | cut -d ' ' -f 2)"
  if [ -z "$host_key_blob" ]; then
    builder_setup_error "The builder host key does not contain a public key blob."
    return
  fi

  staging_directory="$(mktemp -d)"
  if ! builder_setup_validate_host_key "$host_key_blob" "$staging_directory"; then
    rm --force --recursive "$staging_directory"
    return 1
  fi

  builder_setup_transaction "$configuration_directory" "$private_key" "$host_key_blob" "$staging_directory" "$host_key_file" || return
  printf '%s\n' "Aegis builder setup is complete."
}
