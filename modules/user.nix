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

  opencodeShell = pkgs.writeShellApplication {
    name = "opencode-shell";
    runtimeInputs = with pkgs; [ coreutils opencode ];
    text = ''
      ${builtins.readFile ../helpers/config.bash}

      prepare_opencode "/aegis/opencode.jsonc"

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

  # Install the host SSH public key, written by the runner into the
  # configuration directory before boot, for the agent and root before the
  # SSH server starts.
  systemd.services.aegis-ssh-keys = {
    description = "Install the host SSH public key for the agent and root";
    wantedBy = [ "multi-user.target" ];
    before = [ "sshd.service" ];
    after = [ "local-fs.target" ];
    serviceConfig.Type = "oneshot";
    serviceConfig.RemainAfterExit = true;
    script = ''
      if [ -f /aegis/ssh/authorized_keys ]; then
        mkdir -p /home/agent/.ssh
        cp /aegis/ssh/authorized_keys /home/agent/.ssh/authorized_keys
        chown agent:users /home/agent/.ssh/authorized_keys
        chmod 600 /home/agent/.ssh/authorized_keys
        mkdir -p /root/.ssh
        cp /aegis/ssh/authorized_keys /root/.ssh/authorized_keys
        chown root:root /root/.ssh/authorized_keys
        chmod 600 /root/.ssh/authorized_keys
      fi
    '';
  };

  users.users.agent = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    home = "/home/agent";
    shell = lib.getExe pkgs.bash;
  };

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
        temurin-bin-27
        (maven.override { jdk_headless = temurin-bin-27; })
        python3
        go
        cargo
        rustc
        rustfmt
        clippy
      ];

      home.sessionVariables = {
        JAVA_HOME = "${pkgs.temurin-bin-27.home}";
      };

      home.stateVersion = "24.05";
    };
  };
}
