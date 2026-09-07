#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

RUNNER_NIX="${1:?path to helpers/runner.nix}"
LOCK_BASH="${2:?path to helpers/lock.bash}"
SSH_KEY_BASH="${3:?path to helpers/ssh-key.bash}"
RUNNER_LIFECYCLE_BASH="${4:?path to helpers/runner-lifecycle.bash}"
# shellcheck disable=SC1090
source "$LOCK_BASH"
# shellcheck disable=SC1090
source "$SSH_KEY_BASH"
if [ -f "$RUNNER_LIFECYCLE_BASH" ]; then
  # shellcheck disable=SC1090
  source "$RUNNER_LIFECYCLE_BASH"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

count_lines() {
  local count=0
  local line

  while IFS= read -r line; do
    count=$((count + 1))
  done < "$1"
  printf '%s\n' "$count"
}

derive_paths() {
  local workspace_path="${1:?workspace path}"
  local workspace_id

  workspace_id="$(printf '%s' "$workspace_path" | sha256sum | cut -c1-16)"
  printf '%s\n' \
    "$XDG_DATA_HOME/aegis/ssh_host_ed25519" \
    "$XDG_CACHE_HOME/aegis/vzvm" \
    "$XDG_STATE_HOME/aegis/$workspace_id" \
    "$XDG_STATE_HOME/aegis/$workspace_id/run/vm.log"
}

export XDG_CACHE_HOME="$TMP/cache"
export XDG_DATA_HOME="$TMP/data"
export XDG_STATE_HOME="$TMP/state"

# Concurrent first use publishes one matching private and public key pair.
SSH_KEY="$XDG_DATA_HOME/aegis/ssh_host_ed25519"
mkdir -p "$(dirname "$SSH_KEY")"
bash -c 'source "$1"; source "$2"; ensure_ssh_key "$3"' _ "$LOCK_BASH" "$SSH_KEY_BASH" "$SSH_KEY" &
FIRST_KEY_PID=$!
bash -c 'source "$1"; source "$2"; ensure_ssh_key "$3"' _ "$LOCK_BASH" "$SSH_KEY_BASH" "$SSH_KEY" &
SECOND_KEY_PID=$!
if wait "$FIRST_KEY_PID" \
  && wait "$SECOND_KEY_PID" \
  && [ -f "$SSH_KEY" ] \
  && [ -f "$SSH_KEY.pub" ] \
  && [ "$(ssh-keygen -y -f "$SSH_KEY" | cut -d ' ' -f 1,2)" = "$(cut -d ' ' -f 1,2 "$SSH_KEY.pub")" ]; then
  pass "concurrent first use publishes a matching SSH key pair"
else
  fail "concurrent first use publishes a matching SSH key pair"
fi

# Repairing a missing public key preserves the existing private identity.
PRIVATE_KEY_BEFORE="$(ssh-keygen -y -f "$SSH_KEY")"
rm "$SSH_KEY.pub"
if ensure_ssh_key "$SSH_KEY" \
  && [ "$(ssh-keygen -y -f "$SSH_KEY")" = "$PRIVATE_KEY_BEFORE" ] \
  && [ "$(cut -d ' ' -f 1,2 "$SSH_KEY.pub")" = "$(printf '%s\n' "$PRIVATE_KEY_BEFORE" | cut -d ' ' -f 1,2)" ]; then
  pass "missing public key is repaired without replacing the private key"
else
  fail "missing public key is repaired without replacing the private key"
fi

# Repairing a mismatched public key preserves the existing private identity.
ssh-keygen -t ed25519 -f "$TMP/wrong-key" -N "" -q
cp "$TMP/wrong-key.pub" "$SSH_KEY.pub"
if ensure_ssh_key "$SSH_KEY" \
  && [ "$(ssh-keygen -y -f "$SSH_KEY")" = "$PRIVATE_KEY_BEFORE" ] \
  && [ "$(cut -d ' ' -f 1,2 "$SSH_KEY.pub")" = "$(printf '%s\n' "$PRIVATE_KEY_BEFORE" | cut -d ' ' -f 1,2)" ]; then
  pass "mismatched public key is repaired without replacing the private key"
else
  fail "mismatched public key is repaired without replacing the private key"
fi

# An unreadable private identity fails without changing either key file.
INVALID_SSH_KEY="$TMP/invalid-key"
printf '%s\n' "not a private key" > "$INVALID_SSH_KEY"
printf '%s\n' "sentinel public key" > "$INVALID_SSH_KEY.pub"
cp "$INVALID_SSH_KEY" "$TMP/invalid-key.before"
cp "$INVALID_SSH_KEY.pub" "$TMP/invalid-key.pub.before"
if ! ensure_ssh_key "$INVALID_SSH_KEY" \
  && cmp --silent "$INVALID_SSH_KEY" "$TMP/invalid-key.before" \
  && cmp --silent "$INVALID_SSH_KEY.pub" "$TMP/invalid-key.pub.before"; then
  pass "invalid existing private key fails without changing either key file"
else
  fail "invalid existing private key fails without changing either key file"
fi

# A dangling private key link fails without changing either key path.
DANGLING_SSH_KEY="$TMP/dangling-key"
ln -s "$TMP/missing-private-key" "$DANGLING_SSH_KEY"
printf '%s\n' "sentinel public key" > "$DANGLING_SSH_KEY.pub"
DANGLING_LINK_BEFORE="$(readlink "$DANGLING_SSH_KEY")"
cp "$DANGLING_SSH_KEY.pub" "$TMP/dangling-key.pub.before"
if ! ensure_ssh_key "$DANGLING_SSH_KEY" \
  && [ -L "$DANGLING_SSH_KEY" ] \
  && [ "$(readlink "$DANGLING_SSH_KEY")" = "$DANGLING_LINK_BEFORE" ] \
  && cmp --silent "$DANGLING_SSH_KEY.pub" "$TMP/dangling-key.pub.before"; then
  pass "dangling private key link fails without changing either key path"
else
  fail "dangling private key link fails without changing either key path"
fi

derive_paths "$TMP/first workspace" > "$TMP/first-paths"
derive_paths "$TMP/second workspace" > "$TMP/second-paths"
FIRST_SSH_KEY="$(sed -n '1p' "$TMP/first-paths")"
FIRST_CACHE_DIRECTORY="$(sed -n '2p' "$TMP/first-paths")"
FIRST_STATE_DIRECTORY="$(sed -n '3p' "$TMP/first-paths")"
FIRST_VM_LOG="$(sed -n '4p' "$TMP/first-paths")"
SECOND_SSH_KEY="$(sed -n '1p' "$TMP/second-paths")"
SECOND_CACHE_DIRECTORY="$(sed -n '2p' "$TMP/second-paths")"
SECOND_STATE_DIRECTORY="$(sed -n '3p' "$TMP/second-paths")"
SECOND_VM_LOG="$(sed -n '4p' "$TMP/second-paths")"

if [ "$FIRST_SSH_KEY" = "$SECOND_SSH_KEY" ]; then
  pass "distinct workspaces share the SSH key"
else
  fail "distinct workspaces share the SSH key"
fi

if [ "$FIRST_CACHE_DIRECTORY" = "$SECOND_CACHE_DIRECTORY" ]; then
  pass "distinct workspaces share the macOS image cache"
else
  fail "distinct workspaces share the macOS image cache"
fi

if [ "$FIRST_STATE_DIRECTORY" != "$SECOND_STATE_DIRECTORY" ]; then
  pass "distinct workspaces retain separate state directories"
else
  fail "distinct workspaces retain separate state directories"
fi

if [ "$FIRST_VM_LOG" != "$SECOND_VM_LOG" ]; then
  pass "distinct workspaces retain separate VM logs"
else
  fail "distinct workspaces retain separate VM logs"
fi

VM_LOG="$TMP/vm.log"
printf '%s\n' "[vzvm] guest started" > "$VM_LOG"
if declare -F prepare_vm_log >/dev/null && prepare_vm_log "$VM_LOG" && [ ! -s "$VM_LOG" ]; then
  pass "runner lifecycle clears stale VM output synchronously"
else
  fail "runner lifecycle clears stale VM output synchronously"
fi

IMAGE_LOCK="$TMP/cleanup-image.lock"
WORKSPACE_LOCK="$TMP/cleanup-workspace.lock"
acquire_lock "$IMAGE_LOCK" IMAGE_LOCK_DESCRIPTOR
acquire_lock "$WORKSPACE_LOCK" WORKSPACE_LOCK_DESCRIPTOR
VM_PID=""
VIRTIOFSD_PIDS=""
if declare -F cleanup >/dev/null \
  && cleanup \
  && cleanup \
  && [ -z "$IMAGE_LOCK_DESCRIPTOR" ] \
  && [ -z "$WORKSPACE_LOCK_DESCRIPTOR" ] \
  && bash -c 'source "$1"; acquire_lock "$2" IMAGE_DESCRIPTOR; acquire_lock "$3" WORKSPACE_DESCRIPTOR' \
    _ "$LOCK_BASH" "$IMAGE_LOCK" "$WORKSPACE_LOCK"; then
  pass "runner cleanup releases held locks once and permits reacquisition"
else
  fail "runner cleanup releases held locks once and permits reacquisition"
fi

VM_TERM_LOG="$TMP/vm-terms"
bash -c 'trap '\''printf "%s\n" TERM >> "$1"; exit 0'\'' TERM; while true; do :; done' _ "$VM_TERM_LOG" &
VM_PID=$!
VIRTIOFSD_PIDS=""
IMAGE_LOCK_DESCRIPTOR=""
WORKSPACE_LOCK_DESCRIPTOR=""
sleep 0.1
cleanup
cleanup
if ! kill -0 "$VM_PID" 2>/dev/null \
  && [ "$(count_lines "$VM_TERM_LOG")" -eq 1 ]; then
  pass "runner cleanup terminates and reaps the VM child once"
else
  fail "runner cleanup terminates and reaps the VM child once"
fi

KILL_LOG="$TMP/virtiofsd-kills"
VM_PID=""
VIRTIOFSD_PIDS="12345"
IMAGE_LOCK_DESCRIPTOR=""
WORKSPACE_LOCK_DESCRIPTOR=""
REENTER_CLEANUP=true
kill() {
  printf '%s\n' "$1" >> "$KILL_LOG"
  if [ "$REENTER_CLEANUP" = true ]; then
    REENTER_CLEANUP=false
    cleanup
  fi
}
cleanup
unset -f kill
if [ "$(count_lines "$KILL_LOG")" -eq 1 ]; then
  pass "reentrant cleanup signals each virtiofs daemon once"
else
  fail "reentrant cleanup signals each virtiofs daemon once"
fi

INHERITED_LOCK="$TMP/inherited.lock"
CHILD_READY="$TMP/child-ready"
acquire_lock "$INHERITED_LOCK" INHERITED_LOCK_DESCRIPTOR
(
  close_lock_descriptors "$INHERITED_LOCK_DESCRIPTOR"
  touch "$CHILD_READY"
  sleep 10
) &
DESCRIPTOR_CHILD_PID=$!
for _ in $(seq 1 50); do
  [ -f "$CHILD_READY" ] && break
  sleep 0.02
done
if kill -0 "$DESCRIPTOR_CHILD_PID" 2>/dev/null \
  && [ -f "$CHILD_READY" ]; then
  DESCRIPTOR_CHILD_READY=true
else
  DESCRIPTOR_CHILD_READY=false
fi
release_lock "$INHERITED_LOCK_DESCRIPTOR"
if [ "$DESCRIPTOR_CHILD_READY" = true ] \
  && bash -c 'source "$1"; acquire_lock "$2" COMPETING_DESCRIPTOR' _ "$LOCK_BASH" "$INHERITED_LOCK"; then
  pass "runner child helper prevents lock descriptor inheritance"
else
  fail "runner child helper prevents lock descriptor inheritance"
fi
kill "$DESCRIPTOR_CHILD_PID" 2>/dev/null || true
wait "$DESCRIPTOR_CHILD_PID" 2>/dev/null || true

for signal_and_status in "HUP 129" "INT 130" "TERM 143"; do
  read -r SIGNAL EXPECTED_STATUS <<< "$signal_and_status"
  SIGNAL_RELEASE_LOG="$TMP/$SIGNAL-releases"
  SIGNAL_STATUS=0
  if [ -f "$RUNNER_LIFECYCLE_BASH" ] && bash -c '
    source "$1"
    RELEASE_LOG="$2"
    SIGNAL="$3"
    release_lock() { printf "%s\n" "$1" >> "$RELEASE_LOG"; }
    VM_PID=""
    VIRTIOFSD_PIDS=""
    IMAGE_LOCK_DESCRIPTOR="21"
    WORKSPACE_LOCK_DESCRIPTOR="22"
    install_cleanup_traps
    kill -s "$SIGNAL" "$$"
  ' _ "$RUNNER_LIFECYCLE_BASH" "$SIGNAL_RELEASE_LOG" "$SIGNAL"; then
    SIGNAL_STATUS=0
  else
    SIGNAL_STATUS=$?
  fi
  if [ "$SIGNAL_STATUS" -eq "$EXPECTED_STATUS" ] \
    && [ "$(count_lines "$SIGNAL_RELEASE_LOG")" -eq 2 ]; then
    pass "$SIGNAL exits with status $EXPECTED_STATUS and cleans up exactly once through EXIT"
  else
    fail "$SIGNAL exits with status $EXPECTED_STATUS and cleans up exactly once through EXIT"
  fi
done

assert_runner_line() {
  local expected="${1:?expected runner line}"
  local description="${2:?assertion description}"

  if grep -Fq "$expected" "$RUNNER_NIX"; then
    pass "$description"
  else
    fail "$description"
  fi
}

assert_runner_line 'DATA_DIR="'"''"'${XDG_DATA_HOME:-$HOME/.local/share}/aegis"' 'runner uses the host wide Aegis data root'
assert_runner_line 'CACHE_DIR="'"''"'${XDG_CACHE_HOME:-$HOME/.cache}/aegis/vzvm"' 'runner uses the shared macOS image cache'
assert_runner_line 'STATE_DIR="'"''"'${XDG_STATE_HOME:-$HOME/.local/state}/aegis/$WORKSPACE_ID"' 'runner retains workspace state isolation'
assert_runner_line 'mkdir -p "$DATA_DIR" "$CACHE_DIR" "$RUN_DIR" "$OPENCODE_STATE_DIR" "$OPENCODE_SHARE_DIR"' 'runner creates shared and workspace directories'
assert_runner_line 'SSH_KEY="$DATA_DIR/ssh_host_ed25519"' 'runner stores the SSH key under the Aegis data root'
assert_runner_line '${builtins.readFile ./store-cache.bash}' 'runner loads the store cache helpers'
assert_runner_line '${builtins.readFile ./ssh-key.bash}' 'runner loads the SSH key helper'
assert_runner_line '${builtins.readFile ./runner-lifecycle.bash}' 'runner loads the lifecycle helper'
assert_runner_line 'ensure_ssh_key "$SSH_KEY"' 'runner safely creates the host wide SSH key'
assert_runner_line 'wait_for_image_lock "$IMAGE_LOCK_FILE" IMAGE_LOCK_DESCRIPTOR' 'runner waits for the image lock'
assert_runner_line 'VZVM_STATE_DIR="$CACHE_DIR"' 'runner launches macOS with the shared cache directory'
assert_runner_line 'wait_for_guest_start "$VM_PID" "$VM_LOG"' 'runner waits for guest startup'
assert_runner_line 'remove_legacy_store_images "$STATE_DIR" "$CACHE_DIR"' 'runner removes legacy images after startup'
assert_runner_line 'acquire_lock "$WORKSPACE_LOCK_FILE" WORKSPACE_LOCK_DESCRIPTOR' 'runner acquires the workspace lock by descriptor'
assert_runner_line 'close_lock_descriptors "$IMAGE_LOCK_DESCRIPTOR" "$WORKSPACE_LOCK_DESCRIPTOR"' 'VM child closes its inherited lock descriptors'
assert_runner_line 'close_lock_descriptors "$WORKSPACE_LOCK_DESCRIPTOR"' 'runner children close inherited workspace lock descriptors'

truncate_line="$(grep -nF 'prepare_vm_log "$VM_LOG"' "$RUNNER_NIX" | cut -d: -f1 || true)"
branch_line="$(grep -nF 'if [ "$IS_DARWIN" = "true" ]; then' "$RUNNER_NIX" | tail --lines=1 | cut -d: -f1 || true)"
if [ -n "$truncate_line" ] \
  && [ -n "$branch_line" ] \
  && [ "$truncate_line" -lt "$branch_line" ]; then
  pass "runner clears the VM log before selecting either backend"
else
  fail "runner clears the VM log before selecting either backend"
fi

lock_line="$(grep -nF 'wait_for_image_lock "$IMAGE_LOCK_FILE" IMAGE_LOCK_DESCRIPTOR' "$RUNNER_NIX" | cut -d: -f1 || true)"
launch_line="$(grep -nF 'VZVM_STATE_DIR="$CACHE_DIR"' "$RUNNER_NIX" | cut -d: -f1 || true)"
start_line="$(grep -nF 'wait_for_guest_start "$VM_PID" "$VM_LOG"' "$RUNNER_NIX" | cut -d: -f1 || true)"
legacy_line="$(grep -nF 'remove_legacy_store_images "$STATE_DIR" "$CACHE_DIR"' "$RUNNER_NIX" | cut -d: -f1 || true)"
if [ -n "$lock_line" ] \
  && [ -n "$launch_line" ] \
  && [ -n "$start_line" ] \
  && [ -n "$legacy_line" ] \
  && [ "$lock_line" -lt "$launch_line" ] \
  && [ "$launch_line" -lt "$start_line" ] \
  && [ "$start_line" -lt "$legacy_line" ]; then
  pass "runner serializes macOS launch through guest startup and legacy cleanup"
else
  fail "runner serializes macOS launch through guest startup and legacy cleanup"
fi

if [ "$failures" -eq 0 ]; then
  echo "all runner cache tests passed"
else
  echo "$failures runner cache test(s) failed" >&2
  exit 1
fi
