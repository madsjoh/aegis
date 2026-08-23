{ guestSystem }:

{ pkgs, system, flakeRef }:

pkgs.writeShellApplication {
  name = "aegis";
  runtimeInputs = with pkgs; [ coreutils gh git gnugrep nix python3 ];
  text = ''
    HOST_PWD="$(pwd)"
    LOCK_DIR="$HOST_PWD/.aegis.lock"

    ${builtins.readFile ./lock.bash}

    # 1. Acquire a directory lock (one VM per workspace).
    if ! acquire_lock "$LOCK_DIR"; then
      echo "Error: An Aegis VM is already active in this workspace." >&2
      exit 1
    fi
    trap 'rm -rf "$LOCK_DIR"' EXIT

    # 2. Prevent the agent and host from committing the lock file and local secrets.
    if [ -d "$HOST_PWD/.git" ]; then
      EXCLUDE_FILE="$HOST_PWD/.git/info/exclude"
      mkdir -p "$(dirname "$EXCLUDE_FILE")"
      for ignore_target in ".aegis.lock" ".opencode-env"; do
        if ! grep -q "^''${ignore_target}$" "$EXCLUDE_FILE" 2>/dev/null; then
          echo "$ignore_target" >> "$EXCLUDE_FILE"
        fi
      done
    fi

    # 3. Request a dynamic free host TCP port.
    FREE_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("", 0)); print(s.getsockname()[1]); s.close()')

    # 4. Extract Git identity from the host.
    HOST_GIT_NAME="$(git config --global user.name 2>/dev/null || git config user.name 2>/dev/null || echo "")"
    HOST_GIT_EMAIL="$(git config --global user.email 2>/dev/null || git config user.email 2>/dev/null || echo "")"

    # 5. Export environment variables for the VM.
    export HOST_WORKSPACE="$HOST_PWD"
    export VM_HOST_PORT="$FREE_PORT"
    VM_MOUNT_TAG="ws_$(echo -n "$HOST_PWD" | md5sum | cut -c1-8)"
    export VM_MOUNT_TAG
    export VM_GIT_NAME="$HOST_GIT_NAME"
    export VM_GIT_EMAIL="$HOST_GIT_EMAIL"
    ${builtins.readFile ./env.bash}

    forward_secrets "$(gh auth token 2>/dev/null || true)"

    echo " Aegis Active [Host: ${system} | Guest: ${guestSystem system}]"
    echo " OpenCode listening on port: $FREE_PORT | Workspace:$HOST_PWD"

    # 6. Launch the target MicroVM.
    nix run "${flakeRef}#aegis-vm-${system}" --impure -- "$@"
  '';
}
