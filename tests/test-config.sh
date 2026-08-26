#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

CONFIG_BASH="${1:?path to helpers/config.bash}"
# shellcheck disable=SC1090
source "$CONFIG_BASH"

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# resolve picks the first non-empty value.
if [ "$(resolve "" "fallback")" = "fallback" ]; then
  pass "resolve falls back when the first value is empty"
else
  fail "resolve falls back when the first value is empty"
fi

if [ "$(resolve "configured" "fallback")" = "configured" ]; then
  pass "resolve prefers the configured value"
else
  fail "resolve prefers the configured value"
fi

if ! resolve "" ""; then
  pass "resolve fails when no value is set"
else
  fail "resolve fails when no value is set"
fi

# merge_json skips missing files.
if [ "$(merge_json "/nonexistent.json")" = "{}" ]; then
  pass "merge_json returns an empty object for no files"
else
  fail "merge_json returns an empty object for no files"
fi

# merge_json lets later files win and preserves earlier keys.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
printf '%s\n' '{"git":{"name":"user"},"vm":{"cpu":4}}' > "$TMP/user.json"
printf '%s\n' '{"vm":{"cpu":8}}' > "$TMP/workspace.json"
MERGED="$(merge_json "$TMP/user.json" "$TMP/workspace.json")"
if [ "$(printf '%s' "$MERGED" | jq -r '.vm.cpu')" = "8" ] \
  && [ "$(printf '%s' "$MERGED" | jq -r '.git.name')" = "user" ]; then
  pass "merge_json gives later files precedence"
else
  fail "merge_json gives later files precedence"
fi

if [ "$failures" -eq 0 ]; then
  echo "all config tests passed"
else
  echo "$failures config test(s) failed" >&2
  exit 1
fi
