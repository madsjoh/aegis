# Aegis

Aegis runs [OpenCode][opencode] inside an isolated virtual machine sandbox.
The guest is built and configured as a NixOS system with [Metis][metis], so the
agent works against your real workspace through a shared filesystem mount while
the host stays untouched.

## How It Works

The `aegis` runner performs these steps when you launch it in a workspace:

1. Acquires a per workspace lock so only one VM runs at a time.
2. Snapshots the user configuration into the workspace state on first run.
3. Builds a NixOS guest whose CPU and memory come from the merged
   configuration.
4. Shares the workspace, the configuration, and the OpenCode state and
   share directories into the guest.
5. Boots the guest and waits for its SSH server.
6. Attaches OpenCode, which runs inside the guest as the `agent` user.

The guest mounts your workspace at `/workspace` and the configuration at
`/aegis`. OpenCode therefore sees and edits the same files you do on the host,
but every command it runs executes inside the VM.

## Backends

Aegis uses the built-in NixOS virtualisation modules rather than a third party
hypervisor toolkit.

- **Linux hosts** use the [QEMU VM][qemu-vm] backend. The workspace and
  configuration are shared over virtiofs, SSH is served over vsock, and the
  Nix store is an erofs image that exposes only the guest's own closure with a
  writable tmpfs overlay.
- **macOS hosts** use the [Apple Virtualization framework backend][vz-vm]
  (`vzvm`) with Rosetta. Shares use the framework's built-in virtiofs, and SSH
  is forwarded from a host port to the guest over vsock.

## Requirements

- A [Nix][nix] installation with flakes enabled.
- Hardware virtualization support. Linux requires KVM; macOS uses Apple's
  Virtualization framework.
- On macOS, Rosetta must be installed, which the VM requires to start. Install
  it with `softwareupdate --install-rosetta --agree-to-license`.

The flake builds for `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`
hosts. The guest is always Linux, so a Darwin host builds a Linux guest and
needs a Linux builder to do so, such as the nix-darwin `linux-builder` or a
remote builder.

## Usage

Install the `aegis` command into your profile:

```
nix profile install github:madsjoh/aegis
```

Then run Aegis from the root of your workspace:

```
aegis
```

You can also run it without installing:

```
nix run github:madsjoh/aegis
```

Or from a local checkout:

```
nix run .
```

Initialize the user configuration:

```
aegis init
```

This detects OpenCode and the GitHub CLI and prompts to include their
credentials in `~/.config/aegis/config.json`. Aegis runs this step
automatically on the first run, or whenever the configuration is missing.

The runner prints the host and guest systems, then builds and boots the VM and
attaches OpenCode.

## System Integration

To install Aegis through your system configuration, add the flake as an input
and import the module for your host system. For NixOS:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    aegis.url = "github:madsjoh/aegis";
    aegis.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, aegis, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        aegis.nixosModules.default
      ];
    };
  };
}
```

For nix-darwin, use the same module through `darwinModules`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nix-darwin.url = "github:LnL7/nix-darwin";
    aegis.url = "github:madsjoh/aegis";
    aegis.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nix-darwin, aegis, ... }: {
    darwinConfigurations.mymac = nix-darwin.lib.darwinSystem {
      system = "aarch64-darwin";
      modules = [
        aegis.darwinModules.default
      ];
    };
  };
}
```

For Home Manager, import the Home Manager module instead:

```nix
home-manager.users.alice = {
  imports = [ inputs.aegis.homeManagerModules.default ];
};
```

The `nixpkgs.follows` line keeps Aegis on the same nixpkgs revision as the
host, so it does not download a second copy. A Darwin host builds a Linux
guest, so it needs a Linux builder such as the nix-darwin `linux-builder` or a
remote builder.

## Configuration

Aegis reads a single user wide JSON file:

- `~/.config/aegis/config.json` for user wide settings.

When a workspace first runs, this file is copied once to
`~/.local/state/aegis/<workspace-id>/.config/aegis/config.json` and mounted
writable in the guest. Edits the guest makes persist in the workspace copy, and
later changes to the global file do not affect an already initialized
workspace.

The following keys are supported:

| Key                 | Purpose                                                    | Default |
| ------------------- | ---------------------------------------------------------- | ------- |
| `vm.cpu`            | Number of virtual CPUs                                     | `4`     |
| `vm.mem`            | Guest memory in MiB                                        | `4096`  |
| `git.name`          | Git author and committer name                              | Host git |
| `git.email`         | Git author and committer email                             | Host git |
| `github.token`      | GitHub token exported as `GH_TOKEN` for the `gh` CLI       | None    |
| `opencode.auth`     | OpenCode provider credentials written to the guest auth.json | None  |
| `skills.anthropic`  | Enable the Anthropic leaf skills                           | `false` |
| `skills.mattpocock` | Enable the Matt Pocock leaf skills                         | `false` |
| `skills.vercel`     | Enable the Vercel leaf skills                              | `false` |

`git.name` and `git.email` fall back to your host Git configuration when they
are not set. The `opencode.auth` value is an object whose contents are written
to the guest OpenCode auth file. The `skills` keys enable the Metis leaf skills
inside the guest and all default to `false`, so only the superpowers spine is
installed unless you opt in. A complete example:

```json
{
  "vm": {
    "cpu": 4,
    "mem": 8192
  },
  "git": {
    "name": "Jane Doe",
    "email": "jane@example.com"
  },
  "github": {
    "token": "ghp_..."
  },
  "opencode": {
    "auth": {
      "anthropic": {
        "type": "api",
        "key": "sk-ant-..."
      }
    }
  },
  "skills": {
    "anthropic": false,
    "mattpocock": false,
    "vercel": true
  }
}
```

Host wide data lives under `~/.local/share/aegis`. This directory contains the
persisted `ssh_host_ed25519` key used by every workspace. On macOS, closure keyed
VM images are shared under `~/.cache/aegis/vzvm`; only the latest closure image
is retained. The vzvm definition is workspace state and is not stored in this
cache. Override these base directories with `XDG_DATA_HOME` and `XDG_CACHE_HOME`.

Per workspace state lives under `~/.local/state/aegis/<workspace-id>`, where
`<workspace-id>` is a 16 character prefix of the sha256 of the workspace path.
The state directory holds the configuration snapshot, `vzvm.json`, an
`opencode/` directory, and a `run/` directory. The `opencode/` subdirectories
(`state` and `share`) are mounted into the guest under `.local/state/opencode` and `.local/share/opencode`
so sessions and data persist between runs. The OpenCode configuration under
`.config/opencode` is not persisted; it is generated by Metis on every boot.
The `run/` directory holds the workspace lock, the virtiofsd sockets, and the
logs. Override its base directory with `XDG_STATE_HOME`.

## Development

The helpers are plain Bash and Nix modules under `helpers/` and `modules/`.
Run the test suite with:

```
nix flake check
```

The checks exercise the lock, configuration, store cache, and runner cache
helpers plus the guest system mapping.

## License

[MIT][mit]

[metis]: https://github.com/madsjoh/metis
[mit]: https://opensource.org/license/mit
[nix]: https://nixos.org/
[opencode]: https://opencode.ai
[qemu-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/qemu-vm.nix
[vz-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/vz-vm.nix
