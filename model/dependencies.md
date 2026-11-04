# Dependencies (DEP)

### DEP-1 The build hook is mim's only entry point during a build
Intent: proposed
Code: unaudited

Inside an environment, `morloc make` runs
`$MORLOC_BUILD_HOOK sync --name <key> --spec envspec.json --root <dir>
[--installed]` before compiling pools. Exit 0 means the toolchain now
satisfies the spec; any other status fails the build. The hook writes only
to standard error.

### DEP-2 A spec of another version is refused with the direction of the fix
Intent: proposed
Code: unaudited

The envspec and lang-support documents carry versions. mim accepts the
envspec versions it reads (2, and 3, which adds tools; a version 2 spec
reads as one with no tools) and any lang-support schema of the same major
version, and refuses others with "rebuild the program" or "upgrade mim".

### DEP-3 One solved world per environment
Intent: proposed
Code: unaudited

An environment has one set of packages: the solve of the union of every
installed program's spec, every scratch build's spec, the compiler's own
requirements, the user's extra packages and the interpreter pins (DEP-7).
One package has one version. Unsatisfiable requirements fail the command.

### DEP-4 Solves of one environment are serialized
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: the_environment_lock_is_the_one_the_hook_takes, a_waiter_locks_the_stores_current_lock_file, a_lock_for_another_environment_is_refused, only_the_managers_lock_creates_the_store

One exclusive lock per environment, a file in its store, is shared by the
manager, the build hook and the container. Every path that writes the
store, the solver's manifest or lock, or the solved prefix takes it. A
hook holds it once, from the id check (DEP-12) through any snapshot of a
program's local packages, the solve, the binding build and the snapshot
sweep. The manager holds it around its own solves only, never across a
build, which takes it through the hook. A lock is taken on the file the
store holds when it is granted: a waiter whose file was replaced (the
environment removed or renamed, and another made in its place) locks the
current one. Only the manager creates a store. Model: tla/EnvWorld.tla
(no_manager_lock, write_before_check).

### DEP-5 A failed solve leaves the recorded requirements as they were
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: a_failed_solve_leaves_the_store_as_it_was, an_install_supersedes_the_programs_scratch_builds_once_it_solves, a_later_scratch_build_of_an_installed_program_is_in_the_world, an_interrupted_commit_is_finished_by_the_next_lock, a_repeated_sync_takes_the_fast_path, a_fast_path_records_a_missing_interpreter_pin

A sync or clean solves the world it would leave, then writes the specs it
adds, replaces or removes, so a solve that fails, or a process that dies
before it records, leaves the store as it was. A commit that writes one
spec and removes others is one step: a crash part-way through it is
finished, or undone if its spec was never written, by the next taker of
the lock. The interpreter pin follows the prefix and is recorded by every
sync that succeeds. The solver's own files may
be left changed; the next successful solve reconciles them. Model:
tla/EnvWorld.tla (commit_first).

### DEP-6 A solve with unchanged requirements does nothing
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: a_manifest_changed_after_the_digest_is_solved_again

The world's requirements reduce to a canonical digest. When it equals the
digest of the last successful solve, the solver's manifest on disk is the
one that solve wrote, and the solved prefix exists, the solve is skipped.
The digest is written last, so an interrupted solve is redone. Model:
tla/EnvWorld.tla (digest_only).

### DEP-7 Versions move only when the requirements force them
Intent: proposed
Code: deviates F18

A solve first tries the existing lock unchanged and re-solves only if the
lock cannot satisfy the new requirements. Once an interpreter is
provisioned, its minor version is pinned so that a later solve cannot break
the language bindings built against it; a binding that no longer matches is
rebuilt, or refused in an immutable environment.

### DEP-8 Every declared dependency is provided, delegated, or refused
Intent: proposed
Code: unaudited

conda and PyPI packages are solved into the world, and so are tools, the
conda packages whose executables a program runs. Crates are left to
cargo. Sources mim does not
support are refused, never dropped. Two conflicting declarations of one
package (two channels, two local paths, a path and a version) are refused.

### DEP-9 An installed program does not depend on its project directory
Intent: proposed
Code: unaudited

A local Python package of an installed program is copied into the
environment under the hash of its contents and the requirement points at
the copy. Snapshots no installed program references are reclaimed.

### DEP-10 An immutable environment refuses any change to its world
Intent: proposed
Code: unaudited

In an environment marked immutable (a running server, a frozen image), a
sync that would change the world or rebuild a binding fails before writing
anything.

### DEP-11 The build hook works on its environment's store, whatever the state
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: the_hook_store_is_the_environments_not_the_states, a_hook_without_the_store_marker_refuses, a_hook_whose_store_is_gone_refuses_and_creates_nothing, managed_launches_name_the_environments_store

Every managed process carries `MORLOC_ENV_STORE`, the environment's store
as that process sees it. The build hook reads its specs, pins and lock from
it, never from `MORLOC_STATE`, so a build under another state changes the
environment's world and no other. A hook started without it, or whose
store does not exist (the environment was renamed or removed since the
process started), refuses before writing anything. Model: tla/EnvWorld.tla
(state_store).

### DEP-12 A store is never solved into another environment's project
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: a_sync_refuses_a_project_with_another_id, a_sync_refuses_a_missing_id, the_manager_gives_store_and_project_one_id

An environment's store and its pixi project each hold the environment's
id. Only the manager writes them, from the one data dir that holds both:
it creates the store's id if absent and copies it to the project. A sync or
clean whose two ids differ, or where either is absent, refuses before
writing. Model: tla/EnvWorld.tla (no_id_check, wrong_marker).

### DEP-13 A build under another state is a scratch build
Intent: ruled 2026-11-04
Code: conforms 2026-11-04
Tests: a_build_under_another_state_is_recorded_as_scratch, one_state_has_one_scratch_key_however_it_is_spelled, a_build_with_no_state_set_uses_the_compilers_default_state

A sync whose state is not the environment's store records its spec as a
scratch build under its program name qualified by a digest of that state's
canonical path, even with `--installed`. The state is the compiler's:
`MORLOC_STATE`, else `MORLOC_HOME`, else `$HOME/.local/share/morloc`. It never replaces a program installed in
the environment, it does not replace that program's scratch builds, and
`clean` drops it.
