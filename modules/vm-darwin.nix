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
    };

    vz = {
      rosetta.enable = true;
      diskImage = null;
      console = "file";
      consoleLog = "${stateDir}/console.log";
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
