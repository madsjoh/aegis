{ pkgs, system }:

pkgs.writeShellApplication {
  name = "aegis";
  runtimeInputs = with pkgs; [ coreutils gh git gnugrep nix python3 util-linux ];
  text = ''
    HOST_PWD="$(pwd)"
    LOCK_FILE="$HOST_PWD/.aegis.lock"

    # 1. Acquire directory lock (one VM per directory).
    exec 9>"$LOCK_FILE"
    if ! flock --nonblock 9 2>/dev/null; then
      echo "Error: An Aegis VM is already active in this workspace." >&2
      exit 1
    fi

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
    GH_TOKEN="$(gh auth token 2>/dev/null || echo "$GH_TOKEN")"
    export GH_TOKEN
    export GITHUB_TOKEN="$GH_TOKEN"
    export GEMINI_API_KEY="$GEMINI_API_KEY"

    echo " Aegis Active [Host: ${system} | Guest: ${system}]"
    echo " OpenCode listening on port: $FREE_PORT | Workspace:$HOST_PWD"

    # 6. Launch the target MicroVM.
    nix run ".#aegis-vm-${system}" --impure -- "$@"
  '';
}
