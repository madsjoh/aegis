{ guestSystem }:

{ pkgs, system, flakeRef }:

let
  lib = pkgs.lib;
  isDarwin = lib.hasSuffix "-darwin" system;
  isDarwinShell = if isDarwin then "true" else "false";
in
pkgs.writeShellApplication {
  name = "aegis";
  runtimeInputs = with pkgs; [
    coreutils
    git
    gnugrep
    jq
    nix
    openssh
  ] ++ lib.optionals (!isDarwin) [ virtiofsd ];
  text = ''
    IS_DARWIN=${isDarwinShell}
    HOST_PWD="$(pwd)"
    WORKSPACE_ID="$(printf '%s' "$HOST_PWD" | sha256sum | cut -c1-16)"
    DATA_DIR="''${XDG_DATA_HOME:-$HOME/.local/share}/aegis/$WORKSPACE_ID"
    STATE_DIR="''${XDG_STATE_HOME:-$HOME/.local/state}/aegis/$WORKSPACE_ID"
    RUN_DIR="$STATE_DIR/run"
    OPENCODE_CONFIG_DIR="$STATE_DIR/opencode/config"
    OPENCODE_STATE_DIR="$STATE_DIR/opencode/state"
    OPENCODE_SHARE_DIR="$STATE_DIR/opencode/share"
    mkdir -p "$DATA_DIR" "$RUN_DIR" "$OPENCODE_CONFIG_DIR" "$OPENCODE_STATE_DIR" "$OPENCODE_SHARE_DIR"
    LOCK_DIR="$RUN_DIR/lock"

    ${builtins.readFile ./lock.bash}
    ${builtins.readFile ./config.bash}

    # 1. Acquire a directory lock, one VM per workspace.
    if ! acquire_lock "$LOCK_DIR"; then
      echo "Error: An Aegis VM is already active in this workspace." >&2
      exit 1
    fi
    VM_PID=""
    VIRTIOFSD_PIDS=""
    cleanup() {
      if [ -n "$VM_PID" ]; then
        kill "$VM_PID" 2>/dev/null || true
        wait "$VM_PID" 2>/dev/null || true
      fi
      for pid in $VIRTIOFSD_PIDS; do
        kill "$pid" 2>/dev/null || true
      done
      rm -rf "$LOCK_DIR"
    }
    trap cleanup EXIT INT TERM HUP

    # 2. Snapshot the user configuration into the workspace state on first run.
    # The guest mounts this writable copy, so edits never touch the global file.
    USER_CONFIG_DIR="''${XDG_CONFIG_HOME:-$HOME/.config}/aegis"
    USER_CONFIG="$USER_CONFIG_DIR/config.json"
    WORKSPACE_CONFIG_DIR="$STATE_DIR/.config/aegis"
    WORKSPACE_CONFIG="$WORKSPACE_CONFIG_DIR/config.json"
    if [ ! -f "$WORKSPACE_CONFIG" ]; then
      mkdir -p "$WORKSPACE_CONFIG_DIR"
      if [ -f "$USER_CONFIG" ]; then
        cp "$USER_CONFIG" "$WORKSPACE_CONFIG"
      else
        printf '{}\n' > "$WORKSPACE_CONFIG"
      fi
      chmod 600 "$WORKSPACE_CONFIG"
    fi
    MERGED="$(merge_json "$WORKSPACE_CONFIG")"

    # 3. VM resource settings with defaults.
    VM_CPU="$(printf '%s' "$MERGED" | jq -r '.vm.cpu // 4')"
    VM_MEM="$(printf '%s' "$MERGED" | jq -r '.vm.mem // 4096')"

    # 4. Metis leaf skills, disabled by default.
    VM_SKILL_ANTHROPIC="$(printf '%s' "$MERGED" | jq -r '.skills.anthropic // false')"
    VM_SKILL_MATTPOCOCK="$(printf '%s' "$MERGED" | jq -r '.skills.mattpocock // false')"
    VM_SKILL_VERCEL="$(printf '%s' "$MERGED" | jq -r '.skills.vercel // false')"

    # 5. Git identity, configuration first, host git configuration fallback.
    HOST_GIT_NAME="$(git config --global user.name 2>/dev/null || git config user.name 2>/dev/null || true)"
    HOST_GIT_EMAIL="$(git config --global user.email 2>/dev/null || git config user.email 2>/dev/null || true)"
    VM_GIT_NAME="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.name // empty')" "$HOST_GIT_NAME")"
    VM_GIT_EMAIL="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.email // empty')" "$HOST_GIT_EMAIL")"

    # 6. Workspace-derived identifiers, a persisted SSH key, and the SSH
    # transport details. Linux reaches the guest over vsock; macOS reaches it
    # over TCP through a forwarded host port.
    VM_MOUNT_TAG="ws_$(printf '%s' "$WORKSPACE_ID" | cut -c1-8)"
    VM_CID=$(( 3 + $(printf '%d' "0x$(printf '%s' "$WORKSPACE_ID" | cut -c1-4)") % 1000 ))
    VM_SSH_PORT=$(( 20000 + VM_CID ))
    SSH_KEY="$DATA_DIR/ssh_host_ed25519"
    if [ ! -f "$SSH_KEY" ]; then
      ssh-keygen -t ed25519 -f "$SSH_KEY" -N "" -q
    fi
    VM_SSH_PUBLIC_KEY="$(cat "$SSH_KEY.pub")"

    # 7. Export the environment consumed by the VM build.
    export HOST_WORKSPACE="$HOST_PWD"
    export HOST_CONFIG="$WORKSPACE_CONFIG_DIR"
    export HOST_STATE_DIR="$STATE_DIR"
    export VM_MOUNT_TAG VM_CID VM_SSH_PUBLIC_KEY VM_SSH_PORT
    VM_HOST_UID="$(id -u)"
    VM_HOST_GID="$(id -g)"
    export VM_HOST_UID VM_HOST_GID
    export VM_GIT_NAME VM_GIT_EMAIL VM_CPU VM_MEM
    export VM_SKILL_ANTHROPIC VM_SKILL_MATTPOCOCK VM_SKILL_VERCEL

    echo "Aegis Active [Host: ${system} | Guest: ${guestSystem system}]"
    echo "Workspace: $HOST_PWD"

    # 8. Build the target VM.
    echo "Building the guest VM..."
    VM_PATH="$(nix build "${flakeRef}#aegis-vm-${system}" --impure --no-link --print-out-paths)"

    # 9. Start the virtiofs daemons. macOS uses Apple's built-in shares, which
    # need no virtiofsd.
    if [ "$IS_DARWIN" != "true" ]; then
      echo "Starting the virtiofs daemons..."
      start_virtiofsd() {
        local socket="$1" shared_dir="$2" log="$3"
        virtiofsd \
          --socket-path="$socket" \
          --shared-dir="$shared_dir" \
          --thread-pool-size 4 \
          --cache=auto \
          --translate-uid "guest:1000:$VM_HOST_UID:1" \
          --translate-gid "guest:100:$VM_HOST_GID:1" \
          &> "$log" &
        VIRTIOFSD_PIDS="$VIRTIOFSD_PIDS $!"
      }
      VIRTIOFSD_SOCKETS=(
        "$RUN_DIR/workspace.sock"
        "$RUN_DIR/config.sock"
        "$RUN_DIR/opencode-config.sock"
        "$RUN_DIR/opencode-state.sock"
        "$RUN_DIR/opencode-share.sock"
      )
      for socket in "''${VIRTIOFSD_SOCKETS[@]}"; do
        rm -f "$socket"
      done
      start_virtiofsd "$RUN_DIR/workspace.sock" "$HOST_WORKSPACE" "$RUN_DIR/virtiofsd-workspace.log"
      start_virtiofsd "$RUN_DIR/config.sock" "$WORKSPACE_CONFIG_DIR" "$RUN_DIR/virtiofsd-config.log"
      start_virtiofsd "$RUN_DIR/opencode-config.sock" "$OPENCODE_CONFIG_DIR" "$RUN_DIR/virtiofsd-opencode-config.log"
      start_virtiofsd "$RUN_DIR/opencode-state.sock" "$OPENCODE_STATE_DIR" "$RUN_DIR/virtiofsd-opencode-state.log"
      start_virtiofsd "$RUN_DIR/opencode-share.sock" "$OPENCODE_SHARE_DIR" "$RUN_DIR/virtiofsd-opencode-share.log"
      for _ in $(seq 1 50); do
        ready=true
        for socket in "''${VIRTIOFSD_SOCKETS[@]}"; do
          if [ ! -S "$socket" ]; then
            ready=false
            break
          fi
        done
        if [ "$ready" = true ]; then
          break
        fi
        for pid in $VIRTIOFSD_PIDS; do
          if ! kill -0 "$pid" 2>/dev/null; then
            echo "Error: A virtiofs daemon exited before becoming ready." >&2
            for log in "$RUN_DIR"/virtiofsd-*.log; do
              cat "$log" >&2
            done
            exit 1
          fi
        done
        sleep 0.2
      done
    fi

    # 10. Run the VM in the background.
    VM_LOG="$RUN_DIR/vm.log"
    echo "Booting the guest..."
    if [ "$IS_DARWIN" = "true" ]; then
      VZVM_STATE_DIR="$STATE_DIR" "''${VM_PATH}/bin/run-aegis-vm" "$@" &> "$VM_LOG" &
    else
      "''${VM_PATH}/bin/run-aegis-vm" "$@" &> "$VM_LOG" &
    fi
    VM_PID=$!

    # 11. Wait for the guest SSH server.
    SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=2 -o BatchMode=yes)
    if [ "$IS_DARWIN" = "true" ]; then
      SSH_TARGET=(-p "$VM_SSH_PORT" agent@127.0.0.1)
      PROBE_TARGET=(-p "$VM_SSH_PORT" root@127.0.0.1)
    else
      SSH_TARGET=(agent@vsock/"$VM_CID")
      PROBE_TARGET=(root@vsock/"$VM_CID")
    fi
    echo "Waiting for the guest SSH server..."
    for _ in $(seq 1 120); do
      if ssh "''${SSH_OPTS[@]}" "''${PROBE_TARGET[@]}" true 2>/dev/null; then
        break
      fi
      if ! kill -0 "$VM_PID" 2>/dev/null; then
        echo "Error: The Aegis VM exited before SSH became available." >&2
        cat "$VM_LOG" >&2
        exit 1
      fi
      sleep 1
    done

    # 12. Attach OpenCode over SSH.
    ssh -tt -i "$SSH_KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "''${SSH_TARGET[@]}" "exec /run/current-system/sw/bin/opencode-shell"
  '';
}
