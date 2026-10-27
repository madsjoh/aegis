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
    util-linux
  ] ++ lib.optionals (!isDarwin) [ virtiofsd ];
  text = ''
    USER_CONFIG_DIR="''${XDG_CONFIG_HOME:-$HOME/.config}/aegis"
    USER_CONFIG="$USER_CONFIG_DIR/config.json"

    IS_DARWIN=${isDarwinShell}
    HOST_PWD="$(pwd)"
    WORKSPACE_ID="$(printf '%s' "$HOST_PWD" | sha256sum | cut -c1-16)"
    DATA_DIR="''${XDG_DATA_HOME:-$HOME/.local/share}/aegis"
    CACHE_DIR="''${XDG_CACHE_HOME:-$HOME/.cache}/aegis/vzvm"
    STATE_DIR="''${XDG_STATE_HOME:-$HOME/.local/state}/aegis/$WORKSPACE_ID"
    RUN_DIR="$STATE_DIR/run"
    OPENCODE_STATE_DIR="$STATE_DIR/opencode/state"
    OPENCODE_SHARE_DIR="$STATE_DIR/opencode/share"
    mkdir -p "$DATA_DIR" "$CACHE_DIR" "$RUN_DIR" "$OPENCODE_STATE_DIR" "$OPENCODE_SHARE_DIR"
    WORKSPACE_LOCK_FILE="$RUN_DIR/lock"
    IMAGE_LOCK_FILE="$CACHE_DIR/lock"

    ${builtins.readFile ./lock.bash}
    ${builtins.readFile ./config.bash}
    ${builtins.readFile ./scaffold.bash}
    ${builtins.readFile ./runner-lifecycle.bash}
    ${builtins.readFile ./ssh-key.bash}
    ${builtins.readFile ./store-cache.bash}
    ${builtins.readFile ./wait-for-ssh.bash}

    usage() {
      echo "Usage: aegis [run [args...] | help]"
      echo "  run   Boot the guest VM (default)."
      echo "  help  Show this help."
    }

    if [ "$#" -gt 0 ]; then
      case "$1" in
        run) shift ;;
        help|--help|-h) usage; exit 0 ;;
        *) echo "Error: unknown command '$1'." >&2; usage >&2; exit 1 ;;
      esac
    fi

    scaffold_config "$USER_CONFIG_DIR"

    # 1. Acquire the workspace lock, one VM per workspace.
    if ! acquire_lock "$WORKSPACE_LOCK_FILE" WORKSPACE_LOCK_DESCRIPTOR; then
      echo "Error: An Aegis VM is already active in this workspace." >&2
      exit 1
    fi
    VM_PID=""
    VIRTIOFSD_PIDS=""
    IMAGE_LOCK_DESCRIPTOR=""
    install_cleanup_traps

    # 2. Read the current user configuration on every run.
    MERGED="$(merge_json "$USER_CONFIG")"

    # 3. VM resource settings with defaults.
    VM_CPU="$(jq -r '.cpu // 4' "$USER_CONFIG_DIR/vm.json")"
    VM_MEM="$(jq -r '.mem // 4096' "$USER_CONFIG_DIR/vm.json")"

    # 4. Metis leaf skills, disabled by default.
    VM_SKILL_ANTHROPIC="$(jq -r '.anthropic // false' "$USER_CONFIG_DIR/skills.json")"
    VM_SKILL_MATTPOCOCK="$(jq -r '.mattpocock // false' "$USER_CONFIG_DIR/skills.json")"
    VM_SKILL_VERCEL="$(jq -r '.vercel // false' "$USER_CONFIG_DIR/skills.json")"

    # 5. Git identity, configuration first, host git configuration fallback.
    HOST_GIT_NAME="$(git config --global user.name 2>/dev/null || git config user.name 2>/dev/null || true)"
    HOST_GIT_EMAIL="$(git config --global user.email 2>/dev/null || git config user.email 2>/dev/null || true)"
    VM_GIT_NAME="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.name // empty')" "$HOST_GIT_NAME")"
    VM_GIT_EMAIL="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.email // empty')" "$HOST_GIT_EMAIL")"

    # 6. Workspace-derived identifiers, the host wide SSH key, and the SSH
    # transport details. Linux reaches the guest over vsock; macOS reaches it
    # over TCP through a forwarded host port.
    VM_MOUNT_TAG="ws_$(printf '%s' "$WORKSPACE_ID" | cut -c1-8)"
    VM_CID=$(( 3 + $(printf '%d' "0x$(printf '%s' "$WORKSPACE_ID" | cut -c1-4)") % 1000 ))
    VM_SSH_PORT=$(( 20000 + VM_CID ))
    SSH_KEY="$DATA_DIR/ssh_host_ed25519"
    ensure_ssh_key "$SSH_KEY"
    VM_SSH_PUBLIC_KEY="$(cat "$SSH_KEY.pub")"
    mkdir -p "$USER_CONFIG_DIR/ssh"
    printf '%s\n' "$VM_SSH_PUBLIC_KEY" > "$USER_CONFIG_DIR/ssh/authorized_keys"

    # 7. Export the environment consumed by the VM build.
    export HOST_WORKSPACE="$HOST_PWD"
    export HOST_CONFIG="$USER_CONFIG_DIR"
    export HOST_STATE_DIR="$STATE_DIR"
    export VM_MOUNT_TAG VM_CID VM_SSH_PORT
    VM_HOST_UID="$(id -u)"
    VM_HOST_GID="$(id -g)"
    export VM_HOST_UID VM_HOST_GID
    export VM_GIT_NAME VM_GIT_EMAIL VM_CPU VM_MEM
    export VM_SKILL_ANTHROPIC VM_SKILL_MATTPOCOCK VM_SKILL_VERCEL

    echo "Aegis [Host: ${system} | Guest: ${guestSystem system}]"
    echo "Workspace : $HOST_PWD"
    echo "Config    : $USER_CONFIG"
    echo "State     : $STATE_DIR"

    # 8. Build the target VM.
    echo "Building the guest VM..."
    VM_PATH="$(nix build "${flakeRef}#aegis-vm-${system}" --impure --no-link --print-out-paths)"
    if [ "$IS_DARWIN" != "true" ]; then
      REG_INFO_BASENAME="$(nix eval --raw --impure --expr '
        let
          flake = builtins.getFlake "${flakeRef}";
          pkgs = flake.inputs.nixpkgs.legacyPackages.${system};
          vm = flake.nixosConfigurations."aegis-vm-${system}";
          regInfo = pkgs.closureInfo { rootPaths = vm.config.virtualisation.additionalPaths; };
        in builtins.baseNameOf regInfo
      ')"
    fi

    # 9. Start the virtiofs daemons. macOS uses Apple's built-in shares, which
    # need no virtiofsd.
    if [ "$IS_DARWIN" != "true" ]; then
      echo "Starting the virtiofs daemons..."
      start_virtiofsd() {
        local socket="$1" shared_dir="$2" log="$3"
        (
          close_lock_descriptors "$WORKSPACE_LOCK_DESCRIPTOR"
          virtiofsd \
            --socket-path="$socket" \
            --shared-dir="$shared_dir" \
            --thread-pool-size 4 \
            --cache=auto \
            --translate-uid "guest:1000:$VM_HOST_UID:1" \
            --translate-gid "guest:100:$VM_HOST_GID:1"
        ) &> "$log" &
        VIRTIOFSD_PIDS="$VIRTIOFSD_PIDS $!"
      }
      VIRTIOFSD_SOCKETS=(
        "$RUN_DIR/workspace.sock"
        "$RUN_DIR/config.sock"
        "$RUN_DIR/opencode-state.sock"
        "$RUN_DIR/opencode-share.sock"
      )
      for socket in "''${VIRTIOFSD_SOCKETS[@]}"; do
        rm -f "$socket"
      done
      start_virtiofsd "$RUN_DIR/workspace.sock" "$HOST_WORKSPACE" "$RUN_DIR/virtiofsd-workspace.log"
      start_virtiofsd "$RUN_DIR/config.sock" "$USER_CONFIG_DIR" "$RUN_DIR/virtiofsd-config.log"
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
    prepare_vm_log "$VM_LOG"
    if [ "$IS_DARWIN" = "true" ]; then
      wait_for_image_lock "$IMAGE_LOCK_FILE" IMAGE_LOCK_DESCRIPTOR
      link_shared_store_images "$STATE_DIR" "$CACHE_DIR"
      (
        close_lock_descriptors "$IMAGE_LOCK_DESCRIPTOR" "$WORKSPACE_LOCK_DESCRIPTOR"
        VZVM_STATE_DIR="$STATE_DIR" "''${VM_PATH}/bin/run-aegis-vm" "$@"
      ) &>> "$VM_LOG" &
    else
      (
        close_lock_descriptors "$WORKSPACE_LOCK_DESCRIPTOR"
        QEMU_KERNEL_PARAMS="regInfo=/nix/.ro-store/''${REG_INFO_BASENAME}/registration" "''${VM_PATH}/bin/run-aegis-vm" "$@"
      ) &>> "$VM_LOG" &
    fi
    VM_PID=$!
    if [ "$IS_DARWIN" = "true" ]; then
      if ! wait_for_guest_start "$VM_PID" "$VM_LOG"; then
        cat "$VM_LOG" >&2
        exit 1
      fi
      publish_store_images "$STATE_DIR" "$CACHE_DIR"
      release_lock "$IMAGE_LOCK_DESCRIPTOR"
      IMAGE_LOCK_DESCRIPTOR=""
    fi

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
    wait_for_ssh

    # 12. Attach OpenCode over SSH.
    ssh -tt -i "$SSH_KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "''${SSH_TARGET[@]}" "exec /run/current-system/sw/bin/opencode-shell"
  '';
}
