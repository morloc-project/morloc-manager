# Interface with the compiler (IFC)

The documents and process calls that pass between mim and the morloc
compiler. Who reads each one, and when, is in `overview.md`. The rules for
what mim does with them are in DEP, PROV and RUN; this file fixes their
shape.

### IFC-1 The compiler writes and mim reads
Intent: proposed
Code: conforms 2026-11-15

In every contract the compiler or its release CI is the writer and mim the
reader, so compatibility is mim's job. Each document carries a version. The
versions mim reads are defined once, in `morloc-deps::version`, and the
compiler holds the matching values in `Morloc.Version`.

### IFC-2 envspec.json states one program's requirements
Intent: proposed
Code: deviates F76

`morloc make` writes it into the program's build directory and `morloc
envspec` prints it; mim also writes one for the languages pinned with
`--lang`. Its fields:

- `envspec_version`, an integer (DEP-2 gives the versions mim reads);
- `morloc_version`, the writing compiler's version;
- `languages`, one entry per language with `lang` (`py`, `r`, `cpp`,
  `rust`), an optional version `constraint` and, for C++, an optional `std`;
- `packages`, a list of package requirements per language (IFC-3);
- `tools` (version 3), the package requirements whose executables the
  program runs, whatever its languages;
- `system`, system libraries, each a `name` and a `provider` hint
  (`conda-forge`, `host`, `vcpkg` or `unspecified`); all but `host` ones
  are solved as conda packages;
- `modules`, the morloc modules the program was built from, each a `name`
  and an optional `git_hash`. mim reads it and does not use it.

### IFC-3 A package requirement is tagged by its source
Intent: proposed
Code: deviates F82, F86

Each requirement has a `name` and a `source`, which fixes its other fields:

- `conda`: a `constraint` and an optional `channel` (absent means
  conda-forge);
- `pypi`, `crates`, `cran`, `bioconductor`: a `constraint`;
- `local`: a `path` relative to the project root (DEP-1, `--root`) and an
  `editable` flag, honored only where the environment is not immutable;
- `vendored`: written only by mim into its own store, never by the
  compiler.

Crates are resolved by cargo when a pool is built, not by the solve. `cran`
and `bioconductor` are not provisioned.

### IFC-4 morloc lang-support states the compiler's own requirements
Intent: proposed
Code: conforms 2026-11-15

`morloc lang-support` prints a document with a `schema_version`
(`MAJOR.MINOR`; a new language or field is a minor bump), the compiler's
`morloc_version`, a `toolchain` list required for every environment, and a
`languages` map. Each language entry has a `runtime` (the interpreter
package, its allowed version range and a default; null for C++), a
`requires` list, and, for a language not provisioned through conda, an
`install_script`. A requirement is a `package`, a `constraint`, a `phase`
(`build`, `runtime`, `both`) and an `optional` flag. mim solves only the
requirements that are not optional, and ignores `phase`.

### IFC-5 A development environment derives lang-support from source
Intent: ruled 2026-11-15
Code: conforms 2026-11-15
Tests: a_dev_environment_stores_its_dev_conda_packages

For an environment built from a compiler source tree, mim builds the same
document from the tree's language files instead of calling the compiler.
That form may also list `dev-apt` and `dev-conda` packages, needed to build
the compiler itself; a released compiler never emits them.

### IFC-6 The release manifest describes one compiler release
Intent: proposed
Code: conforms 2026-11-15

`morloc-release-manifest.json`, published with each release, holds a
`schema` integer, the release `version`, the name of the runtime source
archive (`runtime_src`, unpacking to `runtime/`), the compiler's contract `versions` (as printed by
`morloc versions`: `morloc_version`, `abi_version`, `envspec_version`,
`lang_support_schema`), a `triples` map from platform (`linux-x86_64`,
...) to that platform's assets (the compiler under `morloc`), and the
`sha256` of every asset. PROV-1 to PROV-3 and
PROV-6 say how mim uses it.

### IFC-7 A managed process tells the compiler which hook to call
Intent: proposed
Code: deviates F78

mim exports `MORLOC_ENV`, `MORLOC_BUILD_HOOK` and `MORLOC_BIN` together
(RUN-2). The compiler calls the hook only when `MORLOC_ENV` is set, and
fails `morloc make` when `MORLOC_ENV` is set without `MORLOC_BUILD_HOOK`.
`MORLOC_BIN` lets the hook call back `morloc lang-support` on the exact
compiler that is building. `MORLOC_MIM_ENV` overrides the hook binary for
development (PROV-8).

### IFC-8 The hook call
Intent: proposed
Code: deviates F79

The command line is DEP-1's. `--name` is the program's key in the store;
`--spec` is the program's envspec (IFC-2); `--root` is the entry module's
directory, against which `local` paths resolve; `--installed` marks a
`morloc make --install` build, whose spec joins the environment's installed
baseline instead of its scratch builds (DEP-13 limits this to builds under
the environment's own state).

### IFC-9 The hook reads its environment from the launcher
Intent: proposed
Code: conforms 2026-11-15
Tests: an_empty_pixi_variable_means_pixi_on_path

Besides IFC-7's variables, the hook reads `MORLOC_HOME` (the runtime,
required), `MORLOC_ENV_STORE` (the environment's store, required; DEP-11),
`MORLOC_PIXI` (the solver, default `pixi` on `PATH`), `MORLOC_PIXI_DIR`
(the solver project, default `<store>/pixi`) and `MORLOC_IMMUTABLE`
(non-empty and not `0` marks the environment immutable; DEP-10).
`MORLOC_STATE` never chooses the store.

### IFC-10 The compiler says whether its runtime and bindings are current
Intent: ruled 2026-11-26
Code: conforms 2026-11-26
Tests: a_compiler_is_asked_only_when_it_offers_the_check_and_is_named, the_compilers_report_is_read_as_written, a_stale_binding_is_rebuilt_once_and_checked_again, current_bindings_run_no_init, a_binding_still_stale_after_init_is_an_error_naming_it, a_runtime_that_does_not_match_is_refused_without_rebuilding, a_binding_that_cannot_be_built_is_refused_without_rebuilding, a_compiler_that_cannot_answer_falls_back, an_immutable_environment_refuses_by_the_bindings_state, a_rebuild_is_judged_on_the_prefixs_languages_only, only_the_programs_languages_are_judged, a_program_with_no_bound_languages_asks_nothing, the_rust_binding_is_asked_about_too, the_unactivated_check_leaves_the_ambient_path_alone, a_rebuild_must_produce_the_bindings_of_the_interpreters_the_prefix_holds

A compiler whose `lang-support` carries `bindings_check` >= 1 answers
`morloc init --check [--lang L]...` (compiler spec SHIM-2): one JSON object
on stdout, exit 0, with `runtime.ok` (and a `reason`) and, per language, a
`state` of `ok`, `absent`, `stale`, `unavailable` or `outside-prefix`. It
builds and writes nothing. mim reads the language markers of such a
compiler never, and deletes them never.

- The build hook asks the compiler named by `MORLOC_BIN`, run as a build
  runs (scrubbed, the activation applied, `MORLOC_HOME` set, the hook
  unset), for the program's languages that have a binding (py, r, cpp,
  and rust, which has no marker of mim's own). A runtime that
  is not ok is refused with `mim update --reinit`; it is never rebuilt from
  the hook. `absent` and `stale` bindings are rebuilt by one incremental
  init, after which the compiler is asked again and anything still not ok
  is an error naming it; `unavailable` and `outside-prefix` are errors.
- In an immutable environment the hook asks before the solve, with the
  ambient environment unscrubbed (the image's own interpreters), and
  refuses a binding that is not ok: `absent` with `mim modify --lang`,
  anything else with `mim update --reinit`.
- After a rebuild, a native environment asks as init ran; a container
  writes the report to the data dir's `.bindings-check.json`. Only the
  languages whose interpreter the prefix holds are judged.
- Only the program's languages (or, after a rebuild, the prefix's) are
  judged; one the report leaves out is not current. A program with no
  bound language asks nothing.
- A compiler without `bindings_check`, or one whose check fails or prints
  no report, is handled as before: markers by existence, the interpreter
  checks of `abi::shim_defects`, and marker deletion to force a rebuild.
  `mim clean` keeps deleting a dropped language's marker, since that
  fallback reads markers.
Why: the user chose the compiler as the one authority (2026-11-26), after
F119 showed two readers of an unspecified marker drifting apart.

