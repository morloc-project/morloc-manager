# Running (RUN)

### RUN-1 A command runs with the environment's installation and the user's directory
Intent: proposed
Code: unaudited

`run` and `shell` execute with the environment's compiler, runtime,
toolchain and installed programs first on `PATH`, `MORLOC_HOME` set to its
runtime, and the user's current directory as the working directory (mounted
at `/work` in a container). Files written there belong to the invoking user.

### RUN-2 A managed command can build
Intent: proposed
Code: deviates F14

Every command mim launches in an environment exports `MORLOC_ENV` (the
environment's name), `MORLOC_BUILD_HOOK` (the agent, PROV-8) and
`MORLOC_BIN` (the compiler), together, so `morloc make` inside it keeps the
toolchain in step (DEP-1).

### RUN-3 The exit status is the command's
Intent: proposed
Code: deviates F15
Tests: a_command_status_is_never_taken_for_an_engine_error

mim exits with the status of the command it ran. A failure of mim or the
engine before the command starts is reported as such and never confused
with a status the command chose.

### RUN-4 Foreign activations never leak into an environment
Intent: proposed
Code: unaudited

Before a solve, a build or a native command, mim removes the variables of
any conda, mamba or pixi environment active on the host, so two package
worlds never mix compiler flags or library paths.

### RUN-5 Only declared variables cross into a container
Intent: proposed
Code: unaudited

A container sees the host's environment variables only as named with
`--env-var` or `--env-file`. Extra engine flags come from the environment's
flag file, by phase and engine, then the command line.

### RUN-6 The environment's toolchain does not depend on the caller's shell
Intent: ruled 2026-10-25
Code: conforms 2026-10-25
Tests: toolchain_vars_are_pinned_to_the_prefix, captured_compiler_resolves_without_path, capture_activates_once_and_records_no_conda_bookkeeping, capture_fails_when_the_compiler_is_not_installed, recorded_activation_drops_conda_bookkeeping, a_recorded_host_compiler_is_reported, only_override_forms_are_scrubbed_at_run_time, cc_rs_target_forms_are_tool_vars, a_recorded_bare_compiler_is_pinned_on_read, a_cached_bare_compiler_is_pinned_on_read, shell_warns_when_startup_files_drop_the_prefix_from_path, shell_warns_when_startup_files_switch_conda_prefix, shell_is_quiet_when_startup_files_keep_the_prefix

mim captures an activation once, from a shell holding none of the caller's
compiler or build-flag variables (the SDK selection stays the caller's),
and fails when `$CC` or `$CXX` names no executable. Every program variable
it records (compilers, binutils, cargo's target linker, the per-target
forms cc-rs reads first) names its tool by absolute path in the
environment's prefix, and a native command sees none of the caller's
per-target program or flag overrides (`TARGET_CC`, `CC_<target>`,
`CFLAGS_<target>`, cargo's target linker) that the record does not set. The record carries no `CONDA_SHLVL`
and no `CONDA_BACKUP_*`, so a conda in the user's shell activates on top of
the environment, ahead of it on `PATH`, rather than deactivating it. Native
`run` and `shell` export `MORLOC_STRICT_CONDA`, so a `morloc init` there
holds its tools to the prefix. `shell` never undoes the user's startup
files; it warns when they moved `CONDA_PREFIX` or dropped the prefix's
`bin` from `PATH`, and when the record names a compiler that is not a
program in the prefix.
