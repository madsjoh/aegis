{ pkgs, builderContext, builderContextIdentity, builderVersion }:

pkgs.writeShellApplication {
  name = "aegis-builder-setup";
  runtimeInputs = with pkgs; [ coreutils docker-client gnugrep netcat nix openssh ];
  text = ''
    ${builtins.readFile ./builder.bash}
    ${builtins.readFile ./builder-setup.bash}

    AEGIS_BUILDER_CONTEXT="''${AEGIS_BUILDER_CONTEXT:-${builderContext}}"
    AEGIS_BUILDER_CONTEXT_IDENTITY="''${AEGIS_BUILDER_CONTEXT_IDENTITY:-${builderContextIdentity}}"
    AEGIS_BUILDER_VERSION="''${AEGIS_BUILDER_VERSION:-${builderVersion}}"
    export AEGIS_BUILDER_CONTEXT AEGIS_BUILDER_CONTEXT_IDENTITY AEGIS_BUILDER_VERSION
    builder_setup
  '';
}
