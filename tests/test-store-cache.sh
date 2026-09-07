#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

LOCK_BASH="${1:?path to helpers/lock.bash}"
STORE_CACHE_BASH="${2:?path to helpers/store-cache.bash}"
# shellcheck disable=SC1090
source "$LOCK_BASH"
# shellcheck disable=SC1090
source "$STORE_CACHE_BASH"

TMP="$(mktemp -d)"
trap 'jobs -pr | xargs kill 2>/dev/null || true; rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# A fresh image lock is acquired immediately and returns its descriptor.
FRESH_LOCK="$TMP/fresh.lock"
if wait_for_image_lock "$FRESH_LOCK" FRESH_LOCK_DESCRIPTOR \
  && [ -n "$FRESH_LOCK_DESCRIPTOR" ]; then
  pass "fresh image lock acquisition"
else
  fail "fresh image lock acquisition"
fi
release_lock "$FRESH_LOCK_DESCRIPTOR"

# A live owner blocks the waiter until its process exits.
LIVE_LOCK="$TMP/live.lock"
(
  # shellcheck disable=SC1090
  source "$LOCK_BASH"
  acquire_lock "$LIVE_LOCK" OWNER_DESCRIPTOR
  touch "$TMP/owner-ready"
  while [ ! -e "$TMP/release-owner" ]; do
    sleep 0.01
  done
) &
OWNER_PID=$!
for _ in {1..100}; do
  [ -e "$TMP/owner-ready" ] && break
  sleep 0.01
done
(
  sleep() {
    touch "$TMP/waiter-blocked"
    command sleep "$@"
  }
  wait_for_image_lock "$LIVE_LOCK" WAITER_DESCRIPTOR
  touch "$TMP/waiter-acquired"
  release_lock "$WAITER_DESCRIPTOR"
) &
WAITER_PID=$!
for _ in {1..100}; do
  [ -e "$TMP/waiter-blocked" ] && break
  sleep 0.01
done
if [ -e "$TMP/waiter-blocked" ] \
  && [ ! -e "$TMP/waiter-acquired" ] \
  && kill -0 "$WAITER_PID" 2>/dev/null; then
  pass "live image lock owner blocks acquisition"
else
  fail "live image lock owner blocks acquisition"
fi
touch "$TMP/release-owner"
wait "$OWNER_PID"
if wait "$WAITER_PID" && [ -e "$TMP/waiter-acquired" ]; then
  pass "image lock acquisition recovers after owner exits"
else
  fail "image lock acquisition recovers after owner exits"
fi

# Guest startup succeeds only after the exact readiness line appears.
VM_LOG="$TMP/started.log"
: > "$VM_LOG"
(
  sleep 0.1
  printf '%s\n' "prefix [vzvm] guest started suffix" >> "$VM_LOG"
  sleep 300
) &
VM_PID=$!
wait_for_guest_start "$VM_PID" "$VM_LOG" &
GUEST_WAITER_PID=$!
sleep 0.2
if kill -0 "$GUEST_WAITER_PID" 2>/dev/null; then
  pass "guest start ignores partial line matches"
else
  fail "guest start ignores partial line matches"
fi
printf '%s\n' "[vzvm] guest started" >> "$VM_LOG"
if wait "$GUEST_WAITER_PID"; then
  pass "guest start detection"
else
  fail "guest start detection"
fi
kill "$VM_PID" 2>/dev/null || true
wait "$VM_PID" 2>/dev/null || true

# Guest startup fails when the VM exits before reporting readiness.
DEAD_VM_LOG="$TMP/dead.log"
printf '%s\n' "VM exited during startup." > "$DEAD_VM_LOG"
(sleep 0.1) &
DEAD_VM_PID=$!
if wait_for_guest_start "$DEAD_VM_PID" "$DEAD_VM_LOG"; then
  fail "VM exit before guest start"
else
  pass "VM exit before guest start"
fi
wait "$DEAD_VM_PID" 2>/dev/null || true

# VM exit takes precedence over a stale readiness marker.
STALE_VM_LOG="$TMP/stale.log"
printf '%s\n' "[vzvm] guest started" > "$STALE_VM_LOG"
(exit 0) &
STALE_VM_PID=$!
wait "$STALE_VM_PID"
if wait_for_guest_start "$STALE_VM_PID" "$STALE_VM_LOG"; then
  fail "VM exit takes precedence over stale readiness"
else
  pass "VM exit takes precedence over stale readiness"
fi

# Legacy cleanup removes only direct image children from a distinct workspace.
WORKSPACE_STATE_DIRECTORY="$TMP/workspace"
SHARED_CACHE_DIRECTORY="$TMP/shared"
mkdir -p "$WORKSPACE_STATE_DIRECTORY/nested" "$SHARED_CACHE_DIRECTORY"
touch "$WORKSPACE_STATE_DIRECTORY/store-old.img"
touch "$WORKSPACE_STATE_DIRECTORY/store-old.raw"
touch "$WORKSPACE_STATE_DIRECTORY/nested/store-nested.img"
remove_legacy_store_images "$WORKSPACE_STATE_DIRECTORY" "$SHARED_CACHE_DIRECTORY"
if [ -e "$WORKSPACE_STATE_DIRECTORY/store-old.img" ]; then
  pass "legacy image is preserved until a shared image exists"
else
  fail "legacy image is preserved until a shared image exists"
fi
touch "$SHARED_CACHE_DIRECTORY/store-current.img"
remove_legacy_store_images "$WORKSPACE_STATE_DIRECTORY" "$SHARED_CACHE_DIRECTORY"
if [ ! -e "$WORKSPACE_STATE_DIRECTORY/store-old.img" ] \
  && [ -e "$WORKSPACE_STATE_DIRECTORY/store-old.raw" ] \
  && [ -e "$WORKSPACE_STATE_DIRECTORY/nested/store-nested.img" ]; then
  pass "legacy image cleanup is restricted to direct matching children"
else
  fail "legacy image cleanup is restricted to direct matching children"
fi

# Cleanup does nothing when the workspace and shared cache are the same directory.
remove_legacy_store_images "$SHARED_CACHE_DIRECTORY/." "$SHARED_CACHE_DIRECTORY"
if [ -e "$SHARED_CACHE_DIRECTORY/store-current.img" ]; then
  pass "shared cache images are preserved"
else
  fail "shared cache images are preserved"
fi

if [ "$failures" -eq 0 ]; then
  echo "all store cache tests passed"
else
  echo "$failures store cache test(s) failed" >&2
  exit 1
fi
