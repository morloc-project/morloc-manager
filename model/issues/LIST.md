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
pin in `deps/abi.rs`.

### F14 `MORLOC_ENV` is the env name natively but `"container"` in OCI
RUN-2. Is it the environment's name or only a flag? The compiler tests only
non-emptiness. README.md and the manual's variable list should then say
which (`serve::oci_managed_markers`).

### F16 A system default can resolve to a local environment
ENV-5 says a system default names a system environment, but defaults are
bare names resolved local-first, and
`the_displayed_default_is_the_one_commands_use` asserts that on purpose.
Also `new --system --set-default` writes the system default, `modify
--set-default` on a system env the caller's local one.

### F17 `view eval --allow` accepts any names
SERVE-2. Eval imports modules, which may be libraries with no launcher; what
counts as installed for the allow-list?

### F25 Flag-file entries run through `sh -c` on every read
`config.rs` interpolates each `env.flags.yaml` entry into `sh -c "printf
..."`, so `$(...)` runs on every mim command, for every user of a system
env; a shell failure silently falls back to whitespace splitting. Expand
entries at all?

### F26 System-dependency providers are handled inconsistently
`pixi.rs` makes every provider except exact `"host"` a conda dependency
(including `vcpkg`); `EnvSpec::native_blockers` lowercases and is used only
by the manager. What does each provider mean in a container?

### F27 Undecided small behaviors
- Native `run` may download pixi (`provision_pixi`); may `run` use the
  network? (`locate_pixi` never downloads.)
- `demos` never matches a `-local` env version, so it always falls back to
  the latest release.

## Needs design (model first)

### F2 Spec removals happen outside the solve lock
DEP-4, DEP-5. `mim sync --installed` with Python locals removes scratch
specs and vendors snapshots before taking `.solve.lock` and restores
nothing on failure (`envagent::sync`); `clean` deletes `scratch/` before
its solve and does not restore it.

### F3 The manager takes no lock
ENV-8, DEP-4. Only the in-env agent takes `.solve.lock`. The manager's
solves and `new`/`update`/`modify`/`rm`/`nuke`/rename race each other and
`mim run -- morloc make`. ENV-8: one lock for every manager mutation, or
only for solves?

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
PROV-2. Only the compiler and rust source are checksummed. Not verified: the
host pixi (`provision_pixi`), pixi in images (`curl pixi.sh/install.sh`),
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

### F20 A re-published tag with the same version is never re-fetched
PROV-4. The release store is keyed by version and gated by a `.provisioned`
stamp, so a moving tag like `dev` keeps the old compiler.

### F62 Off Linux a reused server pid is not detected
SERVE-3. Identity is the leader's start time from `/proc`; on macOS a live
pid is trusted (`serve_leader`), and pre-start-time records fall back to the
command line, which another env's nexus also matches.

### F63 `start` picks a free port, then binds it later
`find_free_host_port` binds and releases; another process can take the port
before the server binds.

## Compiler and nexus (belong in the compiler's issue list)

- F38 IPv6 loopback can never bind: `format!("{}:{}", host, port)` gives
  `::1:9000` (nexus/mcp.rs).
- F39 `MORLOC_EVAL_ALLOW_NO_AUTH=0` waives the eval lock: any non-empty
  value counts (nexus/cli.rs).
- F40 The daemon's own HTTP `/eval`, `/typecheck`, `/bind` serve without a
  token under `--allow-no-auth` (daemon_ffi.rs).
- F41 `morloc install owner/name` may parse as core module `owner`
  (Module.hs `moduleInstallParser` has no `eof`); unverified.
- F42 The default config file is never read without `--config`
  (Subcommands.hs), so `registry`, `lang_overrides`, `plane` are
  unreachable; the manual's "Installing modules" says otherwise (F59).
- F43 `installed_hash` records the requested pin, null when unpinned.
- F44 Registry installs use `plane/<owner>/<name>`, `install --build` looks
  in `plane/<name>`.
- F45 envspec `modules` is first-wins, the resolver closer-wins.
- F46 A program with only system deps gets a warning, not an error, without
  a hook (`packageHasDeps`).
- F47 `morloc init` rebuilds the runtime unless incremental, whatever `-f`
  says; two SystemConfig.hs comments are wrong.
- F48 Empty `MORLOC_HOME`/`MORLOC_STATE` is unset to Haskell, set to the
  nexus.
- F49 `install` writes wrappers and completions under `$MORLOC_HOME`.
- F50 Dead `STATE/src/<base>/...` C++ search paths (Cpp.hs).
- F51 `findBareModuleMaybe` skips the ambiguity check.
- F52 The front end maps every `/call` failure to 500.
- F53 `/call` reaches commands MCP excludes (stream args, `@stdin`).
- F54 Daemon and front end differ on `/health` auth and 401 bodies;
  single-program `mcp --http-port` has no `/health` or OPTIONS.
- F55 MCP `notifications/cancelled` is ignored; a crashed child daemon
  stays down until the next request.
- F56 Stale comments: router scan path and transports (nexus/cli.rs,
  main.rs), expose targets (State.hs, SystemConfig.hs), build hook named
  `mim-env` (Build.hs).
- F57 `GET /eval` with eval off is 405, not 404.
- F58 A `__` collision in module/command names aborts the front end.
- F59 Manual says `registry` is read from `~/.morloc/config.yaml` (F42).
