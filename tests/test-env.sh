#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

ENV_BASH="${1:?path to helpers/env.bash}"
# shellcheck disable=SC1090
source "$ENV_BASH"

failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# Unset secrets must not crash the runner.
unset GH_TOKEN GITHUB_TOKEN
if forward_secrets "" \
  && [ -z "${GH_TOKEN:-}" ] \
  && [ -z "${GITHUB_TOKEN:-}" ]; then
  pass "unset secrets do not crash"
else
  fail "unset secrets do not crash"
fi

# Set secrets are forwarded.
export GH_TOKEN="gh-env-token"
unset GITHUB_TOKEN
if forward_secrets "" \
  && [ "$GH_TOKEN" = "gh-env-token" ] \
  && [ "$GITHUB_TOKEN" = "gh-env-token" ]; then
  pass "set secrets are forwarded"
else
  fail "set secrets are forwarded"
fi

# A token from gh auth takes precedence over the GH_TOKEN environment.
export GH_TOKEN="env-token"
if forward_secrets "gh-cli-token" \
  && [ "$GH_TOKEN" = "gh-cli-token" ] \
  && [ "$GITHUB_TOKEN" = "gh-cli-token" ]; then
  pass "gh token takes precedence"
else
  fail "gh token takes precedence"
fi

if [ "$failures" -eq 0 ]; then
  echo "all env tests passed"
else
  echo "$failures env test(s) failed" >&2
  exit 1
fi
