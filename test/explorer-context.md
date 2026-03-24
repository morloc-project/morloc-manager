<!-- Background context included in every vm-explorer agent session -->
<!-- Edit this file to steer what the explorer agents focus on across all personas -->

## Known behaviors (not bugs)

- **Shell completion paths from `morloc init`**: The paths printed by `morloc init` are container-internal paths. They are correct when working inside `morloc-manager run --shell` or `morloc-manager run bash -c "..."`. On the host, completions are at `~/.local/share/morloc/versions/<version>/completions/`. This is documented in the README troubleshooting section. Do not report this as a bug.

## Scope flags

- `--system` and `--local` are subcommand flags that go AFTER the subcommand name.
- Most subcommands accept them: `install`, `select`, `run`, `info`, `uninstall`, `clean`.
- `env` does NOT accept scope flags — it always uses the active scope.
- `--container-engine` IS a global flag and goes BEFORE the subcommand:
  - Correct: `morloc-manager --container-engine podman install edge`
