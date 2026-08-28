{ guestSystem, builderContext, builderContextIdentity, builderVersion }:

{ pkgs, system, flakeRef }:

let
  isDarwin = pkgs.lib.hasSuffix "-darwin" system;
  isDarwinShell = if isDarwin then "true" else "false";
  runtimeInputs = with pkgs; [ coreutils gawk git gnugrep jq nix openssh ]
    ++ pkgs.lib.optionals isDarwin [ docker-client netcat ];
  builderPreflight = pkgs.lib.optionalString isDarwin ''
    ${builtins.readFile ./builder.bash}
    ${builtins.readFile ./runner.bash}

    AEGIS_BUILDER_CONTEXT="''${AEGIS_BUILDER_CONTEXT:-${builderContext}}"
    AEGIS_BUILDER_CONTEXT_IDENTITY="''${AEGIS_BUILDER_CONTEXT_IDENTITY:-${builderContextIdentity}}"
    AEGIS_BUILDER_VERSION="''${AEGIS_BUILDER_VERSION:-${builderVersion}}"
    AEGIS_BUILDER_SETUP_COMMAND="nix run ${flakeRef}#builder-setup"
    export AEGIS_BUILDER_CONTEXT AEGIS_BUILDER_CONTEXT_IDENTITY AEGIS_BUILDER_SETUP_COMMAND AEGIS_BUILDER_VERSION
    runner_prepare_builder
  '';
in
pkgs.writeShellApplication {
  name = "aegis";
  inherit runtimeInputs;
  text = ''
    IS_DARWIN=${isDarwinShell}
    HOST_PWD="$(pwd)"
    LOCK_DIR="$HOST_PWD/.aegis.lock"

    ${builtins.readFile ./lock.bash}
    ${builtins.readFile ./config.bash}

    # 1. Acquire a directory lock, one VM per workspace.
    if ! acquire_lock "$LOCK_DIR"; then
      echo "Error: An Aegis VM is already active in this workspace." >&2
      exit 1
    fi
    VM_PID=""
    VIRTIOFSD_PID=""
    VIRTIOFSD_CONFIG_PID=""
    SSH_KEY=""
    cleanup() {
      if [ -n "$VM_PID" ]; then
        kill "$VM_PID" 2>/dev/null || true
        wait "$VM_PID" 2>/dev/null || true
      fi
      if [ -n "$VIRTIOFSD_PID" ]; then
        kill "$VIRTIOFSD_PID" 2>/dev/null || true
      fi
      if [ -n "$VIRTIOFSD_CONFIG_PID" ]; then
        kill "$VIRTIOFSD_CONFIG_PID" 2>/dev/null || true
      fi
      if [ -n "$SSH_KEY" ]; then
        rm -f "$SSH_KEY" "$SSH_KEY.pub"
      fi
      rm -rf "$LOCK_DIR"
    }
    trap cleanup EXIT INT TERM HUP

    # 2. Prevent the agent and host from committing the lock file and local secrets.
    if [ -d "$HOST_PWD/.git" ]; then
      EXCLUDE_FILE="$HOST_PWD/.git/info/exclude"
      mkdir -p "$(dirname "$EXCLUDE_FILE")"
      for ignore_target in ".aegis.lock" ".aegis/"; do
        if ! grep -q "^''${ignore_target}$" "$EXCLUDE_FILE" 2>/dev/null; then
          echo "$ignore_target" >> "$EXCLUDE_FILE"
        fi
      done
    fi

    # 3. Merge the user and workspace configuration, workspace wins.
    USER_CONFIG_DIR="''${XDG_CONFIG_HOME:-$HOME/.config}/aegis"
    USER_CONFIG="$USER_CONFIG_DIR/config.json"
    WORKSPACE_CONFIG="$HOST_PWD/.aegis/config.json"
    mkdir -p "$USER_CONFIG_DIR"
    MERGED="$(merge_json "$USER_CONFIG" "$WORKSPACE_CONFIG")"

    # 4. VM resource settings with defaults.
    VM_CPU="$(printf '%s' "$MERGED" | jq -r '.vm.cpu // 4')"
    VM_MEM="$(printf '%s' "$MERGED" | jq -r '.vm.mem // 4096')"
    VM_HYPERVISOR="$(printf '%s' "$MERGED" | jq -r '.vm.hypervisor // "qemu"')"

    # 5. Git identity, configuration first, host git configuration fallback.
    HOST_GIT_NAME="$(git config --global user.name 2>/dev/null || git config user.name 2>/dev/null || true)"
    HOST_GIT_EMAIL="$(git config --global user.email 2>/dev/null || git config user.email 2>/dev/null || true)"
    VM_GIT_NAME="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.name // empty')" "$HOST_GIT_NAME")"
    VM_GIT_EMAIL="$(resolve "$(printf '%s' "$MERGED" | jq -r '.git.email // empty')" "$HOST_GIT_EMAIL")"

    # 6. Workspace-derived identifiers, an ephemeral SSH key, and the SSH
    # transport details. Linux reaches the guest over vsock; macOS reaches it
    # over TCP through a forwarded host port.
    VM_MOUNT_TAG="ws_$(printf '%s' "$HOST_PWD" | md5sum | cut -c1-8)"
    VM_CID=$(( 3 + $(printf '%d' "0x$(printf '%s' "$HOST_PWD" | md5sum | cut -c1-4)") % 1000 ))
    VM_SSH_PORT=$(( 20000 + VM_CID ))
    SSH_KEY="/tmp/aegis-ssh-$VM_MOUNT_TAG"
    rm -f "$SSH_KEY" "$SSH_KEY.pub"
    ssh-keygen -t ed25519 -f "$SSH_KEY" -N "" -q
    VM_SSH_PUBLIC_KEY="$(cat "$SSH_KEY.pub")"

    # 7. Export the environment consumed by the VM build.
    export HOST_WORKSPACE="$HOST_PWD"
    export HOST_CONFIG="$USER_CONFIG_DIR"
    export VM_MOUNT_TAG VM_CID VM_SSH_PUBLIC_KEY VM_SSH_PORT
    VM_HOST_UID="$(id -u)"
    VM_HOST_GID="$(id -g)"
    export VM_HOST_UID VM_HOST_GID
    export VM_GIT_NAME VM_GIT_EMAIL VM_CPU VM_MEM VM_HYPERVISOR

    echo "Aegis Active [Host: ${system} | Guest: ${guestSystem system}]"
    echo "Workspace: $HOST_PWD"

    # 8. Build the target MicroVM.
    ${builderPreflight}
    VM_PATH="$(nix build "${flakeRef}#aegis-vm-${system}" --impure --no-link --print-out-paths)"

    # 9. Start the virtiofs daemons. macOS uses built-in 9p shares, which
    # need no virtiofsd.
    if [ "$IS_DARWIN" != "true" ]; then
      "''${VM_PATH}/bin/virtiofsd-run" &> /tmp/aegis-virtiofsd-"$VM_MOUNT_TAG".log &
      VIRTIOFSD_PID=$!
      "''${VM_PATH}/bin/virtiofsd-config-run" &> /tmp/aegis-virtiofsd-"$VM_MOUNT_TAG"-config.log &
      VIRTIOFSD_CONFIG_PID=$!
      for _ in $(seq 1 50); do
        if [ -S "/tmp/aegis-$VM_MOUNT_TAG.sock" ] && [ -S "/tmp/aegis-$VM_MOUNT_TAG-config.sock" ]; then
          break
        fi
        if ! kill -0 "$VIRTIOFSD_PID" 2>/dev/null; then
          echo "Error: The virtiofs daemon exited before becoming ready." >&2
          cat "/tmp/aegis-virtiofsd-$VM_MOUNT_TAG.log" >&2
          exit 1
        fi
        if ! kill -0 "$VIRTIOFSD_CONFIG_PID" 2>/dev/null; then
          echo "Error: The config virtiofs daemon exited before becoming ready." >&2
          cat "/tmp/aegis-virtiofsd-$VM_MOUNT_TAG-config.log" >&2
          exit 1
        fi
        sleep 0.2
      done
    fi

    # 10. Run the MicroVM in the background.
    VM_LOG="/tmp/aegis-vm-$VM_MOUNT_TAG.log"
    "''${VM_PATH}/bin/microvm-run" "$@" &> "$VM_LOG" &
    VM_PID=$!

    # 11. Wait for the guest SSH server.
    SSH_OPTS=(-i "$SSH_KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=2 -o BatchMode=yes)
    if [ "$IS_DARWIN" = "true" ]; then
      SSH_TARGET=(-p "$VM_SSH_PORT" agent@127.0.0.1)
    else
      SSH_TARGET=(agent@vsock/"$VM_CID")
    fi
    for _ in $(seq 1 120); do
      if ssh "''${SSH_OPTS[@]}" "''${SSH_TARGET[@]}" true 2>/dev/null; then
        break
      fi
      if ! kill -0 "$VM_PID" 2>/dev/null; then
        echo "Error: The Aegis VM exited before SSH became available." >&2
        cat "$VM_LOG" >&2
        exit 1
      fi
      sleep 1
    done

    # 12. Open opencode over SSH.
    ssh -i "$SSH_KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "''${SSH_TARGET[@]}"
  '';
}
