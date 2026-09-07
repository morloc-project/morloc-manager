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
/// `default` environment, so one derivation serves native and container alike.
/// For an OCI container environment the prefix is an engine volume mounted here,
/// which means the path exists in the container but is an empty shadow on the
/// host -- read its records through [`meta_dir`], not through this.
pub fn conda_prefix(pixi_dir: &Path) -> PathBuf {
    pixi_dir.join(".pixi").join("envs").join("default")
}

/// Name of the host-readable mirror of a prefix's `conda-meta`, kept beside the
/// pixi manifest. See [`meta_dir`].
pub const CONDA_META_MIRROR: &str = ".conda-meta";

/// The directory holding a prefix's `conda-meta` records, as reachable from
/// here.
///
/// The prefix's own records win whenever this process can see them, which is
/// always for a native environment and for anything running inside a container.
/// A host looking at an OCI container environment cannot: the prefix is an
/// engine volume and the path is an empty shadow, so it falls back to the mirror
/// materialization leaves beside the pixi manifest. Preferring the real records
/// means a mirror can never shadow the truth, only stand in for it.
pub fn meta_dir(pixi_dir: &Path) -> PathBuf {
    let real = conda_prefix(pixi_dir).join("conda-meta");
    if has_records(&real) {
        return real;
    }
    pixi_dir.join(CONDA_META_MIRROR)
}

/// Whether a directory holds at least one conda record. Distinguishes a solved
/// prefix from the empty mount point an engine leaves on the host under one.
fn has_records(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("json")
            })
        })
        .unwrap_or(false)
}

/// Refresh an existing host-readable mirror of `conda_prefix`'s records, so a
/// host reading [`meta_dir`] never sees a world the prefix has moved on from.
///
/// Creating the mirror belongs to materialization, which is the step that knows
/// the prefix is not on the host; this only keeps an existing one in step. A
/// native environment therefore never pays to copy records nothing would read.
/// The replacement goes through a staging directory, so an interrupted refresh
/// cannot leave a half-written mirror behind.
pub fn refresh_conda_meta_mirror(conda_prefix: &Path, pixi_dir: &Path) -> std::io::Result<()> {
    let dest = pixi_dir.join(CONDA_META_MIRROR);
    if !dest.is_dir() {
        return Ok(());
    }
    let staging = pixi_dir.join(format!("{CONDA_META_MIRROR}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for entry in std::fs::read_dir(conda_prefix.join("conda-meta"))?.flatten() {
        if entry.path().extension().and_then(|x| x.to_str()) == Some("json") {
            std::fs::copy(entry.path(), staging.join(entry.file_name()))?;
        }
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest)
}

/// Walk a directory of `conda-meta` records (see [`meta_dir`]), deserializing
/// each `<pkg>.json` to `T` and handing it to `f`. Best-effort: an unreadable
/// directory or a malformed record is skipped, never fatal. The one home for the
/// conda-meta directory walk, shared by the ABI-version, installed-binaries and
/// case-fold probes so the boilerplate lives once.
pub(crate) fn for_each_conda_meta<T: serde::de::DeserializeOwned>(meta_dir: &Path, mut f: impl FnMut(T)) {
    let entries = match std::fs::read_dir(meta_dir) {
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

/// The installed versions of the ABI-relevant packages. Each installed package
/// leaves a `<name>-<version>-<build>.json` record carrying its name and
/// version. Absent packages (a language the env does not use) are simply not
/// returned; unreadable records yield an empty map (best-effort, never fatal).
fn abi_versions(meta_dir: &Path) -> BTreeMap<String, String> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        version: String,
    }
    let mut found = BTreeMap::new();
    for_each_conda_meta(meta_dir, |m: Meta| {
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
/// in-container prefix (container `ldd`) as appropriate; the records themselves
/// are always read from [`meta_dir`]. An absent package is omitted from the map;
/// a present library-only extra maps to an empty vec. Unlike morloc's own
/// dlopen-shims (which report false "not found"s outside their load context), a
/// normal conda CLI tool resolves cleanly under `ldd`, so this is a sound probe.
pub fn package_binaries(meta_dir: &Path, packages: &[String]) -> BTreeMap<String, Vec<String>> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        #[serde(default)]
        files: Vec<String>,
    }
    let wanted: BTreeSet<&str> = packages.iter().map(String::as_str).collect();
    let mut out = BTreeMap::new();
    for_each_conda_meta(meta_dir, |meta: Meta| {
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
    meta_dir: &Path,
    morloc_version: &str,
    windows: &BTreeMap<String, String>,
) -> Option<EnvSpec> {
    let versions = abi_versions(meta_dir);
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
        let dir = tmp.path().join("conda-meta");
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
            &dir,
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
    fn the_prefix_records_win_over_a_mirror() {
        // A mirror stands in for records this process cannot see; it must never
        // shadow records it can, or a stale copy would outrank the truth.
        let tmp = tempfile::tempdir().unwrap();
        let pixi_dir = tmp.path();
        write_meta(&conda_prefix(pixi_dir).join("conda-meta"), "python", "3.12.5");
        std::fs::create_dir_all(pixi_dir.join(CONDA_META_MIRROR)).unwrap();
        write_meta(&pixi_dir.join(CONDA_META_MIRROR), "python", "3.11.0");
        assert_eq!(meta_dir(pixi_dir), conda_prefix(pixi_dir).join("conda-meta"));
    }

    #[test]
    fn an_empty_prefix_mount_point_falls_back_to_the_mirror() {
        // What a host sees of a container environment: the prefix path exists as
        // the engine's mount point but holds nothing.
        let tmp = tempfile::tempdir().unwrap();
        let pixi_dir = tmp.path();
        std::fs::create_dir_all(conda_prefix(pixi_dir).join("conda-meta")).unwrap();
        write_meta(&pixi_dir.join(CONDA_META_MIRROR), "python", "3.12.5");
        assert_eq!(meta_dir(pixi_dir), pixi_dir.join(CONDA_META_MIRROR));
        assert_eq!(
            abi_versions(&meta_dir(pixi_dir)).get("python"),
            Some(&"3.12.5".to_string())
        );
    }

    #[test]
    fn the_mirror_is_refreshed_but_never_created() {
        let tmp = tempfile::tempdir().unwrap();
        let pixi_dir = tmp.path();
        let prefix = conda_prefix(pixi_dir);
        write_meta(&prefix.join("conda-meta"), "python", "3.12.5");

        // No mirror yet: a native environment reads the prefix directly and must
        // not be made to copy records nothing will read.
        refresh_conda_meta_mirror(&prefix, pixi_dir).unwrap();
        assert!(!pixi_dir.join(CONDA_META_MIRROR).is_dir());

        // Once materialization has established one, it is brought up to date.
        std::fs::create_dir_all(pixi_dir.join(CONDA_META_MIRROR)).unwrap();
        write_meta(&pixi_dir.join(CONDA_META_MIRROR), "python", "3.11.0");
        refresh_conda_meta_mirror(&prefix, pixi_dir).unwrap();
        let mirrored: Vec<String> = std::fs::read_dir(pixi_dir.join(CONDA_META_MIRROR))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(mirrored, vec!["python-3.12.5-0.json".to_string()]);
        assert!(!pixi_dir.join(format!("{CONDA_META_MIRROR}.new")).exists());
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

    /// Write a minimal `<name>-<ver>-<build>.json` record into a conda-meta dir.
    fn write_meta(dir: &Path, name: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
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
