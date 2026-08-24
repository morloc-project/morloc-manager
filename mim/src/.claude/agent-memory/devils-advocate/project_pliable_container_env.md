---
name: project-pliable-container-env
description: Design verdict for "pliable" mutable-env containers (in-container morloc make installs package.yaml deps); why Option B beats A/C
metadata:
  type: project
---

Design review (2026-08-20) of making container-backend envs "pliable" so an in-container
`morloc make` can install a program's package.yaml deps. Failure being fixed: `morloc-env sync`
-> `pixi install` writes `/env/pixi.toml`, but `/env` is root-owned (baked at image build) while
the container runs as the keep-id-mapped non-root host UID -> Permission denied.

**Verdict: Option B (materialize-time solve into host bind mounts), not A (named volume + chmod)
or C (overlay).**

Load-bearing facts (don't rederive):
- Conda prefix is relocation-locked to its SOLVE path (`/env/.pixi/envs/default`); the entrypoint
  ALSO hardcodes it. => any mount MUST present the env at the same path it was solved at. This is
  satisfiable by both A and B (both keep `/env`); it only kills solve-at-X/mount-at-Y schemes.
- You CANNOT have both "solved at build" AND "runtime-mutable at the same path via bind mount":
  bind mounts don't auto-populate; seeding a copy forces a different path => prefix break. So
  build-time-solve + runtime-mutable is ONLY achievable via a named volume (=Option A).
- Therefore the real fork is: A = build-solve + named-volume + chmod (world-writable) + init stays
  at build; B = no solve/init at build, materialize-time container solves + inits INTO host bind
  mounts at `/env` and `/opt/morloc`.

Why B wins:
- Bind mount of a host-user-owned dir is writable under keep-id FOR FREE (no chmod, no image bloat).
- Single mutable env (no A-style split-brain between baked declared-deps and volume ad-hoc-deps).
- Only B can rebuild shims (re-run init) when a dep-add bumps a CORE toolchain pkg; A's shims are
  image-baked and go stale silently.
- A has a nasty stale-volume bug: after an image rebuild with new declared deps, a pre-existing
  named volume won't re-populate (volumes auto-populate only when empty) -> serves the OLD env.
- Freeze-to-rigid is trivial under B: snapshot the prefix-preserved `/env`+`/opt/morloc` into a
  fresh image (COPY at same paths) or `podman commit`.

Kill C (overlay/stacked pixi env): pixi has no stacked-env; cross-prefix dynamic linking breaks
and independent overlay solves reintroduce the incoherence the single-solve coherence-key avoids.

Cost of B (the honest departures):
- `morloc init` MUST move from image-build to a materialize-time container step (build no longer
  has the toolchain). Justified: mutability is the pliable feature; rigid keeps the baked form.
- The compiler-identity solve-cache (`materialized.toml`) MOVES + SPLITS: image cache becomes
  requirement-independent (base+compiler+rust only); a new materialize marker (manifest+lock+
  compiler) guards the mounted solve/init. Not broken, refactored.
- "runtime immutable in image" principle scopes to RIGID only.

**Single biggest risk (applies to any in-place re-solve, A or B): shim/env coherence.** A pliable
dep-add that bumps a core toolchain pkg (python minor / libstdc++ / conda glibc) silently
invalidates the baked shims' rpaths/ABI -> confusing runtime load error. Cheapest de-risk: record
a "shim build manifest" (the toolchain subset of the lock at init time); after each re-solve, diff
core-pkg versions; if changed, REFUSE the add with a clear message first (upgrade to auto-re-init
later). Turns a silent tail-risk into a legible boundary.

Design hinge to confirm with user: MORLOC_HOME (shims) becoming mounted-mutable is the departure
from approach-B; that's the thing to green-light.

Related: [[project_env_architecture_global]] (coherence-key), approach-B "runtime immutable in image".
