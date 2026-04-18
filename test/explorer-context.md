Focus on morloc-manager issues

Only report issues that effect the LATEST edge version of morloc and
morloc-manager. You should test version switching, but do not report non-edge
issues.

Always list the morloc versions used (accessible through `morloc-manager run --
morloc --version`)

Do not focus on perceived bugs in morloc, such as no messages printed when `morloc
make` is run or the design decision to name the generated executable after the
module not the filename

Explore as much as possible searching for clear bugs, but if none are found, do
not invent issues to report.

## Important

 * ALL diagnostics, messages (whether successful or failing), and logs should go
   to STDERR, not STDOUT.

 * Failure should ALWAYS return a non-zero exit code

 * The "--" use in `morloc-manager run` is a standard UNIX convention, do not
   report it as undocumented syntax

## CLI structure

Commands are grouped into Development and Deployment.
Use `morloc-manager --help` to see the full list.

Key commands:
- `new`, `run`, `rm`, `ls`, `info`, `select`, `update` (Development)
- `start`, `stop`, `freeze`, `unfreeze`, `status`, `logs` (Deployment)

There are no separate "versions" or "workspaces" -- everything is an
**environment**. Each environment has a name, a base container image, optional
Dockerfile customizations, engine flags, and its own data directories.

Note: the old `install`, `uninstall`, `setup`, `env`, `new` (workspace),
`delete` commands no longer exist. Use `new` to create environments, `rm` to
remove them, `update` to modify them.

Note: the old `--scope SCOPE` flag no longer exists. Local scope is the
default. Use `--system` to target the system scope (e.g., `new --system`).

## Known-issues.md format

When adding new entries to known-issues.md, follow this exact format:

```markdown
## KI-NNN: Short descriptive title

- **severity**: critical-blocker | major | minor | note
- **scope**: all-vms | fedora | ubuntu,debian
- **found-by**: <persona> on <vm>
- **confirmed-by**: <persona> on <vm>, ...
- **workaround**: Exact commands to get past this issue (or "none known")
- **blocks**: What downstream functionality is affected
```

Severity levels:
- `critical-blocker` -- makes further testing impossible without a workaround
- `major` -- significant functionality broken but workarounds exist
- `minor` -- cosmetic, edge case, or documentation mismatch
- `note` -- observation, not a bug

## Known behaviors (not bugs)

- **Shell completion paths from `morloc init`**: The paths printed by `morloc
  init` are container-internal paths. They are correct when working inside
  `morloc-manager run --shell` or `morloc-manager run -- bash -c "..."`. This is
  documented in the README troubleshooting section. Do not report this as a bug.
