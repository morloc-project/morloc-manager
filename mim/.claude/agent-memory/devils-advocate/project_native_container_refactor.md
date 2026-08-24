---
name: native-container-refactor
description: Design risks in the two-axis (requirements vs runtime) env-model refactor lowering both native and container backends onto one pixi manifest
metadata:
  type: project
---

Refactor: co-design a NATIVE backend (pixi/conda toolchain on host) and a
requirement-DERIVED CONTAINER backend (generated `FROM base; install pixi; pixi
install; morloc init` Dockerfile) on ONE `EnvSpec`->pixi lowering. Retires
hand-authored Dockerfile/.def recipes. Env = two orthogonal axes: Requirements
(what) -> pixi.toml; Runtime config (how) -> ports/mounts/env/shm/network.

**Why:** unify container+native so they stay in sync by construction; native
default sidesteps the container UID/HOME bug class ([[container-uid-home-mismatch]]).

**Implementation state (verify before trusting; from a 2026-08 read):** requirement
axis SCAFFOLDED only. `pixi.rs` RENDERS a manifest but no pixi *solve* is invoked
anywhere; no Dockerfile generator exists; no runtime-config record (still old
`FlagConfig` build/run/start x engine). `main.rs`/`environment.rs` are still the
old hand-authored-recipe world. `hostprobe.rs`/`provision.rs`/`envspec.rs`/
`langsupport.rs`/`types.rs::Backend::Native` are the new parts.

**Durable design holes found (grounded in code, not yet fixed):**
- `pixi.rs::merge_constraint` CONCATENATES comma-atoms; never solves or detects an
  empty intersection. So the "3-way version intersection" cannot be reported by the
  manager -- unsat surfaces as a raw conda solver trace. Author constraints are
  shoved verbatim into conda match-spec syntax (cargo `^`/PEP440 `~=` leak).
- `system` deps with `provider=="host"` are DROPPED in `aggregate` (pixi.rs). apt/
  non-conda system packages have NO home: not in pixi, and the generated Dockerfile
  has no build-time RUN/apt layer. `container_raw_flags` is RUN-phase only. This is
  the biggest structural gap (a retired-Dockerfile capability with no replacement).
- Flag reuse asymmetry: REMOVING a flag fails loud (clap); RE-PURPOSING `--image`/
  `--version` fails silent (old invocations stay valid, new meaning). Rename to
  `--base-image`/`--morloc-version` and hard-error old spellings.
- `pixi.lock` is platform-specific; collides with "freeze on mac -> unfreeze on
  linux". Reproducibility unit (lock vs built image) is undecided. Solve re-runs on
  every `install` (latency regression vs the old cached image pull).
- native `mounts` = "host FS visible, no remap" silently drops the container-side
  target -> programs hardcoding the container path break. Symlink-or-refuse instead.
- `update --engine` = backend migration is a lossy rebuild (platform-specific lock,
  recompiled pools) disguised as a field flip (`environment.rs` ~line 504). Should
  be an explicit `migrate` verb.
- GPU straddles all 3 (toolkit=requirement, `--gpus`/`--nv`=runtime, driver=host);
  relegating device access to raw flags breaks cross-backend behavior parity.
- native serve: plain loopback fine; `--expose`/eval on native = full-host-privilege
  process, a different risk class -> gate behind explicit opt-in.

**Native serve-parity sub-effort (DA pass 2026-08-18; verify code before trusting):**
Grounded in a read of runner.rs (Runner trait, only `run` today), serve.rs
(container-only `serve_environment`, read_only rootfs), main.rs `serve_plan`
(~3768; container/VM security tiers) + `ServeRuntime` (types.rs:668, NO backend
tag, NO pid), Cmd::Eval (main.rs:2500; already a plain HTTP client to
127.0.0.1:port -- backend-neutral).
- No supervisor exists: morloc-manager is a CLI that exits right after the
  detached spawn. Native serve is ALWAYS unsupervised (no restart). Pidfile !=
  weak container supervision; it's observability only. Honest analogue of "engine
  supervises" is systemd-user (`systemd-run --user`); floor is pidfile+pgroup.
- ServeRuntime clobbers on backend migration (one file/env, no discriminator);
  stop/status/logs must dispatch on a stored ServeHandle{Container|Native}, NOT
  ec.backend. `start` has no flock -> double-spawn race (container gets accidental
  dup-name backstop; native has none).
- Biggest omission: nexus child pool daemons. Native stop must `kill(-pgid)`
  (setsid group), not one pid, or pools+/dev/shm/morloc-* leak. Container gets
  cgroup teardown free.
- Security: container serve is read_only+synthetic HOME; native serve of ANY
  module = remote-triggerable code-exec as the user (before eval even enters).
  Loopback is NOT per-user -> multiuser host = every co-tenant reaches it; native
  loopback should default token-ON. Do NOT fork serve_plan's 3 gates
  (plaintext/token/expose) into one `--unsafe`; split into neutral gate-core +
  transport tail. Native eval must also require token + refuse on 0.0.0.0.
- /proc-based liveness is Linux-only (native-on-mac is a real target); use
  kill(pid,0)+starttime. Runner-trait is wrong altitude for status/ls-running
  (global, one `docker ps` for all envs -> per-env trait method regresses to N).
- "Full parity" is a false label if freeze deferred: native freeze = platform-lock
  pixi.lock, different artifact. Call it "operational parity". Extract the seam
  LAST (after both impls exist), not first.
