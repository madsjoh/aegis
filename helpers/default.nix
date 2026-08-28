{ nixpkgs, microvm, home-manager, metis }:

let
  hostSystems = [ "aarch64-darwin" "aarch64-linux" "x86_64-linux" ];

  forAllSystems = nixpkgs.lib.genAttrs hostSystems;

  guestSystem = system:
    if nixpkgs.lib.hasSuffix "-darwin" system
    then nixpkgs.lib.replaceStrings [ "-darwin" ] [ "-linux" ] system
    else system;

  mkRunner = builderConfiguration: import ./runner.nix ({ inherit guestSystem; } // builderConfiguration);
  mkInit = import ./init.nix;
  mkBuilderSetup = import ./builder-setup.nix;

  mkVmConfig = import ./guest.nix {
    inherit nixpkgs microvm home-manager metis guestSystem;
  };
in
{
  inherit hostSystems forAllSystems guestSystem mkRunner mkInit mkBuilderSetup mkVmConfig;
}
