# model/

The specification of `mim`: what an environment is, how mim provisions it,
solves its dependencies, runs commands in it, serves it and freezes it.

The spec states intent. The code is measured against it, not the reverse.
It is deliberately high level; an item is split or elaborated only when a
question about it comes up.

The compiler owns module resolution, `morloc init` and the nexus; their
rules belong to the compiler's `model/`. An item here that depends on one
cites it by ID (`NET-1`) rather than restating it.

## Items

Items use the format of the compiler's `model/README.md`:

    ### DEP-5 A failed solve leaves the recorded requirements as they were
    Intent: proposed
    Code: deviates F1

    The rule, as briefly as possible.

- `Intent` is `ruled <date>` (the user decided it), `proposed` (a draft
  from the code or the docs, never authority), `open` (undecided; the body
  states the question) or `retired <date>`.
- `Code` is `unaudited`, `conforms <date>` or `deviates <ref>`, where
  `<ref>` names a GitHub issue (`#160`) or an entry in `issues/` (`F12`).
- `Tests` names Rust test functions in `mim/` or `morloc-deps/`. A `ruled`
  item cites at least one; an `open` or `retired` item cites none.
- IDs are `<PREFIX>-<n>`, never reused or renumbered. Each prefix belongs
  to one file.
- No inline examples. ASCII only.

## Prefixes

| File | Prefix | Subject |
|---|---|---|
| `overview.md` | - | the parts and the contracts between them |
| `environments.md` | ENV | identity, scope, layout, default, lifecycle |
| `provisioning.md` | PROV | releases, verification, the runtime, images |
| `dependencies.md` | DEP | the build hook, the solved world, the lock |
| `running.md` | RUN | executing commands in an environment |
| `serving.md` | SERVE | views, servers, authentication |
| `freezing.md` | FRZ | deployment images |

## Issues

`issues/` holds known problems as minimal free-form notes, written as soon
as a problem is found, so work can resume after a lost session without a
GitHub issue. When a problem is fixed, trim its text.

## Using the spec

- Before fixing a bug, find the items that govern the behavior. A `ruled`
  item the code breaks: the code is wrong. A `proposed` or `open` item, or
  none: get a ruling first.
- A ruling changes the spec in the same change as any code that follows it.
- Cite an item from code as `// DEP-4: ...`.

## The check

`model/check.sh` fails when an ID is defined twice, a prefix is used in two
files, an `Intent` or `Code` field is missing or malformed, a `ruled` item
cites no test, an `open` or `retired` item cites one, a cited test does not
exist, or a spec file contains a non-ASCII character.
