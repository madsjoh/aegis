{
  description = "Aegis: Isolated QEMU MicroVM Sandbox for OpenCode configured via Metis";

  nixConfig = {
    extra-substituters = [ "https://microvm.cachix.org" ];
    extra-trusted-public-keys = [ "microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=" ];
  };

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
            program = "${aegis.mkRunner { inherit pkgs system; flakeRef = "path:${self.outPath}"; }}/bin/aegis";
          };
        });

      packages = aegis.forAllSystems (system:
        {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.microvm.declaredRunner;
        });

      checks = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          lock = pkgs.runCommand "aegis-lock-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-lock.sh} ${./helpers/lock.bash}
            touch "$out"
          '';

          env = pkgs.runCommand "aegis-env-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-env.sh} ${./helpers/env.bash}
            touch "$out"
          '';

          guest-system = pkgs.runCommand "aegis-guest-system-test" { } ''
            set -o errexit -o nounset -o pipefail
            test "${aegis.guestSystem "aarch64-darwin"}" = "aarch64-linux"
            test "${aegis.guestSystem "aarch64-linux"}" = "aarch64-linux"
            test "${aegis.guestSystem "x86_64-linux"}" = "x86_64-linux"
            touch "$out"
          '';
        });

      nixosConfigurations = nixpkgs.lib.genAttrs
        (map (system: "aegis-vm-${system}") aegis.hostSystems)
        (name: aegis.mkVmConfig (nixpkgs.lib.removePrefix "aegis-vm-" name));
    };
}
