Focus on morloc-manager issues

## CRITICAL: Use the latest morloc version

When creating your FIRST environment, ALWAYS omit `--version` so that
morloc-manager pulls the latest release. Do NOT copy version numbers from
tutorial examples — those are outdated. Example:

    morloc-manager new myenv          # CORRECT — gets latest
    morloc-manager new myenv --version 0.79.3   # WRONG — stale tutorial version

After your primary environment is working with the latest version, you MAY
create a second environment with an older `--version` to test version
selection, but do NOT report errors in old morloc versions.

Always list the morloc versions used (accessible through `morloc-manager run --
morloc --version`)

Explore as much as possible searching for clear bugs, but if none are found, do
not invent issues to report.

## Important

 * Informational commands (`ls`, `info`, `status`, `doctor`) return data on
   STDOUT -- this is their logical return value. Mutational commands (`start`,
   `stop`, `freeze`, `nuke`, `update`) have no return value; all their output
   (progress, success messages, errors) goes to STDERR. Errors and warnings
   always go to STDERR regardless of command type. If a command violates this
   convention, that is a bug.

 * Always create and work from a dedicated project directory. Do not place
   project files directly in `$HOME` or run `morloc make` from the home
   directory -- this is user error, not a bug. Similarly, when using `sudo -u`,
   always `cd` to the target user's directory first; inheriting an inaccessible
   CWD is standard Linux behavior.

 * Failure should ALWAYS return a non-zero exit code

 * The "--" use in `morloc-manager run` is a standard UNIX convention, do not
   report it as undocumented syntax

## CLI structure

Commands are grouped into Development and Deployment.
Use `morloc-manager --help` to see the full list. Read the usage documentation
to learn what the commands do. And see the `morloc-manage <subcommand> --help`
usage statements for detailed info.

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
