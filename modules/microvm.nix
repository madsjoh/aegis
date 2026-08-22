{ ... }:

let
  hostPort = builtins.fromJSON (
    let
      s = builtins.getEnv "VM_HOST_PORT";
    in
    if s == "" then "0" else s
  );

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
in
{
  microvm = {
    hypervisor = "qemu";

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
    ];

    forwardPorts = [
      {
        from = "host";
        host.port = hostPort;
        guest.port = 4000;
      }
    ];
  };

  networking.firewall.allowedTCPPorts = [ 4000 ];
}
