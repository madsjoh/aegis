{ config, lib, pkgs, ... }:

let
  envInt = name: default:
    let
      s = builtins.getEnv name;
    in
    if s == "" then default else builtins.fromJSON s;

  envStr = name: default:
    let
      s = builtins.getEnv name;
    in
    if s == "" then default else s;

  stateDir = envStr "HOST_STATE_DIR" "/tmp/aegis-state";
  sshHostPort = envInt "VM_SSH_PORT" 2222;
in
{
  virtualisation = {
    sharedDirectories = {
      workspace = {
        source = "\"$HOST_WORKSPACE\"";
        target = "/workspace";
      };

      config = {
        source = "\"$HOST_CONFIG\"";
        target = "/aegis";
      };

      opencode-state = {
        source = "\"$HOST_STATE_DIR/opencode/state\"";
        target = "/home/agent/.local/state/opencode";
      };

      opencode-share = {
        source = "\"$HOST_STATE_DIR/opencode/share\"";
        target = "/home/agent/.local/share/opencode";
      };
    };

    vz = {
      rosetta.enable = true;
      diskImage = null;
      console = "file";
      consoleLog = "${stateDir}/run/console.log";
      forwardPorts = [
        {
          host.address = "127.0.0.1";
          host.port = sshHostPort;
          guest.port = 22;
        }
      ];
    };
  };
}
