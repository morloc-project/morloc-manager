# Morloc Manager

Handle Morloc installation, version management, and custom environments.

 <img src="assets/install.png" alt="morloc install example" class="center">


## Prerequisites

You need a container engine: [Docker](https://docs.docker.com/engine/install/) (v20+) or [Podman](https://podman.io/docs/installation) (v3+). No Compose plugin is required.

When both are installed, the manager prefers **podman** over docker. You can
override with `--container-engine docker`, the `MORLOC_CONTAINER_ENGINE`
environment variable, or by setting `container_engine` in
`~/.config/morloc/config`. Note that images pulled by one engine are not visible
to the other — if you switch engines, images will be re-pulled.


## Installation

Download the manager and place it on your PATH:

```sh
BRANCH=dev
curl -o morloc-manager https://raw.githubusercontent.com/morloc-project/morloc-manager/refs/heads/${BRANCH}/morloc-manager.sh
chmod +x morloc-manager

# if using local containers:
mkdir -p ~/.local/bin
mv morloc-manager ~/.local/bin/

# if using system containers:
sudo mv morloc-manager /usr/bin
```

If necessary, update the path:

```sh
PATH="$HOME/.local/bin:$PATH"
```

Then install Morloc:

```sh
morloc-manager install          # latest version
morloc-manager install 0.58.3   # specific version
```

This pulls the container images and sets up the local configuration.


## Usage

`morloc-manager run` executes commands within the morloc container.

### Compile and run a program

Given the morloc program:

```morloc
module foo (double)

import root-py

double :: Int -> Int
double x = 2 * x
```

You can install the required module with

```sh
morloc-manager run -- morloc install root-py
morloc-manager run -- morloc make foo.loc
morloc-manager run -- ./foo -h              # view usage statement
morloc-manager run -- ./foo 21              # output: 42
```

**Note (Fedora/RHEL):** On SELinux-enforcing systems, you must work in a
subdirectory of your home directory (e.g. `~/project/`), not `~` itself,
`/tmp`, or other system directories.

### Interactive shell

Drop into a container shell with the full Morloc toolchain. Including language
support for Python, R, C++ as well niceties like vim

```sh
morloc-manager run --shell
```

### Switching versions

You can install multiple versions side-by-side and switch between them.

```sh
morloc-manager install 0.67.0    # install a new version
morloc-manager select 0.67.0     # switch to it
morloc-manager select 0.58.3     # switch to old version
morloc-manager info              # view installed versions
```


## Custom environments

If your project needs additional system packages or language libraries, you can
create a custom environment that layers on top of the base Morloc image.

### Create a new environment

```sh
morloc-manager env --init ml
```

This creates a stub Dockerfile at `~/.local/share/morloc/deps/ml.Dockerfile`.
Edit it to add your dependencies:

```dockerfile
# Automatically generated section, DO NOT MODIFY
ARG CONTAINER_BASE=scratch
FROM ${CONTAINER_BASE}
LABEL morloc.environment="ml"
ENV MORLOC_ENV_NAME="ml"
# End of automatically generated section

# Add custom setup below this line
RUN pip install scikit-learn matplotlib pandas
```

### Activate the environment

```sh
morloc-manager env ml
```

This builds the custom image (if needed) and updates the active environment. All
subsequent `morloc-manager run` calls use the custom environment — no flag
changes required:

```sh
morloc-manager run -- morloc make -o pipeline pipeline.loc   # runs in the ml environment
```

### Reset to the base environment

```sh
morloc-manager env --reset
```

### List available environments

```sh
morloc-manager env --list
```

## Advanced usage

### How it works

The manager uses directory-based structured config under `~/.config/morloc/`
(local scope) or `/etc/morloc/` (system scope). Key config entries include
`active_version`, `active_scope`, `active_env`, and `container_engine`.
Per-version config lives under `versions/<ver>/config` with `image`
and `host_dir`. Custom environments are stored under
`versions/<ver>/environments/`.

`morloc-manager run` invokes `docker run` / `podman run` directly (no Compose
required), bind-mounting the version data directory and your current working
directory into the container.

### Extra container flags

For changes that go beyond image selection — port mapping, GPU passthrough,
extra volumes — use the `-x` flag to pass additional arguments to the container
engine:

```sh
morloc-manager run -x "--gpus all" -- morloc make foo.loc
```

### Info and diagnostics

```sh
morloc-manager info
```

Shows installed versions, which one is selected, container engine, and
SELinux status.

### Cleaning build artifacts

Remove build artifacts without uninstalling:

```sh
morloc-manager clean --all            # clean all version data
morloc-manager clean --all --dry-run  # preview what would be removed
morloc-manager clean 0.58.3           # clean a specific version only
```

### System-wide installs (multi-user)

A sysadmin can install Morloc system-wide:

```sh
sudo morloc-manager install --system
```

After the system install, **each user** must individually select the system
version before morloc will work for them:

```sh
morloc-manager select --system <version>
```

**Note on rootless podman:** With rootless podman, each user maintains their own
image store. This means the container image will be pulled separately for each
user on first run (~1.4 GB). System uninstall does not remove per-user image
stores. To reclaim disk space for a specific user:

```sh
podman rmi ghcr.io/morloc-project/morloc/morloc-full:<version>
podman rmi ghcr.io/morloc-project/morloc/morloc-tiny:<version>
```

### Uninstall

```sh
morloc-manager uninstall 0.58.3          # remove a specific version
morloc-manager uninstall --all           # remove all local versions
sudo morloc-manager uninstall --system --all  # remove system versions
```

`uninstall --all` removes version data, container images, and configuration.


## Troubleshooting

### "Failed to build morloc libraries" during install

The `morloc init` step inside the container may fail due to file permission
issues. You can skip it and run it manually later:

```sh
morloc-manager install --no-init
morloc-manager run -- morloc init -f
```

### `morloc-nexus: not found` when running compiled programs

The morloc libraries were not initialized. Run:

```sh
morloc-manager run -- morloc init -f
```

### Permission denied errors (root-owned files)

Container processes may have created root-owned files in the data directory.
Fix with:

```sh
sudo chown -R $(id -u):$(id -g) ~/.local/share/morloc/
```

### Shell completion paths from `morloc init`

After install, `morloc init` prints `source` lines for shell completions. These
paths are relative to the container filesystem, so they work as-is inside the
shell (`morloc-manager run --shell`). If you want completions on the host
(outside the container), use the host-side paths instead:

```
~/.local/share/morloc/versions/<version>/completions/
```

For example, to enable bash completions on the host for version 0.68.0:

```sh
source ~/.local/share/morloc/versions/0.68.0/completions/morloc-completions.bash
```

You can find your active version with `morloc-manager info`.

### SELinux bind-mount errors on Fedora/RHEL

Working from `/tmp`, other system directories, or your home directory root
(`~`) is blocked by SELinux. You must work in a subdirectory of your home
directory:

```sh
mkdir -p ~/project && cp -r /tmp/myproject/* ~/project/
cd ~/project
morloc-manager run -- morloc make foo.loc
```
