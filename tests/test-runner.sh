#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

BUILDER_BASH="${1:-helpers/builder.bash}"
RUNNER_BASH="${2:-helpers/runner.bash}"

TMP="$(mktemp -d "${AEGIS_TEST_TMPDIR:-${TMPDIR:-/tmp}}/aegis-runner.XXXXXX")"
trap 'rm --force --recursive "$TMP"' EXIT

failures=0

pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

# shellcheck disable=SC1090
source "$BUILDER_BASH"
# shellcheck disable=SC1090
source "$RUNNER_BASH"

docker() {
  case "$*" in
    "info --format {{.Architecture}}")
      [ "$TEST_SCENARIO" != "docker-stopped" ] || return 1
      printf '%s\n' aarch64
      ;;
    "container inspect aegis-builder")
      return 0
      ;;
    "inspect --format {{.Config.Image}} aegis-builder")
      printf '%s\n' aegis-builder
      ;;
    "inspect --format {{.Image}} aegis-builder")
      printf '%s\n' sha256:builder-image
      ;;
    "image inspect --format {{.Os}}/{{.Architecture}} sha256:builder-image")
      printf '%s\n' linux/arm64
      ;;
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.version\"}} sha256:builder-image" | \
    "inspect --format {{index .Config.Labels \"org.aegis.builder.version\"}} aegis-builder")
      printf '%s\n' 1
      ;;
    "image inspect --format {{index .Config.Labels \"org.aegis.builder.context\"}} sha256:builder-image" | \
    "inspect --format {{index .Config.Labels \"org.aegis.builder.context\"}} aegis-builder")
      printf '%s\n' TEST-CONTEXT
      ;;
    "inspect --format {{index .Config.Labels \"org.aegis.builder.public-key-fingerprint\"}} aegis-builder")
      printf '%s\n' SHA256:CLIENT
      ;;
    "inspect --format {{(index (index .HostConfig.PortBindings \"22/tcp\") 0).HostIp}}:{{(index (index .HostConfig.PortBindings \"22/tcp\") 0).HostPort}} aegis-builder")
      printf '%s\n' 127.0.0.1:31022
      ;;
    "inspect --format {{range .Mounts}}{{if eq .Destination \"/nix\"}}{{.Type}}:{{.Name}}{{end}}{{end}} aegis-builder")
      printf '%s\n' volume:aegis-builder-nix
      ;;
    "inspect --format {{.State.Running}} aegis-builder")
      if [ -f "$TEST_STATE/running" ]; then
        printf '%s\n' true
      else
        printf '%s\n' false
      fi
      ;;
    "start aegis-builder")
      touch "$TEST_STATE/running" "$TEST_STATE/started"
      printf '%s\n' aegis-builder
      ;;
    *)
      printf 'unexpected docker arguments: %s\n' "$*" >&2
      return 1
      ;;
  esac
}

ssh-keygen() {
  if [ "$*" != "-l -E sha256 -f $XDG_DATA_HOME/aegis/builder/id_ed25519.pub" ]; then
    return 1
  fi
  printf '%s\n' '256 SHA256:CLIENT aegis (ED25519)'
}

ssh-keyscan() {
  [ "$*" = "-p 31022 127.0.0.1" ] || return 1
  [ "$TEST_SCENARIO" != "unhealthy-ssh" ] || return 1
  printf '%s\n' '[127.0.0.1]:31022 ssh-ed25519 EXPECTED'
}

ssh() {
  local expected_prefix="-i $XDG_DATA_HOME/aegis/builder/id_ed25519 -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$TEST_STATE/known-hosts -o ConnectTimeout=2 -p 31022 builder@127.0.0.1"

  printf '%s\n' "$*" >> "$TEST_STATE/ssh-commands"
  case "$*" in
    "$expected_prefix true")
      touch "$TEST_STATE/authentication-checked"
      [ "$TEST_SCENARIO" != "authentication-failure" ]
      ;;
    "$expected_prefix nix store ping --store daemon")
      touch "$TEST_STATE/nix-health-checked"
      [ "$TEST_SCENARIO" != "nix-health-failure" ]
      ;;
    *)
      printf 'unexpected ssh arguments: %s\n' "$*" >&2
      return 1
      ;;
  esac
}

sleep() {
  [ "$*" = "1" ]
}

new_state() {
  TEST_SCENARIO="$1"
  TEST_STATE="$TMP/$TEST_SCENARIO"
  HOME="$TEST_STATE/home"
  XDG_DATA_HOME="$TEST_STATE/data"
  AEGIS_BUILDER_SETUP_COMMAND='nix run path:/aegis#builder-setup'
  AEGIS_BUILDER_CONTEXT='/nix/store/aegis-builder-context'
  AEGIS_BUILDER_CONTEXT_IDENTITY='TEST-CONTEXT'
  AEGIS_BUILDER_VERSION='1'
  AEGIS_BUILDER_CONFIGURATION_DIRECTORY="$TEST_STATE/etc-nix"
  AEGIS_BUILDER_KNOWN_HOSTS_FILE="$TEST_STATE/known-hosts"
  export AEGIS_BUILDER_CONFIGURATION_DIRECTORY AEGIS_BUILDER_CONTEXT AEGIS_BUILDER_CONTEXT_IDENTITY AEGIS_BUILDER_KNOWN_HOSTS_FILE AEGIS_BUILDER_SETUP_COMMAND AEGIS_BUILDER_VERSION HOME TEST_SCENARIO TEST_STATE XDG_DATA_HOME
  mkdir --parents "$XDG_DATA_HOME/aegis/builder"
}

write_setup_state() {
  mkdir --parents "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY"
  printf '%s\n' 'PRIVATE KEY' > "$XDG_DATA_HOME/aegis/builder/id_ed25519"
  printf '%s\n' 'ssh-ed25519 CLIENT' > "$XDG_DATA_HOME/aegis/builder/id_ed25519.pub"
  printf '%s\n' 'ssh-ed25519 EXPECTED' > "$XDG_DATA_HOME/aegis/builder/host-key.pub"
  printf '%s\n' '!include /etc/nix/aegis.conf' > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/nix.conf"
  printf '%s\n' \
    "ssh-ng://builder@127.0.0.1:31022 aarch64-linux $XDG_DATA_HOME/aegis/builder/id_ed25519 1 1 - - EXPECTED" \
    > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis-machines"
  printf '%s\n' \
    'builders = @/etc/nix/aegis-machines' \
    'builders-use-substitutes = true' \
    'extra-substituters = https://cache.nixos.org https://microvm.cachix.org' \
    'extra-trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0= microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=' \
    > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
}

run_preflight() {
  runner_prepare_builder
}

new_state never-configured
if run_preflight 2> "$TEST_STATE/error"; then
  fail "missing setup state fails"
elif grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "missing setup state reports the exact setup command"
else
  fail "missing setup state reports the exact setup command"
fi

new_state incomplete-setup
printf '%s\n' 'ssh-ed25519 CLIENT' > "$XDG_DATA_HOME/aegis/builder/id_ed25519.pub"
printf '%s\n' 'ssh-ed25519 EXPECTED' > "$XDG_DATA_HOME/aegis/builder/host-key.pub"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "orphaned credentials fail setup validation"
elif grep --fixed-strings --quiet 'has not been configured' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "orphaned credentials report the setup command"
else
  fail "orphaned credentials report the setup command"
fi

new_state docker-stopped
write_setup_state
if run_preflight 2> "$TEST_STATE/error"; then
  fail "stopped Docker fails"
elif grep --fixed-strings --quiet 'Start Docker' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "stopped Docker reports recovery actions"
else
  fail "stopped Docker reports recovery actions"
fi

new_state duplicate-registration
write_setup_state
printf '%s\n' '!include /etc/nix/aegis.conf' >> "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/nix.conf"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "duplicate runtime includes fail"
elif grep --fixed-strings --quiet 'exactly one active' "$TEST_STATE/error" \
  && [ ! -e "$TEST_STATE/started" ]; then
  pass "duplicate runtime includes fail before builder startup"
else
  fail "duplicate runtime includes fail before builder startup"
fi

new_state registration-drift
write_setup_state
printf '%s\n' 'builders-use-substitutes = false' > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "runtime configuration drift fails"
elif grep --fixed-strings --quiet 'aegis.conf' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "runtime configuration drift reports setup recovery"
else
  fail "runtime configuration drift reports setup recovery"
fi

new_state duplicate-setting
write_setup_state
printf '%s\n' 'builders-use-substitutes = true' >> "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "duplicate active runtime settings fail"
elif grep --fixed-strings --quiet 'exactly one active builders-use-substitutes' "$TEST_STATE/error"; then
  pass "duplicate active runtime settings fail semantic validation"
else
  fail "duplicate active runtime settings fail semantic validation"
fi

new_state extra-setting
write_setup_state
printf '%s\n' 'max-jobs = 8' >> "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "an extra active runtime setting fails"
elif grep --fixed-strings --quiet 'only the four required' "$TEST_STATE/error"; then
  pass "an extra active runtime setting fails exact semantic validation"
else
  fail "an extra active runtime setting fails exact semantic validation"
fi

for invalid_directive in '!include /etc/nix/other.conf' 'malformed directive'; do
  new_state invalid-directive
  write_setup_state
  printf '%s\n' "$invalid_directive" >> "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
  if run_preflight 2> "$TEST_STATE/error"; then
    fail "an active nonsetting directive fails"
  elif grep --fixed-strings --quiet 'only the four required' "$TEST_STATE/error" \
    && [ ! -e "$TEST_STATE/started" ]; then
    pass "an active nonsetting directive fails before builder startup"
  else
    fail "an active nonsetting directive fails before builder startup"
  fi
done

new_state semantic-formatting
write_setup_state
printf '%s\n' \
  '# Managed settings.' \
  ' builders = @/etc/nix/aegis-machines # Builder file.' \
  'builders-use-substitutes=true' \
  'extra-substituters = https://cache.nixos.org https://microvm.cachix.org' \
  'extra-trusted-public-keys=cache.nixos.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0= microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=' \
  > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis.conf"
touch "$TEST_STATE/running"
if run_preflight; then
  pass "semantic runtime settings permit whitespace and comments"
else
  fail "semantic runtime settings permit whitespace and comments"
fi

new_state machine-drift
write_setup_state
printf '%s\n' 'ssh-ng://builder@127.0.0.1:9999 aarch64-linux /wrong/key 1 1 - - WRONG' > "$AEGIS_BUILDER_CONFIGURATION_DIRECTORY/aegis-machines"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "runtime machine drift fails"
elif grep --fixed-strings --quiet 'aegis-machines' "$TEST_STATE/error" \
  && [ ! -e "$TEST_STATE/started" ]; then
  pass "runtime machine drift fails before builder startup"
else
  fail "runtime machine drift fails before builder startup"
fi

new_state stopped-builder
write_setup_state
if run_preflight \
  && [ -f "$TEST_STATE/started" ] \
  && [ "$AEGIS_BUILDER_CONTEXT" = '/nix/store/aegis-builder-context' ]; then
  pass "a stopped builder is started and verified"
else
  fail "a stopped builder is started and verified"
fi

new_state unhealthy-ssh
write_setup_state
touch "$TEST_STATE/running"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "unhealthy SSH fails"
elif grep --fixed-strings --quiet 'SSH did not become ready' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "unhealthy SSH reports the setup recovery command"
else
  fail "unhealthy SSH reports the setup recovery command"
fi

new_state healthy
write_setup_state
touch "$TEST_STATE/running"
if run_preflight \
  && [ "$AEGIS_BUILDER_PUBLIC_KEY_FILE" = "$XDG_DATA_HOME/aegis/builder/id_ed25519.pub" ] \
  && [ "$AEGIS_BUILDER_HOST_KEY" = 'ssh-ed25519 EXPECTED' ] \
  && [ -f "$TEST_STATE/authentication-checked" ] \
  && [ -f "$TEST_STATE/nix-health-checked" ] \
  && [ "$(<"$TEST_STATE/known-hosts")" = '[127.0.0.1]:31022 ssh-ed25519 EXPECTED' ]; then
  pass "healthy setup state loads persistent keys and passes"
else
  fail "healthy setup state loads persistent keys and passes"
fi

new_state authentication-failure
write_setup_state
touch "$TEST_STATE/running"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "builder authentication failure fails"
elif grep --fixed-strings --quiet 'authentication failed' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "builder authentication failure reports an actionable error"
else
  fail "builder authentication failure reports an actionable error"
fi

new_state nix-health-failure
write_setup_state
touch "$TEST_STATE/running"
if run_preflight 2> "$TEST_STATE/error"; then
  fail "remote Nix daemon failure fails"
elif grep --fixed-strings --quiet 'remote Nix daemon is unhealthy' "$TEST_STATE/error" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$TEST_STATE/error"; then
  pass "remote Nix daemon failure reports an actionable error"
else
  fail "remote Nix daemon failure reports an actionable error"
fi

if [ "$failures" -eq 0 ]; then
  printf '%s\n' "all runner tests passed"
else
  printf '%s runner test(s) failed\n' "$failures" >&2
  exit 1
fi
