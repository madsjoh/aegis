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
          builderContext = pkgs.runCommand "aegis-builder-context" { } ''
            mkdir --parents "$out"
            cp ${./builder/Dockerfile} "$out/Dockerfile"
            cp ${./builder/entrypoint.bash} "$out/entrypoint.bash"
          '';
          builderContextIdentity = builtins.hashString "sha256" (builtins.toJSON {
            Dockerfile = builtins.hashFile "sha256" ./builder/Dockerfile;
            entrypoint = builtins.hashFile "sha256" ./builder/entrypoint.bash;
          });
          builderVersion = "1";
          builderConfiguration = { inherit builderContext builderContextIdentity builderVersion; };
        in {
          builder-setup = {
            type = "app";
            program = "${aegis.mkBuilderSetup ({ inherit pkgs; } // builderConfiguration)}/bin/aegis-builder-setup";
          };
          default = {
            type = "app";
            program = "${aegis.mkRunner builderConfiguration { inherit pkgs system; flakeRef = "path:${self.outPath}"; }}/bin/aegis";
          };
          init = {
            type = "app";
            program = "${aegis.mkInit { inherit pkgs; }}/bin/aegis-init";
          };
        });

      packages = aegis.forAllSystems (system:
        {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.microvm.declaredRunner;
        });

      checks = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          builderContext = pkgs.runCommand "aegis-builder-context" { } ''
            mkdir --parents "$out"
            cp ${./builder/Dockerfile} "$out/Dockerfile"
            cp ${./builder/entrypoint.bash} "$out/entrypoint.bash"
          '';
          builderContextIdentity = builtins.hashString "sha256" (builtins.toJSON {
            Dockerfile = builtins.hashFile "sha256" ./builder/Dockerfile;
            entrypoint = builtins.hashFile "sha256" ./builder/entrypoint.bash;
          });
          builderVersion = "1";
          builderConfiguration = { inherit builderContext builderContextIdentity builderVersion; };
        in {
          builder = pkgs.runCommand "aegis-builder-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-builder.sh} ${./helpers/builder.bash}
            touch "$out"
          '';

          builder-image = pkgs.runCommand "aegis-builder-image-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-builder-image.sh} ${./builder/Dockerfile} ${./builder/entrypoint.bash}
            touch "$out"
          '';

          builder-setup = pkgs.runCommand "aegis-builder-setup-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-builder-setup.sh} ${./helpers/builder.bash} ${./helpers/builder-setup.bash}
            touch "$out"
          '';

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

          runner = pkgs.runCommand "aegis-runner-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-runner.sh} ${./helpers/builder.bash} ${./helpers/runner.bash}
            touch "$out"
          '';

          runner-generated =
            let
              darwinRunner = aegis.mkRunner builderConfiguration {
                inherit pkgs;
                flakeRef = "path:/aegis";
                system = "aarch64-darwin";
              };
              linuxRunner = aegis.mkRunner builderConfiguration {
                inherit pkgs;
                flakeRef = "path:/aegis";
                system = "aarch64-linux";
              };
            in pkgs.runCommand "aegis-generated-runner-test" { } ''
              set -o errexit -o nounset -o pipefail
              bash ${./tests/test-runner-generated.sh} ${darwinRunner}/bin/aegis ${linuxRunner}/bin/aegis
              touch "$out"
            '';

          readme = pkgs.runCommand "aegis-readme-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-readme.sh} ${./README.md}
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
