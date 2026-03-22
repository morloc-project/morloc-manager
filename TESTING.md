## Testing

### Static analysis

```sh
make lint     # runs ShellCheck
```

### Agent-based exploratory testing

Testing uses autonomous Claude Code agents that SSH into Vagrant VMs and explore
morloc-manager as different user personas (new user, developer, sysadmin, power
user). Each persona tries real workflows on a fresh VM with both Docker and
Podman, logging any bugs found.

Three VMs cover different Linux security models:

| VM | Distro | Primary concern |
|----|--------|-----------------|
| fedora | Fedora 40 | SELinux enforcing, cgroup v2 |
| ubuntu | Ubuntu 22.04 | AppArmor |
| debian | Debian 12 | cgroup v1 |

To run the full exploration overnight (one VM at a time, disk-friendly):

```sh
./test/run-exploration.sh                  # all VMs
./test/run-exploration.sh fedora           # single VM
```

Results land in `findings/`. After all VMs complete, an analyst agent folds
the bug reports into a single `findings/action-plan.md` grouped by root cause.

Run `make help` to see all targets.
