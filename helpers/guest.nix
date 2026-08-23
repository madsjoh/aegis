{ nixpkgs, microvm, home-manager, metis, guestSystem }:

hostSystem:

nixpkgs.lib.nixosSystem {
  system = guestSystem hostSystem;

  modules = [
    microvm.nixosModules.microvm
    home-manager.nixosModules.home-manager
    (import ../modules/microvm.nix)
    (import ../modules/user.nix { inherit metis; })
    (import ../modules/services.nix)
    ({ ... }: {
      microvm.vmHostPackages = nixpkgs.legacyPackages.${hostSystem};
    })
  ];
}
