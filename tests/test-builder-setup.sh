#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

BUILDER_BASH="${1:-helpers/builder.bash}"
BUILDER_SETUP_BASH="${2:-helpers/builder-setup.bash}"
if [ "$BUILDER_BASH" = "helpers/builder.bash" ] && [ ! -f "$BUILDER_BASH" ]; then
  BUILDER_BASH="$(pwd)/helpers/builder.bash"
  BUILDER_SETUP_BASH="$(pwd)/helpers/builder-setup.bash"
fi

TMP="$(mktemp -d)"
trap 'rm --force --recursive "$TMP"' EXIT

failures=0

pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

# shellcheck disable=SC1090
source "$BUILDER_BASH"
# shellcheck disable=SC1090
source "$BUILDER_SETUP_BASH"

FIXTURE_KEY="$TMP/fixture-host-key"
command ssh-keygen -q -t ed25519 -N '' -f "$FIXTURE_KEY"
HOST_PUBLIC_KEY="$(<"$FIXTURE_KEY.pub")"
HOST_KEY_BLOB="$(printf '%s\n' "$HOST_PUBLIC_KEY" | cut -d ' ' -f 2)"
HOST_PUBLIC_KEY="ssh-ed25519 $HOST_KEY_BLOB"

ssh-keygen() {
  local key_file=""

  if [ "${1:-}" = "-l" ]; then
    command ssh-keygen "$@"
    return
  fi

  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-f" ]; then
      key_file="$2"
      shift 2
    else
      shift
    fi
  done
  [ -n "$key_file" ] || return 1
  printf '%s\n' 'PRIVATE KEY' > "$key_file"
  printf '%s\n' 'ssh-ed25519 AEGIS-CLIENT-KEY' > "$key_file.pub"
}

builder_container_exists() {
  [ -f "$TEST_STATE/container-exists" ]
}

builder_start() {
  printf '%s\n' "$AEGIS_BUILDER_PUBLIC_KEY" > "$TEST_STATE/start-public-key"
  touch "$TEST_STATE/builder-started"
}

builder_verify_host_key() {
  [ "$AEGIS_BUILDER_HOST_KEY" = "$HOST_PUBLIC_KEY" ]
}

ssh-keyscan() {
  [ "$*" = "-t ed25519 -p 31022 127.0.0.1" ] || return 1
  printf '[127.0.0.1]:31022 ssh-ed25519 %s\n' "$HOST_KEY_BLOB"
}

builder_setup_as_root() {
  local arguments="$*"

  if [ "$TEST_SCENARIO" = "temporary-install-failure" ] \
    && [[ "$arguments" == install*'.aegis-new' ]]; then
    touch "${@: -1}"
    return 1
  fi
  if [ "$TEST_SCENARIO" = "temporary-move-failure" ] \
    && [[ "$arguments" == mv*'.aegis-new'* ]]; then
    return 1
  fi
  if [ "$TEST_SCENARIO" = "interrupt-install" ] \
    && [[ "$arguments" == install*'aegis.conf.aegis-new' ]]; then
    kill -INT "$BASHPID"
    return 130
  fi
  if [ "$TEST_SCENARIO" = "terminate-install" ] \
    && [[ "$arguments" == install*'aegis.conf.aegis-new' ]]; then
    kill -TERM "$BASHPID"
    return 143
  fi
  if [ "$1" = "launchctl" ]; then
    shift
    launchctl "$@"
  else
    "$@"
  fi
}

launchctl() {
  [ "$*" = "kickstart -k system/org.nixos.nix-daemon" ] || return 1
  printf '%s\n' reload >> "$TEST_STATE/daemon-reloads"
  [ "$TEST_SCENARIO" != "reload-failure" ]
}

install() {
  if [ "$TEST_SCENARIO" = "interrupt-host-pin" ] \
    && [ "${*: -1}" = "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub.aegis-new" ]; then
    kill -INT "$BASHPID"
    return 130
  fi
  command install "$@"
}

nix() {
  case "$*" in
    "--store daemon store ping")
      local attempts=0
      if [ -f "$TEST_STATE/ping-attempts" ]; then
        attempts="$(<"$TEST_STATE/ping-attempts")"
      fi
      attempts=$((attempts + 1))
      printf '%s\n' "$attempts" > "$TEST_STATE/ping-attempts"
      [ "$attempts" -ge 2 ]
      ;;
    *" build "*)
      printf '%s\n' "$*" >> "$TEST_STATE/build-commands"
      if [ "$TEST_SCENARIO" = "verification-failure" ]; then
        return 1
      fi
      mkdir --parents "$TEST_STATE/result"
      printf '%s\n' verified > "$TEST_STATE/result/value"
      printf '%s\n' "$TEST_STATE/result"
      ;;
    *" store cat "*)
      [ "$*" = "--store daemon store cat $TEST_STATE/result/value" ] || return 1
      cat "$TEST_STATE/result/value"
      ;;
    *)
      printf 'unexpected nix arguments: %s\n' "$*" >&2
      return 1
      ;;
  esac
}

run_setup() {
  HOME="$TEST_STATE/home" \
    AEGIS_BUILDER_CONTEXT_IDENTITY='TEST-CONTEXT' \
    AEGIS_BUILDER_CONFIGURATION_DIRECTORY="$TEST_STATE/etc-nix" \
    AEGIS_BUILDER_CONTEXT="$TEST_STATE/builder-context" \
    AEGIS_BUILDER_VERSION='1' \
    builder_setup
}

new_state() {
  TEST_SCENARIO="${1:-success}"
  TEST_STATE="$TMP/$TEST_SCENARIO-$RANDOM"
  mkdir --parents "$TEST_STATE/etc-nix" "$TEST_STATE/home" "$TEST_STATE/builder-context"
  printf '%s\n' '# Existing comment.' 'experimental-features = nix-command flakes' > "$TEST_STATE/etc-nix/nix.conf"
}

new_state duplicate-includes
printf '%s\n' '!include /etc/nix/aegis.conf' ' !include   /etc/nix/aegis.conf # Duplicate.' >> "$TEST_STATE/etc-nix/nix.conf"
NIX_CONF_BEFORE="$(<"$TEST_STATE/etc-nix/nix.conf")"
if run_setup 2> "$TEST_STATE/error"; then
  fail "duplicate active includes fail setup"
elif grep --fixed-strings --quiet 'more than one active' "$TEST_STATE/error" \
  && [ "$(<"$TEST_STATE/etc-nix/nix.conf")" = "$NIX_CONF_BEFORE" ] \
  && [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/id_ed25519" ] \
  && [ ! -e "$TEST_STATE/builder-started" ]; then
  pass "duplicate active includes fail before setup changes"
else
  fail "duplicate active includes fail before setup changes"
fi

stat_inode() {
  stat -f '%i' "$1" 2>/dev/null || stat --format='%i' "$1"
}

stat_mode() {
  stat -f '%Lp' "$1" 2>/dev/null || stat --format='%a' "$1"
}

new_state
printf '%s\n' '  !include    /etc/nix/aegis.conf   # Managed by another tool.' >> "$TEST_STATE/etc-nix/nix.conf"
INCLUDE_CONTENT="$(<"$TEST_STATE/etc-nix/nix.conf")"
INCLUDE_INODE="$(stat_inode "$TEST_STATE/etc-nix/nix.conf")"
INCLUDE_MODE="$(stat_mode "$TEST_STATE/etc-nix/nix.conf")"
if builder_setup_has_active_include "$TEST_STATE/etc-nix/nix.conf" \
  && run_setup \
  && [ "$(<"$TEST_STATE/etc-nix/nix.conf")" = "$INCLUDE_CONTENT" ] \
  && [ "$(stat_inode "$TEST_STATE/etc-nix/nix.conf")" = "$INCLUDE_INODE" ] \
  && [ "$(stat_mode "$TEST_STATE/etc-nix/nix.conf")" = "$INCLUDE_MODE" ]; then
  pass "a semantic active include preserves nix.conf content, inode, and mode"
else
  fail "a semantic active include preserves nix.conf content, inode, and mode"
fi

new_state
printf '%s\n' '# !include /etc/nix/aegis.conf' >> "$TEST_STATE/etc-nix/nix.conf"
if builder_setup_has_active_include "$TEST_STATE/etc-nix/nix.conf"; then
  fail "a commented include is not active"
elif run_setup \
  && [ "$(grep --extended-regexp --count '^[[:space:]]*!include[[:space:]]+/etc/nix/aegis\.conf([[:space:]]*(#.*)?)?$' "$TEST_STATE/etc-nix/nix.conf")" = "1" ]; then
  pass "setup appends one include when only a commented include exists"
else
  fail "setup appends one include when only a commented include exists"
fi

KEY_DIRECTORY="$TEST_STATE/home/.local/share/aegis/builder"
read -r machine_uri machine_system machine_key machine_jobs machine_speed machine_supported machine_mandatory machine_host_key < "$TEST_STATE/etc-nix/aegis-machines"
printf 'ssh-ed25519 %s\n' "$machine_host_key" > "$TEST_STATE/reconstructed-host-key.pub"
if [ "$machine_uri" = "ssh-ng://builder@127.0.0.1:31022" ] \
  && [ "$machine_system" = "aarch64-linux" ] \
  && [ "$machine_key" = "$KEY_DIRECTORY/id_ed25519" ] \
  && [ "$machine_jobs" = "1" ] \
  && [ "$machine_speed" = "1" ] \
  && [ "$machine_supported" = "-" ] \
  && [ "$machine_mandatory" = "-" ] \
  && [ "$machine_host_key" = "$HOST_KEY_BLOB" ] \
  && command ssh-keygen -l -f "$TEST_STATE/reconstructed-host-key.pub" >/dev/null; then
  pass "field eight reconstructs a valid Ed25519 public key"
else
  fail "field eight reconstructs a valid Ed25519 public key"
fi

FIRST_BUILD="$(sed -n '1p' "$TEST_STATE/build-commands")"
if run_setup; then
  SECOND_BUILD="$(sed -n '2p' "$TEST_STATE/build-commands")"
else
  SECOND_BUILD=""
fi
if [ -n "$SECOND_BUILD" ] \
  && [ "$FIRST_BUILD" != "$SECOND_BUILD" ] \
  && [[ "$SECOND_BUILD" == *"--max-jobs 0"* ]] \
  && [[ "$SECOND_BUILD" == *"--builders @$TEST_STATE/etc-nix/aegis-machines"* ]] \
  && [[ "$SECOND_BUILD" == *"--print-out-paths"* ]] \
  && [ "$(<"$TEST_STATE/ping-attempts")" -ge 2 ]; then
  pass "each setup waits for the daemon and forces a fresh remote build"
else
  fail "each setup waits for the daemon and forces a fresh remote build"
fi

new_state verification-failure
DATA_DIRECTORY="$TEST_STATE/home/.local/share/aegis/builder"
mkdir --parents "$DATA_DIRECTORY"
printf '%s\n' 'PRIVATE KEY' > "$DATA_DIRECTORY/id_ed25519"
printf '%s\n' 'ssh-ed25519 AEGIS-CLIENT-KEY' > "$DATA_DIRECTORY/id_ed25519.pub"
printf 'ssh-ed25519 %s\n' "$HOST_KEY_BLOB" > "$DATA_DIRECTORY/host-key.pub"
PIN_BEFORE="$(<"$DATA_DIRECTORY/host-key.pub")"
printf '%s\n' 'previous machines' > "$TEST_STATE/etc-nix/aegis-machines"
printf '%s\n' 'previous configuration' > "$TEST_STATE/etc-nix/aegis.conf"
chmod 0600 "$TEST_STATE/etc-nix/aegis-machines" "$TEST_STATE/etc-nix/aegis.conf"
NIX_CONF_BEFORE="$(<"$TEST_STATE/etc-nix/nix.conf")"
if run_setup; then
  fail "verification failure rejects the transaction"
elif [ "$(<"$TEST_STATE/etc-nix/aegis-machines")" = "previous machines" ] \
  && [ "$(<"$TEST_STATE/etc-nix/aegis.conf")" = "previous configuration" ] \
  && [ "$(<"$TEST_STATE/etc-nix/nix.conf")" = "$NIX_CONF_BEFORE" ] \
  && [ "$(<"$DATA_DIRECTORY/host-key.pub")" = "$PIN_BEFORE" ] \
  && [ "$(stat_mode "$TEST_STATE/etc-nix/aegis-machines")" = "600" ] \
  && [ "$(stat_mode "$TEST_STATE/etc-nix/aegis.conf")" = "600" ] \
  && [ "$(wc --lines < "$TEST_STATE/daemon-reloads")" = "2" ]; then
  pass "verification failure restores prior Aegis files and the appended include"
else
  fail "verification failure restores prior Aegis files and the appended include"
fi

new_state verification-failure-new-pin
if run_setup; then
  fail "verification failure rejects a new host pin"
elif [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub" ] \
  && [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub.aegis-new" ]; then
  pass "verification failure does not finalize a new host pin"
else
  fail "verification failure does not finalize a new host pin"
fi

new_state interrupt-host-pin
printf '%s\n' 'previous machines' > "$TEST_STATE/etc-nix/aegis-machines"
printf '%s\n' 'previous configuration' > "$TEST_STATE/etc-nix/aegis.conf"
if (run_setup); then
  fail "a signal during host pin installation interrupts setup"
elif [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub" ] \
  && [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub.aegis-new" ] \
  && [ "$(<"$TEST_STATE/etc-nix/aegis-machines")" = "previous machines" ] \
  && [ "$(<"$TEST_STATE/etc-nix/aegis.conf")" = "previous configuration" ] \
  && ! builder_setup_has_active_include "$TEST_STATE/etc-nix/nix.conf"; then
  pass "a signal during host pin installation rolls back configuration and staging"
else
  fail "a signal during host pin installation rolls back configuration and staging"
fi

new_state successful-new-pin
if run_setup \
  && [ "$(<"$TEST_STATE/home/.local/share/aegis/builder/host-key.pub")" = "$HOST_PUBLIC_KEY" ] \
  && [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/host-key.pub.aegis-new" ]; then
  pass "successful verification atomically finalizes the staged host pin"
else
  fail "successful verification atomically finalizes the staged host pin"
fi

new_state reload-failure
printf '%s\n' 'previous machines' > "$TEST_STATE/etc-nix/aegis-machines"
printf '%s\n' 'previous configuration' > "$TEST_STATE/etc-nix/aegis.conf"
if run_setup; then
  fail "daemon reload failure rejects the transaction"
elif [ "$(<"$TEST_STATE/etc-nix/aegis-machines")" = "previous machines" ] \
  && [ "$(<"$TEST_STATE/etc-nix/aegis.conf")" = "previous configuration" ]; then
  pass "daemon reload failure restores prior Aegis files"
else
  fail "daemon reload failure restores prior Aegis files"
fi

new_state missing-credentials
touch "$TEST_STATE/container-exists"
if run_setup 2> "$TEST_STATE/error"; then
  fail "an existing container rejects recreated credentials"
elif grep --fixed-strings --ignore-case --quiet 'remove and recreate the Aegis builder container' "$TEST_STATE/error" \
  && [ ! -e "$TEST_STATE/home/.local/share/aegis/builder/id_ed25519" ] \
  && [ ! -e "$TEST_STATE/etc-nix/aegis-machines" ]; then
  pass "an existing container rejects recreated credentials before host changes"
else
  fail "an existing container rejects recreated credentials before host changes"
fi

for scenario in temporary-install-failure temporary-move-failure; do
  new_state "$scenario"
  printf '%s\n' 'previous machines' > "$TEST_STATE/etc-nix/aegis-machines"
  printf '%s\n' 'previous configuration' > "$TEST_STATE/etc-nix/aegis.conf"
  if run_setup; then
    fail "$scenario rejects the transaction"
  elif [ ! -e "$TEST_STATE/etc-nix/aegis-machines.aegis-new" ] \
    && [ ! -e "$TEST_STATE/etc-nix/aegis.conf.aegis-new" ] \
    && [ "$(<"$TEST_STATE/etc-nix/aegis-machines")" = "previous machines" ] \
    && [ "$(<"$TEST_STATE/etc-nix/aegis.conf")" = "previous configuration" ] \
    && ! builder_setup_has_active_include "$TEST_STATE/etc-nix/nix.conf"; then
    pass "$scenario removes temporary files and restores prior configuration"
  else
    fail "$scenario removes temporary files and restores prior configuration"
  fi
done

for signal_scenario in interrupt-install terminate-install; do
  new_state "$signal_scenario"
  printf '%s\n' 'previous machines' > "$TEST_STATE/etc-nix/aegis-machines"
  printf '%s\n' 'previous configuration' > "$TEST_STATE/etc-nix/aegis.conf"
  if (run_setup); then
    fail "$signal_scenario interrupts the transaction"
  elif [ ! -e "$TEST_STATE/etc-nix/aegis-machines.aegis-new" ] \
    && [ ! -e "$TEST_STATE/etc-nix/aegis.conf.aegis-new" ] \
    && [ "$(<"$TEST_STATE/etc-nix/aegis-machines")" = "previous machines" ] \
    && [ "$(<"$TEST_STATE/etc-nix/aegis.conf")" = "previous configuration" ] \
    && ! builder_setup_has_active_include "$TEST_STATE/etc-nix/nix.conf"; then
    pass "$signal_scenario removes temporary files and restores prior configuration"
  else
    fail "$signal_scenario removes temporary files and restores prior configuration"
  fi
done

new_state invalid-host-key
HOST_PUBLIC_KEY='ssh-ed25519 AAAA'
HOST_KEY_BLOB='AAAA'
if run_setup 2> "$TEST_STATE/error"; then
  fail "an invalid host key fails preflight validation"
elif grep --fixed-strings --quiet 'valid Ed25519 public key' "$TEST_STATE/error" \
  && [ ! -e "$TEST_STATE/etc-nix/aegis-machines" ] \
  && [ ! -e "$TEST_STATE/etc-nix/aegis-machines.aegis-new" ]; then
  pass "an invalid host key fails preflight validation before installation"
else
  fail "an invalid host key fails preflight validation before installation"
fi

if [ "$failures" -eq 0 ]; then
  printf '%s\n' "all builder setup tests passed"
else
  printf '%s builder setup test(s) failed\n' "$failures" >&2
  exit 1
fi
