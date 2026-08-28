#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

DARWIN_RUNNER="${1:?Darwin runner path}"
LINUX_RUNNER="${2:?Linux runner path}"

failures=0

pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

if grep --fixed-strings --quiet 'runner_prepare_builder' "$DARWIN_RUNNER" \
  && grep --fixed-strings --quiet 'AEGIS_BUILDER_CONTEXT=' "$DARWIN_RUNNER" \
  && grep --fixed-strings --quiet 'nix run path:/aegis#builder-setup' "$DARWIN_RUNNER" \
  && grep --extended-regexp --quiet '/nix/store/[^/]+-docker[^/]*/bin' "$DARWIN_RUNNER"; then
  pass "Darwin runner embeds builder preflight and Docker runtime input"
else
  fail "Darwin runner embeds builder preflight and Docker runtime input"
fi

if grep --fixed-strings --quiet 'runner_prepare_builder' "$LINUX_RUNNER" \
  || grep --fixed-strings --quiet 'AEGIS_BUILDER_CONTEXT=' "$LINUX_RUNNER" \
  || grep --fixed-strings --quiet '#builder-setup' "$LINUX_RUNNER" \
  || grep --extended-regexp --quiet '/nix/store/[^/]+-docker[^/]*/bin' "$LINUX_RUNNER"; then
  fail "Linux runner omits builder logic and Docker runtime input"
else
  pass "Linux runner omits builder logic and Docker runtime input"
fi

if [ "$failures" -eq 0 ]; then
  printf '%s\n' "all generated runner tests passed"
else
  printf '%s generated runner test(s) failed\n' "$failures" >&2
  exit 1
fi
