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
  mountTag = envStr "VM_MOUNT_TAG" "workspace";
  configTag = "aegis-config";
  opencodeStateTag = "opencode-state";
  opencodeShareTag = "opencode-share";
  cid = envInt "VM_CID" 3;

  workspaceSocket = "${stateDir}/run/workspace.sock";
  configSocket = "${stateDir}/run/config.sock";
  opencodeStateSocket = "${stateDir}/run/opencode-state.sock";
  opencodeShareSocket = "${stateDir}/run/opencode-share.sock";

  hostPkgs = config.virtualisation.host.pkgs;
  regInfo = hostPkgs.closureInfo { rootPaths = config.virtualisation.additionalPaths; };
  storeClosureInfo = hostPkgs.closureInfo {
    rootPaths = [ config.system.build.toplevel regInfo ];
  };
  storeImage = hostPkgs.runCommand "aegis-store-image" { } ''
    mkdir -p "$out"
    ${import "${pkgs.path}/nixos/lib/erofs-store-image.nix" {
      inherit hostPkgs;
      storePaths = "${storeClosureInfo}/store-paths";
      label = "nix-store";
      destination = ''"$out/store.img"'';
    }}
  '';
in
{
  boot.initrd.availableKernelModules = [ "virtiofs" "vmw_vsock_virtio_transport" ];
  boot.initrd.kernelModules = [ "vmw_vsock_virtio_transport" ];
  boot.kernelModules = [ "virtiofs" ];

  virtualisation = {
    graphics = false;
    diskImage = null;
    useNixStoreImage = false;
    mountHostNixStore = false;
    qemu.enableSharedMemory = true;
    qemu.drives = [
      {
        name = "nix-store";
        file = "${storeImage}/store.img";
        driveExtraOpts.format = "raw";
        driveExtraOpts.readonly = "on";
        deviceExtraOpts.bootindex = "2";
      }
    ];
    qemu.options = [
      "-chardev socket,id=char-${mountTag},path=${workspaceSocket}"
      "-device vhost-user-fs-pci,chardev=char-${mountTag},tag=${mountTag}"
      "-chardev socket,id=char-${configTag},path=${configSocket}"
      "-device vhost-user-fs-pci,chardev=char-${configTag},tag=${configTag}"
      "-chardev socket,id=char-${opencodeStateTag},path=${opencodeStateSocket}"
      "-device vhost-user-fs-pci,chardev=char-${opencodeStateTag},tag=${opencodeStateTag}"
      "-chardev socket,id=char-${opencodeShareTag},path=${opencodeShareSocket}"
      "-device vhost-user-fs-pci,chardev=char-${opencodeShareTag},tag=${opencodeShareTag}"
      "-device vhost-vsock-pci,guest-cid=${toString cid}"
    ];
  };

  virtualisation.fileSystems."/nix/.ro-store" = {
    device = "/dev/disk/by-label/nix-store";
    fsType = "erofs";
    neededForBoot = true;
    options = [ "ro" ];
  };

  virtualisation.fileSystems."/nix/store" = {
    overlay = {
      lowerdir = [ "/nix/.ro-store" ];
      upperdir = "/nix/.rw-store/upper";
      workdir = "/nix/.rw-store/work";
    };
  };

  virtualisation.fileSystems."/workspace" = {
    device = mountTag;
    fsType = "virtiofs";
    options = [ "x-systemd.requires=modprobe@virtiofs.service" ];
  };

  virtualisation.fileSystems."/aegis" = {
    device = configTag;
    fsType = "virtiofs";
    options = [ "x-systemd.requires=modprobe@virtiofs.service" ];
  };

  virtualisation.fileSystems."/home/agent/.local/state/opencode" = {
    device = opencodeStateTag;
    fsType = "virtiofs";
    options = [ "x-systemd.requires=modprobe@virtiofs.service" ];
  };

  virtualisation.fileSystems."/home/agent/.local/share/opencode" = {
    device = opencodeShareTag;
    fsType = "virtiofs";
    options = [ "x-systemd.requires=modprobe@virtiofs.service" ];
  };
}
