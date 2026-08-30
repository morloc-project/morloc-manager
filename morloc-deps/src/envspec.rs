//! Parser for `envspec.json`, the backend-agnostic environment-requirement
//! record emitted by the morloc compiler (`morloc make`) beside `manifest.json`.
//!
//! This is the Rust half of a cross-language contract: the schema is produced by
//! `Morloc.CodeGenerator.EnvSpec.renderEnvSpec` in the compiler. Every package
//! carries an explicit `source` (the package database it is drawn from); the
//! backend routes each package by its stated source rather than guessing from
//! the name.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DepsError, Result};
use crate::version::ENVSPEC_VERSION;

/// How local (filesystem-path) dependencies are relocated when a spec is resolved.
/// One policy per provenance so the two paths share exactly one implementation.
pub enum LocalAnchor<'a> {
    /// Scratch/dev-loop: resolve to the canonical real path under `root`, keep
    /// `editable`. Valid only while that path exists (native host / `/work`).
    Live { root: &'a Path },
    /// Install: copy each Python local's source under `copy_root` and reference it
    /// state-relative (`<state_root>/local/<key>/<name>`, plain install). `root` is
    /// the project root the source paths resolve against.
    Relocated {
        root: &'a Path,
        copy_root: &'a Path,
        state_root: &'a str,
        key: &'a str,
    },
}

impl<'a> LocalAnchor<'a> {
    fn root(&self) -> &Path {
        match self {
            LocalAnchor::Live { root } | LocalAnchor::Relocated { root, .. } => root,
        }
    }
}

/// A source-tree copy the caller must perform for a `Relocated` resolution (the
/// fs copy lives in `mim`, which owns the copy utility; this crate stays fs-light).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyJob {
    pub src: PathBuf,
    pub dest: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangReq {
    pub lang: String,
    #[serde(default)]
    pub constraint: Option<String>,
    /// C++ standard, e.g. "c++20" (cpp only).
    #[serde(default)]
    pub std: Option<String>,
}

/// One external package required by a pool, tagged by the `source` field it
/// carries on the wire. A discriminated union so a registry package can never
/// carry a path and a `local` package can never carry a version/channel. Mirrors
/// the compiler's `Morloc.CodeGenerator.EnvSpec.PackageReq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum PackageReq {
    /// conda package. `channel` is the sub-database, present only for a
    /// NON-conda-forge channel; absent means conda-forge (the universal default).
    Conda {
        name: String,
        constraint: String,
        #[serde(default)]
        channel: Option<String>,
    },
    /// Python Package Index.
    Pypi { name: String, constraint: String },
    /// crates.io (owned by cargo; resolved at pool build).
    Crates { name: String, constraint: String },
    /// Julia's General registry (owned by Pkg.jl; resolved at pool build).
    Pkg { name: String, constraint: String },
    /// R CRAN registry (not yet honored natively; the compiler rejects it).
    Cran { name: String, constraint: String },
    /// R Bioconductor registry (not yet honored natively).
    Bioconductor { name: String, constraint: String },
    /// Local filesystem-path dependency. `path` is module-relative (resolved late,
    /// against a fixed anchor per context); `editable` is an intent (editable in
    /// interactive contexts, a plain snapshot when serving or frozen).
    Local {
        name: String,
        path: String,
        #[serde(default)]
        editable: bool,
    },
}

impl PackageReq {
    /// The package name, regardless of source.
    pub fn name(&self) -> &str {
        match self {
            PackageReq::Conda { name, .. }
            | PackageReq::Pypi { name, .. }
            | PackageReq::Crates { name, .. }
            | PackageReq::Pkg { name, .. }
            | PackageReq::Cran { name, .. }
            | PackageReq::Bioconductor { name, .. }
            | PackageReq::Local { name, .. } => name,
        }
    }

    /// The conda channel, present only for a non-conda-forge conda package.
    pub fn channel(&self) -> Option<&str> {
        match self {
            PackageReq::Conda { channel, .. } => channel.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemReq {
    pub name: String,
    /// Provider hint: "conda-forge" | "host" | "vcpkg" | "unspecified".
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleReq {
    pub name: String,
    #[serde(default)]
    pub git_hash: Option<String>,
}

/// A program's declared environment requirements. Keys of `packages` are
/// canonical morloc language names ("py", "r", "cpp", "rust", "julia").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSpec {
    pub envspec_version: u32,
    pub morloc_version: String,
    #[serde(default)]
    pub languages: Vec<LangReq>,
    #[serde(default)]
    pub packages: std::collections::BTreeMap<String, Vec<PackageReq>>,
    #[serde(default)]
    pub system: Vec<SystemReq>,
    #[serde(default)]
    pub modules: Vec<ModuleReq>,
}

impl EnvSpec {
    /// Parse an EnvSpec from JSON text.
    pub fn from_json(text: &str) -> Result<Self> {
        // Peek the schema version before the full parse. A stale (v1) envspec.json
        // lacks the `source` field and would otherwise fail with an opaque serde
        // error deep in an env-wide operation; peeking lets us name the real fix.
        #[derive(Deserialize)]
        struct VersionPeek {
            envspec_version: u32,
        }
        let peek: VersionPeek = serde_json::from_str(text).map_err(|e| {
            DepsError::Env(format!("Failed to parse envspec.json: {e}"))
        })?;
        if peek.envspec_version < ENVSPEC_VERSION {
            return Err(DepsError::Env(format!(
                "envspec.json is version {} but this mim requires version {}. \
                 Rebuild the program with a current morloc (morloc make).",
                peek.envspec_version, ENVSPEC_VERSION
            )));
        }
        if peek.envspec_version > ENVSPEC_VERSION {
            return Err(DepsError::Env(format!(
                "envspec.json is version {} but this mim understands only \
                 up to version {}. Upgrade mim.",
                peek.envspec_version, ENVSPEC_VERSION
            )));
        }
        let mut spec: EnvSpec = serde_json::from_str(text).map_err(|e| {
            DepsError::Env(format!("Failed to parse envspec.json: {e}"))
        })?;
        spec.normalize_language_codes();
        Ok(spec)
    }

    /// Canonicalize language codes to the names the rest of the manager keys on.
    /// The compiler's canonical code for Julia is `jl` (its lang.yaml `name`), but
    /// the manager's toolchain / runtime / shim-marker tables all key on `julia`
    /// (the conda package name) -- the same morloc-vs-conda split as `py` vs
    /// `python`. Map `jl` -> `julia` once at ingestion so every downstream
    /// consumer (aggregate, abi-lock, shim detection) agrees; the compiler already
    /// keys its per-language `packages` under `julia`, so only the language list
    /// carries the short code.
    fn normalize_language_codes(&mut self) {
        for l in &mut self.languages {
            if l.lang == "jl" {
                l.lang = "julia".to_string();
            }
        }
    }

    /// Read and parse the `envspec.json` sitting in a program's build directory.
    pub fn read_from_build_dir(build_dir: &Path) -> Result<Self> {
        let path = build_dir.join("envspec.json");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            DepsError::Env(format!("Cannot read {}: {e}", path.display()))
        })?;
        Self::from_json(&text)
    }

    /// Build a synthetic spec carrying only language requirements. `--lang` pins
    /// enter the solve exactly like a program's declared language deps, so they
    /// are modeled as a spec with no packages/system/modules.
    pub fn from_languages(morloc_version: &str, languages: Vec<LangReq>) -> EnvSpec {
        EnvSpec {
            envspec_version: ENVSPEC_VERSION,
            morloc_version: morloc_version.to_string(),
            languages,
            packages: std::collections::BTreeMap::new(),
            system: Vec::new(),
            modules: Vec::new(),
        }
    }

    /// Fast, pre-solve reasons this program cannot build on the native backend.
    /// The native backend provides only conda-forge packages and has no build
    /// layer, so a system dependency that must come from the host or another
    /// non-conda provider is a hard blocker. A `conda-forge` provider is fine;
    /// an `unspecified` provider is left to the solve (not a fast blocker). An
    /// empty result means "no fast blocker" -- the pixi solve remains the final
    /// authority on whether the requirements resolve natively.
    pub fn native_blockers(&self) -> Vec<String> {
        self.system
            .iter()
            .filter(|s| {
                let p = s.provider.to_ascii_lowercase();
                p != "conda-forge" && p != "unspecified"
            })
            .map(|s| {
                format!(
                    "system dependency '{}' (provider: {}) cannot be provided by the \
                     native backend; use a container backend (--engine podman)",
                    s.name, s.provider
                )
            })
            .collect()
    }

    /// Resolve every local (filesystem-path) dependency in place, according to the
    /// `anchor` policy, and return the file-copy jobs the caller must perform
    /// (empty for `Live`). ONE resolver shared by both provenances so validation,
    /// path shape, and editable semantics cannot drift:
    ///
    ///   * `Live` (scratch): rewrite each local path to its canonical real path
    ///     under `root`, keeping `editable`. Valid only while that path exists
    ///     (native host / the `/work` mount) -- the transient dev-loop form.
    ///   * `Relocated` (install): copy each PYTHON local's source into the state
    ///     volume and rewrite its path to the state-relative location with
    ///     `editable = false` (a durable snapshot). Rust/other locals are left
    ///     untouched (a rust crate is compiled into the pool binary, not solved).
    ///
    /// A Python local must be pip-installable (a `pyproject.toml`/`setup.py` in the
    /// resolved source dir) under BOTH anchors -- no PYTHONPATH fallback that would
    /// escape the solved world. pixi resolves a relative path against pixi.toml's
    /// dir (not the project), so a local path must be absolute/state-relative here.
    pub fn resolve_local_deps(&mut self, anchor: &LocalAnchor) -> Result<Vec<CopyJob>> {
        let mut jobs = Vec::new();
        for (lang, reqs) in self.packages.iter_mut() {
            for r in reqs.iter_mut() {
                let PackageReq::Local { name, path, editable } = r else { continue };
                // Rust/Julia locals are owned by cargo/Pkg at pool build; under
                // Relocated they need no copy and their solved path is irrelevant
                // (dropped by aggregate), so leave them as-is.
                if lang != "py" && matches!(anchor, LocalAnchor::Relocated { .. }) {
                    continue;
                }
                let real_src = std::fs::canonicalize(anchor.root().join(&*path)).map_err(|e| {
                    DepsError::Env(format!(
                        "local dependency '{name}' path '{path}' does not resolve under \
                         project root {}: {e}",
                        anchor.root().display()
                    ))
                })?;
                if lang == "py"
                    && !real_src.join("pyproject.toml").exists()
                    && !real_src.join("setup.py").exists()
                {
                    return Err(DepsError::Env(format!(
                        "local python dependency '{name}' at {} is not pip-installable \
                         (no pyproject.toml or setup.py). Add a minimal pyproject.toml, \
                         or vendor it differently.",
                        real_src.display()
                    )));
                }
                match anchor {
                    LocalAnchor::Live { .. } => {
                        *path = real_src.to_string_lossy().into_owned();
                    }
                    LocalAnchor::Relocated { copy_root, state_root, key, .. } => {
                        let rel = format!("local/{key}/{name}");
                        jobs.push(CopyJob { src: real_src, dest: copy_root.join(&rel) });
                        *path = format!("{state_root}/{rel}");
                        *editable = false;
                    }
                }
            }
        }
        Ok(jobs)
    }

    /// Whether the spec declares any local (filesystem-path) dependency, in any
    /// language. Lets a caller skip the resolve/re-serialize round-trip entirely
    /// for the overwhelmingly common spec that has none.
    pub fn has_local_deps(&self) -> bool {
        self.packages
            .values()
            .flatten()
            .any(|r| matches!(r, PackageReq::Local { .. }))
    }

    /// Serialize back to JSON (used after `resolve_local_deps` rewrites paths, so
    /// the stored world spec carries absolute local paths).
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| DepsError::Env(format!("cannot serialize envspec: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exactly the shape emitted by the compiler's renderEnvSpec (sorted package
    // map keys; a bare language entry with neither constraint nor std). Each
    // package carries an explicit source.
    const SAMPLE: &str = r#"{"envspec_version":2,"morloc_version":"0.98.2","languages":[{"lang":"py","constraint":">=3.10"},{"lang":"cpp","std":"c++20"},{"lang":"rust"}],"packages":{"cpp":[{"name":"opencv","constraint":">=4.8","source":"conda"}],"py":[{"name":"numpy","constraint":">=2,<3","source":"conda"},{"name":"requests","constraint":"*","source":"pypi"}],"rust":[{"name":"ndarray","constraint":"0.16","source":"crates"}]},"system":[{"name":"blas","provider":"unspecified"}],"modules":[{"name":"tensor-cpp","git_hash":"abc123"}]}"#;

    #[test]
    fn from_json_normalizes_julia_code_jl_to_julia() {
        // The compiler's canonical Julia language code is `jl` (lang.yaml name),
        // but every manager table keys on `julia`; ingestion must bridge them.
        let s = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"jl"},{"lang":"py"}]}"#,
        )
        .unwrap();
        assert_eq!(s.languages[0].lang, "julia");
        assert_eq!(s.languages[1].lang, "py"); // others untouched
    }

    #[test]
    fn parses_compiler_output() {
        let s = EnvSpec::from_json(SAMPLE).unwrap();
        assert_eq!(s.envspec_version, 2);
        assert_eq!(s.morloc_version, "0.98.2");

        assert_eq!(s.languages.len(), 3);
        assert_eq!(s.languages[0].lang, "py");
        assert_eq!(s.languages[0].constraint.as_deref(), Some(">=3.10"));
        assert_eq!(s.languages[1].std.as_deref(), Some("c++20"));
        // A bare language entry has neither constraint nor std.
        assert_eq!(s.languages[2].lang, "rust");
        assert!(s.languages[2].constraint.is_none() && s.languages[2].std.is_none());

        let py = &s.packages["py"];
        assert!(matches!(py[0], PackageReq::Conda { .. }));
        assert_eq!(py[0].name(), "numpy");
        assert!(matches!(py[1], PackageReq::Pypi { .. }));
        assert_eq!(py[1].name(), "requests");
        assert!(matches!(s.packages["cpp"][0], PackageReq::Conda { .. }));
        assert!(matches!(s.packages["rust"][0], PackageReq::Crates { .. }));

        assert_eq!(s.system[0].name, "blas");
        assert_eq!(s.system[0].provider, "unspecified");
        assert_eq!(s.modules[0].name, "tensor-cpp");
        assert_eq!(s.modules[0].git_hash.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_channel_field() {
        // A conda package on a non-conda-forge channel carries the channel; a
        // channel-less conda package defaults to None (conda-forge).
        const S: &str = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"samtools","constraint":"*","source":"conda","channel":"bioconda"},{"name":"numpy","constraint":">=2","source":"conda"}]}}"#;
        let s = EnvSpec::from_json(S).unwrap();
        let py = &s.packages["py"];
        assert_eq!(py[0].name(), "samtools");
        assert_eq!(py[0].channel(), Some("bioconda"));
        assert_eq!(py[1].name(), "numpy");
        assert!(py[1].channel().is_none());
    }

    #[test]
    fn parses_local_source() {
        // A local (filesystem-path) dependency carries a path + editable flag and
        // no version/channel.
        const S: &str = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"local","path":"./vendor/mylib","editable":true}]}}"#;
        let s = EnvSpec::from_json(S).unwrap();
        match &s.packages["py"][0] {
            PackageReq::Local { name, path, editable } => {
                assert_eq!(name, "mylib");
                assert_eq!(path, "./vendor/mylib");
                assert!(*editable);
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn spec_without_channel_defaults_to_none() {
        // A package with no channel field defaults every channel to None
        // (conda-forge).
        let s = EnvSpec::from_json(SAMPLE).unwrap();
        assert_eq!(s.envspec_version, 2);
        assert!(s.packages["py"].iter().all(|p| p.channel().is_none()));
    }

    #[test]
    fn empty_collections_default() {
        let s = EnvSpec::from_json(r#"{"envspec_version":2,"morloc_version":"0.0.0"}"#).unwrap();
        assert!(s.languages.is_empty());
        assert!(s.packages.is_empty());
        assert!(s.system.is_empty());
        assert!(s.modules.is_empty());
    }

    #[test]
    fn rejects_future_version() {
        let r = EnvSpec::from_json(r#"{"envspec_version":999,"morloc_version":"9.9.9"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn rejects_stale_version() {
        // A stale envspec.json (older than the current schema, e.g. the pre-local
        // v1) must fail with a clear "rebuild the program" message, not an opaque
        // serde error over a changed/missing field.
        assert!(EnvSpec::from_json(r#"{"envspec_version":0,"morloc_version":"0.0.0"}"#).is_err());
        assert!(EnvSpec::from_json(r#"{"envspec_version":1,"morloc_version":"0.0.0"}"#).is_err());
    }

    fn spec_with_system(system_json: &str) -> EnvSpec {
        EnvSpec::from_json(&format!(
            r#"{{"envspec_version":2,"morloc_version":"0.0.0","system":{system_json}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn native_blockers_ignores_conda_and_unspecified() {
        // conda-forge is provided natively; unspecified is left to the solve.
        let s = spec_with_system(
            r#"[{"name":"blas","provider":"conda-forge"},{"name":"lapack","provider":"unspecified"}]"#,
        );
        assert!(s.native_blockers().is_empty());
    }

    #[test]
    fn native_blockers_flags_host_and_vcpkg_providers() {
        let s = spec_with_system(
            r#"[{"name":"cuda","provider":"host"},{"name":"zlib","provider":"conda-forge"},{"name":"boost","provider":"vcpkg"}]"#,
        );
        let blockers = s.native_blockers();
        assert_eq!(blockers.len(), 2);
        assert!(blockers.iter().any(|b| b.contains("cuda")));
        assert!(blockers.iter().any(|b| b.contains("boost")));
        assert!(blockers.iter().all(|b| !b.contains("zlib")));
    }

    #[test]
    fn native_blockers_empty_when_no_system_deps() {
        let s = EnvSpec::from_json(r#"{"envspec_version":2,"morloc_version":"0.0.0"}"#).unwrap();
        assert!(s.native_blockers().is_empty());
    }

    #[test]
    fn resolve_local_deps_absolutizes_and_checks_pyproject() {
        let root = tempfile::tempdir().unwrap();
        // A pip-installable python package under the project root.
        let pkg = root.path().join("vendor/mylib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("pyproject.toml"), "[project]\nname='mylib'\n").unwrap();

        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"local","path":"vendor/mylib","editable":true}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        let jobs = spec.resolve_local_deps(&LocalAnchor::Live { root: root.path() }).unwrap();
        assert!(jobs.is_empty(), "Live emits no copy jobs");
        match &spec.packages["py"][0] {
            PackageReq::Local { path, editable, .. } => {
                // The stored path is now absolute and points at the real dir;
                // Live keeps editable.
                assert!(std::path::Path::new(path).is_absolute());
                assert!(path.ends_with("vendor/mylib") || path.contains("mylib"));
                assert!(*editable);
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn resolve_local_deps_relocated_rewrites_and_emits_copy_job() {
        let root = tempfile::tempdir().unwrap();
        let copy_root = tempfile::tempdir().unwrap();
        let pkg = root.path().join("vendor/mylib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("pyproject.toml"), "[project]\nname='mylib'\n").unwrap();
        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"local","path":"vendor/mylib","editable":true}],"rust":[{"name":"mycrate","source":"local","path":"vendor/mycrate","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        let jobs = spec
            .resolve_local_deps(&LocalAnchor::Relocated {
                root: root.path(),
                copy_root: copy_root.path(),
                state_root: "/opt/morloc-state",
                key: "prog",
            })
            .unwrap();
        // Exactly one copy job (the py local); rust is left untouched, no job.
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].src, std::fs::canonicalize(&pkg).unwrap());
        assert_eq!(jobs[0].dest, copy_root.path().join("local/prog/mylib"));
        match &spec.packages["py"][0] {
            PackageReq::Local { path, editable, .. } => {
                assert_eq!(path, "/opt/morloc-state/local/prog/mylib");
                assert!(!editable, "Relocated forces plain install");
            }
            other => panic!("expected Local, got {other:?}"),
        }
        // Rust local path unchanged (compiled into the pool binary, not solved).
        match &spec.packages["rust"][0] {
            PackageReq::Local { path, .. } => assert_eq!(path, "vendor/mycrate"),
            other => panic!("expected rust Local, got {other:?}"),
        }
    }

    #[test]
    fn resolve_local_deps_rejects_non_pip_installable_python() {
        let root = tempfile::tempdir().unwrap();
        // A bare folder of .py files with no pyproject.toml / setup.py.
        std::fs::create_dir_all(root.path().join("bare")).unwrap();
        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"bare","source":"local","path":"bare","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        let err = spec
            .resolve_local_deps(&LocalAnchor::Live { root: root.path() })
            .unwrap_err();
        assert!(format!("{err}").contains("pip-installable"));
    }

    #[test]
    fn resolve_local_deps_rejects_missing_path() {
        let root = tempfile::tempdir().unwrap();
        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"rust":[{"name":"mycrate","source":"local","path":"nope/mycrate","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        assert!(spec
            .resolve_local_deps(&LocalAnchor::Live { root: root.path() })
            .is_err());
    }
}
