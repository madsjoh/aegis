#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

DOCKERFILE="${1:-${DOCKERFILE:-builder/Dockerfile}}"
ENTRYPOINT="${2:-${ENTRYPOINT:-builder/entrypoint.bash}}"

failures=0

pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

assert_contains() {
  local file="${1:?file}"
  local pattern="${2:?pattern}"
  local description="${3:?description}"

  if grep -Eq "$pattern" "$file"; then
    pass "$description"
  else
    fail "$description"
  fi
}

if bash -n "$ENTRYPOINT"; then
  pass "the entrypoint has valid Bash syntax"
else
  fail "the entrypoint has valid Bash syntax"
fi

assert_contains "$DOCKERFILE" '^FROM --platform=linux/arm64 [^@[:space:]]+:[^[:space:]]+' \
  "the image targets a versioned ARM64 base"
assert_contains "$DOCKERFILE" '^VOLUME \["/nix"\]$' \
  "the image declares persistent Nix storage"
assert_contains "$DOCKERFILE" '^EXPOSE 22$' \
  "the image exposes SSH"
assert_contains "$DOCKERFILE" 'adduser .* builder|useradd .* builder' \
  "the image creates a dedicated builder account"
assert_contains "$DOCKERFILE" 'apk add --no-cache.* nix([ \\]|$)' \
  "the image installs Nix"
assert_contains "$DOCKERFILE" 'openssh' \
  "the image installs SSH"
assert_contains "$DOCKERFILE" 'https://cache\.nixos\.org https://microvm\.cachix\.org' \
  "the image configures the standard Nix and MicroVM caches"
assert_contains "$DOCKERFILE" 'cache\.nixos\.org-1:6NCHdD59X431o0gWypbAN9svvLtwWQLBvgXWoamSHB0=' \
  "the image trusts the standard Nix cache key"
assert_contains "$DOCKERFILE" 'microvm\.cachix\.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=' \
  "the image trusts the MicroVM cache key"
assert_contains "$ENTRYPOINT" 'AEGIS_BUILDER_PUBLIC_KEY' \
  "the entrypoint requires the Aegis public key"
assert_contains "$ENTRYPOINT" 'ssh-keygen -l' \
  "the entrypoint validates the public key"
assert_contains "$ENTRYPOINT" 'must contain one public key' \
  "the entrypoint rejects multiple authorized key lines"
assert_contains "$ENTRYPOINT" 'authorized_keys' \
  "the entrypoint installs the authorized key"
assert_contains "$ENTRYPOINT" 'chmod 0?600|install .* -m 0?600' \
  "the entrypoint restricts the authorized key permissions"
assert_contains "$ENTRYPOINT" 'exec .*nix-daemon.*sshd|nix-daemon.*&' \
  "the entrypoint runs the Nix daemon and SSH"
assert_contains "$DOCKERFILE" 'AllowUsers builder' \
  "SSH permits only the builder account"
assert_contains "$DOCKERFILE" 'KbdInteractiveAuthentication no' \
  "SSH disables keyboard interactive authentication"

if grep -Eq '^FROM .*@sha256:' "$DOCKERFILE"; then
  pass "the base image uses a verified immutable digest"
else
  printf 'CONCERN: the Alpine base image digest could not be verified in this environment.\n' >&2
fi

if grep -Eq 'eval|echo .*AEGIS_BUILDER_PUBLIC_KEY' "$ENTRYPOINT"; then
  fail "the entrypoint does not evaluate or echo the public key"
else
  pass "the entrypoint does not evaluate or echo the public key"
fi

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT
FAKE_BIN="$TEST_ROOT/bin"
mkdir -p "$FAKE_BIN" "$TEST_ROOT/home" "$TEST_ROOT/nix"

cat > "$FAKE_BIN/install" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset
mode=""
directory=false
while [ "$#" -gt 0 ]; do
  case "$1" in
    -d) directory=true; shift ;;
    -m) mode="$2"; shift 2 ;;
    -o | -g) shift 2 ;;
    *) break ;;
  esac
done
if "$directory"; then
  mkdir -p "$1"
  chmod "$mode" "$1"
else
  cp "$1" "$2"
  chmod "$mode" "$2"
fi
EOF

cat > "$FAKE_BIN/ssh-keygen" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset
if [ "$1" = "-l" ]; then
  grep -Eq '^ssh-ed25519 [A-Za-z0-9+/]+=*( .*)?$' "$3"
  exit
fi
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-f" ]; then
    printf 'private\n' > "$2"
    printf 'public\n' > "$2.pub"
    exit
  fi
  shift
done
exit 1
EOF

cat > "$FAKE_BIN/nix-daemon" <<'EOF'
#!/usr/bin/env bash
touch "$TEST_ROOT/nix-daemon-started"
trap 'exit 0' TERM
while :; do sleep 1; done
EOF

cat > "$FAKE_BIN/sshd" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" > "$TEST_ROOT/sshd-arguments"
exit 0
EOF

chmod u+x "$FAKE_BIN"/*

run_entrypoint() {
  PATH="$FAKE_BIN:$PATH" \
    TEST_ROOT="$TEST_ROOT" \
    AEGIS_BUILDER_HOME="$TEST_ROOT/home" \
    AEGIS_BUILDER_PUBLIC_KEY="${1-}" \
    AEGIS_BUILDER_SSH_DIRECTORY="$TEST_ROOT/nix/var/aegis/ssh" \
    AEGIS_BUILDER_SSHD="$FAKE_BIN/sshd" \
    bash "$ENTRYPOINT"
}

if run_entrypoint '' >/dev/null 2>&1; then
  fail "the entrypoint rejects a missing public key"
else
  pass "the entrypoint rejects a missing public key"
fi

if run_entrypoint $'ssh-ed25519 QUJD\nssh-ed25519 REVG' >/dev/null 2>&1; then
  fail "the entrypoint rejects multiple public keys"
else
  pass "the entrypoint rejects multiple public keys"
fi

if run_entrypoint 'ssh-ed25519 QUJD' \
  && [ "$(<"$TEST_ROOT/home/.ssh/authorized_keys")" = 'ssh-ed25519 QUJD' ] \
  && [ "$(stat -f '%Lp' "$TEST_ROOT/home/.ssh/authorized_keys" 2>/dev/null || stat -c '%a' "$TEST_ROOT/home/.ssh/authorized_keys")" = 600 ] \
  && [ -f "$TEST_ROOT/nix-daemon-started" ] \
  && [ -f "$TEST_ROOT/nix/var/aegis/ssh/ssh_host_ed25519_key" ] \
  && grep -Fq -- "-h $TEST_ROOT/nix/var/aegis/ssh/ssh_host_ed25519_key" "$TEST_ROOT/sshd-arguments"; then
  pass "the entrypoint installs the key and starts SSH with persistent host keys"
else
  fail "the entrypoint installs the key and starts SSH with persistent host keys"
fi

printf 'existing\n' > "$TEST_ROOT/nix/var/aegis/ssh/ssh_host_ed25519_key"
if run_entrypoint 'ssh-ed25519 QUJD' && [ "$(<"$TEST_ROOT/nix/var/aegis/ssh/ssh_host_ed25519_key")" = existing ]; then
  pass "the entrypoint reuses persistent host keys"
else
  fail "the entrypoint reuses persistent host keys"
fi

if [ "$failures" -eq 0 ]; then
  printf 'all builder image tests passed\n'
else
  printf '%s builder image test(s) failed\n' "$failures" >&2
  exit 1
fi
