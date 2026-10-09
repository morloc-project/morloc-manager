# Issue List

F1-F31 are mim behavior against the spec (the `Code: deviates` refs point
here). F37 is mim docs drift. F38-F59 are compiler and nexus
findings met along the way (out of the mim spec's scope). Fixed entries are
deleted, so numbers have gaps.

## mim: behavior

### F1 A failed re-sync deletes the program's previous requirements (verified)
Spec: DEP-5. `EnvContext::sync` writes the spec to its slot, then on solve
failure calls `remove_spec(&slot)` (deps/envstore.rs:653-692). The slot for
an installed program is `requirements/installed/<K>.json`, the same path the
previous successful install wrote (deps/envstore.rs:158-176), so a failed
reinstall of `K` deletes `K`'s previous, working spec. The next solve then
drops `K`'s packages from the world and can break the installed program.
Same for a scratch rebuild whose world is unchanged (same digest slot). The
unit test `sync_rollback_removes_only_the_slot_it_wrote` covers only a fresh
slot. Fix: save the previous file content and restore it.

### F2 Spec removals happen outside the lock and are never restored (surveyed)
Spec: DEP-5, DEP-4. `mim sync --installed` with Python local deps calls
`remove_named(K, Scratch)` and vendors snapshots before taking
`.solve.lock`, and nothing restores them on failure (mim/envagent.rs:67-80).
`clean` deletes the whole `scratch/` tree before its solve and does not
restore it if the solve fails (deps/envstore.rs:723-755).

### F3 The manager's solves and environment changes take no lock (surveyed)
Spec: ENV-8, DEP-4. Only the in-env agent takes `.solve.lock`. The
manager's native solve writes `pixi.toml` and runs `pixi install` directly
(mim/main.rs:4768-4783), and `new`/`update`/`modify`/`rm`/`nuke`/rename take
no lock at all. So `mim update` can race `mim run -- morloc make` on
`pixi.toml`/`pixi.lock`, the image tag, the volume and the build marker; `rm`
during a `run` deletes the tree under it; two `new X` both pass the
existence check.

### F4 `mim install` on a version-less environment moves it to the latest release (verified)
Spec: ENV-9. `install` passes `current_version_tag(&ec)`, which is `None`
for an environment with no recorded version (`--no-init`, legacy), and
`None` resolves to latest (mim/main.rs:3700-3707, 7082-7089, 7355-7368). The
comment at 3705 says installing "must never bump the toolchain"; `update`
and `modify` use `require_current_version` to refuse this case, `install`
does not.

### F5 `new`, `update` and `modify` are not atomic (surveyed)
Spec: ENV-2, ENV-6.
- `new` writes the flag file, certs, home, image, snapshots, and only then
  `env.yaml` (mim/main.rs:8391-8488, 6752-6791). A failure leaves data and
  config directories without `env.yaml`: invisible to `ls`, unremovable by
  `rm` (which requires the config, mim/environment.rs:142), and silently
  reused by the next `new` of that name. `modify --env-name` refuses an
  existing directory (mim/environment.rs:235), a different existence test.
- `update` has no rollback: a failure leaves the new `pixi.toml`,
  `pixi.lock` and requirements store (mim/main.rs:7351-7435).
- `modify` rolls back only the rebuild step; rename, default, dotfiles,
  `mount_home`, `shm_size`, flag file and module-pin changes made before a
  failing rebuild stay (mim/main.rs:3297-3313). Certificates prepared before
  a later non-rebuild failure are not restored despite the comment at
  mim/main.rs:3060.

### F6 `rm` swallows every removal error and leaves a native server running (verified for errors, surveyed for serve)
Spec: ENV-7. `remove_environment` ignores the result of each
`remove_dir_all` and volume removal with `let _` (mim/environment.rs:
170-186) and returns success. It stops a container server but never a
native serve process, which keeps running against deleted files.
`system-homes/<name>` is also left behind.

### F7 `mim stop` cannot clear a stale container serve record (verified)
Spec: SERVE-3. If the recorded container is gone, `stop_by_handle` ->
`stop_serve_container` errors "No serve container running" (mim/serve.rs:
174-178) and `remove_serve_runtime` is never reached (mim/main.rs:3534-
3539). The record persists, and every later `stop` fails the same way.
Native handles avoid this (ESRCH is success).

### F8 The Apptainer backend does not work end to end (surveyed)
Spec: ENV-4, SERVE-3, SERVE-6, SERVE-7. Already reported in
plans/env-install-audit (C1, 2026-09-02); still true:
- image build emits Docker argv (`build -f -t`) for every engine
  (mim/container.rs:212-216, 711); nothing sets `base_sif`/`layered_sif`, so
  `active_image` falls back to the base image;
- `container_exists` always returns false for Apptainer (mim/container.rs:
  314-319), so `eval` says no server, `logs --env` errors, and `start`
  silently replaces a running instance;
- the serve relies on an image `%startscript` mim never generates
  (mim/serve.rs:806-808);
- serve gets no read-only root, no `MORLOC_IMMUTABLE`, no shm size;
- `run` sets no `MORLOC_ENV`/`MORLOC_BUILD_HOOK`/`MORLOC_BIN`
  (mim/main.rs:9405-9420), so builds inside are unmanaged;
- `--slurm-bridge` requires Apptainer.
Apptainer is still offered by `--engine` and the docs (as experimental).

### F9 Container `doctor` checks the wrong directory for the runtime (verified)
Spec: ENV-11. `check_data_dirs` requires `lib`, `bin`, `opt` directly under
the data dir (mim/doctor.rs:1238-1255) and the container doctor passes it
the data dir (mim/doctor.rs:147, 157). Container environments keep the
runtime under `data_dir/runtime` (mim/config.rs:20-38, whose comment warns
about exactly this). Expected: every container env fails doctor with
"Missing directories: lib, bin, opt". Not run.

### F10 Downloads that are not verified (surveyed)
Spec: PROV-2. Only the compiler binary and rust source are verified. Not
verified: the host pixi binary (mim/provision.rs:754-765), pixi installed in
images by `curl pixi.sh/install.sh | bash` (mim/dockerfile.rs:560-564),
`stdlib-snapshot.txt` (mim/provision.rs:173-176), the demos tarball and
manifest (mim/demos.rs), the ghcup installer in dev images
(mim/dockerfile.rs:620). `scripts/install.sh` skips its checksum silently
when the `.sha256` fetch fails or no sha tool exists (scripts/install.sh:
76-89).

### F11 The release contract gate admits releases the parsers then reject (surveyed)
Spec: PROV-3. `check_supported` refuses only versions above mim's
(mim/provision.rs:75-104), but envspec parsing requires exactly version 2
(deps/envspec.rs:143-155) and lang-support requires major exactly 1
(deps/langsupport.rs:112-132). A release declaring envspec 1 or
lang-support 0.x installs and then fails on every build. The unit test
sample uses `envspec_version: 1` (mim/provision.rs:914).

### F12 `abi_version` is parsed and discarded (surveyed)
Spec: PROV-6 (open). `versions.abi_version` is read into
`MorlocVersions` and never used; `ABI_VERSION` (deps/version.rs) is
referenced nowhere; doctor has no ABI check. Note also that "ABI" means two things in mim: this
version, and the interpreter-minor pin in deps/abi.rs.

### F13 Native `morloc init` symlinks into the user's `~/.local/bin` (surveyed)
Spec: PROV-5. Native provisioning runs `morloc init -f` without setting
`MORLOC_BIN_LINK_DIR` (mim/main.rs:4313-4337), and the compiler then links
`morloc-nexus` (and `mim`) into `~/.local/bin` when it exists
(compiler/library/Morloc/CodeGenerator/SystemConfig.hs:214-255). The last
environment materialized wins the user's global `morloc-nexus`. Container
materialize does not set it either; only the run path sets `""`.

### F14 `MORLOC_ENV` holds the literal "container" in OCI containers (verified)
Spec: RUN-2. Native sets `MORLOC_ENV=<env name>` (mim/main.rs:1411); OCI
containers set the literal `"container"` (mim/serve.rs:725); Apptainer sets
nothing (F8). README.md:141-147 says it stores the environment name and that
mim exports all three variables wherever it launches. The compiler only
tests non-emptiness, so this is harmless today but the contract is not what
the README states.

### F15 Exit statuses >= 125 are reported as engine errors (surveyed)
Spec: RUN-3. mim/main.rs:9494-9504 treats any container exit >= 125 as a
container engine error, so a program exiting 126, 127, or 128+n (killed by
a signal, including 130 for Ctrl-C and 141 for a closed pipe) is misreported
and its status replaced.

### F16 Default-environment resolution crosses scopes (surveyed)
Spec: ENV-5.
- The default is stored as a bare name. A system default `X` resolves
  through local-first lookup, so it picks a user's local `X` when one
  exists (mim/environment.rs:550-558), contradicting the comment at
  mim/environment.rs:436.
- `nuke` clears the scope's default unconditionally, even when it named an
  environment in the other scope (mim/main.rs:2066-2073); `rm` is careful
  (mim/environment.rs:192-206).
- `new --system --set-default` writes the system default; `modify
  --set-default` on a system env writes the local one.

### F17 The eval allow-list is not validated and outlives `view rm` (surveyed)
Spec: SERVE-2. `view eval --allow` accepts any names; `start` checks only
the MCP and API views (mim/main.rs:8794). `view rm M` removes `M` from MCP
and API only (mim/types.rs:771-776).

### F18 A failed `--locked` install silently re-solves (surveyed)
Spec: DEP-7. Any non-zero exit of `pixi install --locked`, including a
network error, falls through to a full `pixi install` that may move
versions (deps/pixi.rs:705-779). The docs describe the locked path as
"zero drift".

### F19 `mim spec` does not print what `sync` solves (surveyed)
`spec` renders `gather()` without the interpreter pin or adopted languages
(mim/envagent.rs:304-312), so its output can differ from the manifest the
next sync solves.

### F20 A re-published tag under the same version is never re-fetched (surveyed)
Spec: PROV-4. The release store is keyed by the manifest's version and
gated only by a `.provisioned` stamp (mim/provision.rs:664-694). A moving
tag such as `dev` re-published with the same version string keeps the old
compiler forever.

### F21 `freeze` writes the detected version back into the environment (surveyed)
Spec: FRZ-4. On success `freeze` updates `env.yaml` with the version it
detected in the image (mim/main.rs:3395-3399).

### F22 Possible path traversal through a local package name (surveyed)
`vendor_snapshot` joins the package `name` from the envspec into a path
under `pixi/local/` without validation (mim/localdeps.rs:66);
`validate_program_name` guards only the program key. The envspec comes from
the user's own `package.yaml`, so the impact is a user writing outside the
env's tree by mistake rather than an attack, but the path is unchecked.

### F23 A corrupt `env-inputs.json` silently drops the language pins (surveyed)
`read_env_inputs` treats an unreadable file as "no pins" (mim/config.rs:
514), so the next rebuild drops them without a word.

### F24 Configuration read-modify-write is unlocked (surveyed)
Spec: ENV-8. Writes are locked and atomic (mim/config.rs:387-443), but
reads are not, and read-modify-write sequences (default, rename pointer,
`modify`) can lose a concurrent update. `install_flag_file` uses a
non-atomic copy (mim/config.rs:644).

### F25 The flag file is evaluated by `sh -c` on every read (verified)
Each `env.flags.yaml` entry is interpolated unquoted into
`sh -c "printf '%s\0' <entry>"` (mim/config.rs:684-705). Expansion is
documented, but it also means `$(...)` in the file runs every time any mim
command reads the env, including for every user of a system environment
(file owned by root). If the shell fails, the entry silently falls back to
whitespace splitting. A ruling is needed on whether entries should be
expanded at all.

### F26 System-dependency providers are handled inconsistently (surveyed)
Every provider except the exact string `"host"` becomes a conda dependency,
including `vcpkg` (deps/pixi.rs:482-486); `native_blockers` lowercases
before comparing (deps/envspec.rs:214) and is called only by the manager.

### F27 Smaller mim behavior notes (surveyed)
- `info` omits `MORLOC_BUILD_HOOK` and `MORLOC_BIN` from a native env's
  exports although it claims to mirror the runner (mim/main.rs:1417-1440).
- Native `run` may download pixi (`provision_pixi`, mim/main.rs:4153).
- `mim eval` ignores whether eval is enabled and always uses 127.0.0.1.
- `doctor --system` without `--env` is silently ignored (mim/main.rs:
  3925-3931).
- `demos`: a failed download leaves an empty destination that then needs
  `--force`; the tarball goes to a predictable `$TMPDIR/morloc-demos-<pid>`;
  `-local` versions never match (mim/demos.rs:115-198).
- The cache key for another platform's mim is the crate version, but the
  download uses `MIM_RELEASE_TAG` (mim/provision.rs:437-458).
- `update --latest` resolves through the GitHub latest when modules declare
  a Morloc range, but through `$MORLOC_RELEASE_TAG` otherwise
  (mim/main.rs:7109-7116 vs 4536-4538).

### F28 The frozen image serves on 8080; `mim start` defaults to 9000 for MCP (surveyed)
Not a bug, but two defaults for one server. Freeze CMD is fixed at 8080
(mim/freeze.rs:646-658).

### F29 Native serve is unsupervised (surveyed)
A native server is a detached process group with no restart and no reboot
survival (mim/main.rs:8961); a crashed child daemon is restarted only on the
next request to it (F55). Recorded as an observation; no spec item claims
supervision.

### F31 An installed module list is never validated against the views at start (surveyed)
`start` checks that each MCP/API module is installed, but a module removed
after `view add` makes `start` fail rather than the view being cleaned.
Observation; no item yet.

## mim: docs

### F37 `MORLOC_ENV` value semantics in docs
The manual's environment-variable list and README.md ("stores a Morloc
environment name") should say what `MORLOC_ENV` holds; waits on the F14
ruling.

## Compiler and nexus (outside the mim spec)

### F38 IPv6 loopback bind can never work (verified)
`is_loopback` accepts `"::1"` (nexus/mcp.rs:1438) but both HTTP servers
build the address with `format!("{}:{}", host, port)` (nexus/mcp.rs:1077,
1908), giving `::1:9000`, which does not parse. The daemon rejects IPv6
outright.

### F39 `MORLOC_EVAL_ALLOW_NO_AUTH=0` waives the eval lock (verified)
Any non-empty value counts (nexus/cli.rs:939-942), so `=0` and `=false`
turn the waiver on: a security-relevant surprise in a frozen image.

### F40 The daemon's own HTTP `/eval`, `/typecheck`, `/bind` have no eval token rule (surveyed)
`morloc-nexus daemon --http-host 0.0.0.0 --allow-no-auth` serves eval with
no token, unlike the front end, where eval stays locked
(compiler/data/rust/morloc-runtime/src/daemon_ffi.rs:1363-1377).

### F41 `morloc install owner/name` likely parses as core module `owner` (surveyed, unverified)
`moduleInstallParser` has no `eof` and `parseCoreModule` wins without a
remote prefix (compiler/library/Morloc/Module.hs:1374-1378, 1421-1423), so
`morloc install weena/calendar` (documented, compiler/executable/UI.hs:304)
may install `morloclib/weena` and drop `/calendar`. No test of install
strings found.

### F42 The default compiler config file is never read (surveyed)
With no `--config`, `loadMorlocConfig Nothing` returns built-in defaults
(compiler/executable/Subcommands.hs:125-128), so `registry`,
`lang_overrides`, `plane` and `plane-core` are unreachable;
`getDefaultConfigFilepath` is unused. A config without `home` gets the
literal unexpanded `"~/.local/share/morloc"`.

### F43 `installed_hash` records the requested pin, not the checked-out commit (surveyed)
Unpinned installs record null, so they cannot be reproduced and skip the
coherence check (compiler/library/Morloc/Module.hs:1130-1132, 881-883).

### F44 Registry installs and git installs use different layouts (surveyed)
Registry: `plane/<owner>/<name>`; `install --build` looks in
`plane/<name>` (Module.hs:837 vs Subcommands.hs:224).

### F45 envspec `modules` is first-wins, the resolver is closer-wins (surveyed)
compiler/library/Morloc/.../EnvSpec.hs:177-180 vs Module.hs:126-147.
mim ignores `modules` today, so no effect yet.

### F46 A program with only system deps gets a warning, not an error, without a hook (surveyed)
`packageHasDeps` ignores `dependencies` (Build.hs:264-270).

### F47 `morloc init` always rebuilds the runtime unless incremental (surveyed)
The condition rebuilds when `MORLOC_INIT_INCREMENTAL` is unset
(SystemConfig.hs:166-175), so `-f` does not control the runtime rebuild;
the comment at 167-168 says otherwise. Also the comment calling the
in-container Rust source "a read-only image layer" (SystemConfig.hs:365) is
wrong; mim makes it world-writable on purpose.

### F48 Empty `MORLOC_HOME`/`MORLOC_STATE` mean different things (surveyed)
Haskell treats empty as unset (Config.hs:86, 147); the nexus treats it as
set (nexus/main.rs:37, 49).

### F49 The home/state split leaks writes into the runtime root (surveyed)
`install` writes wrappers to `$MORLOC_HOME/bin` and completions to
`$MORLOC_HOME/completions`; `configTmpDir` stays under home.

### F50 Dead C++ search paths (surveyed)
`STATE/src/<base>/...` candidates (Cpp.hs:2466, 2485) do not match where
modules live.

### F51 Auto-install and resolution disagree on ambiguity (surveyed)
`findBareModuleMaybe` skips the ambiguity check (Module.hs:457-474);
acknowledged in its comment.

### F52 The front end maps every `/call` failure to 500 (surveyed)
Bad arguments, unknown commands and a recovering daemon (503 on the daemon)
all become 500 (nexus/mcp.rs:2602-2605); the comment at daemon_ffi.rs:545
claims the router maps statuses the same way.

### F53 `/call` is not filtered like MCP (surveyed)
Commands excluded from MCP (stream arguments, `@stdin`) remain reachable
through `/call` (nexus/mcp.rs:2589-2594).

### F54 `/health` and 401 bodies differ between daemon and front end (surveyed)
Daemon `/health` requires the token, front end's is open; 401 bodies
differ. Single-program `mcp --http-port` has no `/health` and no OPTIONS.

### F55 No MCP cancellation; no proactive child restart (surveyed)
`notifications/cancelled` is ignored; a crashed child daemon stays down
until the next request, while front-end `/health` stays 200.

### F56 Stale router comments (surveyed)
nexus/cli.rs:315-320 and nexus/main.rs:928-929 say the router scans
`exe/<name>/manifest.json` and serves HTTP/TCP/Unix; it serves only named
programs from `exe/<name>/<name>-build/manifest.json`, over HTTP only.
Compiler comments on expose targets (State.hs:842-843,
SystemConfig.hs:182-185) name `$MORLOC_HOME`; the real location is
`$STATE/modules`. ProgramBuilder/Build.hs:219 names the build hook
`mim-env`; it is `mim`.

### F57 `GET /eval` with eval off is 405, not 404 (surveyed)
Method check precedes the enabled check (nexus/mcp.rs:2053-2056 vs 2307).

### F58 `__` in names can abort the front end (surveyed)
A module or command name containing `__` that collides after joining
aborts startup (nexus/mcp.rs:1810-1817).

### F59 Manual says `registry` is read from `~/.morloc/config.yaml` (surveyed)
docs `ecosystem.asc` ("Installing modules"); that file is never read
without `--config` (F42). Fix with F42.

## Spec questions for a ruling

- PROV-6: what mim should do with the release's ABI version (F12).
- Should flag-file entries be shell-expanded at all (F25)?
- ENV-4: keep Apptainer (and fix F8) or remove it from `--engine`.
- RUN-2: is `MORLOC_ENV` the environment's name or only a flag (F14)?
- ENV-8: one lock per environment for every manager mutation, or only for
  solves?
