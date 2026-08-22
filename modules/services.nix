{ config, pkgs, ... }:

{
  environment.sessionVariables = {
    GH_TOKEN = builtins.getEnv "GH_TOKEN";
    GITHUB_TOKEN = builtins.getEnv "GITHUB_TOKEN";
    GEMINI_API_KEY = builtins.getEnv "GEMINI_API_KEY";
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
