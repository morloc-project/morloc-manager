//! Removing a HOST conda/mamba/pixi activation from a child environment.
//!
//! A morloc environment owns one solved conda world. Everything morloc runs
//! against that world -- the pixi solve, the activation capture, `morloc init`,
//! and every later `run` -- must see that world and nothing else. But mim is
//! launched from the user's shell, and that shell may already have a conda or
//! pixi environment activated. Its activation is not passive: it exports a
//! toolchain (`$CC`, `$AR`, `$CFLAGS`, `$LDFLAGS`, `$CONDA_BUILD_SYSROOT`, ...)
//! and puts its own `bin` on `PATH`, and conda's own activate.d scripts READ
//! those variables -- appending to the flag variables and honoring an
//! already-set `$CONDA_BUILD_SYSROOT` instead of computing one. So a foreign
//! activation does not merely sit alongside morloc's: it leaks into it, mixing
//! include/library paths from two conda worlds into one build, and can make
//! morloc's own activation scripts fail outright.
//!
//! The rule enforced here: when a foreign conda/pixi prefix is active, strip its
//! entire contribution from the child environment before morloc's own activation
//! is applied on top. Detection is by the prefixes the foreign activation itself
//! advertises (`$CONDA_PREFIX`, `$CONDA_PREFIX_<n>`, `$CONDA_EXE`,
//! `$MAMBA_ROOT_PREFIX`, `$PIXI_PROJECT_ROOT`, ...), so a host with no conda
//! active is left byte-identical to before.

use std::collections::BTreeSet;
use std::process::Command;

/// `PATH` used when scrubbing would otherwise leave it empty (every entry lived
/// inside the foreign conda prefix). A child with no `PATH` at all cannot even
/// find `bash`.
const FALLBACK_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// What to change in the child environment. Split out from the `Command`
/// mutation so the decision is testable against a synthetic environment.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScrubPlan {
    /// Variables to unset entirely.
    pub remove: Vec<String>,
    /// Variables to overwrite (path lists with the foreign entries filtered out).
    pub set: Vec<(String, String)>,
}

impl ScrubPlan {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.set.is_empty()
    }
}

/// Variables that are always shell/session state and never a conda activation's
/// to own. Exempt from the value-references-the-prefix rule below so that, for
/// instance, a conda prefix installed directly in `$HOME` cannot cause `$HOME`
/// itself to be unset.
fn is_session_var(key: &str) -> bool {
    matches!(
        key,
        "HOME" | "USER" | "LOGNAME" | "SHELL" | "TERM" | "PWD" | "OLDPWD" | "SHLVL"
            | "TMPDIR" | "TMP" | "TEMP" | "LANG" | "DISPLAY" | "HOSTNAME"
    ) || key.starts_with("LC_")
}

/// Variables a conda activation exports whose VALUE may be a bare name rather
/// than a path into the prefix (`CC=x86_64-conda-linux-gnu-gcc`,
/// `HOST=arm64-apple-darwin20.0.0`), so the value-references-the-prefix rule
/// cannot catch them. Mirrors the set `pixi::is_forced_keep` preserves on the way
/// in: what morloc's own activation is careful to KEEP is exactly what a foreign
/// activation must not be allowed to supply.
fn is_toolchain_var(key: &str) -> bool {
    matches!(
        key,
        "CC" | "CXX" | "CPP" | "FC" | "F77" | "F90"
            | "AR" | "AS" | "LD" | "NM" | "RANLIB" | "STRIP"
            | "OBJCOPY" | "OBJDUMP" | "READELF" | "ADDR2LINE" | "SIZE" | "STRINGS"
            | "GCC" | "GXX" | "GCC_AR" | "GCC_NM" | "GCC_RANLIB"
            | "CFLAGS" | "CXXFLAGS" | "CPPFLAGS" | "LDFLAGS" | "FFLAGS" | "FORTRANFLAGS"
            | "DEBUG_CFLAGS" | "DEBUG_CXXFLAGS" | "DEBUG_CPPFLAGS" | "DEBUG_FFLAGS"
            | "CMAKE_ARGS" | "MESON_ARGS" | "CONDA_BUILD_SYSROOT" | "SDKROOT"
            | "MACOSX_DEPLOYMENT_TARGET" | "OSX_DEPLOYMENT_TARGET"
            | "CONDA_TOOLCHAIN_BUILD" | "CONDA_TOOLCHAIN_HOST" | "HOST" | "BUILD"
            | "PYTHONHOME" | "R_HOME" | "_PYTHON_SYSCONFIGDATA_NAME"
    )
}

/// Bookkeeping a conda/mamba/pixi activation writes to describe ITSELF. Left
/// behind, these advertise the foreign environment to anything that inspects
/// them -- including morloc's own activation capture, which asserts on
/// `$CONDA_PREFIX`.
fn is_manager_var(key: &str) -> bool {
    // CONDA_OVERRIDE_<pkg> is not activation state: it is a deliberate solver
    // directive (overriding a detected virtual package such as __glibc/__cuda)
    // that the user sets for the solve morloc is about to run. Keep it.
    if key.starts_with("CONDA_OVERRIDE_") {
        return false;
    }
    key.starts_with("CONDA_")
        || key.starts_with("PIXI_")
        || key.starts_with("MAMBA_")
        || key.starts_with("_CE_")
}

/// Colon-separated search paths: a foreign conda contributes SOME entries, so
/// these are filtered entry-wise rather than dropped whole (the user's own
/// entries are theirs to keep).
fn is_path_list(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "LD_LIBRARY_PATH" | "LD_RUN_PATH" | "LIBRARY_PATH" | "CPATH" | "C_INCLUDE_PATH"
            | "CPLUS_INCLUDE_PATH" | "DYLD_LIBRARY_PATH" | "DYLD_FALLBACK_LIBRARY_PATH"
            | "PYTHONPATH" | "PERL5LIB" | "R_LIBS" | "R_LIBS_USER" | "R_LIBS_SITE"
            | "PKG_CONFIG_PATH" | "PKG_CONFIG_LIBDIR" | "CMAKE_PREFIX_PATH"
            | "MANPATH" | "INFOPATH" | "JULIA_DEPOT_PATH"
    )
}

/// The conda/mamba/pixi prefixes an ambient activation advertises, longest
/// first (so a nested env is stripped before its base). Empty when no such
/// environment is active, which is the signal to leave the child alone.
fn ambient_prefixes(env: &[(String, String)]) -> Vec<String> {
    let get = |k: &str| {
        env.iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };
    let mut set: BTreeSet<String> = BTreeSet::new();
    // Directly advertised prefixes, including the CONDA_PREFIX_<n> stack conda
    // pushes when environments are nested.
    for (k, v) in env {
        let named = k == "CONDA_PREFIX"
            || k == "CONDA_ROOT"
            || k == "MAMBA_ROOT_PREFIX"
            || k == "PIXI_PROJECT_ROOT"
            || k.strip_prefix("CONDA_PREFIX_").is_some_and(|n| {
                !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
            });
        if named && v.starts_with('/') {
            set.insert(v.trim_end_matches('/').to_string());
        }
    }
    // The installation root behind `conda`/`micromamba` itself: <root>/bin/conda.
    for exe in ["CONDA_EXE", "MAMBA_EXE"].into_iter().filter_map(get) {
        if let Some(root) = std::path::Path::new(exe).parent().and_then(|p| p.parent()) {
            let root = root.to_string_lossy();
            if root.starts_with('/') && root != "/" {
                set.insert(root.trim_end_matches('/').to_string());
            }
        }
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out
}

/// Is `entry` a directory inside one of `prefixes` (or a prefix itself)?
fn under_any(prefixes: &[String], entry: &str) -> bool {
    let entry = entry.trim_end_matches('/');
    prefixes
        .iter()
        .any(|p| entry == p || entry.starts_with(&format!("{p}/")))
}

/// Decide what to strip from `env` so that no ambient conda/mamba/pixi
/// activation reaches the child. Returns an empty plan when none is active.
pub fn scrub_plan(env: &[(String, String)]) -> ScrubPlan {
    let prefixes = ambient_prefixes(env);
    if prefixes.is_empty() {
        return ScrubPlan::default();
    }
    let mut plan = ScrubPlan::default();
    for (key, value) in env {
        if is_path_list(key) {
            let kept: Vec<&str> = value
                .split(':')
                .filter(|e| !e.is_empty() && !under_any(&prefixes, e))
                .collect();
            if kept.len() == value.split(':').filter(|e| !e.is_empty()).count() {
                continue;
            }
            match (kept.is_empty(), key.as_str()) {
                (true, "PATH") => plan.set.push((key.clone(), FALLBACK_PATH.to_string())),
                (true, _) => plan.remove.push(key.clone()),
                (false, _) => plan.set.push((key.clone(), kept.join(":"))),
            }
            continue;
        }
        if is_session_var(key) {
            continue;
        }
        // Anything the foreign activation set: named toolchain/bookkeeping
        // variables, plus any variable pointing into its prefix (GDAL_DATA,
        // PROJ_LIB, SSL_CERT_FILE, ... -- the long tail no list can enumerate).
        if is_manager_var(key) || is_toolchain_var(key) || under_any(&prefixes, value) {
            plan.remove.push(key.clone());
        }
    }
    plan
}

/// Strip the ambient conda/mamba/pixi activation from `cmd`'s inherited
/// environment. Call BEFORE applying morloc's own activation map, which is then
/// the only conda world the child sees. A no-op when no such environment is
/// active in this process.
pub fn scrub(cmd: &mut Command) {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let plan = scrub_plan(&env);
    for key in &plan.remove {
        cmd.env_remove(key);
    }
    for (key, value) in &plan.set {
        cmd.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn no_ambient_conda_means_no_change() {
        let e = env(&[
            ("PATH", "/usr/bin:/bin"),
            ("CC", "clang"),
            ("CFLAGS", "-O2"),
        ]);
        assert!(scrub_plan(&e).is_empty());
    }

    #[test]
    fn foreign_conda_toolchain_and_path_entries_are_stripped() {
        let e = env(&[
            ("CONDA_PREFIX", "/home/u/miniconda3/envs/foo"),
            ("CONDA_DEFAULT_ENV", "foo"),
            ("PATH", "/home/u/miniconda3/envs/foo/bin:/usr/local/bin:/usr/bin"),
            ("CC", "x86_64-conda-linux-gnu-gcc"),
            ("CXX", "x86_64-conda-linux-gnu-g++"),
            ("CFLAGS", "-march=nocona -I/home/u/miniconda3/envs/foo/include"),
            ("CONDA_BUILD_SYSROOT", "/opt/MacOSX10.9.sdk"),
            ("HOME", "/home/u"),
        ]);
        let plan = scrub_plan(&e);
        for key in ["CONDA_PREFIX", "CONDA_DEFAULT_ENV", "CC", "CXX", "CFLAGS", "CONDA_BUILD_SYSROOT"] {
            assert!(plan.remove.iter().any(|k| k == key), "{key} not removed");
        }
        assert!(!plan.remove.iter().any(|k| k == "HOME"));
        assert_eq!(
            plan.set,
            vec![("PATH".to_string(), "/usr/local/bin:/usr/bin".to_string())]
        );
    }

    #[test]
    fn variables_pointing_into_the_foreign_prefix_are_stripped_by_value() {
        let e = env(&[
            ("CONDA_PREFIX", "/opt/conda"),
            ("PATH", "/usr/bin"),
            ("SSL_CERT_FILE", "/opt/conda/ssl/cacert.pem"),
            ("PROJ_LIB", "/opt/conda/share/proj"),
            ("EDITOR", "vim"),
        ]);
        let plan = scrub_plan(&e);
        assert!(plan.remove.iter().any(|k| k == "SSL_CERT_FILE"));
        assert!(plan.remove.iter().any(|k| k == "PROJ_LIB"));
        assert!(!plan.remove.iter().any(|k| k == "EDITOR"));
    }

    #[test]
    fn nested_and_installer_root_prefixes_are_all_detected() {
        let e = env(&[
            ("CONDA_PREFIX", "/opt/conda/envs/foo"),
            ("CONDA_PREFIX_1", "/opt/conda"),
            ("CONDA_EXE", "/opt/mc3/bin/conda"),
            (
                "PATH",
                "/opt/conda/envs/foo/bin:/opt/conda/condabin:/opt/mc3/bin:/usr/bin",
            ),
        ]);
        let plan = scrub_plan(&e);
        assert_eq!(plan.set, vec![("PATH".to_string(), "/usr/bin".to_string())]);
    }

    #[test]
    fn path_emptied_by_scrubbing_falls_back_to_a_usable_default() {
        let e = env(&[
            ("CONDA_PREFIX", "/opt/conda"),
            ("PATH", "/opt/conda/bin"),
            ("PYTHONPATH", "/opt/conda/lib/python3.12/site-packages"),
        ]);
        let plan = scrub_plan(&e);
        assert_eq!(
            plan.set,
            vec![("PATH".to_string(), FALLBACK_PATH.to_string())]
        );
        // A search path with nothing left is unset, not set to an invented value.
        assert!(plan.remove.iter().any(|k| k == "PYTHONPATH"));
    }

    #[test]
    fn a_sibling_directory_sharing_a_name_prefix_is_kept() {
        let e = env(&[
            ("CONDA_PREFIX", "/opt/env"),
            ("PATH", "/opt/env2/bin:/opt/env/bin:/usr/bin"),
        ]);
        let plan = scrub_plan(&e);
        assert_eq!(
            plan.set,
            vec![("PATH".to_string(), "/opt/env2/bin:/usr/bin".to_string())]
        );
    }

    #[test]
    fn conda_override_solver_directives_survive() {
        let e = env(&[
            ("CONDA_PREFIX", "/opt/conda"),
            ("PATH", "/usr/bin"),
            ("CONDA_OVERRIDE_GLIBC", "2.17"),
        ]);
        let plan = scrub_plan(&e);
        assert!(plan.remove.iter().any(|k| k == "CONDA_PREFIX"));
        assert!(!plan.remove.iter().any(|k| k == "CONDA_OVERRIDE_GLIBC"));
    }

    #[test]
    fn a_relative_conda_prefix_is_not_treated_as_a_prefix() {
        // Only absolute prefixes are actionable; a relative value would match
        // far too much under `starts_with`.
        let e = env(&[("CONDA_PREFIX", "envs/foo"), ("PATH", "/usr/bin")]);
        assert!(scrub_plan(&e).is_empty());
    }
}
