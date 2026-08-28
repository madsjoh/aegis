#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

README="${1:-README.md}"
TEST_ROOT="$(mktemp -d)"
trap 'rm --force --recursive "$TEST_ROOT"' EXIT

if grep --ignore-case --quiet 'rancher desktop' "$README"; then
  printf '%s\n' "FAIL: the Docker builder documentation names a Docker provider" >&2
  exit 1
else
  printf '%s\n' "PASS: the Docker builder documentation is provider neutral"
fi

RECIPE="$TEST_ROOT/uninstall.bash"
FAKE_BIN="$TEST_ROOT/bin"
mkdir --parents "$FAKE_BIN"

awk '
  /^### Uninstall$/ { uninstall = 1; next }
  uninstall && /^```/ {
    if (fence) exit
    fence = 1
    next
  }
  fence { print }
' "$README" > "$RECIPE"

if [ ! -s "$RECIPE" ]; then
  printf '%s\n' "FAIL: the fenced uninstall recipe was not found" >&2
  exit 1
fi

cat > "$FAKE_BIN/awk" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset

if [ "$AEGIS_TEST_AWK_RESULT" = failure ]; then
  exit 1
fi
printf '%s\n' 'experimental-features = nix-command flakes'
EOF

cat > "$FAKE_BIN/mktemp" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset

printf '%s\n' "$AEGIS_TEST_ROOT/nix.conf.filtered"
EOF

cat > "$FAKE_BIN/rm" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset

exit 0
EOF

cat > "$FAKE_BIN/sudo" <<'EOF'
#!/usr/bin/env bash
set -o errexit -o nounset

case "${1:-}" in
  cp)
    printf '%s\n' called > "$AEGIS_TEST_ROOT/backup-called"
    ;;
  install)
    printf '%s\n' called > "$AEGIS_TEST_ROOT/install-called"
    ;;
  *)
    printf 'unexpected sudo command: %s\n' "$*" >&2
    exit 1
    ;;
esac
EOF

chmod u+x "$FAKE_BIN/awk" "$FAKE_BIN/mktemp" "$FAKE_BIN/rm" "$FAKE_BIN/sudo"

run_recipe() {
  AEGIS_TEST_AWK_RESULT="$1" \
    AEGIS_TEST_ROOT="$TEST_ROOT" \
    PATH="$FAKE_BIN:$PATH" \
    bash "$RECIPE"
}

if run_recipe failure; then
  printf '%s\n' "FAIL: the uninstall recipe succeeds when filtering fails" >&2
  exit 1
elif [ -e "$TEST_ROOT/install-called" ]; then
  printf '%s\n' "FAIL: the uninstall recipe installs after filtering fails" >&2
  exit 1
else
  printf '%s\n' "PASS: the uninstall recipe does not install after filtering fails"
fi

if run_recipe success && [ -e "$TEST_ROOT/install-called" ]; then
  printf '%s\n' "PASS: the uninstall recipe installs after filtering succeeds"
else
  printf '%s\n' "FAIL: the uninstall recipe does not install after filtering succeeds" >&2
  exit 1
fi
