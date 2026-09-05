//! Shim-ABI coherence lock: pin the environment's interpreter minor versions to
//! the ones morloc's language shims were built against.
//!
//! `morloc init` produces version-embedded artifacts -- the Python binding is
//! tagged to a CPython ABI (`pymorloc.cpython-3XY-*.so`), the R binding loads a
//! specific `libR`. If a later dependency solve bumped an interpreter's MINOR
//! version those shims would no longer load, so this module derives a pin (from
//! the solved conda prefix) folded back into the requirement set: a dependency
//! demanding a different interpreter minor then fails the solve as a legible
//! conflict instead of silently breaking the shims. (Which packages are pinned,
//! and why libstdc++/glibc are not, is documented on `ABI_PACKAGES`.)

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::envspec::{EnvSpec, LangReq};

/// conda packages whose MINOR version is baked into a morloc language shim,
/// paired with the morloc language name the pin must be expressed under (so it
/// flows through the same language-runtime clamp as the rest of the toolchain).
///
/// This is the complete set of ABI-minor-sensitive shims today (python + R). It
/// is a SAFETY list: a shim language missing from it is silently unprotected -- a
/// dependency could bump its interpreter minor and break the shim with no error.
/// Any future shim that bakes an interpreter minor into its artifact (e.g. a
/// Julia binder, whose embedding C-API is minor-sensitive) MUST be added here. Do
/// not derive this from "has a versioned runtime": Rust has a runtime entry but is
/// C-ABI to libmorloc and NOT minor-sensitive, so that would over-pin it.
const ABI_PACKAGES: &[(&str, &str)] = &[("python", "py"), ("r-base", "r")];

/// The solved conda prefix under a pixi project dir. Both backends solve into the
/// `default` environment, and the container bind-mounts its `/env` from this same
/// host dir, so one derivation serves native and container alike.
pub fn conda_prefix(pixi_dir: &Path) -> PathBuf {
    pixi_dir.join(".pixi").join("envs").join("default")
}

/// Read the installed versions of the ABI-relevant packages from a solved conda
/// prefix's `conda-meta/`. Each installed package leaves a
/// `<name>-<version>-<build>.json` record carrying its name and version. Absent
/// packages (a language the env does not use) are simply not returned; an
/// unreadable prefix yields an empty map (best-effort, never fatal).
/// Walk a solved prefix's `conda-meta/`, deserializing each `<pkg>.json` record to
/// `T` and handing it to `f`. Best-effort: an unreadable prefix or a malformed
/// record is skipped, never fatal. The one home for the conda-meta directory walk,
/// shared by the ABI-version and installed-binaries probes so the boilerplate lives
/// once.
fn for_each_conda_meta<T: serde::de::DeserializeOwned>(conda_prefix: &Path, mut f: impl FnMut(T)) {
    let entries = match std::fs::read_dir(conda_prefix.join("conda-meta")) {
        Ok(e) => e,
        Err(_) => return,
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if let Ok(rec) = serde_json::from_str::<T>(&text) {
            f(rec);
        }
    }
}

fn abi_versions(conda_prefix: &Path) -> BTreeMap<String, String> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        version: String,
    }
    let mut found = BTreeMap::new();
    for_each_conda_meta(conda_prefix, |m: Meta| {
        if ABI_PACKAGES.iter().any(|(pkg, _)| *pkg == m.name) {
            found.insert(m.name, m.version);
        }
    });
    found
}

/// For each of the requested `packages` present in the prefix, its prefix-RELATIVE
/// executable paths (`bin/<tool>`), read from that package's
/// `conda-meta/<name>-<ver>-<build>.json` `files` list (matched on the record's
/// `name` field, not the filename). Scans `conda-meta/` ONCE for the whole set -- a
/// caller probing many extras pays one directory walk, not one per package. Paths
/// are relative so a caller can join the HOST conda prefix (native `ldd`) or the
/// in-container prefix (container `ldd`) as appropriate; `conda_prefix`'s
/// `conda-meta/` is always read host-side (for a container env, `<env>/pixi/...` is
/// the bind-mount source). An absent package is omitted from the map; a present
/// library-only extra maps to an empty vec. Unlike morloc's own dlopen-shims (which
/// report false "not found"s outside their load context), a normal conda CLI tool
/// resolves cleanly under `ldd`, so this is a sound probe.
pub fn package_binaries(conda_prefix: &Path, packages: &[String]) -> BTreeMap<String, Vec<String>> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        #[serde(default)]
        files: Vec<String>,
    }
    let wanted: BTreeSet<&str> = packages.iter().map(String::as_str).collect();
    let mut out = BTreeMap::new();
    for_each_conda_meta(conda_prefix, |meta: Meta| {
        if wanted.contains(meta.name.as_str()) {
            let bins = meta
                .files
                .into_iter()
                .filter(|f| f.starts_with("bin/"))
                .collect();
            out.insert(meta.name, bins);
        }
    });
    out
}

/// The `<lib> => not found` lines in `ldd` stdout -- the shared libraries a binary
/// could not resolve. Empty means the binary is loadable. Shared by the doctor
/// linkage checks so the "unresolved" parse lives in one place.
pub fn unresolved_libs(ldd_stdout: &str) -> Vec<String> {
    ldd_stdout
        .lines()
        .filter(|l| l.contains("not found"))
        .map(|l| l.trim().to_string())
        .collect()
}

/// A `>=MAJOR.MINOR,<MAJOR.(MINOR+1)` match-spec holding the minor while leaving
/// the patch free, or None if the version lacks two leading numeric components.
/// The interval form (rather than a fuzzy `MAJOR.MINOR.*` atom) matches the shape
/// `constraint::range.to_spec` already emits for the toolchain, so this merges
/// with the language's existing `>=..,<..` range as plain comma-separated
/// intervals -- no mixed fuzzy/interval spec for the conda parser to choke on.
fn minor_pin(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    Some(format!(">={major}.{minor},<{major}.{}", minor + 1))
}

/// Build the ABI-lock spec -- a languages-only `EnvSpec` pinning each shim
/// interpreter to its solved minor -- from a solved conda prefix. Returns None
/// when no ABI-relevant interpreter is present (e.g. a C++/Rust-only env), so the
/// caller can clear rather than write a spurious lock.
///
/// `windows` gives morloc's supported version range per language (short code ->
/// conda match-spec). An interpreter whose solved version falls OUTSIDE its window
/// is NOT pinned: such a version is never a valid ABI target for this morloc (a
/// shim build always clamps to the window), so it can only have arrived by being
/// pulled transitively by an unrelated package -- conda then picks the latest
/// release, which may be a version morloc bans (e.g. python 3.14 broke the CPython
/// C-API pymorloc.c uses). Pinning it would fold an unsatisfiable interval into
/// every later solve (`>=3.10,<3.14` AND `>=3.14,<3.15`); skipping it lets the
/// solve proceed and lets a re-provision clear the spurious lock.
pub fn abi_lock_spec(
    conda_prefix: &Path,
    morloc_version: &str,
    windows: &BTreeMap<String, String>,
) -> Option<EnvSpec> {
    let versions = abi_versions(conda_prefix);
    let langs: Vec<LangReq> = ABI_PACKAGES
        .iter()
        .filter_map(|(pkg, lang)| {
            let version = versions.get(*pkg)?;
            // Only pin an interpreter this morloc actually supports; an
            // out-of-window version is a transitive pull, not a shim target.
            if let Some(window) = windows.get(*lang) {
                if !crate::constraint::VersionRange::parse(window)
                    .map(|r| r.satisfies(version))
                    .unwrap_or(true)
                {
                    return None;
                }
            }
            let pin = minor_pin(version)?;
            Some(LangReq { lang: lang.to_string(), constraint: Some(pin), std: None })
        })
        .collect();
    if langs.is_empty() {
        None
    } else {
        Some(EnvSpec::from_languages(morloc_version, langs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_pin_keeps_major_minor_frees_patch() {
        assert_eq!(minor_pin("3.12.5"), Some(">=3.12,<3.13".to_string()));
        assert_eq!(minor_pin("4.3.3"), Some(">=4.3,<4.4".to_string()));
        // A bare major (no minor) cannot express an ABI minor -> no pin.
        assert_eq!(minor_pin("3"), None);
        assert_eq!(minor_pin("dev"), None);
        // A non-numeric minor is rejected (not silently pinned).
        assert_eq!(minor_pin("3.x.1"), None);
    }

    #[test]
    fn package_binaries_reads_bin_files_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path();
        let dir = prefix.join("conda-meta");
        std::fs::create_dir_all(&dir).unwrap();
        // neovim installs a bin/ tool plus a lib and a share file; only bin/ is a
        // loadability target. Matched on the record `name`, not the filename.
        std::fs::write(
            dir.join("neovim-0.10.0-h1.json"),
            r#"{"name":"neovim","version":"0.10.0","files":["bin/nvim","lib/libnvim.so","share/nvim/runtime/x"]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("ripgrep-14.1-h0.json"),
            r#"{"name":"ripgrep","version":"14.1","files":["bin/rg"]}"#,
        )
        .unwrap();

        // One scan resolves the whole set. Prefix-RELATIVE bin paths; only bin/
        // entries (not the lib/share files).
        let got = package_binaries(
            prefix,
            &["neovim".into(), "ripgrep".into(), "nvim".into(), "absent".into()],
        );
        assert_eq!(got.get("neovim"), Some(&vec!["bin/nvim".to_string()]));
        assert_eq!(got.get("ripgrep"), Some(&vec!["bin/rg".to_string()]));
        // Matched on the record `name`: `nvim` is not a record (the package is
        // `neovim`), and `absent` is not installed -- neither appears in the map.
        assert_eq!(got.get("nvim"), None);
        assert_eq!(got.get("absent"), None);
    }

    #[test]
    fn unresolved_libs_extracts_not_found_lines() {
        let ldd = "\tlinux-vdso.so.1 (0x0)\n\
                   \tlibunibilium.so.4 => not found\n\
                   \tlibc.so.6 => /usr/lib/libc.so.6 (0x0)\n\
                   \tlibtermkey.so.1 => not found\n";
        assert_eq!(
            unresolved_libs(ldd),
            vec![
                "libunibilium.so.4 => not found".to_string(),
                "libtermkey.so.1 => not found".to_string(),
            ]
        );
        // A clean binary reports nothing.
        assert!(unresolved_libs("\tlibc.so.6 => /usr/lib/libc.so.6 (0x0)\n").is_empty());
    }

    /// Write a minimal `conda-meta/<name>-<ver>-<build>.json` record.
    fn write_meta(prefix: &Path, name: &str, version: &str) {
        let dir = prefix.join("conda-meta");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}-{version}-0.json")),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
    }

    /// morloc's supported windows for the ABI interpreters (mirrors the real
    /// `requirements.yaml`: python capped below 3.14, r-base open above 4.0).
    fn windows() -> BTreeMap<String, String> {
        [("py", ">=3.10,<3.14"), ("r", ">=4.0")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn abi_lock_pins_present_interpreters_only() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path();
        write_meta(prefix, "python", "3.12.5");
        write_meta(prefix, "r-base", "4.3.3");
        // A non-ABI package must not enter the lock.
        write_meta(prefix, "numpy", "2.1.0");

        let spec = abi_lock_spec(prefix, "0.99.0", &windows()).expect("some interpreter present");
        let mut pins: Vec<(String, String)> = spec
            .languages
            .iter()
            .map(|l| (l.lang.clone(), l.constraint.clone().unwrap()))
            .collect();
        pins.sort();
        assert_eq!(
            pins,
            vec![
                ("py".to_string(), ">=3.12,<3.13".to_string()),
                ("r".to_string(), ">=4.3,<4.4".to_string()),
            ]
        );
    }

    #[test]
    fn abi_lock_skips_interpreter_outside_supported_window() {
        // A python pulled transitively (no py program, so no window clamp) resolves
        // to the latest release, 3.14 -- outside morloc's `>=3.10,<3.14` window.
        // Pinning it would poison every later solve, so it must be dropped; the
        // in-window r-base beside it is still pinned.
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path();
        write_meta(prefix, "python", "3.14.0");
        write_meta(prefix, "r-base", "4.3.3");

        let spec = abi_lock_spec(prefix, "0.99.0", &windows()).expect("r-base is in window");
        let pins: Vec<(String, String)> = spec
            .languages
            .iter()
            .map(|l| (l.lang.clone(), l.constraint.clone().unwrap()))
            .collect();
        assert_eq!(pins, vec![("r".to_string(), ">=4.3,<4.4".to_string())]);
    }

    #[test]
    fn abi_lock_absent_when_every_interpreter_out_of_window() {
        // Only an out-of-window python: nothing valid to pin -> no lock at all.
        let tmp = tempfile::tempdir().unwrap();
        write_meta(tmp.path(), "python", "3.14.0");
        assert!(abi_lock_spec(tmp.path(), "0.99.0", &windows()).is_none());
    }

    #[test]
    fn abi_lock_pins_when_no_window_known() {
        // No window for the language (absent from the table) -> pin as before,
        // rather than silently dropping an interpreter we cannot check.
        let tmp = tempfile::tempdir().unwrap();
        write_meta(tmp.path(), "python", "3.14.0");
        let spec = abi_lock_spec(tmp.path(), "0.99.0", &BTreeMap::new())
            .expect("pinned when unchecked");
        assert_eq!(spec.languages[0].constraint.as_deref(), Some(">=3.14,<3.15"));
    }

    #[test]
    fn abi_lock_absent_when_no_interpreter() {
        let tmp = tempfile::tempdir().unwrap();
        // Only a non-interpreter package: nothing to protect.
        write_meta(tmp.path(), "libstdcxx-ng", "14.1.0");
        assert!(abi_lock_spec(tmp.path(), "0.99.0", &windows()).is_none());
        // A missing prefix is also just "no lock", never a panic.
        assert!(abi_lock_spec(&tmp.path().join("nope"), "0.99.0", &windows()).is_none());
    }
}
