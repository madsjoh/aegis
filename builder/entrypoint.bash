#!/usr/bin/env bash
set -o errexit -o nounset -o pipefail

builder_home="${AEGIS_BUILDER_HOME:-/home/builder}"
host_key_directory="${AEGIS_BUILDER_SSH_DIRECTORY:-/nix/var/aegis/ssh}"
sshd="${AEGIS_BUILDER_SSHD:-/usr/sbin/sshd}"

if [ -z "${AEGIS_BUILDER_PUBLIC_KEY:-}" ]; then
  printf 'aegis builder: AEGIS_BUILDER_PUBLIC_KEY is required.\n' >&2
  exit 1
fi
if [[ "$AEGIS_BUILDER_PUBLIC_KEY" == *$'\n'* || "$AEGIS_BUILDER_PUBLIC_KEY" == *$'\r'* ]]; then
  printf 'aegis builder: AEGIS_BUILDER_PUBLIC_KEY must contain one public key.\n' >&2
  exit 1
fi
case "$AEGIS_BUILDER_PUBLIC_KEY" in
  ssh-ed25519\ * | ecdsa-sha2-nistp256\ * | ecdsa-sha2-nistp384\ * | ecdsa-sha2-nistp521\ * | ssh-rsa\ *)
    ;;
  *)
    printf 'aegis builder: AEGIS_BUILDER_PUBLIC_KEY is not a supported public key.\n' >&2
    exit 1
    ;;
esac

temporary_key="$(mktemp)"
trap 'rm -f "$temporary_key"' EXIT
printf '%s\n' "$AEGIS_BUILDER_PUBLIC_KEY" > "$temporary_key"
if ! ssh-keygen -l -f "$temporary_key" >/dev/null 2>&1; then
  printf 'aegis builder: AEGIS_BUILDER_PUBLIC_KEY is invalid.\n' >&2
  exit 1
fi

install -d -m 0700 -o builder -g builder "$builder_home/.ssh"
install -m 0600 -o builder -g builder "$temporary_key" "$builder_home/.ssh/authorized_keys"
unset AEGIS_BUILDER_PUBLIC_KEY
rm -f "$temporary_key"
trap - EXIT

install -d -m 0700 "$host_key_directory"
host_key_arguments=()
for key_type in ed25519 ecdsa rsa; do
  host_key="$host_key_directory/ssh_host_${key_type}_key"
  if [ ! -f "$host_key" ]; then
    ssh-keygen -q -N '' -t "$key_type" -f "$host_key"
  fi
  chmod 0600 "$host_key"
  host_key_arguments+=( -h "$host_key" )
done

nix-daemon &
nix_daemon_pid=$!
"$sshd" -D -e "${host_key_arguments[@]}" &
sshd_pid=$!

stop_services() {
  kill "$nix_daemon_pid" "$sshd_pid" 2>/dev/null || true
  wait "$nix_daemon_pid" "$sshd_pid" 2>/dev/null || true
}
trap stop_services EXIT INT TERM

set +o errexit
wait -n "$nix_daemon_pid" "$sshd_pid"
status=$?
set -o errexit
exit "$status"
