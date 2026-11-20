# Issue List

Open problems only; a fixed entry is deleted, so numbers have gaps. Spec
items cite these as `Code: deviates F<n>`.

## Needs a ruling

### F64 Bridge resources encode "unset" as a sentinel
`submit.resources` fields are required `i32`s, and the morloc runtime sends
-1 for a resource the label leaves out. The meaning of each value is spread
across two copies of the flag rules (`bridge.rs::resource_args`, runtime
`slurm_ffi.rs::sbatch_resource_args`), and a client that omits a field fails
to decode. Optional fields (absent = unset) would state it in the wire
format; that changes the bridge protocol, so it needs a ruling.

### F6 System-env `rm` leaves each user's `system-homes/<name>`
ENV-7. The private home lives in each user's own data dir
(`config::system_env_user_home`), which root's `rm` cannot reach. Sweep
lazily, or leave them as the user's files?

### F8 Apptainer does not work end to end
ENV-4, SERVE-3/6/7. Keep it (and fix) or drop it from `--engine`? Broken:
image build emits Docker argv and never sets `base_sif`/`layered_sif`;
`container_exists` is always false; serve needs a `%startscript` mim never
writes, and gets no read-only root, `MORLOC_IMMUTABLE` or shm size; `run`
exports no `MORLOC_ENV`/`MORLOC_BUILD_HOOK`/`MORLOC_BIN`.

### F12 `abi_version` is parsed and never used
PROV-6 (open). `MorlocVersions::abi_version` and `version::ABI_VERSION`
are unused; doctor has no ABI check. "ABI" also names the interpreter-minor
pin in `deps/abi.rs`. The field is required by serde, so a manifest
that drops it (PROV-6 candidate c) is refused by this mim.

### F14 `MORLOC_ENV` is the env name natively but `"container"` in OCI
RUN-2. Is it the environment's name or only a flag? The compiler tests only
non-emptiness. IFC-7 and the manual's variable list should then say
which (`serve::oci_managed_markers`).

### F16 A system default can resolve to a local environment
ENV-5 says a system default names a system environment, but defaults are
bare names resolved local-first, and
`the_displayed_default_is_the_one_commands_use` asserts that on purpose.
Also `new --system --set-default` writes the system default, `modify
--set-default` on a system env the caller's local one.

### F17 `view eval --allow` accepts modules that are not installed
SERVE-2. Entries are checked to be one module name, not to be installed.
Eval imports modules, which may be libraries with no launcher; what counts
as installed for the allow-list?

### F25 Flag-file entries run through `sh -c` on every read
`config.rs` interpolates each `env.flags.yaml` entry into `sh -c "printf
..."`, so `$(...)` runs on every mim command, for every user of a system
env; a shell failure silently falls back to whitespace splitting. Expand
entries at all?

### F26 System-dependency providers are handled inconsistently
`pixi.rs` makes every provider except exact `"host"` a conda dependency
(including `vcpkg`); `EnvSpec::native_blockers` lowercases and is used only
by the manager. What does each provider mean in a container? Since compiler
PKG-12 (2026-11-19) the compiler emits no `system` entries (C++ link names
were sent there and solved as conda packages), so only a hand-written
envspec reaches this path.

### F27 Undecided small behaviors
- Native `run` may download pixi (`provision_pixi`); may `run` use the
  network? (`locate_pixi` never downloads.)
- `demos` never matches a `-local` env version, so it always falls back to
  the latest release.

## Needs design (model first)

### F3 Configuration-only changes are not serialized
ENV-8. The manager takes the environment lock (DEP-4) around its solves,
`rm`, rename, and `modify`'s rebuild from its snapshot through its
rollback. The rest of `new`/`update`/`modify` (config record, default,
dotfiles, home, shm, flag file, certificates) runs unlocked, so two such
commands on one environment interleave. Ruled 2026-11-04 for `modify`'s
rebuild; the configuration steps remain.

### F5 `new`, `update` and `modify` are not atomic
ENV-2, ENV-6. `new` writes everything before `env.yaml`, so a failure leaves
an invisible, unremovable directory that the next `new` reuses. `update` has
no rollback. `modify` rolls back only the rebuild; earlier changes (rename,
default, dotfiles, home, shm, flag file, pins, certificates) stay.

### F24 Configuration read-modify-write is unlocked
ENV-8. Writes are locked and atomic, reads are not, so default, rename and
`modify` updates can be lost; `install_flag_file` copies non-atomically.

### F65 Crates are outside the environment's solve
DEP-3, DEP-7, DEP-8. Ruled 2026-10-14: crates join the one solved
environment. One cargo lock per environment, resolved over a workspace of
the runtime crates and every program's rust-deps; pools build against it
and the compiler's per-pool lock merge goes. A crate has one version per
semver-compatible line (cargo links several major lines into one binary,
and the ecosystem depends on it). Compiler side and evidence:
compiler model/runtime/issues/build-and-text.md.

## Other

### F10 Unverified downloads
PROV-2. Checksummed: the compiler, the mim agent, the rust source and the
lang-support asset (refused when its published digest fails). Not verified:
the host pixi (`provision_pixi`), pixi in images (`curl pixi.sh/install.sh`),
`stdlib-snapshot.txt`, the demos tarball and manifest, ghcup in dev images.

### F15 A command's own exit 125 reads as an engine error
RUN-3. docker/podman use 125 for their own failures (apptainer 255), and a
command exiting with that status is indistinguishable
(`container_command_status`). Needs the engine's stderr or an in-container
wrapper.

### F18 A failed `--locked` install re-solves on any error
DEP-7. `pixi::install_locked` treats every non-zero exit (a network error
too) as a stale lock, and the fallback solve may move versions. Telling
them apart needs pixi's exact stale-lock message.

### F62 Off Linux a reused server pid is not detected
SERVE-3. Identity is the leader's start time from `/proc`; on macOS a live
pid is trusted (`serve_leader`), and pre-start-time records fall back to the
command line, which another env's nexus also matches.

### F63 `start` picks a free port, then binds it later
`find_free_host_port` binds and releases; another process can take the port
before the server binds.

### F68 A concurrent rebuild can drop a first install's spec
DEP-3. `seed_installed_store` (main.rs) drops an installed spec whose
`exe/<name>` is absent. The hook records `installed/<name>` before the
compiler creates `exe/<name>` (after the pools compile), so a `mim update`
in that window drops the spec and solves its packages out of the prefix.
Read, not reproduced.

### F69 A build under another state is not superseded by an install
DEP-13. An install with Python locals supersedes scratch specs named exactly
`<name>`; a scratch build `<name>@<state>` from another state keeps its live
path, which meets the install's snapshot of the same package in one solve.
Read; how the renderer resolves two sources for one package is unchecked.

### F70 The hook can misjudge the build's state
DEP-13. The hook derives the compiler's state (`MORLOC_STATE`, else
`MORLOC_HOME`, else `$HOME/.local/share/morloc`) but not a `home:` from the
compiler's config file or `--config`, and resolves a relative
`MORLOC_STATE` against its own cwd (the staging build dir). Each error
records an own-state build as scratch. Fix: the compiler passes its resolved
state to the hook (an `interface.md` change).

### F71 flock across a VM bind mount is assumed
DEP-4. On macOS Docker Desktop and podman machine the store reaches the
container through a VM file share; whether flock there excludes the host's
lock is unverified.

### F72 A failed install leaves its Python snapshot behind
DEP-4. `vendor_py_locals` copies an install's Python locals into the pixi
project before the solve; a solve that fails leaves the copy, and only the
next successful install sweeps it. Disk only: nothing references it.

### F66 The native record is not refreshed by an in-env re-solve
RUN-1, RUN-6. The record `run`/`shell`/`doctor` replay is written only at
materialize (`materialize_native_env`). The in-env agent re-solves the
same prefix and re-captures into
`.morloc-activation.json` (`envstore::solve_world`), which the record never
reads, so a package that adds an activation (a new compiler, a rust
activation) reaches `morloc make` builds but not a later `shell`. Read, not
reproduced.

### F76 mim's own --lang spec can name a language IFC-2 does not list
IFC-2. The envspec mim writes for `--lang` pins may carry `futhark`; IFC-2
lists `py`, `r`, `cpp`, `rust`. The item or the writer changes. Read, not
reproduced.

### F78 morloc make with MORLOC_ENV and no hook only warns for language-only programs
IFC-7. The compiler's `runSync` throws only when the program declares
package deps (`requireHook = declaresDeps`); a program with pools and no
deps builds with no provisioning. Compiler side. Read, not reproduced.

### F79 An install's store key is the build key, not the installed name
IFC-8, DEP-3. `morloc make --install main.loc` calls the hook with `--name
main` but installs under `exe/<module>`; two such installs share
`installed/main.json` and the second solve drops the first's packages until
the next manager solve reseeds by `exe/<name>`. Compiler side. Read, not
reproduced.

### F80 A lang-support table without schema_version is accepted
DEP-2. An absent `schema_version` reads as the current one
(`lang_schema_version` default, asserted by a test), so a table from a
compiler that predates the field is not refused. Ruling: refuse it, or say
in DEP-2 that absent means 1.0?

### F81 The manager's solve drops scratch builds' local Python packages
DEP-3. `gather_env_specs` strips every scratch spec's Python locals before a
manager solve, so `mim update`/`modify` uninstalls a scratch-built program's
local package. They are stripped because a container `/work` path is not a
host path. Read, not reproduced.

### F82 A conda-forge request merges into another channel
DEP-8, IFC-3. `merge_channel` lets any explicit channel beat an absent one,
but on the wire absent IS conda-forge; conda-forge `x` and bioconda `x` from
two programs solve to bioconda with no error. Read, not reproduced.

### F83 A conda package and a Python local or pypi package of one name are not compared
DEP-8. conda and pypi requirements merge in separate maps, so both reach
pixi with no conflict check. Read, not reproduced.

### F86 Serves and frozen images never run the hook under the immutable mark
DEP-10, IFC-3. The hook now refuses, before writing, a sync under
MORLOC_IMMUTABLE that would solve or needs a binding rebuilt. But mim never
sets the mark together with the hook: serves and frozen images export no
MORLOC_ENV/MORLOC_BUILD_HOOK, so a `morloc make` there builds with no
provisioning instead of being refused. IFC-3's "editable honored only where
not immutable" is then moot: any new local is a world change and refused.
Once the hook does run there, a world whose solve record is gone and whose
manifest was rewritten by runtime adoption reads as changed and is refused.

### F87 No test compares the contract versions with the compiler's
Comments in `version.rs` and the compiler's `Version.hs` cite a conformance
test guarding the two sides' version constants; none exists.

### F93 Toolchain and package cache live outside the scope data roots
ENV-3. The docker/podman prefix is an engine volume
(`serve::prefix_volume`); the container package cache is one machine-wide
volume (`morloc-pixi-cache`); native solves use `~/.cache/rattler`. The
spec names these or the code moves them. Read, not reproduced.

### F94 doctor may fail healthy dev and prerelease environments
ENV-11. A fresh `new --dev` env has no `morloc init` output until the
developer builds the compiler, so `check_data_dirs` fails on the runtime dirs
and `exe` (is a dev env "healthy" before its first build?).
`check_staleness` compares a prerelease manifest version with `morloc
--version`, which may drop the suffix (unverified). The `/dev/shm` scan calls
a segment leaked when its pid is in another pid namespace. Container doctor
has no agent check. Read, not reproduced.

### F96 Runtime builds can write outside the environment
PROV-5. Apptainer exports MORLOC_BIN_LINK_DIR under the host home, which
the agent's `morloc init` honors; neither apptainer nor native init sets
CARGO_HOME, so cargo's registry may land in `~/.cargo` unless the rust
activation sets it. Read, not reproduced.

### F98 mim's own downloads ignore the environment's CA bundle
PROV-9. `provision::curl_base` passes no `--cacert`, so releases, the rust
source, pixi, foreign agents, the stdlib snapshot and demos use only the
host store. Behind TLS inspection with a CA the host does not trust, `new
--cert-bundle` fails at the first download. Needs the environment's bundle
threaded to every download. Read, not reproduced.

### F99 run does not always use the user's directory or own its files
RUN-1. Under SELinux from `$HOME`, `run` drops the `/work` mount and runs
in `/opt/morloc`; rootless docker's `--user uid:gid` writes files as a
subuid; apptainer puts a host-wide link dir ahead of the env's programs on
PATH; a native run with a deleted cwd runs in the env data dir. Read, not
reproduced.

### F100 Apptainer runs keep foreign activations
RUN-4. Apptainer runs pass the host environment through (no `--cleanenv`),
so a host conda activation reaches in-container builds. See F101. Read, not
reproduced.

### F101 Apptainer forwards the whole host environment
RUN-5. `build_apptainer_args` and `serve_apptainer_instance` emit no
`--cleanenv`, so every host variable crosses into apptainer runs, shells and
serves. Also unclear: a one-shot `--flagfile` replaces the env's flag file.
Read, not reproduced.

### F104 The in-env solve's CONDA_SSL_VERIFY is scrubbed
Inside a native env the sandbox scrub strips CONDA_* (CONDA_SSL_VERIFY
included) and envstore passes no cert file. Likely no effect on pixi. Read,
not reproduced.

### F105 start --mcp serves a module no view names
SERVE-1. `resolve_serve_spec` turns `--mcp <program>` into a one-module
spec without reading views.yaml. Keep it as a stated exception or drop it.
Read, not reproduced.

### F106 The serve token is still stored by the engine
SERVE-4. mim now names the token on the engine's command line and values it
in the engine process's environment, but docker/podman keep it in the
container's stored config (`docker inspect`); avoiding that needs a secret
file the nexus reads, a compiler change, or a ruling that scopes "never
written to disk" to mim's own files. `--env-var`/`--env-file` values still
go on the engine's argv (the engine process's own environment is no place
for them), and `--auth-token` puts the token on mim's argv
(`MORLOC_MCP_TOKEN` in mim's environment is the argv-free form). Read.

### F110 The serve token reaches every pool and eval child
Nothing removes MORLOC_MCP_TOKEN after the nexus reads it, so any served
function can read it. Compiler side. Read, not reproduced.

### F114 Freeze writes the environment's home and mounts it read-write
FRZ-4. Before validating the programs, freeze creates the env home
(`ensure_env_home`) and runs them with the environment mounted read-write,
so a program's `--help` can write into it. It also takes no environment
lock, so a concurrent install can be captured half-written (see F3).

### F116 The slim cut keeps build-phase packages missing from its name list
FRZ-8. `prune::BUILD_ONLY_STEMS` is a fixed list and lang-support's `phase`
is ignored, so pip and setuptools stay in every slim Python image. The
linkage check skips the language bindings and "started" is `--help` only.
Read, not reproduced.

### F117 A loopback serve under a VM-backed engine on Linux is unreachable
`serve_plan` assumes `--network host` reaches the host loopback on Linux;
with Docker Desktop for Linux it binds the VM's. Read, low confidence.

### F118 Runtime entries provisioned before digest stamps are never compared
PROV-4. A `.provisioned` stamp holding only a version predates the asset
digests and is accepted as is, so a re-published release is detected only
for entries provisioned since. Read.
