# Aegis

Aegis runs [OpenCode][opencode] inside an isolated QEMU MicroVM sandbox. The
guest is built and configured as a NixOS system with [microvm.nix][microvm-nix]
and [Metis][metis], so the agent works against your real workspace through a
[virtiofs][virtiofs] mount while the host stays untouched.

## How It Works

The `aegis` runner performs these steps when you launch it in a workspace:

1. Acquires a per workspace lock so only one VM runs at a time.
2. Merges the user and workspace configuration files into one document.
3. Builds a NixOS MicroVM whose CPU, memory, and hypervisor come from the
   merged configuration.
4. Shares the workspace and the configuration directory into the guest with
   virtiofs.
5. Boots the guest and waits for its SSH server over vsock.
6. Attaches OpenCode, which runs inside the guest as the `agent` user.

The guest mounts your workspace at `/workspace` and the configuration
directory at `/aegis`. OpenCode therefore sees and edits the same files you do
on the host, but every command it runs executes inside the VM.

## Requirements

- A [Nix][nix] installation with flakes enabled.
- Hardware virtualization support. Linux requires KVM; macOS uses the built in
  hypervisor framework through QEMU.

The flake builds for `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`
hosts. The guest is always Linux, so a Darwin host builds a Linux guest and
needs a Linux builder to do so, such as the nix-darwin `linux-builder` or a
remote builder.

On Linux the guest is reached over vsock and its shares use virtiofs. On macOS
the shares use built in QEMU 9p instead, and SSH is reached over a forwarded
TCP port, since virtiofsd and vsock are unavailable there.

To avoid building the MicroVM toolchain from source, configure the
[microvm.nix][microvm-nix] binary cache on the host:

```
extra-substituters = https://microvm.cachix.org
extra-trusted-public-keys = microvm.cachix.org-1:oXnBc6hRE3eX5rSYdRyMYXnfzcCxC7yKPTbZXALsqys=
```

Add these lines to your `nix.conf`, or through the `nix.settings` module option
on NixOS. Aegis cannot set this itself because the public key option is
restricted to trusted users.

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
attaches OpenCode. Pass additional arguments after `--` to forward them to the
microvm runner.

## Configuration

Aegis reads two JSON files and merges them, with the workspace file taking
precedence:

- `~/.config/aegis/config.json` for user wide settings.
- `.aegis/config.json` in the workspace for project specific settings.

The following keys are supported:

| Key              | Purpose                                                     | Default |
| ---------------- | ----------------------------------------------------------- | ------- |
| `vm.cpu`         | Number of virtual CPUs                                      | `4`     |
| `vm.mem`         | Guest memory in MiB                                         | `4096`  |
| `vm.hypervisor`  | MicroVM hypervisor passed to microvm.nix                    | `qemu`  |
| `git.name`       | Git author and committer name                               | Host git |
| `git.email`      | Git author and committer email                              | Host git |
| `github.token`   | GitHub token exported as `GH_TOKEN` for the `gh` CLI        | None    |
| `opencode.auth`  | OpenCode provider credentials written to the guest auth.json | None    |
| `skills.anthropic`  | Enable the Anthropic leaf skills                          | `false` |
| `skills.mattpocock` | Enable the Matt Pocock leaf skills                        | `false` |
| `skills.vercel`     | Enable the Vercel leaf skills                             | `false` |

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

The runner also appends `.aegis.lock` and `.aegis/` to the local Git exclude
file so the agent and host never commit the lock or workspace secrets.

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
[microvm-nix]: https://github.com/astro/microvm.nix
[mit]: https://opensource.org/license/mit
[nix]: https://nixos.org/
[opencode]: https://opencode.ai
[virtiofs]: https://virtio-fs.gitlab.io/
