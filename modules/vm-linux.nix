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
  cid = envInt "VM_CID" 3;

  workspaceSocket = "${stateDir}/fs.sock";
  configSocket = "${stateDir}/fsc.sock";
in
{
  boot.initrd.availableKernelModules = [ "virtiofs" "vmw_vsock_virtio_transport" ];
  boot.initrd.kernelModules = [ "vmw_vsock_virtio_transport" ];
  boot.kernelModules = [ "virtiofs" ];

  services.openssh.enable = true;

  virtualisation = {
    graphics = false;
    diskImage = null;
    useNixStoreImage = true;
    qemu.enableSharedMemory = true;
    qemu.options = [
      "-chardev socket,id=char-${mountTag},path=${workspaceSocket}"
      "-device vhost-user-fs-pci,chardev=char-${mountTag},tag=${mountTag}"
      "-chardev socket,id=char-${configTag},path=${configSocket}"
      "-device vhost-user-fs-pci,chardev=char-${configTag},tag=${configTag}"
      "-device vhost-vsock-pci,guest-cid=${toString cid}"
    ];
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
}
