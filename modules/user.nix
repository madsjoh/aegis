{ metis }:

{ pkgs, lib, ... }:

let
  envBool = name: default:
    let
      value = builtins.getEnv name;
    in
    if value == "" then default else builtins.fromJSON value;

  proxyPort = builtins.getEnv "VM_PROXY_PORT";
  gatewayEnabled = proxyPort != "";
  proxyUrl = "http://127.0.0.1:${proxyPort}";
  caBundle = "/run/aegis/ca-bundle.crt";
  githubPlaceholder = "github_pat_aegis_placeholder";
  githubExtraHeader = "Authorization: Basic eC1hY2Nlc3MtdG9rZW46Z2l0aHViX3BhdF9hZWdpc19wbGFjZWhvbGRlcg==";
  noProxy = "127.0.0.1,localhost,::1";

  opencodeShell = pkgs.writeShellApplication {
    name = "opencode-shell";
    runtimeInputs = with pkgs; [ coreutils opencode ];
    text = ''
      ${builtins.readFile ../helpers/config.bash}

      prepare_opencode "/aegis/opencode.jsonc"

      export GIT_CONFIG_GLOBAL=/run/aegis/gitconfig/.gitconfig

      export SSL_CERT_FILE="${caBundle}"
      export GIT_SSL_CAINFO="${caBundle}"
      export NIX_SSL_CERT_FILE="${caBundle}"
      export REQUESTS_CA_BUNDLE="${caBundle}"
      export NODE_EXTRA_CA_CERTS="${caBundle}"
      ${lib.optionalString gatewayEnabled ''
        export HTTPS_PROXY="${proxyUrl}"
        export HTTP_PROXY="${proxyUrl}"
        export ALL_PROXY="${proxyUrl}"
        export https_proxy="${proxyUrl}"
        export http_proxy="${proxyUrl}"
        export all_proxy="${proxyUrl}"
        export NO_PROXY="${noProxy}"
        export no_proxy="${noProxy}"
        export GH_TOKEN="${githubPlaceholder}"
        export GIT_CONFIG_COUNT=1
        export GIT_CONFIG_KEY_0="http.https://github.com/.extraHeader"
        export GIT_CONFIG_VALUE_0="${githubExtraHeader}"
      ''}

      if [ -f /home/agent/.local/share/opencode/credentials.json ]; then
        opencode auth import --standalone /home/agent/.local/share/opencode/credentials.json || true
      fi

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
        SSL_CERT_FILE = caBundle;
        GIT_SSL_CAINFO = caBundle;
        NIX_SSL_CERT_FILE = caBundle;
        REQUESTS_CA_BUNDLE = caBundle;
        NODE_EXTRA_CA_CERTS = caBundle;
      } // lib.optionalAttrs gatewayEnabled {
        HTTPS_PROXY = proxyUrl;
        HTTP_PROXY = proxyUrl;
        ALL_PROXY = proxyUrl;
        https_proxy = proxyUrl;
        http_proxy = proxyUrl;
        all_proxy = proxyUrl;
        NO_PROXY = noProxy;
        no_proxy = noProxy;
        GH_TOKEN = githubPlaceholder;
        GIT_CONFIG_COUNT = "1";
        GIT_CONFIG_KEY_0 = "http.https://github.com/.extraHeader";
        GIT_CONFIG_VALUE_0 = githubExtraHeader;
      };

      home.stateVersion = "24.05";
    };
  };
}
