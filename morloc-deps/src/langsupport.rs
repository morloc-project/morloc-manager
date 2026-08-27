//! Parser for the language-support table emitted by `morloc lang-support`.
//!
//! Cross-language contract: the schema is produced by
//! `Morloc.LangSupport.renderLangSupport` in the compiler. It records, per morloc
//! release, the conda packages morloc's own binders require and the language
//! runtime versions morloc supports -- morloc's contribution to a native
//! environment's constraints, independent of any user program's dependencies.
//! The pixi lowering (`pixi.rs`) intersects it with each program's EnvSpec.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{DepsError, Result};
use crate::layout;

fn any_constraint() -> String {
    "*".to_string()
}

/// One conda package morloc requires, with a version match-spec.
///
/// Reads both the emitted JSON (`constraint`) and the source YAML (`version`): the
/// compiler's `pkgJson` renames the field on the way out, so the alias lets the
/// same struct parse either side. An undeclared `phase` key in the YAML is simply
/// ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct PkgReq {
    pub package: String,
    #[serde(default = "any_constraint", alias = "version")]
    pub constraint: String,
    /// Optional: a binder can use it if present but does not need it for basic
    /// operation (feature/type-gated, e.g. pyarrow). Included in a full env,
    /// omitted in a minimal one.
    #[serde(default)]
    pub optional: bool,
    // `phase` (build|runtime|both) is in the JSON but unused here: the manifest
    // carries all deps; phase matters only for serve-image trimming later.
}

/// A language's versioned runtime (absent for C++, whose "version" is the
/// standard, not a package).
#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeSpec {
    pub package: String,
    #[serde(default = "any_constraint")]
    pub version: String,
    #[serde(default)]
    pub default: Option<String>,
}

/// Support for one language: its runtime (if versioned) plus morloc's extra
/// binder packages, OR -- for a script-provisioned language (futhark) -- the
/// container install script that stands in for conda entirely.
#[derive(Debug, Clone, Deserialize)]
pub struct LangEntry {
    #[serde(default)]
    pub runtime: Option<RuntimeSpec>,
    #[serde(default)]
    pub requires: Vec<PkgReq>,
    /// A container `install.sh` provisioning this language outside conda (its
    /// binary is not on conda-forge). Present => the language is provisioned by
    /// running this script in the (OCI-only) image build, not by the pixi solve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_script: Option<String>,
}

/// The whole language-support table for a morloc release.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LangSupport {
    /// Contract version, semver "MAJOR.MINOR". Absent when built from a source
    /// tree; defaults to the current schema. Enforced in `from_json`.
    #[serde(default = "crate::version::lang_schema_version")]
    pub schema_version: String,
    #[serde(default)]
    pub morloc_version: String,
    /// Core toolchain, always required (libmorloc + any shim).
    #[serde(default)]
    pub toolchain: Vec<PkgReq>,
    /// APT packages a dev environment needs at IMAGE-BUILD time, before the conda
    /// env exists, so the ghcup + stack bootstrap binaries can install and run
    /// (libgmp, libtinfo, ...). Dev-only: parsed from the source YAML's `dev-apt`
    /// key by `from_source_dir`; released envs never apt-install, so this is absent
    /// from the emitted lang-support JSON and defaults to empty there.
    #[serde(default, alias = "dev-apt")]
    pub dev_apt: Vec<String>,
    /// Conda packages a dev environment adds to the pixi solve so the compiler's
    /// Haskell dependencies can be built at RUN time. GHC links those deps through
    /// the (active) conda toolchain, so the C libs they need (gmp, zlib, ...) must
    /// live in the conda env, not just apt. Dev-only, same lifecycle as `dev_apt`:
    /// parsed from the source YAML's `dev-conda` key, absent from released JSON.
    #[serde(default, alias = "dev-conda")]
    pub dev_conda: Vec<String>,
    /// Per-language entries, keyed by canonical short name (py/r/cpp/rust).
    #[serde(default)]
    pub languages: BTreeMap<String, LangEntry>,
}

impl LangSupport {
    /// Parse the table from `morloc lang-support` JSON output, rejecting a schema
    /// MAJOR this build does not understand.
    pub fn from_json(text: &str) -> Result<Self> {
        let table: LangSupport = serde_json::from_str(text).map_err(|e| {
            DepsError::Env(format!("Failed to parse the language-support table: {e}"))
        })?;
        table.check_schema()?;
        Ok(table)
    }

    /// Reject a table whose schema MAJOR this build does not understand.
    fn check_schema(&self) -> Result<()> {
        let major = self
            .schema_version
            .split('.')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| {
                DepsError::Env(format!(
                    "lang-support schema_version '{}' is not a MAJOR.MINOR version",
                    self.schema_version
                ))
            })?;
        if major != crate::version::LANG_SCHEMA_MAJOR {
            return Err(DepsError::Env(format!(
                "lang-support schema_version {} is incompatible with this mim \
                 (supports {}.x); upgrade mim or morloc.",
                self.schema_version, crate::version::LANG_SCHEMA_MAJOR
            )));
        }
        Ok(())
    }

    /// Build the table by parsing a morloc SOURCE tree's requirement YAMLs
    /// directly, for a dev environment where no compiler exists yet to emit it.
    /// Mirrors the compiler's `Morloc.LangSupport.parseRequirements`, but takes
    /// `morloc_version` (the env's stdlib base) instead of the compiler's own
    /// version. `source` is the repo root.
    pub fn from_source_dir(source: &Path, morloc_version: &str) -> Result<Self> {
        let read = |rel: std::path::PathBuf| -> Result<String> {
            std::fs::read_to_string(&rel).map_err(|e| {
                DepsError::Env(format!("cannot read {}: {e}", rel.display()))
            })
        };
        // The shared types carry serde aliases for the YAML key spellings
        // (`version`/`dev-apt`), so they deserialize the source YAMLs directly --
        // no separate DTO layer. The core file names only `toolchain`/`dev-apt`;
        // `morloc_version` and `languages` fall to their defaults and are filled in
        // below.
        let core: LangSupport = serde_yaml::from_str(&read(source.join(layout::CORE_REQUIREMENTS))?)
            .map_err(|e| {
                DepsError::Env(format!("Failed to parse {}: {e}", layout::CORE_REQUIREMENTS))
            })?;
        let install_script = |lang: &str| -> Option<String> {
            std::fs::read_to_string(layout::lang_install_script(source, lang)).ok()
        };
        let mut languages = BTreeMap::new();
        for name in layout::LANGUAGES {
            let path = layout::lang_requirements(source, name);
            let mut entry: LangEntry = serde_yaml::from_str(&read(path)?).map_err(|e| {
                DepsError::Env(format!("Failed to parse {}/requirements.yaml: {e}", name))
            })?;
            entry.install_script = install_script(name);
            languages.insert(name.to_string(), entry);
        }
        // Script-provisioned languages have no requirements.yaml; the install.sh
        // IS their provisioning (they contribute nothing to the conda solve).
        for name in layout::SCRIPT_LANGUAGES {
            let script = read(layout::lang_install_script(source, name))?;
            languages.insert(
                name.to_string(),
                LangEntry { runtime: None, requires: Vec::new(), install_script: Some(script) },
            );
        }
        Ok(LangSupport {
            schema_version: crate::version::lang_schema_version(),
            morloc_version: morloc_version.to_string(),
            toolchain: core.toolchain,
            dev_apt: core.dev_apt,
            dev_conda: core.dev_conda,
            languages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"schema_version":"1.0","morloc_version":"0.99.0",
      "toolchain":[{"package":"c-compiler","constraint":"*","phase":"build","optional":false},
                   {"package":"rust","constraint":"*","phase":"build","optional":false}],
      "languages":{
        "py":{"runtime":{"package":"python","version":">=3.10,<3.14","default":"3.12"},
              "requires":[{"package":"numpy","constraint":">=1.22,<3","phase":"both","optional":false},
                          {"package":"setuptools","constraint":"*","phase":"build","optional":false},
                          {"package":"pyarrow","constraint":"*","phase":"runtime","optional":true}]},
        "cpp":{"runtime":null,"requires":[{"package":"cxx-compiler","constraint":"*","phase":"build","optional":false}]}
      }}"#;

    #[test]
    fn parses_the_table() {
        let t = LangSupport::from_json(SAMPLE).unwrap();
        assert_eq!(t.schema_version, "1.0");
        assert_eq!(t.morloc_version, "0.99.0");
        assert_eq!(t.toolchain.len(), 2);
        let py = t.languages.get("py").unwrap();
        assert_eq!(py.runtime.as_ref().unwrap().package, "python");
        assert_eq!(py.runtime.as_ref().unwrap().default.as_deref(), Some("3.12"));
        // pyarrow is optional; numpy/setuptools are not
        let opt = |n: &str| py.requires.iter().find(|p| p.package == n).map(|p| p.optional);
        assert_eq!(opt("pyarrow"), Some(true));
        assert_eq!(opt("numpy"), Some(false));
        // cpp has no runtime, just a requirement
        assert!(t.languages.get("cpp").unwrap().runtime.is_none());
    }

    #[test]
    fn missing_fields_default() {
        let t = LangSupport::from_json(r#"{"morloc_version":"0"}"#).unwrap();
        // Absent schema_version defaults to the current schema (from the central
        // version module), so a table that omits it still parses.
        assert_eq!(t.schema_version, "1.0");
        assert!(t.toolchain.is_empty());
        assert!(t.dev_apt.is_empty());
        assert!(t.languages.is_empty());
    }

    #[test]
    fn rejects_incompatible_major() {
        // A future MAJOR (2.x) is refused; a MINOR bump within MAJOR 1 is accepted
        // (older consumers ignore fields they do not know).
        assert!(LangSupport::from_json(r#"{"schema_version":"2.0","morloc_version":"0"}"#).is_err());
        assert!(LangSupport::from_json(r#"{"schema_version":"1.7","morloc_version":"0"}"#).is_ok());
    }

    /// Parse the actual repo `data/lang/*.yaml` (three levels up from this crate).
    /// This is the conformance guard: the Rust `from_source_dir` must agree with
    /// the Haskell `renderLangSupport` on the same inputs -- so it asserts the
    /// version->constraint DTO mapping, the dev-apt list, and language parsing
    /// against the real data. Skips gracefully if run outside the source tree.
    #[test]
    fn from_source_dir_parses_repo_yamls() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        if !root.join(crate::layout::CORE_REQUIREMENTS).is_file() {
            return; // out-of-tree build; nothing to check
        }
        let ls = LangSupport::from_source_dir(&root, "0.98.1").unwrap();
        // The stdlib base is injected, not read from the source's own version.
        assert_eq!(ls.morloc_version, "0.98.1");
        // Core toolchain carries rust. dev-apt lists the Phase-1 bootstrap libs the
        // ghcup/stack binaries need at image build (GHC itself comes from ghcup);
        // dev-conda lists the Phase-2 C libs GHC links the compiler's Haskell deps
        // against, through the active conda toolchain.
        assert!(ls.toolchain.iter().any(|p| p.package == "rust"));
        // build-essential provides the binutils (strip/ar) the Haskell build shells
        // out to; the libs are linked by the ghcup/stack bootstrap.
        assert!(ls.dev_apt.iter().any(|p| p == "build-essential"));
        assert!(ls.dev_apt.iter().any(|p| p == "libncurses-dev"));
        assert!(ls.dev_apt.iter().any(|p| p == "libgmp-dev"));
        assert!(ls.dev_conda.iter().any(|p| p == "gmp"));
        assert!(ls.dev_conda.iter().any(|p| p == "zlib"));
        // Languages parsed; py has a python runtime with a default; pyarrow is
        // optional, numpy is not.
        let py = ls.languages.get("py").unwrap();
        assert_eq!(py.runtime.as_ref().unwrap().package, "python");
        assert!(py.runtime.as_ref().unwrap().default.is_some());
        let opt = |n: &str| py.requires.iter().find(|p| p.package == n).map(|p| p.optional);
        assert_eq!(opt("pyarrow"), Some(true));
        assert_eq!(opt("numpy"), Some(false));
        // cpp has no runtime; rust is present.
        assert!(ls.languages.get("cpp").unwrap().runtime.is_none());
        assert!(ls.languages.contains_key("rust"));
        // futhark is script-provisioned: an empty conda entry (no runtime/requires)
        // carrying its install.sh, so the pixi solve skips it.
        let fut = ls.languages.get("futhark").expect("futhark entry");
        assert!(fut.runtime.is_none() && fut.requires.is_empty());
        assert!(fut.install_script.as_deref().unwrap_or("").contains("futhark-lang.org"));
        // conda languages carry no install script.
        assert!(ls.languages.get("py").unwrap().install_script.is_none());
    }
}
