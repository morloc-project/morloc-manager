# Environments (ENV)

### ENV-1 An environment is a name in a scope
Intent: ruled 2026-11-15
Code: conforms 2026-11-15
Tests: a_lookup_refuses_a_name_that_leaves_the_tree, a_listed_legacy_environment_is_found

The scope is *local* (one user) or *system* (every user on the machine).
A name is 1-64 characters of ASCII letters, digits, `-`, `_` and `.`,
starting with a letter or digit, and is unique within its scope. A lookup
by name tries local, then system.

### ENV-2 An environment exists exactly when its configuration record does
Intent: ruled 2026-11-20
Code: conforms 2026-11-20
Tests: a_failed_new_leaves_nothing_behind, a_committed_new_keeps_its_files_and_drops_its_marker, an_interrupted_new_is_removed_at_its_commit, a_marked_remnant_is_listed_removed_and_replaced, a_remnant_whose_lock_is_held_is_left_alone, an_unmarked_remnant_is_refused_by_new_and_removed_by_rm, a_container_remnant_names_its_image_and_volume, a_failed_native_new_leaves_no_remnant, a_recorded_environment_is_never_cleaned_as_a_remnant

An environment exists exactly when its configuration record does. A `new`
that fails, or is interrupted, removes what it created before mim returns.
After a killed `new`, `ls` lists the remnant as incomplete, and `rm` or the
next `new` of that name removes it.

### ENV-3 Each scope has a configuration root and a data root
Intent: proposed
Code: deviates F93

An environment's configuration (settings, views, the serve record) lives
under the scope's configuration root and is never visible inside the
environment. Its data (runtime, toolchain, requirements, installed programs,
logs) lives under the data root. Downloads shared by environments (compiler
releases, pixi, mim binaries for other platforms) live once per scope in the
data root.

### ENV-4 The backend is fixed when the environment is created
Intent: proposed
Code: deviates F8

The backend is *native* (the toolchain runs on the host) or a container
engine (Docker, Podman, Apptainer). Without a choice, mim picks native where
the host supports it, else a configured or the only installed engine, and
asks when several are installed. Every backend mim offers supports every
command not listed as backend-specific.

### ENV-5 The default environment is one name per scope
Intent: proposed
Code: deviates F16
Tests: removal_clears_only_the_defaults_that_named_the_environment, nuke_keeps_a_default_that_names_another_scope

A command without `--env` acts on the local default if it names an existing
environment, else on the system default. A system default names a system
environment; a local default may name either. Removing an environment clears
only the defaults that named it.

### ENV-6 A failed change leaves the environment as it was
Intent: proposed
Code: deviates F5

`new`, `update`, `modify` and `install` either complete or leave the
environment's configuration, requirements and solved toolchain as they were
before the command. Rebuilt images and caches may remain.

### ENV-7 Removal removes everything mim made for the environment
Intent: proposed
Code: deviates F6
Tests: removal_stops_a_native_server, removal_deletes_read_only_directories_it_owns

`rm` stops the environment's server, then removes its configuration, data,
image and volumes. It does not touch directories the user supplied (a
mounted home) or shared caches. If anything it should remove remains, it
fails.

### ENV-8 Commands that change one environment are serialized
Intent: proposed
Code: deviates F3, F24

Two mim commands that change the same environment (its configuration, its
requirements, its toolchain, its image) never interleave; the second waits.
This includes a solve started by the build hook (DEP-4).

### ENV-9 The Morloc version moves only when asked
Intent: proposed
Code: conforms 2026-11-15
Tests: install_into_a_version_less_environment_is_refused, an_unreadable_module_manifest_is_an_error

An environment records the Morloc version it was provisioned with. Only
`update --latest` and `update --morloc-version` change it; every other
command keeps it or fails. A move is refused if an installed module's
declared Morloc range excludes the target, unless overridden.

### ENV-10 A system environment is changed only by root
Intent: proposed
Code: conforms 2026-11-15

Creating, changing, serving and removing a system environment require root.
Running commands in it and making it a personal default do not.

### ENV-11 doctor passes on every healthy environment
Intent: proposed
Code: deviates F94
Tests: container_data_dirs_are_checked_under_the_runtime_prefix, a_read_only_data_dir_fails_only_whoever_administers_it

`doctor` checks the invariants of this spec that it can observe and exits
non-zero on any failure (or any warning with `--strict`). A freshly created
environment of every backend passes.

### ENV-12 clean removes only what the environment regenerates
Intent: ruled 2026-11-15
Code: conforms 2026-11-15
Tests: clean_removes_old_pool_outputs_and_keeps_the_crate_cache, clean_keeps_a_pool_built_in_the_last_hour, clean_crates_removes_the_crate_cache_but_not_locks_or_pins, clean_dry_run_removes_nothing, clean_waits_while_cargo_holds_a_lock, clean_removes_only_old_empty_eval_dirs_and_killed_lock_merges, clean_inside_a_native_environment_uses_the_compilers_state

`mim clean` removes the Rust pool build outputs from the shared cargo
target dir and stale temporary files: empty `morloc eval` work dirs and the
staging files of a killed lock merge. `-c`/`--crates` also removes the
compiled crate cache. `-u`/`--uninstalled` also drops the requirements of
programs built but never installed and re-solves the environment to its
installed programs (DEP-5). `--dry-run` reports what would go. It never
removes an installed program, a module, the runtime, the requirement
store's installed specs, the environment cargo lock or cargo's lock files.
It holds every cargo lock of the target dir while it works, and leaves any
pool build or temporary file changed in the last hour, so a build in
flight is not broken. On the host it runs inside the environment.
