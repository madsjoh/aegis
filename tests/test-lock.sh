#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

LOCK_BASH="${1:?path to helpers/lock.bash}"
# shellcheck disable=SC1090
source "$LOCK_BASH"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# A fresh directory is acquired and records the pid.
FRESH="$TMP/fresh"
if acquire_lock "$FRESH" && [ -f "$FRESH/pid" ]; then
  pass "fresh acquire"
else
  fail "fresh acquire"
fi

# A live owner blocks acquisition.
LIVE="$TMP/live"
mkdir -p "$LIVE"
sleep 300 &
LIVE_PID=$!
echo "$LIVE_PID" > "$LIVE/pid"
if acquire_lock "$LIVE"; then
  fail "live owner block"
else
  pass "live owner block"
fi
kill "$LIVE_PID" 2>/dev/null || true
wait "$LIVE_PID" 2>/dev/null || true

# A stale lock with a dead pid is reclaimed.
STALE="$TMP/stale"
mkdir -p "$STALE"
echo "99999999" > "$STALE/pid"
if acquire_lock "$STALE" && [ -f "$STALE/pid" ]; then
  pass "stale reclaim"
else
  fail "stale reclaim"
fi

if [ "$failures" -eq 0 ]; then
  echo "all lock tests passed"
else
  echo "$failures lock test(s) failed" >&2
  exit 1
fi
