# morloc-manager test suite

## Prerequisites

The test framework is [BATS](https://github.com/bats-core/bats-core) (Bash
Automated Testing System), included as git submodules under `test/lib/`. After
cloning, initialize them:

```
make setup
```

No other dependencies are needed for unit and integration tests. End-to-end
tests require Docker or Podman. VM tests require Vagrant with the libvirt
provider.

## Directory layout

```
test/
  lib/
    bats/              # bats-core (submodule)
    bats-support/      # assertion helpers (submodule)
    bats-assert/       # assert_output, assert_success, etc. (submodule)
    bats-file/         # assert_file_exists, assert_dir_exists, etc. (submodule)
  helpers/
    common.bash        # Shared setup: HOME isolation, BATS library loading,
                       #   source_morloc_manager, assert_file_contains
    mock_engine.bash   # Mock Docker/Podman binaries for offline testing
  unit/                # Pure function tests (no container engine)
    detect_shell.bats
    normalize_path.bats
    path_management.bats
    config_files.bats
    container_engine.bats
    argument_parsing.bats
  integration/         # Subcommand tests with mock engine
    install.bats
    uninstall.bats
    select.bats
    env.bats
    script_generation.bats
    update.bats
  e2e/                 # Full workflow tests (need real engine or CI)
    fresh_install.bats
    compile_and_run.bats
    dependency_env.bats
    version_switch.bats
    post_install_validation.bats
  vm/                  # VM-specific kernel/security tests
    selinux.bats       # SELinux enforcing mode tests
    apparmor.bats      # AppArmor profile tests
    cgroup.bats        # cgroup v1/v2 hierarchy tests
    engine_modes.bats  # Docker/Podman rootless/rootful matrix
    rootful.bats       # Rootful acceptance criteria (all skip)
  distro/              # Dockerfiles for the CI distro matrix
    Dockerfile.ubuntu-22.04
    Dockerfile.ubuntu-24.04
    Dockerfile.fedora-39
    Dockerfile.debian-12
    Dockerfile.alpine-3.19
  fixtures/
    hello.loc          # Minimal morloc program for smoke tests
    sample.Dockerfile  # Sample custom environment Dockerfile
  run-vm-tests.sh      # Orchestrator for Tier 2 VM tests
```

## Running tests

A Makefile provides shortcuts for all common operations. Run `make help` to see
everything, or use the targets below.

### Unit + integration (fast, offline)

```
make test              # unit + integration (the default target)
make unit              # unit tests only
make integration       # integration tests only
make lint              # ShellCheck
make check             # lint + unit + integration
```

These create a temporary `$HOME` for each test and clean it up afterward.
Nothing touches your real home directory. A mock container engine binary is
injected into `$PATH` so no Docker or Podman installation is needed.

To run a single test file or filter by name, call BATS directly:

```
test/lib/bats/bin/bats test/unit/detect_shell.bats
test/lib/bats/bin/bats --filter "idempotent" test/integration/
```

### End-to-end (needs container engine)

E2E tests detect whether Docker or Podman is available and skip automatically
if neither is found.

```
make e2e                                                  # auto-detect engine
MORLOC_CONTAINER_ENGINE=podman make e2e                   # force Podman
make all                                                  # unit + integration + e2e
```

Some e2e tests (`compile_and_run.bats`, `post_install_validation.bats`) also
require the morloc container images to be pulled. They skip if the images are
absent.

### Distro matrix (Docker-in-Docker)

Build test containers for every distro, then run the e2e suite inside each:

```
make distro-test       # builds all containers, then runs e2e in each
make distro-build      # build only (no tests)
```

For Podman (daemonless, no socket needed), run the tests directly on the host.

### VM tests (Tier 2 -- enterprise and rootful environments)

The VM tier exists to test the manager on real enterprise Linux configurations
that containers alone cannot reproduce.  Two goals drive this tier:

 1. **Enterprise compatibility.** Many production Linux systems run mandatory
    access control (SELinux, AppArmor) and mixed cgroup hierarchies.  Bind
    mounts, shared memory, and UID mapping behave differently under these
    policies, and the only way to verify correct behavior is inside a full VM
    with the relevant kernel modules and policies active.

 2. **Rootful container support (planned).** The manager currently only
    supports rootless Podman and Docker-with-user-access.  A major planned
    feature is first-class rootful operation (`sudo podman`, system-wide
    Docker daemon) for locked-down servers where users cannot run rootless
    containers.  The VM tier provides the controlled environment needed to
    develop and validate this support: each VM can be configured with
    specific privilege models, socket permissions, and storage drivers that
    match real rootful deployments.

#### 3-VM layout

Each VM gets **both** Docker and Podman installed, a `testuser` for rootless
testing, and morloc images pulled during provisioning.

| VM       | Distro        | Primary concern         | Also tests                     |
|----------|---------------|-------------------------|--------------------------------|
| `fedora` | Fedora 40     | SELinux enforcing, cgv2 | Docker+Podman rootless/rootful |
| `ubuntu` | Ubuntu 22.04  | AppArmor                | Docker+Podman rootless/rootful |
| `debian` | Debian 12     | cgroup v1               | Docker+Podman rootless/rootful |

#### VM test files

VM-specific tests live in `test/vm/`:

| File                | Tests                                                      |
|---------------------|------------------------------------------------------------|
| `selinux.bats`      | SELinux enforcing, AVC denials, `:z` labels (future)       |
| `apparmor.bats`     | AppArmor active, denial checks, bind mount permissions     |
| `cgroup.bats`       | cgroup v1/v2 detection, `--shm-size` behavior              |
| `engine_modes.bats` | Docker/Podman rootless, rootful (future)                   |
| `rootful.bats`      | All skip -- acceptance criteria for rootful support        |

#### Engine x mode matrix

The test runner exercises each VM with each engine in each mode:

```
For each VM (fedora, ubuntu, debian):
  1. Unit + integration (sanity check)
  2. test/vm/*.bats (VM-specific kernel tests)
  3. For each engine in {docker, podman}:
     a. MORLOC_CONTAINER_ENGINE=$engine bats test/e2e/ (rootless)
     b. (future) rootful e2e tests
```

Output format: `[VM] [engine] [mode] [suite] PASS/FAIL/SKIP`

#### Running VM tests

Prerequisites: `vagrant`, `vagrant-libvirt` plugin, KVM/libvirt running.

```
make vm-test              # full suite: start VMs, run all tests, prompt to destroy
make vm-test-quick        # unit + integration only inside VMs (faster)
make vm-up                # start VMs only
make vm-destroy           # tear down all VMs
```

Run tests on a single VM:

```
vagrant up fedora
vagrant ssh fedora -c "cd /vagrant && bats test/vm/"
vagrant ssh fedora -c "cd /vagrant && bats test/vm/selinux.bats"
```

Run the full matrix on one VM:

```
./test/run-vm-tests.sh fedora
./test/run-vm-tests.sh --no-destroy fedora ubuntu
```

#### Adding rootful tests

When rootful support is added to `morloc-manager.sh`:

1. Remove `skip` lines from `test/vm/rootful.bats`
2. Remove `require_rootful_support` calls from rootful tests in `engine_modes.bats`
3. Uncomment the rootful e2e section in `test/run-vm-tests.sh`
4. The existing tests define the acceptance criteria -- they should pass as-is

## How the test harness works

`morloc-manager.sh` has a guard at the bottom:

```sh
if [ "${MORLOC_MANAGER_TESTING:-}" != "1" ]; then
    main "$@"
fi
```

When BATS sources the script with `MORLOC_MANAGER_TESTING=1`, `main` is not
called. All functions (`detect_shell`, `normalize_path`, `script_menv`, etc.)
become available for direct invocation in tests.

Every test that touches the filesystem uses `setup_isolated_home` (defined in
`helpers/common.bash`), which creates a fresh temporary `$HOME` and restores the
original in `teardown`. This guarantees tests cannot interfere with each other
or with the developer's real environment.

The mock engine (`helpers/mock_engine.bash`) creates a minimal shell script at a
temporary path that responds to `--version`, `run`, `pull`, `build`, `images`,
etc. with canned output. It is prepended to `$PATH` before the manager script
is sourced, so the auto-detection logic finds it as if it were a real engine.

## CI workflows

| Workflow             | Trigger                  | What runs                        | Time   |
|----------------------|--------------------------|----------------------------------|--------|
| `lint.yml`           | Every push, PRs          | ShellCheck                       | ~30s   |
| `unit.yml`           | Every push, PRs          | `bats test/unit/`                | ~30s   |
| `integration.yml`    | Every push, PRs          | `bats test/integration/` x {docker, podman} | ~2m |
| `e2e-matrix.yml`     | Weekly, release tags     | Distro x engine matrix + ARM64   | ~30m   |

## Writing new tests

1. Put pure-function tests in `test/unit/`. These should not need a container
   engine or network access.
2. Put subcommand-level tests that use mock engines in `test/integration/`.
3. Put tests that need a real container engine in `test/e2e/`. Always check for
   engine availability and `skip` if absent.
4. Use `setup_isolated_home` / `teardown_isolated_home` in every test that
   writes to the filesystem.
5. Use `setup_mock_engine` (not in a subshell) when you need a fake
   Docker/Podman.
6. Use `assert_file_contains` / `assert_file_not_contains` for checking
   generated script content -- they handle patterns that start with dashes.
