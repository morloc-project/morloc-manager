# morloc-manager testing

Cross-environment testing for the morloc-manager binary. The binary itself lives
in the compiler repo (`morloc-workspace/compiler/morloc/data/rust/morloc-manager/`); this
repo provides Vagrant VMs and agent-based exploratory testing to validate it
across Linux distributions and security models.

For morloc language documentation, see the official [docs](https://morloc-project.github.io).

For quick usage information, call `morloc-manager -h` for global usage or
`morloc-manager <subcommand> -h` for subcommand usage.

## Using morloc-manager

### Setup (first time only)

```sh
# Pick a default container engine (podman or docker)
morloc-manager setup --engine podman

# Check current config
morloc-manager setup
```

If only one container engine is installed, `new` auto-detects it. If both are
installed, you must run `setup` first.

### Creating an environment

```sh
# Interactive wizard (prompts for name, base image)
morloc-manager new

# Non-interactive: create from a specific morloc version
morloc-manager new myenv --version 0.73.0

# Default (when --version and --image are both omitted): pulls the :edge image
# from the morloc registry and records the resolved version

# With a custom Dockerfile layer
morloc-manager new myenv --version 0.73.0 --dockerfile ./Dockerfile

# With a stub Dockerfile for later customization
morloc-manager new myenv --version 0.73.0 --dockerfile-stub

# Include files in the Dockerfile build context
morloc-manager new myenv --version 0.73.0 --dockerfile ./Dockerfile -i ./data.csv

# Check what's installed
morloc-manager info
```

### Running morloc inside the container

Once an environment is created and selected, use `morloc-manager run` to execute
commands inside the container. The current working directory is bind-mounted
automatically.

Use `--` to separate `morloc-manager` flags from inner command flags. Without
`--`, flags like `-o` or `--version` are consumed by `morloc-manager run`
itself rather than passed to the inner command.

```sh
# Install a standard library module
morloc-manager run -- morloc install root

# Build a morloc program (-- is required so -o goes to morloc, not to run)
morloc-manager run -- morloc make -o hello hello.loc

# Run the compiled program (no flags to inner command, so -- is optional)
morloc-manager run ./hello foo '["x","y"]'

# Open an interactive shell in the container
morloc-manager run --shell
```

### Managing environments

```sh
# List all environments
morloc-manager ls

# Switch to a different environment
morloc-manager select myenv

# Show detailed info for an environment
morloc-manager info myenv

# Remove an environment
morloc-manager rm myenv
```

### Updating environments

The `update` command modifies an existing environment. It accepts the same flags
as `new` but defaults to keeping existing values.

```sh
# Rebuild the active environment (after editing its Dockerfile)
morloc-manager update

# Change shared memory size
morloc-manager update --shm-size 1g

# Replace the Dockerfile and rebuild
morloc-manager update --dockerfile ./new.Dockerfile

# Add files to the build context
morloc-manager update -i ./newdata.txt

# Re-run morloc init (e.g., after changing the base image)
morloc-manager update --version 0.74.0 --reinit
```

### Serving

`start` launches a network service for an environment using bind-mounted
state. It's fast, works with any environment, and requires no build step.

```sh
# Serve the active environment on :8080
morloc-manager start

# Serve a specific environment on a custom port
morloc-manager start myenv -p 9090:8080

# List running servers, stop
morloc-manager status
morloc-manager stop myenv
```

### Freezing (exporting for external deployment)

`freeze` packages an environment's state as a portable tarball. `unfreeze`
turns that tarball into a standalone Docker image suitable for registries
or Kubernetes.

```sh
# Export state from the active environment (requires compiled programs)
morloc-manager freeze -o ./my-freeze/

# Build a portable serve image (bakes state into the image)
morloc-manager unfreeze --from ./my-freeze/state.tar.gz --tag my-app:v1

# Deploy externally (docker push, helm install, etc.)
# The resulting image runs morloc-nexus on :8080 as its entrypoint.
docker run -d -p 8080:8080 my-app:v1
```

`freeze`/`unfreeze` produce images intended for **external** deployment.
For local serving, use `start` directly — no freeze step needed.

### System-wide environments

System-wide environments (`--system`) are stored under `/etc/morloc/` and
`/usr/local/share/morloc/` and are useful for shared servers. An admin creates
and builds the environment; regular users can select and run it.

```sh
# Admin creates and builds a system environment
sudo morloc-manager new shared-env --version 0.76.0 --system

# Regular user selects and uses it
morloc-manager select shared-env
morloc-manager run -- morloc --version
```

**Podman note.** Podman stores images per-user, so root's images are not
visible to regular users by default. On Fedora and Debian (where
`/var/lib/containers/storage` is the rootful graphroot), the commonly suggested
`additionalimagestores` workaround causes locking conflicts. For system
environments, use Docker instead:

```sh
sudo morloc-manager setup --engine docker
sudo usermod -aG docker $USER
```

Docker's socket-based model shares images across users without storage
conflicts.

**Write permissions.** System environment data directories are read-only for
regular users. Operations that write to the module store -- `morloc install`
and `morloc make --install` -- will fail with permission denied. Plain
`morloc make` still works: it writes the nexus binary and pool files to the
current working directory, not to the system data directory. Users who need
to install new modules should create a local environment
(`morloc-manager new myproject`) alongside the system one.

## Morloc language examples

These examples show what morloc programs look like. To run them, save the
`.loc` file and any supporting source files in the same directory, then:

```sh
morloc-manager run -- morloc install root root-py root-cpp
morloc-manager run -- morloc make -o example main.loc
morloc-manager run ./example <subcommand> <args>
```

### Arithmetic (pure morloc + Python)

A minimal example using Python arithmetic operators via `root-py`:

```morloc
-- main.loc
module main (foo)

import root-py

foo x = x + 2.0 * 20.0
```

```sh
$ morloc-manager run ./example foo 2
42
```

### Function composition (Python)

Compose functions with the `.` operator, just like Haskell:

```morloc
-- main.loc
module main (foo, bar)

import root-py

source py from "paste.py" (
    "morloc_paste" as paste
    )

source py ("abs")

abs :: Real -> Real
paste :: Str -> Str -> Str

foo :: [Str] -> [Str]
foo xs = map (paste "a" . paste "b" . paste "c") xs

bar :: Real -> Real
bar = abs . (-) 1.0 . abs
```

```python
# paste.py
def morloc_paste(x, y):
    return x + y
```

```sh
$ morloc-manager run ./example foo '["x","y"]'
["abcx","abcy"]
$ morloc-manager run ./example bar -5
1
```

### Cross-language recursion (Python + C++)

A factorial function where multiplication is in Python and subtraction is in
C++. Morloc handles the cross-language calls and serialization automatically:

```morloc
-- main.loc
module main (fact)

import root-py
import root-cpp

source Py from "py_helpers.py" ("py_mul" as mul)
mul :: Int -> Int -> Int

source Cpp from "cpp_helpers.hpp" ("cpp_sub" as sub)
sub :: Int -> Int -> Int

fact :: Int -> Int
fact n
  ? n == 0 = 1
  : mul n (fact (sub n 1))
```

```python
# py_helpers.py
def py_mul(a, b):
    return a * b
```

```cpp
// cpp_helpers.hpp
int cpp_sub(int a, int b) {
    return a - b;
}
```

```sh
$ morloc-manager run ./example fact 0
1
$ morloc-manager run ./example fact 1
1
$ morloc-manager run ./example fact 5
120
```

### Records and pure evaluation

Morloc supports records and can evaluate pure expressions (no imports needed)
directly in the nexus:

```morloc
-- main.loc
module main (greeting, point, getX)

record Point = Point { x :: Int, y :: Int }

greeting :: Str
greeting = let name = "world" in "hello"

point :: Point
point = let p = { x = 10, y = 20 } in p

getX :: Int
getX = let p = { x = 10, y = 20 } in .x p
```

```sh
$ morloc-manager run ./example greeting
"hello"
$ morloc-manager run ./example point
{"x":10,"y":20}
$ morloc-manager run ./example getX
10
```

## Prerequisites

- [Vagrant](https://www.vagrantup.com/) with the
  [vagrant-libvirt](https://github.com/vagrant-libvirt/vagrant-libvirt) plugin
- [Claude Code](https://claude.ai/code) CLI (`claude`)
- A container engine on the host: Docker or Podman (for Vagrant provisioning)

## Quick start

```sh
# Run exploration on all VMs (overnight, sequential)
make explore

# Single VM
make explore-vm VM=fedora

# Just start the VMs for manual testing
make vm-up
```


## How it works

Autonomous Claude Code agents SSH into Vagrant VMs and exercise
morloc-manager as different user personas (new user, developer, sysadmin,
power user). Each persona runs real workflows on a fresh VM with both Docker
and Podman installed, logging any bugs found.

Three VMs cover different Linux security models:

| VM | Distro | Primary concern |
|----|--------|-----------------|
| fedora | Fedora 40 | SELinux enforcing, cgroup v2 |
| ubuntu | Ubuntu 22.04 | AppArmor |
| debian | Debian 12 | cgroup v1 |

Results land in `findings/`. After all VMs complete, an analyst agent
consolidates bug reports into `findings/action-plan.md` (grouped by root
cause) and `findings/ux-report.md`.

See [TESTING.md](TESTING.md) for details on personas and methodology.


## Make targets

```
make explore       Run all personas on all VMs (overnight, sequential)
make explore-vm    Run all personas on one VM (e.g., VM=fedora)
make vm-up         Start Vagrant VMs
make vm-destroy    Destroy Vagrant VMs
make clean         Remove exploration findings
make help          Show available targets
```
