#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

LOCK_BASH="${1:?path to helpers/lock.bash}"
# shellcheck disable=SC1090
source "$LOCK_BASH"

TMP="$(mktemp -d)"
trap 'jobs -pr | xargs kill 2>/dev/null || true; rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

FRESH_LOCK="$TMP/fresh.lock"
if acquire_lock "$FRESH_LOCK" FRESH_LOCK_DESCRIPTOR \
  && [ -f "$FRESH_LOCK" ] \
  && [ -n "$FRESH_LOCK_DESCRIPTOR" ]; then
  pass "fresh acquisition returns a descriptor"
else
  fail "fresh acquisition returns a descriptor"
fi

SECOND_LOCK="$TMP/second.lock"
if acquire_lock "$SECOND_LOCK" SECOND_LOCK_DESCRIPTOR \
  && [ "$FRESH_LOCK_DESCRIPTOR" != "$SECOND_LOCK_DESCRIPTOR" ]; then
  pass "one process can hold two locks"
else
  fail "one process can hold two locks"
fi

if bash -c 'source "$1"; acquire_lock "$2" COMPETING_DESCRIPTOR' _ "$LOCK_BASH" "$FRESH_LOCK"; then
  fail "competing process exclusion"
else
  pass "competing process exclusion"
fi

release_lock "$FRESH_LOCK_DESCRIPTOR"
if acquire_lock "$FRESH_LOCK" REACQUIRED_LOCK_DESCRIPTOR; then
  pass "explicit release permits reacquisition"
else
  fail "explicit release permits reacquisition"
fi
release_lock "$REACQUIRED_LOCK_DESCRIPTOR"

if bash -c 'source "$1"; acquire_lock "$2" OWNER_DESCRIPTOR' _ "$LOCK_BASH" "$SECOND_LOCK"; then
  fail "release closes only the supplied descriptor"
else
  pass "release closes only the supplied descriptor"
fi
release_lock "$SECOND_LOCK_DESCRIPTOR"

EXIT_LOCK="$TMP/exit.lock"
bash -c 'source "$1"; acquire_lock "$2" OWNER_DESCRIPTOR' _ "$LOCK_BASH" "$EXIT_LOCK"
if acquire_lock "$EXIT_LOCK" EXIT_LOCK_DESCRIPTOR; then
  pass "owner process exit releases lock"
else
  fail "owner process exit releases lock"
fi
release_lock "$EXIT_LOCK_DESCRIPTOR"

if [ "$failures" -eq 0 ]; then
  echo "all lock tests passed"
else
  echo "$failures lock test(s) failed" >&2
  exit 1
fi
