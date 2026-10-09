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

/// The pixi-manifest-relative directory under which installed local Python
/// dependencies are snapshotted: `local/<name>/<content hash>`. Inside the pixi
/// project dir so the path is the same string on the host, in a container's
/// `/env` mount, and in a frozen image, and pixi records it relatively in the
/// lock.
pub const VENDOR_DIR: &str = "local";

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
    /// A snapshot of a local Python dependency taken when its program was
    /// installed. `path` is relative to the pixi manifest dir
    /// (`local/<name>/<content hash>`); the install is never editable. Written
    /// only by the environment manager into its requirements store, never by
    /// the compiler.
    Vendored { name: String, path: String },
}

impl PackageReq {
    /// The package name, regardless of source.
    pub fn name(&self) -> &str {
        match self {
            PackageReq::Conda { name, .. }
            | PackageReq::Pypi { name, .. }
            | PackageReq::Crates { name, .. }
            | PackageReq::Cran { name, .. }
            | PackageReq::Bioconductor { name, .. }
            | PackageReq::Local { name, .. }
            | PackageReq::Vendored { name, .. } => name,
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
/// canonical morloc language names ("py", "r", "cpp", "rust").
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
        serde_json::from_str(text).map_err(|e| {
            DepsError::Env(format!("Failed to parse envspec.json: {e}"))
        })
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

    /// Resolve every local (filesystem-path) dependency in place to its canonical
    /// real path under `root`, keeping `editable`: the live dev-loop form, valid
    /// only while that path exists (a native host, or the `/work` mount). An
    /// installed program's Python locals are snapshotted instead (see
    /// `PackageReq::Vendored`), which the environment manager does before calling
    /// this.
    pub fn resolve_local_deps(&mut self, root: &Path) -> Result<()> {
        for (lang, reqs) in self.packages.iter_mut() {
            for r in reqs.iter_mut() {
                let PackageReq::Local { name, path, .. } = r else { continue };
                *path = local_source(root, lang, name, path)?
                    .to_string_lossy()
                    .into_owned();
            }
        }
        Ok(())
    }

    /// Whether the spec declares a Python local (filesystem-path) dependency
    /// that has not been snapshotted.
    pub fn has_py_locals(&self) -> bool {
        self.packages
            .get("py")
            .map(|reqs| reqs.iter().any(|r| matches!(r, PackageReq::Local { .. })))
            .unwrap_or(false)
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

    /// Serialize back to JSON (used after local paths were resolved or
    /// snapshotted, so the stored world spec never carries a module-relative path).
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|e| DepsError::Env(format!("cannot serialize envspec: {e}")))
    }
}

/// The canonical real directory of a local dependency declared as `path`
/// (module-relative) under `root`, checked for existence. A Python local must be
/// pip-installable (a `pyproject.toml` or `setup.py` there): there is no
/// PYTHONPATH fallback that would escape the solved world.
pub fn local_source(root: &Path, lang: &str, name: &str, path: &str) -> Result<PathBuf> {
    let real = std::fs::canonicalize(root.join(path)).map_err(|e| {
        DepsError::Env(format!(
            "local dependency '{name}' path '{path}' does not resolve under project root {}: {e}",
            root.display()
        ))
    })?;
    if lang == "py" && !real.join("pyproject.toml").exists() && !real.join("setup.py").exists() {
        return Err(DepsError::Env(format!(
            "local python dependency '{name}' at {} is not pip-installable (no pyproject.toml \
             or setup.py). Add a minimal pyproject.toml, or vendor it differently.",
            real.display()
        )));
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exactly the shape emitted by the compiler's renderEnvSpec (sorted package
    // map keys; a bare language entry with neither constraint nor std). Each
    // package carries an explicit source.
    const SAMPLE: &str = r#"{"envspec_version":2,"morloc_version":"0.98.2","languages":[{"lang":"py","constraint":">=3.10"},{"lang":"cpp","std":"c++20"},{"lang":"rust"}],"packages":{"cpp":[{"name":"opencv","constraint":">=4.8","source":"conda"}],"py":[{"name":"numpy","constraint":">=2,<3","source":"conda"},{"name":"requests","constraint":"*","source":"pypi"}],"rust":[{"name":"ndarray","constraint":"0.16","source":"crates"}]},"system":[{"name":"blas","provider":"unspecified"}],"modules":[{"name":"tensor-cpp","git_hash":"abc123"}]}"#;

    #[test]
    fn rejects_pkg_source() {
        let r = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"py"}],"packages":{"py":[{"name":"x","constraint":"*","source":"pkg"}]}}"#,
        );
        assert!(r.is_err());
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
        spec.resolve_local_deps(root.path()).unwrap();
        match &spec.packages["py"][0] {
            PackageReq::Local { path, editable, .. } => {
                // The stored path is now the real dir; editable is kept.
                assert_eq!(
                    std::path::Path::new(path),
                    std::fs::canonicalize(&pkg).unwrap()
                );
                assert!(*editable);
            }
            other => panic!("expected Local, got {other:?}"),
        }
    }

    #[test]
    fn vendored_round_trips_and_is_not_a_py_local() {
        // The manager's snapshot form: tagged `vendored`, manifest-relative path,
        // no editable flag. It is no longer an unresolved local.
        const S: &str = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"vendored","path":"local/mylib/0123456789ab"}]}}"#;
        let spec = EnvSpec::from_json(S).unwrap();
        match &spec.packages["py"][0] {
            PackageReq::Vendored { name, path } => {
                assert_eq!(name, "mylib");
                assert_eq!(path, "local/mylib/0123456789ab");
            }
            other => panic!("expected Vendored, got {other:?}"),
        }
        assert!(!spec.has_py_locals());
        assert!(!spec.has_local_deps());
        let again = EnvSpec::from_json(&spec.to_json().unwrap()).unwrap();
        assert_eq!(again.packages["py"][0], spec.packages["py"][0]);
    }

    #[test]
    fn resolve_local_deps_rejects_non_pip_installable_python() {
        let root = tempfile::tempdir().unwrap();
        // A bare folder of .py files with no pyproject.toml / setup.py.
        std::fs::create_dir_all(root.path().join("bare")).unwrap();
        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"bare","source":"local","path":"bare","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        let err = spec.resolve_local_deps(root.path()).unwrap_err();
        assert!(format!("{err}").contains("pip-installable"));
    }

    #[test]
    fn resolve_local_deps_rejects_missing_path() {
        let root = tempfile::tempdir().unwrap();
        let s = r#"{"envspec_version":2,"morloc_version":"0","packages":{"rust":[{"name":"mycrate","source":"local","path":"nope/mycrate","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(s).unwrap();
        assert!(spec.resolve_local_deps(root.path()).is_err());
    }
}
