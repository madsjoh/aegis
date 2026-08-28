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
needs a Linux builder to do so. Aegis can manage this builder through Docker
on Apple Silicon.

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

## Docker Linux Builder

### Prerequisites

The managed builder supports Apple Silicon macOS only. Install and start
Docker. The Docker command must be available and report an ARM64 architecture.

Builder setup uses `sudo` to change system Nix configuration and reload the Nix
daemon. Obtain approval from your corporate IT or security team before running
it on a managed device. Docker containers, local port forwarding, and changes
under `/etc/nix` may be restricted by company policy.

### Setup

Run the idempotent setup command initially and whenever you want to validate the
builder configuration:

```
nix run github:madsjoh/aegis#builder-setup
```

From a local checkout, run `nix run .#builder-setup` instead. Setup requests
administrator access, creates and verifies the builder, reloads the Nix daemon,
and performs a fresh `aarch64-linux` test build through the remote builder.

Setup manages these resources:

* Docker image `aegis-builder`, labeled with the Aegis builder version and a
  deterministic identity derived from the packaged Docker context.
* Docker container `aegis-builder`, labeled with the same image identity and
  the configured client public key fingerprint.
* Docker volume `aegis-builder-nix`, mounted at `/nix` to persist the Nix store
  and SSH host keys.
* SSH forwarding from `127.0.0.1:31022` to port 22 in the container. SSH is not
  exposed on other host interfaces.
* User state in `~/.local/share/aegis/builder`, or
  `$XDG_DATA_HOME/aegis/builder` when `XDG_DATA_HOME` is set. This directory
  contains the client key pair and pinned builder host key.
* `/etc/nix/aegis-machines`, which defines the `aarch64-linux` builder.
* `/etc/nix/aegis.conf`, which enables the builder and its binary caches.
* One active `!include /etc/nix/aegis.conf` line appended to
  `/etc/nix/nix.conf` if an equivalent active include is absent. Setup preserves
  every unrelated Nix setting.

The host and builder use the standard `https://cache.nixos.org` cache and the
`https://microvm.cachix.org` cache, including their trusted public keys. The
builder image currently uses the mutable `alpine:3.22` base tag rather than an
immutable digest. A later upstream tag update can therefore change a newly
built image, which is a reproducibility and supply chain risk.

### Automatic Operation

On macOS, each Aegis launch requires exactly one active Aegis include and
checks the exact managed settings, machine definition, image labels, container
labels, pinned SSH host key, SSH authentication, and remote Nix daemon health
before building the MicroVM. It creates the image and container when absent
and starts a stopped container automatically. An unused image with stale
identity labels is rebuilt. A running container is never silently replaced;
identity, credential, and host key mismatches are fatal. Linux launches bypass
all managed builder checks.

### Troubleshooting

* If Docker is unavailable, start Docker and run `docker info`.
* If Docker reports an architecture other than ARM64, correct the Docker
  environment architecture before retrying.
* If port `31022` is occupied, stop the process using
  `127.0.0.1:31022`, then rerun setup.
* If setup reports an SSH host key mismatch, do not bypass it. Confirm that the
  expected `aegis-builder` container and `aegis-builder-nix` volume are present.
  If intentional replacement is required, follow the complete uninstall steps
  before running setup again.
* If credentials under `~/.local/share/aegis/builder` are missing while the
  container remains, remove and recreate the container through the uninstall
  and setup procedures. A newly generated client key cannot authenticate to an
  existing container.
* If setup reports credential drift, confirm that no build uses the container,
  run `docker container rm --force aegis-builder`, and rerun setup. Setup does
  not silently replace an existing container.
* If setup reports duplicate active Aegis includes, remove the duplicates so
  exactly one semantic `!include /etc/nix/aegis.conf` line remains.
* For a stopped builder, rerun setup. Setup starts and validates the existing
  container.
* For a running but unhealthy builder, inspect it with
  `docker container logs aegis-builder` and `docker container inspect
  aegis-builder`. Remove only the container, then rerun setup to recreate and
  validate it safely:

  ```
  docker container rm --force aegis-builder
  nix run github:madsjoh/aegis#builder-setup
  ```

### Uninstall

Back up `/etc/nix/nix.conf`, then remove every active semantic Aegis include
while preserving comments and all unrelated settings. This portable command
writes a replacement and installs it only if filtering succeeds:

```
sudo cp -p /etc/nix/nix.conf /etc/nix/nix.conf.aegis-backup
temporary_file="$(mktemp /tmp/nix.conf.aegis-uninstall.XXXXXX)"
if awk '!/^[[:space:]]*!include[[:space:]]+\/etc\/nix\/aegis[.]conf([[:space:]]*(#.*)?)?$/' /etc/nix/nix.conf > "$temporary_file"
then
  sudo install -m 0644 "$temporary_file" /etc/nix/nix.conf
  rm "$temporary_file"
else
  rm "$temporary_file"
  exit 1
fi
```

Then remove the dedicated files, reload Nix, and remove the builder resources:

```
sudo rm /etc/nix/aegis.conf /etc/nix/aegis-machines
sudo launchctl kickstart -k system/org.nixos.nix-daemon
docker container rm --force aegis-builder
docker image rm aegis-builder
docker volume rm aegis-builder-nix
rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/aegis/builder"
```

Do not delete or replace `/etc/nix/nix.conf`; it may contain unrelated Nix
configuration. If Docker reports that the image or volume is in use, confirm
that no other container uses it before removing that resource.

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

`git.name` and `git.email` fall back to your host Git configuration when they
are not set. The `opencode.auth` value is an object whose contents are written
to the guest OpenCode auth file, for example:

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
