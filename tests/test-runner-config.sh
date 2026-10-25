#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

RUNNER="${1:?path to the Aegis runner}"
TEMPORARY_DIRECTORY="$(mktemp --directory)"
trap 'rm --recursive --force "$TEMPORARY_DIRECTORY"' EXIT

export HOME="$TEMPORARY_DIRECTORY/home"
export XDG_CACHE_HOME="$TEMPORARY_DIRECTORY/cache"
export XDG_DATA_HOME="$TEMPORARY_DIRECTORY/data"
export XDG_STATE_HOME="$TEMPORARY_DIRECTORY/state"
export GIT_CONFIG_GLOBAL="$TEMPORARY_DIRECTORY/git-configuration"
export GIT_CONFIG_NOSYSTEM=1
export CAPTURE_FILE="$TEMPORARY_DIRECTORY/build-environment"
unset XDG_CONFIG_HOME
mkdir --parents "$HOME/.config/aegis" "$TEMPORARY_DIRECTORY/workspace"
printf '%s\n' '[user]' 'name = Test User' 'email = test@example.com' > "$GIT_CONFIG_GLOBAL"

# Stop at the VM build boundary after capturing the runner's resolved settings.
nix() {
  [ "$1" = build ] || return 2
  printf '%s\n' "$HOST_CONFIG" "$VM_CPU" "$VM_MEM" > "$CAPTURE_FILE"
  return 1
}
export -f nix

cd "$TEMPORARY_DIRECTORY/workspace"

check_configuration() {
  local configuration_directory="$1" expected_cpu="$2" expected_memory="$3"
  local status=0
  bash "$RUNNER" > "$TEMPORARY_DIRECTORY/output" 2>&1 || status=$?
  [ "$status" -eq 1 ]
  printf '%s\n' "$configuration_directory" "$expected_cpu" "$expected_memory" > "$TEMPORARY_DIRECTORY/expected"
  if ! cmp --silent "$TEMPORARY_DIRECTORY/expected" "$CAPTURE_FILE"; then
    echo "FAIL: runner must read and share the current user configuration." >&2
    diff --unified "$TEMPORARY_DIRECTORY/expected" "$CAPTURE_FILE" >&2 || true
    return 1
  fi
  if ! grep --fixed-strings --quiet "Config    : $configuration_directory/config.json" "$TEMPORARY_DIRECTORY/output"; then
    echo "FAIL: runner must display the user configuration file." >&2
    return 1
  fi
}

printf '%s\n' '{"vm":{"cpu":6,"mem":8192}}' > "$HOME/.config/aegis/config.json"
check_configuration "$HOME/.config/aegis" 6 8192
for workspace_configuration in "$XDG_STATE_HOME"/aegis/*/.config/aegis/config.json; do
  if [ -f "$workspace_configuration" ]; then
    echo "FAIL: runner must not create a workspace configuration snapshot." >&2
    exit 1
  fi
done

# An old workspace snapshot must not hide subsequent user configuration edits.
for state_directory in "$XDG_STATE_HOME"/aegis/*; do
  mkdir --parents "$state_directory/.config/aegis"
  printf '%s\n' '{"vm":{"cpu":1,"mem":1024}}' > "$state_directory/.config/aegis/config.json"
done
printf '%s\n' '{"vm":{"cpu":8,"mem":16384}}' > "$HOME/.config/aegis/config.json"
check_configuration "$HOME/.config/aegis" 8 16384

export XDG_CONFIG_HOME="$TEMPORARY_DIRECTORY/custom configuration"
mkdir --parents "$XDG_CONFIG_HOME/aegis"
printf '%s\n' '{"vm":{"cpu":2,"mem":2048}}' > "$XDG_CONFIG_HOME/aegis/config.json"
check_configuration "$XDG_CONFIG_HOME/aegis" 2 2048

echo "All runner configuration tests passed."
