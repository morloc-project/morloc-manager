# The Morloc Installation Manager (mim)

`mim` provisions and manages [morloc](https://github.com/morloc-project/morloc)
environments: it installs the morloc compiler + runtime, resolves each program's
cross-language package dependencies into a coherent world (via conda/pixi), and
runs, serves, freezes, and inspects morloc programs. It works natively or in
containers.

This repository holds three Rust crates:

 - **`mim`**: defines the user-facing manager

 - **`mim-env`**: the in-environment dependency agent. The morloc compiler
   invokes it during `morloc make` to provision a program's declared
   dependencies.

 - **`morloc-deps`**: shared dependencies between `mim` and `mim-env`.

## Contracts with the morloc compiler

The compiler and the manager are coupled **only** across process
boundaries. There are four such contracts. In all of them the **compiler writes
and the manager reads**. The manager is responsible for compatibility. The
versions are set in the Morloc repo and here in `morloc-deps::version`.

### 1. `envspec.json` — a program's declared environment requirements

- **Producer:** the compiler (`morloc make` / `morloc envspec`); also `mim` itself
  (`EnvSpec::from_languages`) for `--lang` pins during bootstrap.
- **Consumer:** `morloc-deps`, lowered to a pixi manifest by `pixi.rs`.
- **Transport:** a JSON file in the program's build dir, handed to the build hook.
- **Version:** integer `envspec_version`.

Example:

```json
{
  "envspec_version": 1,
  "morloc_version": "0.99.0",
  "languages": [ { "lang": "py", "constraint": ">=3.10" }, { "lang": "cpp", "std": "c++20" }, { "lang": "rust" } ],
  "packages": {
    "py":   [ { "name": "numpy", "constraint": ">=2,<3", "source": "conda" },
              { "name": "requests", "constraint": "*", "source": "pypi" } ],
    "cpp":  [ { "name": "opencv", "constraint": ">=4.8", "source": "conda" } ],
    "rust": [ { "name": "ndarray", "constraint": "0.16", "source": "crates" } ]
  },
  "system":  [ { "name": "blas", "provider": "unspecified" } ],
  "modules": [ { "name": "tensor-cpp", "git_hash": "abc123" } ]
}
```

### 2. `morloc lang-support` — the compiler's own environment contribution

 - **Producer:** `morloc lang-support`. `mim` can also derive it from a morloc
   source tree for a from-source dev env.

 - **Consumer:** `morloc-deps`.

 - **Version:** semver `schema_version` `"MAJOR.MINOR"`. Adding a language or
   field is a **MINOR** bump.

Example:

```json
{
  "schema_version": "1.0",
  "morloc_version": "0.99.0",
  "toolchain": [ { "package": "c-compiler", "constraint": "*", "phase": "build", "optional": false } ],
  "languages": {
    "py":  { "runtime": { "package": "python", "version": ">=3.10,<3.14", "default": "3.12" },
             "requires": [ { "package": "numpy", "constraint": ">=1.22,<3", "phase": "both", "optional": false },
                           { "package": "pyarrow", "constraint": "*", "phase": "runtime", "optional": true } ] },
    "cpp": { "runtime": null, "requires": [ { "package": "cxx-compiler", "constraint": "*", "phase": "build", "optional": false } ] }
  }
}
```

 - `runtime`: the versioned interpreter (null for C++).

 - `optional`: usable-if-present, included in a full env and omitted in a
   minimal one.

 - `phase` (build|runtime|both) is emitted but currently unused by the consumer.

A script-provisioned language (e.g. futhark) carries an `install_script` instead
of a conda `runtime`.

### 3. `morloc-release-manifest.json` — the native-install manifest

 - **Producer:** Morloc's GitHub release CI.

 - **Consumer:** `mim` -- fetched to discover, SHA-verify, and download the
   prebuilt compiler + rust source for a native install.

 - **Version:** integer `schema`. The compiler's own contract versions are
   inlined under `versions{}` (from `morloc versions`), so `mim` gates on
   compatibility from the one fetch it already makes.

Example:

```json
{
  "schema": 1,
  "version": "0.99.0",
  "rust_src": "morloc-rust-src.tar.gz",
  "versions": { "morloc_version": "0.99.0", "abi_version": 1, "envspec_version": 1, "lang_support_schema": "1.0" },
  "triples": { "linux-x86_64": { "morloc": "morloc-linux-x86_64" } },
  "sha256": { "morloc-linux-x86_64": "<hex>", "morloc-rust-src.tar.gz": "<hex>" }
}
```

- `versions` is checked at install time: if any declared contract version *exceeds*
  what this `mim` supports, the install is refused with "upgrade mim".

  `abi_version` is recorded (for doctor / frozen-image coherence), not gated, while
  libmorloc is built from the compiler's own source.

- `sha256` is **mandatory**: every asset the manager downloads is verified
  against its digest.


### 4. The build-hook CLI — how `morloc make` invokes the provisioner

During `morloc make`, before compiling pools, the compiler runs an external
provisioner:

```
"$MORLOC_BUILD_HOOK" sync --name <program-key> --spec envspec.json
```

 - **`MORLOC_BUILD_HOOK`** names the provisioner (our `mim-env`). If `MORLOC_ENV`
   is set but this is unset, `morloc make` **errors** — a managed env with no provisioner.

 - **`MORLOC_BIN`** is the compiler's own path, so the hook's reverse `morloc
   lang-support` call resolves the exact driving compiler without relying on PATH.

 - **`MORLOC_ENV`** stores a Morloc environment name. The dependency management
   through `mim` only occurs when it is set.

`mim` is the reference hook. It exports all three variables together wherever it
launches a managed `morloc make`.

## History

`morloc-manager` began as a shell script in this repo. Then it moved to the main
morloc repo as a large Rust program. Now it is back home.
