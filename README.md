# morloc-manager agentic testing

Cross-environment exploratory testing of the `morloc-manager` binary using
Claude Code agents. The binary itself lives in the compiler repo; this repo
provides Vagrant VMs and the agent harness that exercises the binary across
Linux distros and security models.

## How it works

Several persona-based **tester agents** take turns probing `morloc-manager`
on a single VM, each tackling the same task from a different perspective
(new user, developer, sysadmin, power user, mathematician). After all
testers finish, an **analyst agent** folds their per-persona reports and
the shared issue log into one consolidated final report, validating
findings against the morloc compiler source.

The fold:

```
task prompt ──► tester₁ ──► tester₂ ──► … ──► testerₙ ──► analyst ──► findings/report.md
                  │            │                  │
                  └─── findings/log.md ◄──────────┘
                       (cross-tester issue log)
                  └─── findings/<persona>/report.md
                       (per-persona narrative)
```

Each tester reads `findings/log.md` first so it can skip blockers that
earlier testers already hit. If a tester encounters a setup-level disaster
that makes work impossible, it writes a `findings/HALT` sentinel; the
orchestration script then skips remaining testers but still runs the
analyst on whatever was collected.

Three VMs cover different Linux security models:

| VM     | Distro       | Primary concern              |
|--------|--------------|------------------------------|
| fedora | Fedora 40    | SELinux enforcing, cgroup v2 |
| ubuntu | Ubuntu 22.04 | AppArmor                     |
| debian | Debian 12    | cgroup v1                    |

You run one VM per session. Persona Linux users (`developer`, `newbie`,
`poweruser`, `sysadmin`, `mathematician`) are created on demand at run
time — their names come straight from `test/personas/*.md`, the single
source of truth.

## Prerequisites

- [Vagrant](https://www.vagrantup.com/) with the
  [vagrant-libvirt](https://github.com/vagrant-libvirt/vagrant-libvirt) plugin
- [Claude Code](https://claude.ai/code) CLI (`claude`)
- Docker or Podman on the host (for Vagrant provisioning)

## Setup

Three symlinks at the repo root (all gitignored):

```sh
ln -s /path/to/morloc-workspace/compiler/morloc/out/morloc-manager morloc-manager
ln -s /path/to/morloc-workspace/compiler/morloc morloc
ln -s /path/to/morloc-project.github.io morloc-project.github.io
```

The first is the binary under test. The latter two give the analyst
read-only access to compiler source and docs for bug validation.

## Quick start

```sh
make up VM=fedora                                            # provision the VM
make push VM=fedora                                          # build binary + rsync
make explore VM=fedora PROMPT=path/to/your-task-prompt.md    # run the fold
make down VM=fedora                                          # destroy when done
```

A task prompt is just a markdown file describing what you want the testers
to investigate (e.g., "Test the freeze/thaw lifecycle on rootless
podman."). The testers and analyst supply their own context — your prompt
only specifies the task.

### Tuning the agents

Both **model** and **max turns** can be tuned globally or per agent. Defaults:
`sonnet` for both agents; explorer max turns = 50, analyst max turns = 80.

```sh
make explore VM=fedora PROMPT=... MODEL=haiku                          # both agents on haiku
make explore VM=fedora PROMPT=... MODEL=haiku ANALYST_MODEL=opus       # cheap testers, smart analyst
make explore VM=fedora PROMPT=... MAX_TURNS=120                        # both agents up to 120 turns
make explore VM=fedora PROMPT=... ANALYST_MODEL=opus ANALYST_MAX_TURNS=150
```

`MODEL` and `MAX_TURNS` set both agents at once. `EXPLORER_*` and `ANALYST_*`
override individually and take precedence. The same flags exist on
`run-exploration.sh` directly: `--model`, `--explorer-model`,
`--analyst-model`, `--max-turns`, `--explorer-max-turns`,
`--analyst-max-turns`. Useful because the explorer runs once per persona
(typically 5×) while the analyst runs once — pairing a cheaper explorer
with a stronger analyst (and a larger turn budget) is often the right
tradeoff for hard problems.

## File map

| Path                          | Purpose                                                  |
|-------------------------------|----------------------------------------------------------|
| `Vagrantfile`                 | VM definitions (Docker, Podman, tools, image pulls)      |
| `Makefile`                    | Build, sync, and exploration targets (`make help`)       |
| `test/run-exploration.sh`     | Pure orchestration: discover personas, loop, run analyst |
| `test/explorer-context.md`    | Single source of truth for tester agents                 |
| `test/analyst-context.md`     | Single source of truth for the analyst agent             |
| `test/personas/*.md`          | Persona descriptions (filename = VM username)            |
| `.claude/agents/vm-*.md`      | Thin agent definitions (YAML frontmatter only)           |
| `findings/`                   | Run output (gitignored)                                  |

## Findings layout

After a run:

```
findings/
├── <persona>/
│   ├── report.md            # narrative, this persona's perspective
│   └── session.log          # raw claude session log
├── log.md                   # shared cross-tester issue log
├── HALT                     # only if a tester aborted the run
├── analyst-session.log
└── report.md                # final consolidated analyst report
```

## Editing the harness

- **Add a persona**: drop a new `test/personas/<name>.md` describing the
  tester's approach and perspective. The script picks it up automatically
  and creates the matching VM user on demand. No other files need to change.
- **Change tester instructions** (workflow, formats, mechanics): edit
  `test/explorer-context.md`. That file is the only place these live.
- **Change analyst instructions** (validation rules, report format): edit
  `test/analyst-context.md`.
- **Change orchestration** (VM list, paths, agent invocation): edit
  `test/run-exploration.sh`. It contains no agent prose — only mechanics.

## Make targets

```
make up VM=...                       Start a VM
make down VM=...                     Destroy a VM
make sync VM=...                     Rsync files into a running VM
make push VM=...                     Rebuild binary + rsync into VM
make build-images                    Build morloc-tiny + morloc-full containers
make build-all                       Binaries + containers
make push-image VM=...               Build + load image into a VM
make explore VM=... PROMPT=...       Run all personas on a VM
make explore-sync VM=... PROMPT=...  Sync + run all personas on a VM
make quick-test VM=...               Push + smoke test
make clean                           Remove per-persona findings
make pristine                        Remove all findings (log, report, HALT)
make help                            Show available targets
```
