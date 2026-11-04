# Interface with the compiler (IFC)

The documents and process calls that pass between mim and the morloc
compiler. Who reads each one, and when, is in `overview.md`. The rules for
what mim does with them are in DEP, PROV and RUN; this file fixes their
shape.

### IFC-1 The compiler writes and mim reads
Intent: proposed
Code: unaudited

In every contract the compiler or its release CI is the writer and mim the
reader, so compatibility is mim's job. Each document carries a version. The
versions mim reads are defined once, in `morloc-deps::version`, and the
compiler holds the matching values in `Morloc.Version`.

### IFC-2 envspec.json states one program's requirements
Intent: proposed
Code: unaudited

`morloc make` and `morloc envspec` write it into the program's build
directory; mim also writes one for the languages pinned with `--lang`. Its
fields:

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
Code: unaudited

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
Code: unaudited

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
Intent: proposed
Code: unaudited

For an environment built from a compiler source tree, mim builds the same
document from the tree's language files instead of calling the compiler.
That form may also list `dev-apt` and `dev-conda` packages, needed to build
the compiler itself; a released compiler never emits them.

### IFC-6 The release manifest describes one compiler release
Intent: proposed
Code: unaudited

`morloc-release-manifest.json`, published with each release, holds a
`schema` integer, the release `version`, the name of the runtime source
archive (`rust_src`), the compiler's contract `versions` (as printed by
`morloc versions`: `morloc_version`, `abi_version`, `envspec_version`,
`lang_support_schema`), a map from platform (`linux-x86_64`, ...) to the
compiler asset name, and the `sha256` of every asset. PROV-1 to PROV-3 and
PROV-6 say how mim uses it.

### IFC-7 A managed process tells the compiler which hook to call
Intent: proposed
Code: unaudited

mim exports `MORLOC_ENV`, `MORLOC_BUILD_HOOK` and `MORLOC_BIN` together
(RUN-2). The compiler calls the hook only when `MORLOC_ENV` is set, and
fails `morloc make` when `MORLOC_ENV` is set without `MORLOC_BUILD_HOOK`.
`MORLOC_BIN` lets the hook call back `morloc lang-support` on the exact
compiler that is building. `MORLOC_MIM_ENV` overrides the hook binary for
development (PROV-8).

### IFC-8 The hook call
Intent: proposed
Code: unaudited

The command line is DEP-1's. `--name` is the program's key in the store;
`--spec` is the program's envspec (IFC-2); `--root` is the entry module's
directory, against which `local` paths resolve; `--installed` marks a
`morloc make --install` build, whose spec joins the environment's installed
baseline instead of its scratch builds (DEP-13 limits this to builds under
the environment's own state).

### IFC-9 The hook reads its environment from the launcher
Intent: proposed
Code: unaudited

Besides IFC-7's variables, the hook reads `MORLOC_HOME` (the runtime,
required), `MORLOC_ENV_STORE` (the environment's store, required; DEP-11),
`MORLOC_PIXI` (the solver, default `pixi` on `PATH`), `MORLOC_PIXI_DIR`
(the solver project, default `<store>/pixi`) and `MORLOC_IMMUTABLE`
(non-empty and not `0` marks the environment immutable; DEP-10).
`MORLOC_STATE` never chooses the store.
