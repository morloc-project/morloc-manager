---
name: dev-env-schema
description: Design risks in the morloc-manager `--dev` env redesign (explicit dev opt-in, per-env dev block, is_dev-gated update/freeze, external-vs-build roadmap)
metadata:
  type: project
---

Redesign replacing the OLD implicit/global dev override (ambient `MORLOC_COMPILER_BIN`
+ `MORLOC_RUST_DIR` silently symlinked local build, recorded `morloc_version: "dev"`
-> None -> broke `modify`). New: explicit `--dev`, per-env `dev:` block in env.yaml,
`update`/`info`/`freeze` gated on `is_dev`, `--morloc-version` reinterpreted (dev = stdlib
base only, compiler=local), roadmap Model 2 = `dev.source: build` (build compiler in
requirement-derived container via EnvSpec->pixi). Model 1 = `dev.source: external`
(bind-mount host rust_dir, run host compiler host-side for lang-support table).

**Why:** original bug = implicit ambient switch overrode explicit user version request +
recorded unusable version. Onboarding goal = identical dev container for humans + AI agents.

**Durable design holes found (DA pass 2026-08-23; UNBUILT, verify before trusting):**
- GO/NO-GO: does the host-built compiler ever exec IN-container (`morloc make` for pools,
  or after dev `update` stdlib swap)? If yes, cross-libc breaks external-container (same
  wound as [[project_from_source_runtime_provisioning]] / `log2@GLIBC_2.29`). "Runs
  host-side only" must be proven, not assumed.
- `morloc_version` overloaded by is_dev = ORIGINAL BUG one layer down: in dev it means
  stdlib base, actual compiler version unrecorded/uncontrolled. Every consumer must branch
  on is_dev. Fix: separate `stdlib_version` field/flag with ONE uniform meaning.
- Drop `is_dev` bool; dev-ness = presence of tagged `dev: Option<DevConfig>` (serde
  internally-tagged on `source`). is_dev+separate-block = 2 sources of truth, admits
  is_dev:true/no-block (= "no recorded version" crash reborn) and is_dev:false/block-present.
- Record compiler fingerprint (git sha/version) at provision even in external mode; rebuild
  drift is otherwise SILENT (record-vs-reality drift = the original defect). Feed
  [[project_doctor_health_checks]] staleness.
- Record container mount TARGET for rust_dir, not just host path (repeat of
  [[native-container-refactor]] native-mounts no-remap gap).
- Migration: legacy `morloc_version: "dev"` envs are UNMIGRATABLE (paths/stdlib lost, old
  override used globals). Must DETECT explicitly + "recreate with new --dev", not generic
  "no recorded version".
- `freeze` must REFUSE (or force-materialize -> build) dev-external: host mounts = not
  reproducible; freezing bakes a fake-reproducible artifact.
- Image cache key MUST include dev-ness + mount config, else clean env picks up dev-tainted
  image or vice versa.
- `update` on dev = no-pull stdlib-only vs non-dev full-pull, same verb gated on bool =
  trap. `update --latest` on dev = newest stdlib + stale local compiler skew, presented as
  success, no guard (version->stdlib mapping deferred to dep mgr). Require explicit
  `--stdlib-version` object; gate/warn `--latest`.
- Need re-point/repair command for moved/deleted checkout; validate paths every use.
- system-scope dev-external = perm/drift mess (home checkout unreadable by co-tenants);
  refuse or warn.
- external vs build share SCHEMA envelope only, NOT behavior (portability, where compiler
  runs, reproducibility, freeze, cache, mounts all fork). Code must `match dev.source` from
  day one, never `if is_dev { mount }`, or Model 2 forces rewrite.
- Onboarding framing OVERSOLD: Model 1 = inner-loop for already-set-up devs; it does
  NOTHING for identical-container onboarding or AI agents (fresh agent has no host checkout
  to mount). Danger = prioritization drift: Model 1 ships, "dev containers done", Model 2
  stalls, agents handed the non-portable external path.
- env.yaml is persisted => schema decisions least reversible => scrutinize schema most.
