{ metis }:

{ pkgs, lib, ... }:

let
  envBool = name: default:
    let
      value = builtins.getEnv name;
    in
    if value == "" then default else builtins.fromJSON value;

  envStrOrNull = name:
    let
      value = builtins.getEnv name;
    in
    if value == "" then null else value;

  gitUserName = envStrOrNull "VM_GIT_NAME";
  gitUserEmail = envStrOrNull "VM_GIT_EMAIL";

  maven = pkgs.maven.override { jdk_headless = pkgs.temurin-bin-26; };

  opencodeShell = pkgs.writeShellApplication {
    name = "opencode-shell";
    runtimeInputs = with pkgs; [ coreutils git jq opencode ];
    text = ''
      ${builtins.readFile ../helpers/config.bash}

      USER_CONFIG="/aegis/config.json"

      MERGED="$(mktemp)"
      cleanup_merged() { rm -f "$MERGED"; }
      trap cleanup_merged EXIT

      merge_json "$USER_CONFIG" > "$MERGED"

      GITHUB_TOKEN_VALUE="$(jq -r '.github.token // empty' "$MERGED")"
      if [ -n "$GITHUB_TOKEN_VALUE" ]; then
        export GH_TOKEN="$GITHUB_TOKEN_VALUE"
      fi

      GIT_NAME_VALUE="$(resolve "$(jq -r '.git.name // empty' "$MERGED")" "${builtins.getEnv "VM_GIT_NAME"}")"
      GIT_EMAIL_VALUE="$(resolve "$(jq -r '.git.email // empty' "$MERGED")" "${builtins.getEnv "VM_GIT_EMAIL"}")"
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
        chmod 600 "$HOME/.local/share/opencode/auth.json"
      fi

      rm -f "$MERGED"
      trap - EXIT

      cd /workspace
      exec opencode /workspace
    '';
  };
in
{
  environment.systemPackages = [ opencodeShell ];

  # The virtiofs mounts create their parent directories as root. Reclaim them
  # and prepare the Home Manager directories before activation.
  systemd.services.aegis-agent-home = {
    description = "Prepare the agent home directory for Home Manager activation";
    wantedBy = [ "multi-user.target" ];
    before = [ "home-manager-agent.service" ];
    after = [ "local-fs.target" ];
    serviceConfig.Type = "oneshot";
    script = ''
      mkdir -p /home/agent/.config /home/agent/.local /home/agent/.local/share /home/agent/.local/state
      chown agent:users /home/agent/.config /home/agent/.local /home/agent/.local/share /home/agent/.local/state
      mkdir -p /home/agent/.local/state/nix/profiles /home/agent/.local/state/home-manager/gcroots
      chown -R agent:users /home/agent/.local/state/nix /home/agent/.local/state/home-manager
    '';
  };

  users.users.agent = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    home = "/home/agent";
    shell = lib.getExe pkgs.bash;
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
        config = {
          lsp = {
            jdtls.disabled = true;
            java = {
              command = [ "jdtls" "-data" "/tmp/jdtls-workspace" "-Djava.import.generatesMetadataFilesAtProjectRoot=false" ];
              extensions = [ ".java" ];
            };
            csharp.command = [ "csharp-ls" ];
            fish = {
              command = [ "fish-lsp" "start" ];
              extensions = [ ".fish" ];
            };
          };
        };
        skills.anthropic.enable = envBool "VM_SKILL_ANTHROPIC" false;
        skills.mattpocock.enable = envBool "VM_SKILL_MATTPOCOCK" false;
        skills.vercel.enable = envBool "VM_SKILL_VERCEL" false;
      };

      programs.git = {
        enable = true;
        settings = lib.optionalAttrs (gitUserName != null || gitUserEmail != null) {
          user = lib.filterAttrs (_: value: value != null) {
            name = gitUserName;
            email = gitUserEmail;
          };
        };
        ignores = [
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
        temurin-bin-26
        maven
        python3
        go
        cargo
        rustc
        rustfmt
        clippy
      ];

      home.sessionVariables = {
        JAVA_HOME = "${pkgs.temurin-bin-26.home}";
      };

      home.stateVersion = "24.05";
    };
  };
}
