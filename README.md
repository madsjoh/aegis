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
4. Shares the workspace and the configuration directory into the guest.
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
  Nix store is a read-only erofs image that exposes only the guest's own
  closure.
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

Initialize the user configuration:

```
nix run github:madsjoh/aegis#init
```

This detects OpenCode and the GitHub CLI and prompts to include their
credentials in `~/.config/aegis/config.json`.

Run Aegis from the root of your workspace:

```
nix run github:madsjoh/aegis
```

You can also run it from a local checkout:

```
nix run .
```

The runner prints the host and guest systems, then builds and boots the VM and
attaches OpenCode.

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

Per workspace state and the SSH key live outside the workspace under
`~/.local/state/aegis/<workspace-id>` and `~/.local/share/aegis/<workspace-id>`,
where `<workspace-id>` is a 16 character prefix of the sha256 of the workspace
path. The state directory holds the lock, the configuration snapshot, the
virtiofsd sockets, and the logs; the share directory holds the persisted SSH
key. Override the base directories with `XDG_STATE_HOME` and `XDG_DATA_HOME`.

## Development

The helpers are plain Bash and Nix modules under `helpers/` and `modules/`.
Run the test suite with:

```
nix flake check
```

The checks exercise the lock and configuration helpers plus the guest system
mapping.

## License

[MIT][mit]

[metis]: https://github.com/madsjoh/metis
[mit]: https://opensource.org/license/mit
[nix]: https://nixos.org/
[opencode]: https://opencode.ai
[qemu-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/qemu-vm.nix
[vz-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/vz-vm.nix
