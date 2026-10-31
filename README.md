# Aegis

Aegis runs [OpenCode][opencode] inside an isolated virtual machine sandbox.
The guest is built and configured as a NixOS system with [Metis][metis], so the
agent works against your real workspace through a shared filesystem mount while
the host stays untouched.

## How It Works

The `aegis` runner performs these steps when you launch it in a workspace:

1. Acquires a per workspace lock so only one VM runs at a time.
2. Reads the user configuration files, creating any that are missing.
3. Builds a NixOS guest whose CPU and memory come from `vm.json`.
4. Shares the workspace, the configuration, the OpenCode state and share
   directories, and the host Git configuration into the guest.
5. Boots the guest and waits for its SSH server.
6. Attaches OpenCode, which runs inside the guest as the `agent` user.

The guest mounts your workspace at `/workspace`, the configuration at `/aegis`,
and a copy of the host Git configuration at `/run/aegis/gitconfig`. The
workspace is writable, so OpenCode edits the same files you do on the host; the
configuration and Git configuration mounts are read-only, and the agent runs
without sudo, so it cannot modify them. Every command OpenCode runs executes
inside the VM.

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

The runner accepts `aegis run` as the default command and `aegis help`. Aegis
scaffolds any missing configuration file on the first run.

The runner prints the host and guest systems, then builds and boots the VM and
attaches OpenCode.

## Upgrading

Aegis previously kept every setting in a single `~/.config/aegis/config.json`
file. That file is no longer read or written. Move your settings by hand into
the new files: `vm` → `vm.json`, `skills` → `skills.json`, and `opencode`
(authentication excluded) → `opencode.jsonc`. The `github.token` and
`opencode.auth` settings are removed. Credentials come from the gateway,
which reads a sops encrypted `secrets.yaml` file; see the Gateway section.
Aegis scaffolds any missing file with its defaults on the first run.

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

Aegis keeps its user configuration in `~/.config/aegis`, or
`$XDG_CONFIG_HOME/aegis` when that variable is set. Four files live there:

- `vm.json` holds the VM resources: `{"cpu":4,"mem":4096}`.
- `skills.json` enables the Metis leaf skills:
  `{"anthropic":false,"mattpocock":false,"vercel":false}`.
- `opencode.jsonc` holds the OpenCode configuration.
- `secrets.yaml` holds the gateway credentials, encrypted with sops. Create
  it yourself; it is not scaffolded.

Aegis creates any missing file with its defaults when it runs and leaves
existing files untouched. The files are read on every run, so changes apply to
all workspaces the next time they launch. The configuration directory is
mounted read-only at `/aegis` in the guest, so the guest cannot modify the
host's configuration.

`vm.json` supports these keys:

| Key   | Purpose                | Default |
| ----- | ---------------------- | ------- |
| `cpu` | Number of virtual CPUs | `4`     |
| `mem` | Guest memory in MiB    | `4096`  |

`skills.json` supports these keys:

| Key          | Purpose                            | Default |
| ------------ | ---------------------------------- | ------- |
| `anthropic`  | Enable the Anthropic leaf skills   | `false` |
| `mattpocock` | Enable the Matt Pocock leaf skills | `false` |
| `vercel`     | Enable the Vercel leaf skills      | `false` |

All the `skills.json` keys default to `false`, so only the superpowers spine is
installed unless you opt in.

`opencode.jsonc` is the single place for OpenCode settings, including
providers, models, credentials, permissions, agents, MCP servers, and the
provider allowlist configured through `policies`. It uses OpenCode's
configuration format, so it accepts JSONC comments and is validated by OpenCode
when it starts. Aegis forwards the file verbatim to OpenCode through
`OPENCODE_CONFIG_CONTENT`, overriding matching settings generated by Metis.
Credentials are not configured here; the gateway imports placeholder values
into OpenCode's database, so secrets never live in the file or the environment.

Here is an example `opencode.jsonc`:

```jsonc
{
  // Credentials are imported into OpenCode's database by the gateway.
  "model": "provider/model",
  "permission": "allow"
}
```

Git identity comes from the host's `~/.gitconfig`. Aegis copies it into the run
directory each run, so the host's real file is never exposed to the guest, and
mounts the copy read-only at `/run/aegis/gitconfig/.gitconfig`. On Linux the
virtiofs daemon also refuses writes with `--readonly`; the macOS host-side
read-only flag is pending an upstream vzvm change. When the host has no global
configuration, the guest receives an empty `[user]` section so Git remains
functional. Only `~/.gitconfig` is forwarded; the XDG `~/.config/git/config`
file and any `[include]` targets are not resolved into the guest.

Host wide data lives under `~/.local/share/aegis`. This directory contains the
persisted `ssh_host_ed25519` key used by every workspace. On macOS, closure keyed
VM images are shared under `~/.cache/aegis/vzvm`; only the latest closure image
is retained. The vzvm definition is workspace state and is not stored in this
cache. Override these base directories with `XDG_DATA_HOME` and `XDG_CACHE_HOME`.

Per workspace state lives under `~/.local/state/aegis/<workspace-id>`, where
`<workspace-id>` is a 16 character prefix of the sha256 of the workspace path.
The state directory holds `vzvm.json`, an `opencode/` directory, and a `run/`
directory. The `opencode/` subdirectories
(`state` and `share`) are mounted into the guest under `.local/state/opencode` and `.local/share/opencode`
so sessions and data persist between runs. The OpenCode configuration under
`.config/opencode` is not persisted; Metis generates it on every boot, and
`opencode.jsonc` overrides matching settings.
The `run/` directory holds the workspace lock, the virtiofsd sockets, the
staged git configuration, and the logs. Override its base directory with
`XDG_STATE_HOME`.

## Gateway

Aegis runs a gateway on the host that sits between the guest and the public
internet. The guest never sees a real credential: it sends placeholder tokens,
and the gateway swaps them for the real secret on the way out. For GitHub it
also narrows scope, so the agent can only touch the repositories and
permissions you grant.

The gateway intercepts HTTPS only for the hosts it knows. Everything else
passes through untouched. Credentials live in a sops encrypted file at
`~/.config/aegis/secrets.yaml`, decrypted in memory at run time and never
written to disk. When the file is absent, the gateway and proxy are skipped and
the guest behaves as before.

### Supported providers

| Provider  | Host                                | Auth header       | Secret field |
| --------- | ----------------------------------- | ----------------- | ------------ |
| GitHub    | `github.com`, `api.github.com`      | `authorization`   | `pat`        |
| Anthropic | `api.anthropic.com`                 | `x-api-key`       | `api_key`    |
| OpenAI    | `api.openai.com`                    | `authorization`   | `api_key`    |
| Google    | `generativelanguage.googleapis.com` | `x-goog-api-key`  | `api_key`    |
| DeepSeek  | `api.deepseek.com`                  | `authorization`   | `api_key`    |

GitHub Copilot is not in the table because it uses short lived OAuth tokens
minted from your GitHub token, so it has no secret of its own. It is enabled
with a boolean instead, as shown in the example below.

### secrets.yaml

Each top level key is an integration holding one or more named credentials.
Only the values are encrypted by sops; the keys stay readable. For GitHub you
also configure the repositories and permissions the credential may use.

```yaml
github:
  personal:
    pat: ghp_xxxxxxxxxxxxxxxx
    repositories:
      - pattern: "madsjoh/aegis"
        permissions:
          contents: write
          pull_requests: write
          issues: read
      - pattern: "madsjoh/*"
        permissions:
          contents: read

anthropic:
  default:
    api_key: sk-ant-xxxxxxxx

openai:
  default:
    api_key: sk-xxxxxxxx

google:
  default:
    api_key: AIza-xxxxxxxx

deepseek:
  default:
    api_key: sk-xxxxxxxx

copilot:
  default:
    enabled: true
```

GitHub permissions use the fine grained names with `read` or `write` values.
Any permission not listed is denied, and `metadata: read` is always implied for
the repository itself. A pattern is `owner/repo`, `org/*` for every repository
in an organization, or `*` for all repositories; the most specific match wins.
Requests outside any repository allow reads but block writes.

The age key that decrypts `secrets.yaml` comes from macOS Keychain, the
freedesktop Secret Service, or the default sops age key file at
`~/.config/sops/age/keys.txt`.

## Development

The runner is Rust under `crates/aegis-runner`, alongside the gateway under
`crates/aegis-gateway` and its shared library `crates/aegis-core`. The guest
configuration lives in the Nix modules under `modules/`. The guest uses the
latest OpenCode from the main `nixpkgs` input, the same revision as every other
guest package.

Run the test suite with:

```
nix flake check
```

The checks run the Rust unit and integration tests through `cargo-test`, verify
the packaged runner binary, and exercise the guest system mapping.

## License

[MIT][mit]

[metis]: https://github.com/madsjoh/metis
[mit]: https://opensource.org/license/mit
[nix]: https://nixos.org/
[opencode]: https://opencode.ai
[qemu-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/qemu-vm.nix
[vz-vm]: https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/virtualisation/vz-vm.nix
