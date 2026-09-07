#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

WAIT_FOR_SSH_BASH="${1:?path to helpers/wait-for-ssh.bash}"
# shellcheck disable=SC1090
source "$WAIT_FOR_SSH_BASH"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# Readiness succeeds after more probes than the former fixed limit allowed.
set +o errexit
long_startup_output="$(
  (
  attempts=0
  VM_PID=123
  VM_LOG="$TMP/running-vm.log"
  SSH_OPTS=()
  PROBE_TARGET=()

  ssh() {
    attempts=$((attempts + 1))
    echo "SSH probe failed." >&2
    [ "$attempts" -gt 301 ]
  }
  kill() { return 0; }
  sleep() { return 0; }

  wait_for_ssh
  ) 2>&1
)"
long_startup_status=$?
set -o errexit
if [ "$long_startup_status" -eq 0 ]; then
  pass "readiness survives 301 failed probes"
else
  fail "readiness survives 301 failed probes"
fi

expected_progress=$'Still waiting for the guest SSH server...\nStill waiting for the guest SSH server...'
if [ "$long_startup_output" = "$expected_progress" ]; then
  pass "readiness reports progress every 150 failed probes"
else
  fail "readiness reports progress every 150 failed probes"
fi

if [[ "$long_startup_output" != *"SSH probe failed."* ]]; then
  pass "readiness suppresses SSH probe errors"
else
  fail "readiness suppresses SSH probe errors"
fi

# A dead VM fails immediately and prints its diagnostics.
printf '%s\n' "VM terminated during startup." > "$TMP/dead-vm.log"
set +o errexit
dead_vm_output="$({
  VM_PID=456
  VM_LOG="$TMP/dead-vm.log"
  SSH_OPTS=()
  PROBE_TARGET=()

  ssh() { return 1; }
  kill() { return 1; }
  sleep() { echo "SLEEP_CALLED"; }

  wait_for_ssh
} 2>&1)"
dead_vm_status=$?
set -o errexit

if [ "$dead_vm_status" -eq 1 ]; then
  pass "dead VM returns status 1"
else
  fail "dead VM returns status 1"
fi

if [[ "$dead_vm_output" == *"Error: The Aegis VM exited before SSH became available."* ]]; then
  pass "dead VM prints the readiness error"
else
  fail "dead VM prints the readiness error"
fi

if [[ "$dead_vm_output" == *"VM terminated during startup."* ]]; then
  pass "dead VM prints the VM log"
else
  fail "dead VM prints the VM log"
fi

if [[ "$dead_vm_output" != *"SLEEP_CALLED"* ]]; then
  pass "dead VM detection does not sleep"
else
  fail "dead VM detection does not sleep"
fi

if [ "$failures" -eq 0 ]; then
  echo "all SSH readiness tests passed"
else
  echo "$failures SSH readiness test(s) failed" >&2
  exit 1
fi
