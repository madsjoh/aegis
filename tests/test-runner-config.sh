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
  printf '%s\n' "$HOST_CONFIG" "$VM_CPU" "$VM_MEM" \
    "$VM_SKILL_ANTHROPIC" "$VM_SKILL_MATTPOCOCK" "$VM_SKILL_VERCEL" > "$CAPTURE_FILE"
  return 1
}
export -f nix

cd "$TEMPORARY_DIRECTORY/workspace"

check_configuration() {
  local configuration_directory="$1" expected_cpu="$2" expected_memory="$3"
  local expected_anthropic="$4" expected_mattpocock="$5" expected_vercel="$6"
  local status=0
  bash "$RUNNER" > "$TEMPORARY_DIRECTORY/output" 2>&1 || status=$?
  [ "$status" -eq 1 ]
  printf '%s\n' "$configuration_directory" "$expected_cpu" "$expected_memory" \
    "$expected_anthropic" "$expected_mattpocock" "$expected_vercel" > "$TEMPORARY_DIRECTORY/expected"
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

printf '%s\n' '{"cpu":6,"mem":8192}' > "$HOME/.config/aegis/vm.json"
printf '%s\n' '{"anthropic":true}' > "$HOME/.config/aegis/skills.json"
check_configuration "$HOME/.config/aegis" 6 8192 true false false
for scaffolded_file in vm.json skills.json opencode.jsonc; do
  if [ ! -f "$HOME/.config/aegis/$scaffolded_file" ]; then
    echo "FAIL: runner must scaffold $scaffolded_file." >&2
    exit 1
  fi
done

if bash "$RUNNER" init > "$TEMPORARY_DIRECTORY/init-output" 2>&1; then
  echo "FAIL: the init command must no longer exist." >&2
  exit 1
elif ! grep --fixed-strings --quiet "unknown command 'init'" "$TEMPORARY_DIRECTORY/init-output"; then
  echo "FAIL: init must report an unknown command." >&2
  cat "$TEMPORARY_DIRECTORY/init-output" >&2
  exit 1
fi

for workspace_configuration in "$XDG_STATE_HOME"/aegis/*/.config/aegis/config.json; do
  if [ -f "$workspace_configuration" ]; then
    echo "FAIL: runner must not create a workspace configuration snapshot." >&2
    exit 1
  fi
done

# An old workspace snapshot must not hide subsequent user configuration edits.
for state_directory in "$XDG_STATE_HOME"/aegis/*; do
  mkdir --parents "$state_directory/.config/aegis"
  printf '%s\n' '{"cpu":1,"mem":1024}' > "$state_directory/.config/aegis/vm.json"
done
printf '%s\n' '{"cpu":8,"mem":16384}' > "$HOME/.config/aegis/vm.json"
check_configuration "$HOME/.config/aegis" 8 16384 true false false

export XDG_CONFIG_HOME="$TEMPORARY_DIRECTORY/custom configuration"
mkdir --parents "$XDG_CONFIG_HOME/aegis"
printf '%s\n' '{"cpu":2,"mem":2048}' > "$XDG_CONFIG_HOME/aegis/vm.json"
check_configuration "$XDG_CONFIG_HOME/aegis" 2 2048 false false false

echo "All runner configuration tests passed."
