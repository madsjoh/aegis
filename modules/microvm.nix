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

  sshHostPort = envInt "VM_SSH_PORT" 2222;

  stateDir =
    let
      s = builtins.getEnv "HOST_STATE_DIR";
    in
    if s == "" then "/tmp/aegis-state" else s;

  virtiofsSocket = "${stateDir}/virtiofsd-workspace.sock";

  configSocket = "${stateDir}/virtiofsd-config.sock";

  translateArgs = [
    "--translate-uid" "guest:1000:${toString hostUid}:1"
    "--translate-gid" "guest:100:${toString hostGid}:1"
  ];

  isDarwinHost = config.microvm.vmHostPackages.stdenv.hostPlatform.isDarwin;

  virtiofsd = config.microvm.virtiofsd.package;

  workspaceShare =
    if isDarwinHost then
      {
        proto = "9p";
        tag = mountTag;
        source = workspaceSource;
        mountPoint = "/workspace";
      }
    else
      {
        proto = "virtiofs";
        tag = mountTag;
        source = workspaceSource;
        mountPoint = "/workspace";
        socket = virtiofsSocket;
        posixAcl = false;
        extraArgs = translateArgs;
      };

  configShare =
    if isDarwinHost then
      {
        proto = "9p";
        tag = "aegis-config";
        source = configSource;
        mountPoint = "/aegis";
        readOnly = true;
      }
    else
      {
        proto = "virtiofs";
        tag = "aegis-config";
        source = configSource;
        mountPoint = "/aegis";
        socket = configSocket;
        readOnly = true;
        posixAcl = false;
        extraArgs = translateArgs;
      };
in
{
  boot.kernelParams = [ "systemd.getty_auto=0" ];

  microvm = {
    inherit hypervisor;
    vcpu = cpu;
    mem = mem;
    writableStoreOverlay = "/nix/.rw-store";

    vsock = lib.mkIf (!isDarwinHost) {
      inherit cid;
      ssh.enable = true;
    };

    forwardPorts = lib.optionals isDarwinHost [
      {
        from = "host";
        host.port = sshHostPort;
        guest.port = 22;
      }
    ];

    interfaces = [
      {
        type = "user";
        id = "usernet";
        mac = "02:00:00:01:01:01";
      }
    ];

    shares = [ workspaceShare configShare ];

    # Run virtiofsd directly as the invoking user. The stock runner wraps
    # virtiofsd in supervisord as root, which cannot start from a non-root
    # runner. 9p shares on macOS do not need a virtiofsd at all.
    binScripts.virtiofsd-run = lib.mkIf (!isDarwinHost) (lib.mkForce ''
      rm -f ${virtiofsSocket}
      exec ${virtiofsd}/bin/virtiofsd \
        --socket-path=${virtiofsSocket} \
        --shared-dir=${workspaceSource} \
        --thread-pool-size 4 \
        --cache=auto \
        ${lib.concatStringsSep " " translateArgs}
    '');
    binScripts.virtiofsd-config-run = lib.mkIf (!isDarwinHost) ''
      rm -f ${configSocket}
      exec ${virtiofsd}/bin/virtiofsd \
        --socket-path=${configSocket} \
        --shared-dir=${configSource} \
        --readonly \
        ${lib.concatStringsSep " " translateArgs}
    '';
  };

  services.openssh.enable = lib.mkIf isDarwinHost true;

  networking.hostName = "aegis";
}
