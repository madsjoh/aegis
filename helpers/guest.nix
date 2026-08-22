{ nixpkgs, microvm, home-manager, metis }:

guestSystem:

nixpkgs.lib.nixosSystem {
  system = guestSystem;

  modules = [
    microvm.nixosModules.microvm
    home-manager.nixosModules.home-manager
    (import ../modules/microvm.nix)
    (import ../modules/user.nix { inherit metis; })
    (import ../modules/services.nix)
  ];
}
