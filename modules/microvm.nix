{ ... }:

let
  envInt = name: default:
    let
      s = builtins.getEnv name;
    in
    if s == "" then default else builtins.fromJSON s;

  cpu = envInt "VM_CPU" 4;

  mem = envInt "VM_MEM" 4096;

  hypervisor =
    let
      s = builtins.getEnv "VM_HYPERVISOR";
    in
    if s == "" then "qemu" else s;

  mountTag =
    let
      s = builtins.getEnv "VM_MOUNT_TAG";
    in
    if s == "" then "workspace" else s;

  workspaceSource =
    let
      s = builtins.getEnv "HOST_WORKSPACE";
    in
    if s == "" then "/tmp/aegis-workspace" else s;

  configSource =
    let
      s = builtins.getEnv "HOST_CONFIG";
    in
    if s == "" then "/tmp/aegis-config" else s;
in
{
  boot.kernelParams = [ "systemd.getty_auto=0" ];

  microvm = {
    inherit hypervisor;
    vcpu = cpu;
    mem = mem;
    writableStoreOverlay = "/nix/.rw-store";

    interfaces = [
      {
        type = "user";
        id = "usernet";
        mac = "02:00:00:01:01:01";
      }
    ];

    shares = [
      {
        proto = "9p";
        tag = mountTag;
        source = workspaceSource;
        mountPoint = "/workspace";
      }
      {
        proto = "9p";
        tag = "aegis-config";
        source = configSource;
        mountPoint = "/aegis";
        readOnly = true;
      }
    ];
  };

  networking.hostName = "aegis";
}
