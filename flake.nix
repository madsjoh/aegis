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

      craneFor = system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          craneLib = crane.mkLib pkgs;
          cargoSrc = pkgs.lib.cleanSourceWith {
            src = craneLib.path ./.;
            filter = path: type:
              (pkgs.lib.hasInfix "/tests/fixtures/" path)
              || (craneLib.filterCargoSources path type);
          };
          isDarwin = pkgs.lib.hasSuffix "-darwin" system;
        in { inherit pkgs craneLib cargoSrc isDarwin; };
    in {
      nixosModules.default = systemModule;

      darwinModules.default = systemModule;

      homeManagerModules.default = { pkgs, ... }: {
        home.packages = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
      };

      apps = aegis.forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/aegis";
        };
      });

      packages = aegis.forAllSystems (system:
        let
          inherit (craneFor system) pkgs craneLib cargoSrc isDarwin;
          runner-tools = [ pkgs.nix pkgs.openssh pkgs.sops ]
            ++ pkgs.lib.optionals (!isDarwin) [ pkgs.virtiofsd ];
          aegis-runner = craneLib.buildPackage {
            src = cargoSrc;
            pname = "aegis-runner";
            version = "0.1.0";
            cargoExtraArgs = "--package aegis-runner";
            strictDeps = true;
            doCheck = true;
            nativeCheckInputs = [ pkgs.openssh ];
            AEGIS_FLAKE_REF = flakeRef;
            AEGIS_HOST_SYSTEM = system;
            AEGIS_GUEST_SYSTEM = aegis.guestSystem system;
          };
        in {
          "aegis-vm-${system}" = self.nixosConfigurations."aegis-vm-${system}".config.system.build.vm;
          aegis-gateway = craneLib.buildPackage {
            src = cargoSrc;
            pname = "aegis-gateway";
            version = "0.1.0";
            cargoExtraArgs = "--package aegis-gateway";
            strictDeps = true;
            doCheck = true;
            nativeCheckInputs = [ pkgs.sops ];
          };
          default = pkgs.runCommand "aegis" {
            nativeBuildInputs = [ pkgs.makeWrapper ];
          } ''
            mkdir -p "$out/bin"
            makeWrapper "${aegis-runner}/bin/aegis" "$out/bin/aegis" \
              --prefix PATH : "${pkgs.lib.makeBinPath runner-tools}"
          '';
        });

      checks = aegis.forAllSystems (system:
        let
          inherit (craneFor system) pkgs craneLib cargoSrc;
          cargoArtifacts = craneLib.buildDepsOnly {
            src = cargoSrc;
            pname = "aegis-workspace";
            version = "0.1.0";
            strictDeps = true;
          };
        in {
          cargo-test = craneLib.cargoTest {
            inherit cargoArtifacts;
            src = cargoSrc;
            pname = "aegis-workspace";
            version = "0.1.0";
            strictDeps = true;
            nativeCheckInputs = [ pkgs.sops pkgs.openssh ];
          };

          runner-binary = pkgs.runCommand "aegis-runner-binary-test" {
            buildInputs = [ pkgs.gnugrep ];
          } ''
            set -o errexit -o nounset -o pipefail
            ${self.packages.${system}.default}/bin/aegis help > "$TMPDIR/help" 2>&1 || { cat "$TMPDIR/help"; exit 1; }
            grep --fixed-strings --quiet 'Usage: aegis [run [args...] | help]' "$TMPDIR/help"
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
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.systemd.services.aegis-trust-ca.enable}" = "1"
            test "${builtins.toString (builtins.length self.nixosConfigurations.aegis-vm-x86_64-linux.config.users.users.agent.openssh.authorizedKeys.keys)}" = "0"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.cores}" = "1"
            test "${builtins.toString self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.memorySize}" = "1024"
            test "${(nixpkgs.lib.findFirst (package: (package.pname or "") == "opencode") (throw "Guest OpenCode package is missing") self.nixosConfigurations.aegis-vm-aarch64-darwin.config.home-manager.users.agent.home.packages).version}" = "2.0.32"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.fileSystems."/run/aegis/gitconfig".options)}" = "1"
            test "${builtins.toString (nixpkgs.lib.elem "ro" self.nixosConfigurations.aegis-vm-x86_64-linux.config.virtualisation.fileSystems."/aegis".options)}" = "1"
            test "${builtins.toJSON self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.programs.git.settings}" = "{}"
            test "${builtins.toString (self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.home.sessionVariables ? HTTPS_PROXY)}" = ""
            test "${self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.home.sessionVariables.SSL_CERT_FILE}" = "/run/aegis/ca-bundle.crt"
            test "${self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.home.sessionVariables.REQUESTS_CA_BUNDLE}" = "/run/aegis/ca-bundle.crt"
            test "${self.nixosConfigurations.aegis-vm-x86_64-linux.config.home-manager.users.agent.home.sessionVariables.NODE_EXTRA_CA_CERTS}" = "/run/aegis/ca-bundle.crt"
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
