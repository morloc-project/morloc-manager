//! Central definition of the cross-repo contract schema versions.
//!
//! Every schema version this build produces or accepts is defined HERE, once, and
//! referenced everywhere else, so a version can never drift between its definition
//! and a check. The morloc compiler holds the matching values in `Morloc.Version`;
//! the conformance test guards the two sides against drift.

/// `envspec.json` `envspec_version` (integer): the single schema version this
/// build produces (via `EnvSpec::from_languages`) and accepts (via
/// `EnvSpec::from_json`). v2 adds the `local` package source (local
/// filesystem-path dependencies) and makes `source` a discriminated tag.
pub const ENVSPEC_VERSION: u32 = 2;

/// `morloc lang-support` `schema_version` major/minor (semver "MAJOR.MINOR").
/// A new language or field is a MINOR bump (older consumers ignore what they do
/// not know); a breaking change is a MAJOR bump. Consumers accept iff MAJOR
/// matches.
pub const LANG_SCHEMA_MAJOR: u32 = 1;
pub const LANG_SCHEMA_MINOR: u32 = 0;

/// The `schema_version` string, e.g. "1.0", derived from the numeric parts so the
/// string and the MAJOR check can never disagree.
pub fn lang_schema_version() -> String {
    format!("{LANG_SCHEMA_MAJOR}.{LANG_SCHEMA_MINOR}")
}

/// `morloc-release-manifest.json` `schema` (integer): the highest manifest schema
/// this build understands.
pub const MANIFEST_SCHEMA: u32 = 1;

/// The morloc C ABI version this build expects. Today libmorloc is built from the
/// compiler's own source at `morloc init`, so a native runtime is ABI-coherent by
/// construction -- this is RECORDED (for doctor / frozen-image coherence), not a
/// hard install gate. It becomes a live gate when libmorloc ships as a standalone
/// artifact that can pair with a mismatched compiler.
pub const ABI_VERSION: u32 = 1;
