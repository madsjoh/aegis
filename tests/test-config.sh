#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

CONFIG_BASH="${1:?path to helpers/config.bash}"
# shellcheck disable=SC1090
source "$CONFIG_BASH"

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

TMP="$(mktemp --directory)"
trap 'rm --recursive --force "$TMP"' EXIT

# prepare_opencode forwards the OpenCode configuration verbatim, including
# JSONC comments that OpenCode itself validates and strips.
printf '%s\n' '// forward this file to OpenCode as is' '{"model":"example/model","permission":"allow"}' > "$TMP/opencode.jsonc"
EXPECTED="$(cat "$TMP/opencode.jsonc")"
if declare -F prepare_opencode >/dev/null \
  && prepare_opencode "$TMP/opencode.jsonc" \
  && [ "$OPENCODE_CONFIG_CONTENT" = "$EXPECTED" ]; then
  pass "prepare_opencode forwards the OpenCode configuration verbatim"
else
  fail "prepare_opencode forwards the OpenCode configuration verbatim"
fi

# prepare_opencode fails when the file is missing.
if declare -F prepare_opencode >/dev/null \
  && ! prepare_opencode "$TMP/missing.jsonc"; then
  pass "prepare_opencode fails on a missing file"
else
  fail "prepare_opencode fails on a missing file"
fi

for removed_function in opencode_auth_entries import_opencode_auth; do
  if declare -F "$removed_function" >/dev/null; then
    fail "$removed_function must be removed"
  else
    pass "$removed_function is removed"
  fi
done

if [ "$failures" -eq 0 ]; then
  echo "all config tests passed"
else
  echo "$failures config test(s) failed" >&2
  exit 1
fi
