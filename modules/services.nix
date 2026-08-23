{ config, lib, pkgs, ... }:

{
  system.stateVersion = lib.trivial.release;

  # The home-manager activation requires nix-daemon, which the microvm module
  # disables by default when the store is read-only.
  systemd.services.nix-daemon.enable = true;
  systemd.sockets.nix-daemon.enable = true;

  environment.sessionVariables = {
    GH_TOKEN = builtins.getEnv "GH_TOKEN";
    GITHUB_TOKEN = builtins.getEnv "GITHUB_TOKEN";
    GIT_AUTHOR_NAME = builtins.getEnv "VM_GIT_NAME";
    GIT_AUTHOR_EMAIL = builtins.getEnv "VM_GIT_EMAIL";
    GIT_COMMITTER_NAME = builtins.getEnv "VM_GIT_NAME";
    GIT_COMMITTER_EMAIL = builtins.getEnv "VM_GIT_EMAIL";
  };

  systemd.services.opencode-agent = {
    wantedBy = [ "multi-user.target" ];
    environment = config.environment.sessionVariables;

    serviceConfig = {
      User = "agent";
      ExecStart = "${pkgs.opencode}/bin/opencode serve --port 4000 --hostname 0.0.0.0";
      WorkingDirectory = "/workspace";
      Restart = "always";
      EnvironmentFile = "-/workspace/.opencode-env";
    };
  };
}
