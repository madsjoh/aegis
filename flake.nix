{
  description = "Aegis: Isolated VM Sandbox for OpenCode configured via Metis";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    crane.url = "github:ipetkov/crane";

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    metis = {
      url = "github:madsjoh/metis";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, home-manager, metis }:
    let
      aegis = import ./helpers { inherit nixpkgs home-manager metis; };
      flakeRef = if (self.rev or "") != "" then "github:madsjoh/aegis/${self.rev}" else "path:${self.outPath}";
      systemModule = { pkgs, ... }: {
        environment.systemPackages = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
      };
    in {
      nixosModules.default = systemModule;

      darwinModules.default = systemModule;

      homeManagerModules.default = { pkgs, ... }: {
        home.packages = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
      };

      apps = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = {
            type = "app";
            program = "${aegis.mkRunner { inherit pkgs system flakeRef; }}/bin/aegis";
          };
        });

      packages = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          craneLib = crane.mkLib pkgs;
        in {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.system.build.vm;
          aegis-gateway = craneLib.buildPackage {
            src = craneLib.cleanCargoSource (craneLib.path ./.);
            pname = "aegis-gateway";
            version = "0.1.0";
            cargoExtraArgs = "--package aegis-gateway";
            strictDeps = true;
            doCheck = false;
          };
          default = aegis.mkRunner { inherit pkgs system flakeRef; };
        });

      checks = aegis.forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          lock = pkgs.runCommand "aegis-lock-test" {
            buildInputs = [ pkgs.util-linux ];
          } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-lock.sh} ${./helpers/lock.bash}
            touch "$out"
          '';

          runner-cache = pkgs.runCommand "aegis-runner-cache-test" {
            buildInputs = [ pkgs.openssh pkgs.util-linux ];
          } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-runner-cache.sh} ${./helpers/runner.nix} ${./helpers/lock.bash} ${./helpers/ssh-key.bash} ${./helpers/runner-lifecycle.bash}
            touch "$out"
          '';

          runner-config = pkgs.runCommand "aegis-runner-config-test" {
            buildInputs = [ pkgs.coreutils ];
          } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-runner-config.sh} ${self.packages.${system}.default}/bin/aegis
            touch "$out"
          '';

          store-cache = pkgs.runCommand "aegis-store-cache-test" {
            buildInputs = [ pkgs.util-linux ];
          } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-store-cache.sh} ${./helpers/lock.bash} ${./helpers/store-cache.bash}
            touch "$out"
          '';

          config = pkgs.runCommand "aegis-config-test" { buildInputs = [ pkgs.jq ]; } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-config.sh} ${./helpers/config.bash}
            touch "$out"
          '';

          qemu-override = pkgs.runCommand "aegis-qemu-override-test" {
            buildInputs = [ pkgs.gnugrep ];
          } ''
            grep --fixed-strings --quiet 'memory-backend=aegis-ram' ${self.packages.${system}.default}/bin/aegis
            touch "$out"
          '';

          scaffold = pkgs.runCommand "aegis-scaffold-test" {
            buildInputs = [ pkgs.jq pkgs.coreutils ];
          } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-scaffold.sh} ${./helpers/scaffold.bash}
            touch "$out"
          '';

          wait-for-ssh = pkgs.runCommand "aegis-wait-for-ssh-test" { } ''
            set -o errexit -o nounset -o pipefail
            bash ${./tests/test-wait-for-ssh.sh} ${./helpers/wait-for-ssh.bash}
            touch "$out"
          '';

          guest-system = pkgs.runCommand "aegis-guest-system-test" { } ''
            set -o errexit -o nounset -o pipefail
            test "${aegis.guestSystem "aarch64-darwin"}" = "aarch64-linux"
            test "${aegis.guestSystem "aarch64-linux"}" = "aarch64-linux"
            test "${aegis.guestSystem "x86_64-linux"}" = "x86_64-linux"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-aarch64-darwin.config.services.openssh.enable}" = "1"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-aarch64-linux.config.services.openssh.enable}" = "1"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.systemd.services.aegis-ssh-keys.enable}" = "1"
            test "${builtins.toString (builtins.length self.nixosConfigurations.aegis-vm-x86_64-linux.config.users.users.agent.openssh.authorizedKeys.keys)}" = "0"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.cores}" = "1"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.memorySize}" = "1024"
            test "${(nixpkgs.lib.findFirst (package: (package.pname or "") == "opencode") (throw "Guest OpenCode package is missing") self.nixosConfigurations.aegis-vm-aarch64-darwin.config.home-manager.users.agent.home.packages).version}" = "2.0.32"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.fileSystems."/run/aegis/gitconfig".options)}" = "1"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.fileSystems."/aegis".options)}" = "1"
            test "${builtins.toJSON self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.programs.git.settings}" = "{}"
            test "${builtins.toString (builtins.length self.nixosConfigurations.aegis-vm-x86_64-linux.config.users.users.agent.extraGroups)}" = "0"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-aarch64-darwin.config.fileSystems."/aegis".options)}" = "1"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-aarch64-darwin.config.fileSystems."/run/aegis/gitconfig".options)}" = "1"
            touch "$out"
          '';
        });

      nixosConfigurations = nixpkgs.lib.genAttrs
        (map (system: "aegis-vm-${system}") aegis.hostSystems)
        (name: aegis.mkVmConfig (nixpkgs.lib.removePrefix "aegis-vm-" name));
    };
}
