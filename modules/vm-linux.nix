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
      "-chardev socket,id=char-${opencodeStateTag},path=${opencodeStateSocket}"
      "-device vhost-user-fs-pci,chardev=char-${opencodeStateTag},tag=${opencodeStateTag}"
      "-chardev socket,id=char-${opencodeShareTag},path=${opencodeShareSocket}"
      "-device vhost-user-fs-pci,chardev=char-${opencodeShareTag},tag=${opencodeShareTag}"
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
