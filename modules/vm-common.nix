{ config, lib, pkgs, ... }:

let
  envInt = name: default:
    let
      s = builtins.getEnv name;
    in
    if s == "" then default else builtins.fromJSON s;
in
{
  boot.kernelParams = [ "systemd.getty_auto=0" ];

  environment.enableAllTerminfo = true;

  networking.hostName = "aegis";

  virtualisation = {
    cores = envInt "VM_CPU" 4;
    memorySize = envInt "VM_MEM" 4096;
    writableStore = true;
    writableStoreUseTmpfs = true;
  };
}
