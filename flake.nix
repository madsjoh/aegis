{
  description = "Aegis: Isolated VM Sandbox for OpenCode configured via Metis";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    metis = {
      url = "github:madsjoh/metis";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, home-manager, metis }:
    let
      aegis = import ./helpers { inherit nixpkgs home-manager metis; };
    in {
      apps = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = {
            type = "app";
            program = "${aegis.mkRunner { inherit pkgs system; flakeRef = "path:${self.outPath}"; }}/bin/aegis";
          };
          init = {
            type = "app";
            program = "${aegis.mkInit { inherit pkgs; }}/bin/aegis-init";
          };
        });

      packages = aegis.forAllSystems (system:
        {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.system.build.vm;
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

          config = pkgs.runCommand "aegis-config-test" { buildInputs = [ pkgs.jq ]; } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-config.sh} ${./helpers/config.bash}
            touch "$out"
          '';

          init = pkgs.runCommand "aegis-init-test" { buildInputs = [ pkgs.jq ]; } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-init.sh} ${./helpers/init.bash}
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
