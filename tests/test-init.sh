#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

INIT_BASH="${1:?path to helpers/init.bash}"
# shellcheck disable=SC1090
source "$INIT_BASH"

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

if is_valid_json '{"a":1}'; then
  pass "is_valid_json accepts a JSON object"
else
  fail "is_valid_json accepts a JSON object"
fi

if is_valid_json 'not json'; then
  fail "is_valid_json rejects invalid JSON"
else
  pass "is_valid_json rejects invalid JSON"
fi

if [ "$(merge_object '{}' '{}')" = "{}" ]; then
  pass "merge_object returns an empty object for empty inputs"
else
  fail "merge_object returns an empty object for empty inputs"
fi

MERGED="$(merge_object '{"git":{"name":"user"},"vm":{"cpu":4}}' '{"vm":{"cpu":8}}')"
if [ "$(printf '%s' "$MERGED" | jq -r '.vm.cpu')" = "8" ] \
  && [ "$(printf '%s' "$MERGED" | jq -r '.git.name')" = "user" ]; then
  pass "merge_object lets additions override while preserving other keys"
else
  fail "merge_object lets additions override while preserving other keys"
fi

if [ "$failures" -eq 0 ]; then
  echo "all init tests passed"
else
  echo "$failures init test(s) failed" >&2
  exit 1
fi
