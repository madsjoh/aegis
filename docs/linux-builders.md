# Linux Builder Setup

Aegis builds a NixOS guest. A macOS host therefore needs a Linux builder,
even though the `aegis` command itself runs on macOS. Installing the Aegis
system or Home Manager module does not enable a builder.

## NixOS and Other Linux Hosts

An `aarch64-linux` or `x86_64-linux` host can build the matching Linux guest
locally. No separate Linux builder is required, with either standard Nix
or Determinate Nix. Aegis still requires access to KVM to run its guest.

The examples below configure a builder on Apple Silicon macOS, the macOS
architecture supported by Aegis. They are nix-darwin system modules, not
NixOS or Home Manager modules.

## Standard Nix on macOS

Use this option when nix-darwin manages the Nix installation. Add the
following module to your existing `darwinConfigurations.<host>.modules`:

```nix
{
  nix.linux-builder.enable = true;
}
```

This uses the default QEMU builder. The [nix-darwin builder module][darwin-builder]
registers it with the Nix daemon and manages its service and SSH settings.
It requires `nix.enable = true`, which is the nix-darwin default.

## Determinate Nix on macOS

Use the [Determinate nix-darwin module][determinate-darwin] when Determinate
manages the Nix installation. If your host does not already import it,
add the input to your host flake:

```nix
inputs.determinate.url = "https://flakehub.com/f/DeterminateSystems/determinate/3";
```

Include `determinate` in the flake's `outputs` arguments, then add these
entries to `darwinConfigurations.<host>.modules`:

```nix
determinate.darwinModules.default
{
  determinateNix = {
    enable = true;
    nixosVmBasedLinuxBuilder.enable = true;
  };
}
```

Keep the `nix.linux-builder` option disabled. The Determinate module sets
`nix.enable = false`, so the standard nix-darwin builder cannot be enabled.
The Determinate option configures the Nixpkgs QEMU builder and registers it
with Determinate Nix. It also disables Determinate's separate Native Linux
Builder to prevent both builders from running.

## Start the Default Builder First

Apply the selected configuration from your host flake directory. Replace
`<host>` with your existing Darwin configuration name:

```sh
sudo darwin-rebuild switch --flake '.#<host>'
```

Keep the default builder package and resources for this first activation.
The [Nixpkgs builder documentation][nixpkgs-builder] explains why changes
to the guest can require an existing Linux builder to build the new image.
If the initial image is unavailable from your configured binary caches,
use an existing remote Linux builder to bootstrap it.

## Optional VZ Builder and Resources

After the default builder works, you can switch to Apple's Virtualization
framework and allocate eight cores and 8192 MiB of memory. For standard
Nix, replace the earlier module with:

```nix
{ pkgs, ... }:
{
  nix.linux-builder = {
    config.virtualisation = {
      cores = 8;
      darwin-builder.memorySize = 8192;
      vz.rosetta.enable = false;
    };
    enable = true;
    package = pkgs.darwin.linux-builder-vz;
    supportedFeatures = [ "benchmark" "big-parallel" ];
    systems = [ "aarch64-linux" ];
    workingDirectory = "/var/lib/aegis-linux-builder-vz";
  };
}
```

For Determinate Nix, use the same attributes under
`determinateNix.nixosVmBasedLinuxBuilder` instead of `nix.linux-builder`.
Retain the Determinate module import and `determinateNix.enable = true`.
Use a Nixpkgs revision that provides `pkgs.darwin.linux-builder-vz`.
The VZ backend requires macOS 13 or newer.
The separate working directory prevents reuse of the QEMU builder's
incompatible disk image. The old builder data remains in its original
directory.
Apply the host configuration again after this change.

This builder accepts only `aarch64-linux` work, so it does not need Rosetta.
It does not enable nested virtualization and must not advertise `kvm`.
These settings affect the builder only. The Aegis guest still requires
Rosetta as described in the README, and its resources come from `vm.json`.

## Verify a Linux Build

On macOS, build a small derivation that records its operating system:

```sh
nix build --impure --no-link --print-out-paths --expr '
  let
    pkgs = (builtins.getFlake "github:NixOS/nixpkgs/nixos-unstable")
      .legacyPackages.aarch64-linux;
  in pkgs.runCommand "aegis-linux-builder-check" { } "uname -s > $out"
'
```

Read the printed store path with `cat`. Its contents must be `Linux`.
This checks that Nix can build Linux derivations; a successful download of
a cached Linux package alone does not test the builder.

If Nix reports that it needs an `aarch64-linux` builder, check the selected
module, rebuild the host configuration, and confirm that the builder service
started. Once this test succeeds, run `aegis` from your workspace.

[darwin-builder]: https://github.com/nix-darwin/nix-darwin/blob/master/modules/nix/linux-builder.nix
[determinate-darwin]: https://github.com/DeterminateSystems/determinate/blob/main/modules/nix-darwin/default.nix
[nixpkgs-builder]: https://github.com/NixOS/nixpkgs/blob/master/doc/packages/darwin-builder.section.md
