# CLAUDE.md

Guidance for Claude Code when working in the mim repository (crates `mim`
and `morloc-deps`). Workspace-wide rules are in `/work/CONVENTIONS.md`.

## Spec

`model/` is the specification of mim. Read `model/README.md` first. Before
fixing a bug, find the items that govern the behavior; a `ruled` item the
code breaks means the code is wrong, and a `proposed` or `open` item needs a
ruling before the fix. A ruling changes the spec in the same change as the
code. Cite items from code as `// DEP-4: ...`. Run `model/check.sh` after
editing the spec.

Compiler-owned behavior (module resolution, `morloc init`, the nexus) is
specified in the compiler's `model/`, not here.

## Issues

Record every problem in mim the moment it is found in `model/issues/`, as
minimal free-form text: enough to resume the work after a lost session, no
more. When an issue is fixed, trim its text. This replaces GitHub issues for
day-to-day bug work. A spec item's `Code: deviates F<n>` points at an entry
here.

## Contracts with the compiler

The four compiler <-> mim contracts are described in `README.md`. In each,
the compiler writes and mim reads, so compatibility is mim's job.

## Safety

- Never run `mim sync` by hand; it is the build hook and re-solves a shared
  environment.
- The installed `mim` (`/opt/morloc-runtime/mim`) and its environments are
  the user's. Do not create, update or remove environments without being
  asked.
- Run `cargo build`/`cargo test` under `/work/scripts/memguard`.
