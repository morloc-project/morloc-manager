# Morloc Manager

Handle Morloc installation, version management, and custom environments.

 <img src="assets/install.png" alt="morloc install example" class="center">


## Prerequisites

You need a container engine with Compose support:

 * [Docker](https://docs.docker.com/engine/install/) (v2 plugin — `docker compose` — is included in modern Docker Desktop and Engine installs)
 * [Podman](https://podman.io/docs/installation) with `podman compose` or standalone `podman-compose`

The manager auto-detects whichever is available. You can override the choice with `--container-engine` or the `MORLOC_CONTAINER_ENGINE` environment variable.


## Installation

Download the manager and place it on your PATH:

```sh
curl -o morloc-manager https://raw.githubusercontent.com/morloc-project/morloc-manager/refs/heads/main/morloc-manager.sh
chmod +x morloc-manager
mv morloc-manager ~/.local/bin/
```

Then install Morloc:

```sh
morloc-manager install          # latest version
morloc-manager install 0.58.3   # specific version
```

This pulls the container images, generates a Docker Compose configuration, and
creates the `menv` wrapper script in `~/.local/bin/`.


## Usage

`menv` is the single command for running anything inside the Morloc container.
Your current working directory is automatically mounted into the container.

### Compile and run a program

```sh
menv morloc make -o foo foo.loc
menv ./foo double 21              # output: 42
```

### Interactive shell

Drop into a container shell with the full Morloc toolchain. Including language
support for Python, R, C++ as well niceties like vim

```sh
menv --shell
```

### Compiler development

The dev container includes Haskell tools for building the compiler from source:

```sh
menv --dev stack build            # build the compiler
menv --dev stack test             # run the test suite
menv --dev --shell                # interactive dev shell
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
ARG CONTAINER_BASE
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

This builds the custom image (if needed) and updates `.env` so that `menv` uses
it. All subsequent `menv` calls run inside the custom environment — no flag
changes required:

```sh
menv morloc make -o pipeline pipeline.loc   # runs in the ml environment
```

### Reset to the base environment

```sh
morloc-manager env --reset
```

### List available environments

```sh
morloc-manager env --list
```

### Other options

```sh
morloc-manager env ml --dev            # apply only to the dev container
morloc-manager env ml --usr            # apply only to the user container
```


## Advanced usage

### How it works

The manager generates three files in `~/.local/share/morloc/`:

| File | Purpose |
|---|---|
| `docker-compose.yml` | Defines two services: `morloc` (user) and `morloc-dev` (compiler development). Regenerated on `install`. |
| `.env` | Stores the active version, image tags, host paths, and container engine. Edited by `select` and `env` — no script regeneration needed. |
| `docker-compose.override.yml` | Optional. If you create this file, Compose auto-merges it. The manager never writes, reads, or deletes it. |

The `menv` wrapper script reads these files and calls `docker compose run`
(or `podman compose run`) with the appropriate service.

### Editing `.env` directly

Advanced users can edit `~/.local/share/morloc/.env` to:

| Goal | Variable to change |
|---|---|
| Pin a different version | `MORLOC_VERSION` and `MORLOC_IMAGE` |
| Use a custom image | `MORLOC_IMAGE=my-registry/my-image:tag` |
| Switch container engine | `MORLOC_CONTAINER_ENGINE=podman` |

### Override file

For changes that go beyond image selection — port mapping, GPU passthrough,
extra volumes — create `~/.local/share/morloc/docker-compose.override.yml`:

```yaml
services:
  morloc:
    ports:
      - "8080:8080"
    deploy:
      resources:
        reservations:
          devices:
            - capabilities: [gpu]
```

Compose merges this automatically with the generated `docker-compose.yml`.

### Info and diagnostics

```sh
morloc-manager info
```

Shows installed versions, which one is selected, compose file locations,
container engine, and SELinux status.

### Uninstall

```sh
morloc-manager uninstall 0.58.3   # remove a specific version
morloc-manager uninstall --all    # remove everything
```

`uninstall --all` removes version data, compose files, and container images.
The override file (if any) is preserved with a warning. Scripts in
`~/.local/bin/` are not removed — the output tells you how to delete them.


## Testing

The manager has a test suite that checks everything from individual helper
functions up through the full new-user installation experience. Tests are
organized in four tiers:

 - **Unit tests** verify that the script's internal functions (shell detection,
   path management, config file editing, argument parsing) behave correctly in
   isolation. These run instantly and need nothing beyond Bash.

 - **Integration tests** exercise each subcommand (install, uninstall, select,
   env, update) against a mock container engine, checking that the right
   directories are created, compose files have the right content, `.env` values
   are correct, and error cases are handled gracefully.

 - **End-to-end tests** run the actual installation workflow with a real
   Docker or Podman engine, including compiling and running a morloc program
   inside a container.

 - **VM tests** spin up full virtual machines to validate the manager on
   enterprise Linux configurations (SELinux enforcing, AppArmor, cgroup v1/v2)
   and to provide a testing environment for rootful container support, which is
   a major planned feature. Running inside real VMs is the only way to test
   these kernel-level security and container runtime behaviors.

To run the fast tests locally (no container engine required):

```
make test
```

Run `make help` to see all targets, or see [test/README.md](test/README.md)
for full details on running every tier.
