# Morloc Manager

Handle Morloc installation and removal

 <img src="assets/install.png" alt="morloc install example" class="center"> 

Setup morloc containers, scripts, and home for either the latest version of
Morloc or for the specified version.


## Installation

The only dependency is a container engine, currently two
[docker](https://docs.docker.com/engine/install/) and
[podman](https://podman.io/docs/installation) are supported.

You can retrieve the manager with curl:

```
curl -o morloc-manager https://raw.githubusercontent.com/morloc-project/morloc-manager/refs/heads/main/morloc-manager.sh
```

Then move it into your path (e.g., move it to ~/.local/bin) and make it
executable.

## Usage

Basic usage information is available for the main script and all subcommands: 

```
morloc-manager -h
morloc-manager install -h
morloc-manager uninstall -h
```

To install the latest version of Morloc, run `install` with no arguments:

```
morloc-manager install
```

This will retrieve required containers, create the morloc home directory, and
make four executable scripts:

 1. menv: runs commands in a Morloc container. Examples:

```
$ menv morloc make -o foo foo.loc
$ menv ./foo double 21
```

 2. morloc-shell: enter the "full" container in a shell
    - contains Python, R, and C++ compiler
    - contains vim and other conveniences

 3. menv-dev: runs commands in a dev container
    - contains Haskell tools for building from source
    - can access all system executables

 4. morloc-shell-dev: enter the dev shell


## Testing

The manager has a test suite that checks everything from individual helper
functions up through the full new-user installation experience. Tests are
organized in four tiers:

 - **Unit tests** verify that the script's internal functions (shell detection,
   path management, config file editing, argument parsing) behave correctly in
   isolation. These run instantly and need nothing beyond Bash.

 - **Integration tests** exercise each subcommand (install, uninstall, select,
   env, update) against a mock container engine, checking that the right
   directories are created, wrapper scripts have the right content, and error
   cases are handled gracefully.

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
