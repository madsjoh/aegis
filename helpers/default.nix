{ nixpkgs, microvm, home-manager, metis }:

let
  supportedSystems = [ "aarch64-linux" "x86_64-linux" ];

  forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

  mkRunner = import ./runner.nix;

  mkVmConfig = import ./guest.nix {
    inherit nixpkgs microvm home-manager metis;
  };
in
{
  inherit supportedSystems forAllSystems mkRunner mkVmConfig;
}
