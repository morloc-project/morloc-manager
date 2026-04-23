## Testing

### Agent-based exploratory testing

Testing uses autonomous Claude Code agents that SSH into a Vagrant VM and
explore morloc-manager as different user personas. Each persona has its own
Linux user account on the VM, so all personas can run without resetting state
between them. Agents start in dirty sessions with whatever artifacts remain
from previous runs.

Four personas cover different testing approaches:

| Persona    | Linux user  | Focus                                      |
|------------|-------------|--------------------------------------------|
| new-user   | newuser     | Discoverability, defaults, error messages   |
| developer  | developer   | End-to-end workflows, engine parity         |
| power-user | poweruser   | Edge cases, invalid inputs, state conflicts |
| sysadmin   | sysadmin    | System scope, permissions, multi-user       |

Three VMs cover different Linux security models:

| VM     | Distro       | Primary concern                |
|--------|--------------|--------------------------------|
| fedora | Fedora 40    | SELinux enforcing, cgroup v2   |
| ubuntu | Ubuntu 22.04 | AppArmor                       |
| debian | Debian 12    | cgroup v1                      |

To run exploration on a single VM:

```sh
./test/run-exploration.sh --vm fedora test/prompts/full-exploration.md
./test/run-exploration.sh --vm fedora test/prompts/full-exploration.md --personas developer,new-user
```

Results land in `findings/`. After all personas complete, an analyst agent
folds the bug reports into a single `findings/action-plan.md` grouped by root
cause.

Run `make help` to see all targets.
