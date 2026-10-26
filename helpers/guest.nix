{ nixpkgs, home-manager, metis, guestSystem }:

hostSystem:

let
  isDarwinHost = nixpkgs.lib.hasSuffix "-darwin" hostSystem;

  virtualisationModules = "${nixpkgs}/nixos/modules/virtualisation";
in

nixpkgs.lib.nixosSystem {
  system = guestSystem hostSystem;

  modules = [
    home-manager.nixosModules.home-manager
    (import ../modules/vm-common.nix)
    (import ../modules/user.nix { inherit metis; })
    (import ../modules/services.nix)
    ({ ... }: {
      virtualisation.host.pkgs = nixpkgs.legacyPackages.${hostSystem};
    })
  ] ++ (if isDarwinHost then [
    "${virtualisationModules}/vz-vm.nix"
    (import ../modules/vm-darwin.nix)
  ] else [
    "${virtualisationModules}/qemu-vm.nix"
    (import ../modules/vm-linux.nix)
  ]);
}
