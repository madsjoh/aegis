{ metis }:

{ pkgs, lib, ... }:

let
  envBool = name: default:
    let
      value = builtins.getEnv name;
    in
    if value == "" then default else builtins.fromJSON value;

  opencodeShell = pkgs.writeShellApplication {
    name = "opencode-shell";
    runtimeInputs = with pkgs; [ coreutils git jq opencode ];
    text = ''
      ${builtins.readFile ../helpers/config.bash}

      USER_CONFIG="/aegis/config.json"
      WORKSPACE_CONFIG="/workspace/.aegis/config.json"

      MERGED="$(mktemp)"
      cleanup_merged() { rm -f "$MERGED"; }
      trap cleanup_merged EXIT

      merge_json "$USER_CONFIG" "$WORKSPACE_CONFIG" > "$MERGED"

      GITHUB_TOKEN_VALUE="$(jq -r '.github.token // empty' "$MERGED")"
      if [ -n "$GITHUB_TOKEN_VALUE" ]; then
        export GH_TOKEN="$GITHUB_TOKEN_VALUE"
      fi

      GIT_NAME_VALUE="$(resolve "$(jq -r '.git.name // empty' "$MERGED")" "''${VM_GIT_NAME:-}")"
      GIT_EMAIL_VALUE="$(resolve "$(jq -r '.git.email // empty' "$MERGED")" "''${VM_GIT_EMAIL:-}")"
      if [ -n "$GIT_NAME_VALUE" ]; then
        export GIT_AUTHOR_NAME="$GIT_NAME_VALUE"
        export GIT_COMMITTER_NAME="$GIT_NAME_VALUE"
      fi
      if [ -n "$GIT_EMAIL_VALUE" ]; then
        export GIT_AUTHOR_EMAIL="$GIT_EMAIL_VALUE"
        export GIT_COMMITTER_EMAIL="$GIT_EMAIL_VALUE"
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
  users.users.agent = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    home = "/home/agent";
    shell = lib.getExe opencodeShell;
    openssh.authorizedKeys.keys = [
      (builtins.getEnv "VM_SSH_PUBLIC_KEY")
    ];
  };

  users.users.root.openssh.authorizedKeys.keys = [
    (builtins.getEnv "VM_SSH_PUBLIC_KEY")
  ];

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;

    users.agent = {
      imports = [
        metis.homeManagerModules.default
      ];

      metis.opencode = {
        enable = true;
        skills.anthropic.enable = envBool "VM_SKILL_ANTHROPIC" false;
        skills.mattpocock.enable = envBool "VM_SKILL_MATTPOCOCK" false;
        skills.vercel.enable = envBool "VM_SKILL_VERCEL" false;
      };

      programs.git = {
        enable = true;
        ignores = [
          ".aegis.lock"
          ".aegis/"
          ".opencode/"
        ];
      };

      home.packages = with pkgs; [
        opencode
        gh
        git
        ripgrep
        fd
        bash
        fish
      ];

      home.stateVersion = "24.05";
    };
  };
}
