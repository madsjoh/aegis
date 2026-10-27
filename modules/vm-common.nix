{ ... }:

{
  boot.kernelParams = [ "systemd.getty_auto=0" ];

  environment.enableAllTerminfo = true;

  networking.hostName = "aegis";

  nix.settings.experimental-features = [ "nix-command" "flakes" ];

  services.openssh.enable = true;

  virtualisation = {
    writableStore = true;
    writableStoreUseTmpfs = true;
  };
}
