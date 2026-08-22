{
  description = "Aegis: Isolated QEMU MicroVM Sandbox for OpenCode configured via Metis";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    microvm = {
      url = "github:astro/microvm.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    metis = {
      url = "github:madsjoh/metis";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, microvm, home-manager, metis }:
    let
      aegis = import ./helpers { inherit nixpkgs microvm home-manager metis; };
    in {
      apps = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = {
            type = "app";
            program = "${aegis.mkRunner { inherit pkgs system; }}/bin/aegis";
          };
        });

      packages = aegis.forAllSystems (system:
        {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.microvm.declaredRunner;
        });

      nixosConfigurations = nixpkgs.lib.genAttrs
        (map (system: "aegis-vm-${system}") aegis.supportedSystems)
        (name: aegis.mkVmConfig (nixpkgs.lib.removePrefix "aegis-vm-" name));
    };
}
