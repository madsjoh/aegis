#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

SCAFFOLD_BASH="${1:?path to helpers/scaffold.bash}"
# shellcheck disable=SC1090
source "$SCAFFOLD_BASH"

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

TMP="$(mktemp --directory)"
trap 'rm --recursive --force "$TMP"' EXIT

CONFIG_DIRECTORY="$TMP/config"
scaffold_config "$CONFIG_DIRECTORY"

if [ "$(cat "$CONFIG_DIRECTORY/vm.json")" = '{"cpu":4,"mem":4096}' ]; then
  pass "scaffold_config writes the default vm.json"
else
  fail "scaffold_config writes the default vm.json"
fi

if [ "$(cat "$CONFIG_DIRECTORY/skills.json")" = '{"anthropic":false,"mattpocock":false,"vercel":false}' ]; then
  pass "scaffold_config writes the default skills.json"
else
  fail "scaffold_config writes the default skills.json"
fi

if [ "$(cat "$CONFIG_DIRECTORY/opencode.jsonc")" = '{}' ]; then
  pass "scaffold_config writes the default opencode.jsonc"
else
  fail "scaffold_config writes the default opencode.jsonc"
fi

printf '%s\n' '{"cpu":8,"mem":8192}' > "$CONFIG_DIRECTORY/vm.json"
scaffold_config "$CONFIG_DIRECTORY"

if [ "$(cat "$CONFIG_DIRECTORY/vm.json")" = '{"cpu":8,"mem":8192}' ]; then
  pass "scaffold_config leaves an existing vm.json unchanged"
else
  fail "scaffold_config leaves an existing vm.json unchanged"
fi

if [ "$failures" -eq 0 ]; then
  echo "all scaffold tests passed"
else
  echo "$failures scaffold test(s) failed" >&2
  exit 1
fi
