# morloc-manager testing

Cross-environment testing for the morloc-manager binary. The binary itself lives
in the compiler repo (`morloc-workspace/compiler/morloc/data/rust/morloc-manager/`); this
repo provides Vagrant VMs and agent-based exploratory testing to validate it
across Linux distributions and security models.

For morloc-manager usage, see the tutorials:
- [Part 1: Development](tutorial-development.asc) -- installation, environments, writing programs
- [Part 2: Deployment](tutorial-deployment.asc) -- serving, freezing, system environments

For morloc language documentation, see the official [docs](https://morloc-project.github.io).

For quick usage information, call `morloc-manager -h` for global usage or
`morloc-manager <subcommand> -h` for subcommand usage.

## Prerequisites

- [Vagrant](https://www.vagrantup.com/) with the
  [vagrant-libvirt](https://github.com/vagrant-libvirt/vagrant-libvirt) plugin
- [Claude Code](https://claude.ai/code) CLI (`claude`)
- A container engine on the host: Docker or Podman (for Vagrant provisioning)

## Setup

The analyst agent needs read-only access to the morloc compiler source and
documentation to validate bugs and diagnose root causes. Create symlinks in
the repo root pointing to your local checkouts:

```sh
ln -s /path/to/morloc-workspace/compiler/morloc morloc
ln -s /path/to/morloc-project.github.io morloc-project.github.io
```

These symlinks are gitignored. The `morloc-manager` binary symlink (already
documented) is also required:

```sh
ln -s /path/to/morloc-workspace/compiler/morloc/out/morloc-manager morloc-manager
```

## Quick start

```sh
# Start a VM
make up VM=fedora

# Run all personas on that VM
make explore VM=fedora PROMPT=test/prompts/full-exploration.md

# Rebuild binary + sync into VM
make push VM=fedora

# Destroy when done
make down VM=fedora
```

## How it works

Autonomous Claude Code agents SSH into a Vagrant VM and exercise
morloc-manager as different user personas (new user, developer, sysadmin,
power user). Each persona has its own Linux user account on the VM, so no
state is reset between runs -- agents start in dirty sessions with whatever
artifacts remain from past runs. Both Docker and Podman are installed on
each VM.

You select a single VM per run. Three VMs cover different Linux security models:

| VM | Distro | Primary concern |
|----|--------|-----------------|
| fedora | Fedora 40 | SELinux enforcing, cgroup v2 |
| ubuntu | Ubuntu 22.04 | AppArmor |
| debian | Debian 12 | cgroup v1 |

Results land in `findings/`. After all personas complete, an analyst agent
consolidates bug reports into `findings/action-plan.md` (grouped by root
cause) and `findings/report.md`.

See [TESTING.md](TESTING.md) for details on personas and methodology.

## Make targets

```
make up VM=...       Start a VM
make down VM=...     Destroy a VM
make sync VM=...     Rsync files into a running VM
make push VM=...     Rebuild binary + rsync into VM
make explore VM=... PROMPT=...  Run all personas on a VM
make quick-test      Push + smoke test
make clean           Remove exploration findings
make help            Show available targets
```
