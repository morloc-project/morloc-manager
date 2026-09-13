//! Cut the build toolchain out of a solved conda prefix.
//!
//! A frozen program never compiles anything, but the prefix it was built in
//! holds everything the environment declared, and the largest part of that
//! is what built it: rustc, the GNU compilers with their sysroot, binutils,
//! make, git. The cut is computed over the prefix's own `conda-meta` records,
//! which carry each package's dependencies and its file list, so the result
//! is exact for the bytes that were installed rather than for what a
//! manifest asked for.
//!
//! The walk starts from the packages the manifest names, minus the ones on
//! the build-only list, and keeps everything reachable from there without
//! passing through a build-only package. Whatever is left is removed. That
//! reaches further than the manifest alone can: conda-forge's `r-base`
//! depends on the full GNU toolchain so that `install.packages` can compile
//! from source, and a manifest cannot exclude a dependency of something it
//! keeps. Deleting the toolchain from the graph before the walk drops it and
//! what only it pulled in, while libraries the compilers share with the
//! runtime (libgfortran5, libstdcxx, libgcc) stay because r-base and python
//! reach them directly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

/// One installed package, as its `conda-meta` record describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondaRecord {
    pub name: String,
    /// The record's own file name under `conda-meta/`, so a consumer of the
    /// plan can check the record exists where the plan is applied.
    pub record_file: String,
    /// Names only; the version constraint is irrelevant to reachability.
    pub depends: Vec<String>,
    /// Prefix-relative paths the package installed.
    pub files: Vec<String>,
}

/// Read every record under a `conda-meta` directory. Best-effort like the
/// other conda-meta walks: a malformed record is skipped.
pub fn read_records(meta_dir: &Path) -> Vec<CondaRecord> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        #[serde(default)]
        depends: Vec<String>,
        #[serde(default)]
        files: Vec<String>,
    }
    let entries = match std::fs::read_dir(meta_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(meta) = serde_json::from_str::<Meta>(&text) else { continue };
        let record_file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(CondaRecord {
            name: meta.name,
            record_file,
            depends: meta.depends.iter().map(|d| dependency_name(d)).collect(),
            files: meta.files,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The package a conda match-spec names: the first whitespace-delimited
/// token (`"libgcc >=13"`, `"python_abi 3.13.* *_cp313"`).
fn dependency_name(spec: &str) -> String {
    spec.split_whitespace().next().unwrap_or("").to_string()
}

/// The conda packages a rendered `pixi.toml` names under `[dependencies]`.
/// The manifest is the manager's own rendering -- one quoted key per line --
/// so the section is read line by line rather than through a TOML parser.
pub fn manifest_dependencies(pixi_toml: &str) -> Vec<String> {
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in pixi_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line == "[dependencies]";
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line.split('=').next().unwrap_or("").trim().trim_matches('"');
        if !key.is_empty() {
            out.push(key.to_string());
        }
    }
    out
}

/// Packages that exist to build things. Matched on the name's stem, since the
/// GNU toolchain packages carry the platform in their name
/// (`gcc_impl_linux-64`, `sysroot_linux-aarch64`). `rust` matches itself and
/// `rust-std-<triple>`.
const BUILD_ONLY_STEMS: &[&str] = &[
    "rust",
    "c-compiler",
    "cxx-compiler",
    "fortran-compiler",
    "compilers",
    "make",
    "cmake",
    "pkg-config",
    "git",
    "gcc_",
    "gxx_",
    "gfortran_",
    "gcc_impl_",
    "gxx_impl_",
    "gfortran_impl_",
    "binutils_",
    "binutils_impl_",
    "sysroot_",
    "kernel-headers_",
    "libstdcxx-devel_",
    "libgcc-devel_",
];

/// Whether a package is on the build-only list.
pub fn is_build_only(name: &str) -> bool {
    BUILD_ONLY_STEMS.iter().any(|stem| {
        if let Some(prefix) = stem.strip_suffix('_') {
            // A platform-suffixed name: `gcc_impl_linux-64` for stem `gcc_impl_`.
            name.starts_with(stem) && name.len() > prefix.len() + 1
        } else {
            name == *stem || name.starts_with(&format!("{stem}-std-"))
        }
    })
}

/// What a prune removes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrunePlan {
    /// Packages kept, sorted.
    pub kept: Vec<String>,
    /// Packages removed, sorted.
    pub removed: Vec<String>,
    /// The `conda-meta` record file of each removed package.
    pub removed_records: Vec<String>,
    /// Every prefix-relative file the removed packages installed, plus their
    /// records, sorted. Applying the plan is deleting these.
    pub removed_files: Vec<String>,
}

/// Compute the prune: keep what `roots` reach without crossing a build-only
/// package, remove the rest. A root that is itself build-only is dropped
/// silently, since the manifest names the toolchain and that is the point. A
/// root the records do not know is an error: the manifest and the prefix
/// disagree, and a plan built on that would delete something the manifest
/// still wants.
pub fn plan_prune(roots: &[String], records: &[CondaRecord]) -> Result<PrunePlan, String> {
    let by_name: BTreeMap<&str, &CondaRecord> =
        records.iter().map(|r| (r.name.as_str(), r)).collect();
    let mut keep: BTreeSet<&str> = BTreeSet::new();
    let mut todo: Vec<&str> = Vec::new();
    for root in roots {
        if is_build_only(root) {
            continue;
        }
        if !by_name.contains_key(root.as_str()) {
            return Err(format!(
                "the manifest names '{root}' but the prefix has no record of it; the \
                 environment's toolchain does not match its lock"
            ));
        }
        todo.push(root.as_str());
    }
    while let Some(name) = todo.pop() {
        if is_build_only(name) || !keep.insert(name) {
            continue;
        }
        if let Some(rec) = by_name.get(name) {
            for dep in &rec.depends {
                if by_name.contains_key(dep.as_str()) {
                    todo.push(dep.as_str());
                }
            }
        }
    }
    let mut plan = PrunePlan::default();
    for rec in records {
        if keep.contains(rec.name.as_str()) {
            plan.kept.push(rec.name.clone());
        } else {
            plan.removed.push(rec.name.clone());
            plan.removed_records.push(rec.record_file.clone());
            plan.removed_files.extend(rec.files.iter().cloned());
            plan.removed_files
                .push(format!("conda-meta/{}", rec.record_file));
        }
    }
    plan.kept.sort();
    plan.removed.sort();
    plan.removed_records.sort();
    plan.removed_files.sort();
    plan.removed_files.dedup();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, depends: &[&str], files: &[&str]) -> CondaRecord {
        CondaRecord {
            name: name.to_string(),
            record_file: format!("{name}-1.0-0.json"),
            depends: depends.iter().map(|d| d.to_string()).collect(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// The shape conda-forge actually ships: r-base depends on the GNU
    /// toolchain, python and numpy on runtime libraries the toolchain also
    /// depends on.
    fn prefix() -> Vec<CondaRecord> {
        vec![
            rec("r-base", &["libgfortran5", "gcc_impl_linux-64", "gfortran_impl_linux-64", "libgcc"], &["bin/R", "lib/R/lib/libR.so"]),
            rec("python", &["libgcc", "ld_impl_linux-64"], &["bin/python3"]),
            rec("numpy", &["python", "libstdcxx"], &["lib/python3.13/site-packages/numpy/__init__.py"]),
            rec("libgfortran5", &["libgcc"], &["lib/libgfortran.so.5"]),
            rec("libstdcxx", &["libgcc"], &["lib/libstdc++.so.6"]),
            rec("libgcc", &[], &["lib/libgcc_s.so.1"]),
            rec("ld_impl_linux-64", &[], &["bin/ld"]),
            rec("gcc_impl_linux-64", &["libsanitizer", "sysroot_linux-64", "libgcc", "libstdcxx"], &["bin/gcc"]),
            rec("gfortran_impl_linux-64", &["gcc_impl_linux-64", "libgfortran5"], &["bin/gfortran"]),
            rec("libsanitizer", &["libgcc"], &["lib/libasan.so"]),
            rec("sysroot_linux-64", &["kernel-headers_linux-64"], &["x86_64-conda-linux-gnu/sysroot/lib/libc.so.6"]),
            rec("kernel-headers_linux-64", &[], &["x86_64-conda-linux-gnu/sysroot/usr/include/linux/a.h"]),
            rec("rust", &["rust-std-x86_64-unknown-linux-gnu", "libgcc"], &["bin/rustc"]),
            rec("rust-std-x86_64-unknown-linux-gnu", &[], &["lib/rustlib/x.rlib"]),
            rec("git", &["perl"], &["bin/git"]),
            rec("perl", &[], &["bin/perl"]),
            rec("c-compiler", &["gcc_linux-64"], &[]),
            rec("gcc_linux-64", &["gcc_impl_linux-64", "binutils_linux-64"], &["etc/conda/activate.d/activate-gcc_linux-64.sh"]),
            rec("binutils_linux-64", &["binutils_impl_linux-64"], &[]),
            rec("binutils_impl_linux-64", &["ld_impl_linux-64"], &["bin/as"]),
        ]
    }

    fn roots(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// The toolchain goes, and with it what only the toolchain pulled in;
    /// the runtime libraries the toolchain shares with r-base stay.
    #[test]
    fn the_compiler_chain_behind_r_base_is_cut_but_its_runtime_libs_stay() {
        let plan = plan_prune(&roots(&["r-base", "python", "numpy", "rust", "c-compiler", "git"]), &prefix()).unwrap();
        assert_eq!(
            plan.kept,
            vec!["ld_impl_linux-64", "libgcc", "libgfortran5", "libstdcxx", "numpy", "python", "r-base"]
        );
        for gone in ["gcc_impl_linux-64", "gfortran_impl_linux-64", "libsanitizer", "sysroot_linux-64", "kernel-headers_linux-64", "rust", "rust-std-x86_64-unknown-linux-gnu", "git", "perl", "c-compiler", "gcc_linux-64", "binutils_linux-64", "binutils_impl_linux-64"] {
            assert!(plan.removed.contains(&gone.to_string()), "{gone} kept");
        }
        assert!(plan.removed_files.contains(&"bin/gfortran".to_string()));
        assert!(plan.removed_files.contains(&"conda-meta/rust-1.0-0.json".to_string()));
        assert!(!plan.removed_files.contains(&"lib/libgfortran.so.5".to_string()));
        assert!(plan.removed_records.contains(&"perl-1.0-0.json".to_string()));
    }

    /// A package only the toolchain wanted is not kept just because nothing
    /// else depends on it either.
    #[test]
    fn an_orphan_of_the_toolchain_is_removed() {
        let plan = plan_prune(&roots(&["python"]), &prefix()).unwrap();
        assert!(plan.removed.contains(&"libsanitizer".to_string()));
        assert!(plan.removed.contains(&"perl".to_string()));
        assert_eq!(plan.kept, vec!["ld_impl_linux-64", "libgcc", "python"]);
    }

    #[test]
    fn a_root_the_prefix_lacks_is_an_error() {
        let err = plan_prune(&roots(&["python", "opencv"]), &prefix()).unwrap_err();
        assert!(err.contains("opencv"), "{err}");
    }

    #[test]
    fn build_only_names_match_on_their_platform_stem() {
        for name in ["rust", "rust-std-x86_64-unknown-linux-gnu", "gcc_impl_linux-64", "sysroot_linux-aarch64", "binutils_linux-64", "libstdcxx-devel_linux-64", "git", "make", "cxx-compiler"] {
            assert!(is_build_only(name), "{name}");
        }
        for name in ["libgcc", "libstdcxx", "libgfortran5", "python", "r-base", "gsl", "gettext", "rustworkx", "git-lfs-nope", "makeflow"] {
            assert!(!is_build_only(name), "{name}");
        }
    }

    #[test]
    fn manifest_dependencies_are_the_keys_of_one_section() {
        let toml = r#"[workspace]
name = "morloc-env"
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
"c-compiler" = "<2"
"numpy" = "<3,>=1.22"
"python" = "<3.14,>=3.13"
"r-base" = { version = ">=4.5,<4.6", channel = "conda-forge" }

[pypi-dependencies]
"requests" = "*"
"#;
        assert_eq!(manifest_dependencies(toml), vec!["c-compiler", "numpy", "python", "r-base"]);
    }

    #[test]
    fn records_are_read_from_conda_meta() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("numpy-2.1.0-py313h1.json"),
            r#"{"name":"numpy","version":"2.1.0","depends":["python >=3.13,<3.14.0a0","libstdcxx >=13"],"files":["lib/python3.13/site-packages/numpy/__init__.py"]}"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("history"), "").unwrap();
        let got = read_records(tmp.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "numpy");
        assert_eq!(got[0].record_file, "numpy-2.1.0-py313h1.json");
        assert_eq!(got[0].depends, vec!["python", "libstdcxx"]);
    }
}
