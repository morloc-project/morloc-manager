# Morloc Installation Manager (mim)

`mim` handles morloc installation and dependencies.


## Contracts with the morloc compiler

`mim` and `mim-env` are coupled to the morloc compiler only across process
boundaries. There is no linking, or FFI. There are four contracts: two JSON
documents, one CLI/behavioral contract, and one release-manifest.

1. envspec.json: a program's declared environment requirements

  - Producer: the morloc compiler (morloc make, morloc envspec)

  - Consumer: morloc-deps, lowered to a pixi manifest.

  - Transport: a JSON file written into the program's build dir; handed to the
    build hook as --spec envspec.json.

  - Versioning: integer `envspec_version`..

```json
{
  "envspec_version": 1,
  "morloc_version": "0.98.2",
  "languages": [
    { "lang": "py",  "constraint": ">=3.10" },
    { "lang": "cpp", "std": "c++20" },
    { "lang": "rust" }
  ],
  "packages": {
    "py":   [ { "name": "numpy",    "constraint": ">=2,<3", "source": "conda" },
              { "name": "requests", "constraint": "*",      "source": "pypi"  } ],
    "cpp":  [ { "name": "opencv",   "constraint": ">=4.8",  "source": "conda" } ],
    "rust": [ { "name": "ndarray",  "constraint": "0.16",   "source": "crates" } ]
  },
  "system":  [ { "name": "blas", "provider": "unspecified" } ],
  "modules": [ { "name": "tensor-cpp", "git_hash": "abc123" } ]
}
```

  |      Field      |         Type         |                             Notes                             |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | envspec_version | int (required)       | Contract version.                                             |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | morloc_version  | string (required)    | The compiler that emitted it.                                 |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | languages       | array                | { lang, constraint?, std? }                                   |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | packages        | [PackageReq]         | PackageReq = { name, constraint, source, channel? }.          |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | system          | array                | { name, provider }                                            |
  | --------------- | -------------------- | ------------------------------------------------------------- |
  | modules         | array                | { name, git_hash? }                                           |

 - language:
   - lang: is a canonical name (py/r/cpp/rust/julia)
   - constraint: a version match-spec
   - std: a C++ standard (cpp only)
 - packages:
   - `channel` is conda channel where "conda-forge" is default
   - `source` ∈ conda | pypi | cran | bioconductor | crates | pkg
 - system:
   - provider ∈ conda-forge | host | vcpkg | unspecified


2. morloc lang-support — morloc's own environment dependencies

  - Producer: the compiler subcommand `morloc lang-support`. `mim` can also
    derive it from a morloc source tree for a dev env with no compiler yet

  - Consumer: morloc-deps, intersected with each program's envspec (clamps
    runtime versions, injects binder deps)

  - Transport: stdout of `morloc lang-support` (invoked by `mim-env` via
    `MORLOC_BIN`); cached as `lang-support.json` in the runtime store.

  - Versioning: semver `schema_version` "MAJOR.MINOR". Adding a language/field
    is a minor bump (consumers ignore what they don't know); a breaking change
    is major.

```json
{
  "schema_version": "1.0",
  "morloc_version": "0.99.0",
  "toolchain": [
    { "package": "c-compiler", "constraint": "*", "optional": false },
    { "package": "rust",       "constraint": "*", "optional": false }
  ],
  "languages": {
    "py":  { "runtime": { "package": "python", "version": ">=3.10,<3.14", "default": "3.12" },
             "requires": [ { "package": "numpy",   "constraint": ">=1.22,<3", "optional": false },
                           { "package": "pyarrow", "constraint": "*",         "optional": true } ] },
    "cpp": { "runtime": null,
             "requires": [ { "package": "cxx-compiler", "constraint": "*", "optional": false } ] },
    "futhark": { "runtime": null, "requires": [], "install_script": "#!/bin/sh ... futhark-lang.org ..." }
  }
}
```

  |     Field      |       Type      |                          Notes                           |
  |----------------|-----------------|----------------------------------------------------------|
  | schema_version | string (semver) | Contract version.                                        |
  |----------------|-----------------|----------------------------------------------------------|
  | morloc_version | string          | Release the table describes.                             |
  |----------------|-----------------|----------------------------------------------------------|
  | toolchain      | [PkgReq]        | Core conda packages always required (libmorloc + shims). |
  |----------------|-----------------|----------------------------------------------------------|
  | languages      | [LangEntry]     | Keyed py/r/cpp/rust/futhark/…                            |

 - PkgReq = { package, constraint, optional, phase }
   - constraint is a conda match-spec (default "*")
   - key is constraint, source YAML uses version)
   - optional deps are included in a full env, omitted in a minimal one

 - LangEntry = { runtime?, install_script?, requires }
   - `runtime` is { package, version, default?, std? } (std for C++)
   - `install_script` for script-provisioned languages (e.g. futhark), which
     contribute nothing to the conda solve and are installed by running the
     script in an OCI image build.
   - `requires` is a list of packages

3. The build-hook CLI — how morloc make invokes the provisioner

A behavioral contract (no JSON body of its own; it carries envspec.json). During
morloc make, before compiling pools, the compiler runs an external provisioner:

```
"$MORLOC_BUILD_HOOK" sync --name <program-key> --spec envspec.json
```

|      Field      |         Type       |                             Notes                     |
| --------------- | ------------------ | ----------------------------------------------------- |
| envspec_version | int (required)     | Contract version.                                     |
| --------------- | ------------------ | ----------------------------------------------------- |
| morloc_version  | string (required)  | The compiler that emitted it.                         |
| --------------- | ------------------ | ----------------------------------------------------- |
|                 |                    | { lang, constraint?, std? }                           |
|  languages      | array              | lang is a canonical name (py/r/cpp/rust/julia)        |
|                 |                    | constraint a version match-spec                       |
|                 |                    | std a C++ standard (cpp only)                         |
| --------------- | ------------------ | ----------------------------------------------------- |
| packages        | [PackageReq]       | PackageReq = { name, constraint, source, channel? }.  |                                                     │
| --------------- | ------------------ | ----------------------------------------------------- |
| system          | array              | { name, provider }                                    |
|                 |                    | provider ∈ conda-forge | host | vcpkg | unspecified   |
| --------------- | ------------------ | ----------------------------------------------------- |
| modules         | array              | { name, git_hash? }.                                  |


  "modules": [ { "name": "tensor-cpp", "git_hash": "abc123" } ]


 - PackageReq.source ∈ conda | pypi | cran | bioconductor | crates | pkg
 - channel is conda-only, set only for a non-conda-forge channel (e.g. bioconda)
 - consumer routing:
   - conda -> pixi [dependencies] (R feedstocks become r-<lowercase> on conda-forge)
   - pypi -> [pypi-dependencies]
   - crates/pkg -> excluded (cargo/Pkg.jl resolve them at pool build; only the
     toolchain is injected);
   - cran/bioconductor -> not yet provisioned (fail-closed with an actionable error).


4. morloc-release-manifest.json -- the native-install download manifest

  - Producer: morloc's release CI (attached to each morloc GitHub release).

  - Consumer: mim (provision.rs), which fetches it to discover + SHA-verify the
    prebuilt assets for a native install.

  - Transport: a release asset fetched from
    github.com/morloc-project/morloc/releases/<tag>.

  - Versioning: integer schema (mim accepts up to SUPPORTED_MANIFEST_SCHEMA; a
    higher value yields a clean "upgrade mim" error). current: mim supports 2;
    morloc now emits 3 as the transition marker — WS5 makes mim schema-3-aware
    and renames the manager asset to mim + adds mim-env.

``` json
{
  "schema": 3,
  "version": "0.98.3",
  "rust_src": "morloc-rust-src.tar.gz",
  "triples": {
    "linux-x86_64": { "morloc": "morloc-linux-x86_64", "manager": "mim-linux-x86_64" },
    "linux-arm64":  { "morloc": "morloc-linux-arm64",  "manager": "mim-linux-arm64"  }
  },
  "sha256": {
    "morloc-linux-x86_64": "<hex>",
    "mim-linux-x86_64":     "<hex>",
    "morloc-rust-src.tar.gz": "<hex>"
  }
}
```

|  Field   |         Type        |                                       Notes                   |
| -------- | ------------------- | ------------------------------------------------------------- |
| schema   | int                 | Manifest contract version.                                    |
| -------- | ------------------- | ------------------------------------------------------------- |
| version  | string              | The release version.                                          |
| -------- | ------------------- | ------------------------------------------------------------- |
| rust_src | string              | Asset name of the Rust-workspace-source tarball.              |
| -------- | ------------------- | ------------------------------------------------------------- |
| triples  | { morloc, manager } | manager -> mim + a mim_env asset after WS5.                   |
|          |                     | Only complete triples appear                                  |
|          |                     | omitted (no prebuilt compiler → falls through to a container) |
| -------- | ------------------- | ------------------------------------------------------------- |
| sha256   | map asset → hex     | Every downloaded asset present here is verified after fetch   |
|          | digest (default {}) | Absent entries are fetched but unverified (back-compat).      |
