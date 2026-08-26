{ lib, pkgs, ... }:

let
  launcher = pkgs.writeShellApplication {
    name = "opencode-console";
    runtimeInputs = with pkgs; [ coreutils git jq opencode ];
    text = ''
      ${builtins.readFile ../helpers/config.bash}

      USER_CONFIG="/aegis/config.json"
      WORKSPACE_CONFIG="/workspace/.aegis/config.json"

      MERGED="$(mktemp)"
      cleanup_merged() { rm -f "$MERGED"; }
      trap cleanup_merged EXIT

      merge_json "$USER_CONFIG" "$WORKSPACE_CONFIG" > "$MERGED"

      # Terminal geometry captured from the host at boot.
      if [ -n "''${VM_COLS:-}" ] && [ -n "''${VM_ROWS:-}" ]; then
        stty cols "$VM_COLS" rows "$VM_ROWS" 2>/dev/null || true
      fi

      GITHUB_TOKEN_VALUE="$(jq -r '.github.token // empty' "$MERGED")"
      if [ -n "$GITHUB_TOKEN_VALUE" ]; then
        export GH_TOKEN="$GITHUB_TOKEN_VALUE"
        export GITHUB_TOKEN="$GITHUB_TOKEN_VALUE"
      fi

      GIT_NAME_VALUE="$(resolve "$(jq -r '.git.name // empty' "$MERGED")" "''${VM_GIT_NAME:-}")"
      GIT_EMAIL_VALUE="$(resolve "$(jq -r '.git.email // empty' "$MERGED")" "''${VM_GIT_EMAIL:-}")"
      if [ -n "$GIT_NAME_VALUE" ]; then
        git config --global user.name "$GIT_NAME_VALUE"
      fi
      if [ -n "$GIT_EMAIL_VALUE" ]; then
        git config --global user.email "$GIT_EMAIL_VALUE"
      fi

      if jq -e '.opencode.auth' "$MERGED" >/dev/null 2>&1; then
        mkdir -p "$HOME/.local/share/opencode"
        jq '.opencode.auth' "$MERGED" > "$HOME/.local/share/opencode/auth.json"
      fi

      rm -f "$MERGED"
      trap - EXIT

      cd /workspace
      exec opencode /workspace
    '';
  };
in
{
  system.stateVersion = lib.trivial.release;

  systemd.services.opencode-console = {
    wantedBy = [ "multi-user.target" ];
    after = [ "home-manager-agent.service" ];

    environment = {
      TERM = "xterm-256color";
      VM_COLS = builtins.getEnv "VM_COLS";
      VM_ROWS = builtins.getEnv "VM_ROWS";
      VM_GIT_NAME = builtins.getEnv "VM_GIT_NAME";
      VM_GIT_EMAIL = builtins.getEnv "VM_GIT_EMAIL";
    };

    serviceConfig = {
      User = "agent";
      WorkingDirectory = "/workspace";
      ExecStart = lib.getExe launcher;
      StandardInput = "tty";
      StandardOutput = "tty";
      StandardError = "journal";
      TTYPath = "/dev/ttyS0";
      TTYReset = "yes";
      Restart = "no";
      SuccessAction = "poweroff";
      FailureAction = "poweroff";
    };
  };
}
