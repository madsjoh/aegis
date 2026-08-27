{ config, lib, ... }:

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

  hostUid = envInt "VM_HOST_UID" 1000;

  hostGid = envInt "VM_HOST_GID" 100;

  cid = envInt "VM_CID" 3;

  virtiofsSocket = "/tmp/aegis-${mountTag}.sock";

  virtiofsd = config.microvm.virtiofsd.package;
in
{
  boot.kernelParams = [ "systemd.getty_auto=0" ];

  microvm = {
    inherit hypervisor;
    vcpu = cpu;
    mem = mem;
    writableStoreOverlay = "/nix/.rw-store";

    vsock = {
      inherit cid;
      ssh.enable = true;
    };

    interfaces = [
      {
        type = "user";
        id = "usernet";
        mac = "02:00:00:01:01:01";
      }
    ];

    shares = [
      {
        proto = "virtiofs";
        tag = mountTag;
        source = workspaceSource;
        mountPoint = "/workspace";
        socket = virtiofsSocket;
        posixAcl = false;
        extraArgs = [
          "--translate-uid" "guest:1000:${toString hostUid}:1"
          "--translate-gid" "guest:100:${toString hostGid}:1"
        ];
      }
      {
        proto = "9p";
        tag = "aegis-config";
        source = configSource;
        mountPoint = "/aegis";
        readOnly = true;
      }
    ];

    # Run virtiofsd directly as the invoking user. The stock runner wraps
    # virtiofsd in supervisord as root, which cannot start from a non-root
    # runner.
    binScripts.virtiofsd-run = lib.mkForce ''
      rm -f ${virtiofsSocket}
      exec ${virtiofsd}/bin/virtiofsd \
        --socket-path=${virtiofsSocket} \
        --shared-dir=${workspaceSource} \
        --thread-pool-size 4 \
        --cache=auto \
        --translate-uid guest:1000:${toString hostUid}:1 \
        --translate-gid guest:100:${toString hostGid}:1
    '';
  };

  networking.hostName = "aegis";
}
