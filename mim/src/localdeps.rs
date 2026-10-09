//! Snapshots of local (filesystem-path) Python dependencies for installed
//! programs.
//!
//! A scratch build (`morloc make` in the env) keeps a live editable install of
//! the source where it is (`EnvSpec::resolve_local_deps`). An installed program
//! gets a durable copy instead: its source is copied into the env's pixi
//! project dir at `pixi/local/<name>/<content hash>/` and the spec entry becomes
//! `PackageReq::Vendored` with that manifest-relative path. Inside the pixi
//! project dir, the path is the same string on the host, in a container's
//! `/env` mount and in a frozen image, and pixi records it relatively in the
//! lock. Content addressing makes a re-install of unchanged source a no-op and
//! lets two programs vendoring identical source share one requirement.
//!
//! Rust local crates are compiled into the pool binary at build and are never
//! copied.

use std::path::{Path, PathBuf};

use morloc_deps::envspec::{local_source, EnvSpec, PackageReq, VENDOR_DIR};
use sha2::{Digest, Sha256};

use crate::error::{ManagerError, Result};
use crate::provision::copy_dir_excluding;

/// Directory names not worth copying into a snapshot.
const SKIP_DIRS: &[&str] = &["target", ".git", "__pycache__", ".venv", "node_modules", "build"];

/// Hex digits of the content hash that name a snapshot directory.
const HASH_LEN: usize = 12;

/// Whether the spec declares a Python local dependency that has not been
/// snapshotted.
pub fn has_py_locals(spec: &EnvSpec) -> bool {
    spec.has_py_locals()
}

/// Remove Python local deps from a spec. Used for a solve that cannot carry
/// them: the pre-build install solve (the snapshot is only made once the
/// program builds) and the manager's own solve of a scratch spec (its live
/// path is valid only where the build ran).
pub fn strip_py_locals(spec: &mut EnvSpec) {
    if let Some(reqs) = spec.packages.get_mut("py") {
        reqs.retain(|r| !matches!(r, PackageReq::Local { .. }));
    }
}

/// Snapshot every Python local of `spec` (declared relative to the project
/// `root`) into `pixi_dir` and rewrite each to its `Vendored` form.
pub fn vendor_py_locals(spec: &mut EnvSpec, root: &Path, pixi_dir: &Path) -> Result<()> {
    let Some(reqs) = spec.packages.get_mut("py") else { return Ok(()) };
    for r in reqs.iter_mut() {
        let PackageReq::Local { name, path, .. } = r else { continue };
        let src = local_source(root, "py", name, path)?;
        let rel = vendor_snapshot(&src, name, pixi_dir)?;
        *r = PackageReq::Vendored { name: name.clone(), path: rel };
    }
    Ok(())
}

/// A Python distribution name (PEP 508): ASCII letters and digits, with `.`,
/// `_` and `-` inside. It is used as a path component, so nothing else passes.
fn validate_distribution_name(name: &str) -> Result<()> {
    let edge_ok = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let ok = edge_ok(name.chars().next())
        && edge_ok(name.chars().last())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(ManagerError::EnvError(format!(
            "local Python dependency name '{name}' is not a valid distribution name \
             (letters, digits, and '.', '_' or '-' between them)"
        )))
    }
}

/// Copy the pip-installable source at `src` into
/// `<pixi_dir>/local/<name>/<hash>/` and return that manifest-relative path.
/// The copy is built in a sibling temp dir, hashed, and renamed into place, so
/// a crash never leaves a half-populated snapshot and a snapshot with the same
/// content (already present, or produced by a concurrent install) is reused.
pub fn vendor_snapshot(src: &Path, name: &str, pixi_dir: &Path) -> Result<String> {
    validate_distribution_name(name)?;
    let parent = pixi_dir.join(VENDOR_DIR).join(name);
    std::fs::create_dir_all(&parent)
        .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", parent.display())))?;
    let tmp = parent.join(format!(".tmp.{}", std::process::id()));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    copy_dir_excluding(src, &tmp, SKIP_DIRS, true)?;
    let hash = tree_hash(&tmp)?;
    let dest = parent.join(&hash);
    if dest.is_dir() {
        let _ = std::fs::remove_dir_all(&tmp);
    } else if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_dir_all(&tmp);
        if !dest.is_dir() {
            return Err(ManagerError::EnvError(format!(
                "cannot install local dep into {}: {e}",
                dest.display()
            )));
        }
    }
    Ok(format!("{VENDOR_DIR}/{name}/{hash}"))
}

/// A content hash of a directory tree: every regular file's relative path and
/// bytes, in sorted order.
fn tree_hash(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_dir(dir, dir, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize())[..HASH_LEN].to_string())
}

fn hash_dir(root: &Path, dir: &Path, hasher: &mut Sha256) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| ManagerError::EnvError(format!("cannot read {}: {e}", dir.display())))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            hash_dir(root, &path, hasher)?;
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = std::fs::read(&path)
            .map_err(|e| ManagerError::EnvError(format!("cannot read {}: {e}", path.display())))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(())
}

/// How long a snapshot is left alone after it was taken, so a sweep never
/// removes one a concurrent install has copied but not yet recorded in the
/// store.
const SWEEP_GRACE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Remove every snapshot under `<pixi_dir>/local` older than [`SWEEP_GRACE`]
/// that no spec in `specs` references, returning the manifest-relative paths
/// removed. Temp dirs of an in-flight copy are left alone.
pub fn sweep_unreferenced_snapshots(pixi_dir: &Path, specs: &[EnvSpec]) -> Result<Vec<String>> {
    sweep_snapshots(pixi_dir, specs, SWEEP_GRACE)
}

fn sweep_snapshots(
    pixi_dir: &Path,
    specs: &[EnvSpec],
    grace: std::time::Duration,
) -> Result<Vec<String>> {
    let referenced: std::collections::HashSet<&str> = specs
        .iter()
        .flat_map(|s| s.packages.values().flatten())
        .filter_map(|r| match r {
            PackageReq::Vendored { path, .. } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    let mut removed = Vec::new();
    let vendor_root = pixi_dir.join(VENDOR_DIR);
    let Ok(names) = std::fs::read_dir(&vendor_root) else { return Ok(removed) };
    for name_entry in names.flatten() {
        let name_dir = name_entry.path();
        if !name_dir.is_dir() {
            continue;
        }
        let name = name_entry.file_name().to_string_lossy().into_owned();
        let Ok(hashes) = std::fs::read_dir(&name_dir) else { continue };
        for hash_entry in hashes.flatten() {
            let hash = hash_entry.file_name().to_string_lossy().into_owned();
            if hash.starts_with('.') || !hash_entry.path().is_dir() {
                continue;
            }
            let rel = format!("{VENDOR_DIR}/{name}/{hash}");
            if referenced.contains(rel.as_str()) {
                continue;
            }
            let fresh = hash_entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|age| age < grace)
                .unwrap_or(true);
            if fresh {
                continue;
            }
            std::fs::remove_dir_all(hash_entry.path()).map_err(|e| {
                ManagerError::EnvError(format!(
                    "cannot remove unreferenced snapshot {}: {e}",
                    hash_entry.path().display()
                ))
            })?;
            removed.push(rel);
        }
        if std::fs::read_dir(&name_dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&name_dir);
        }
    }
    Ok(removed)
}

/// Where an environment kept an installed program's local-dep copies before
/// snapshots moved into the pixi project dir: `<env_dir>/local/<key>/<name>`.
fn legacy_copy(env_dir: &Path, key: &str, name: &str) -> PathBuf {
    env_dir.join("local").join(key).join(name)
}

/// Bring an installed program's spec, read from its build dir with the
/// compiler's module-relative local paths, to the store form: every Python
/// local becomes a snapshot under the env's pixi dir. The source is taken from,
/// in order, a copy an earlier mim made under `<env_dir>/local/<key>/`, the
/// program's source mirror `<env_dir>/exe/<key>/`, or an absolute path the spec
/// already carries; a program whose source none of these hold must be
/// reinstalled.
pub fn repair_installed_py_locals(env_dir: &Path, key: &str, spec: &mut EnvSpec) -> Result<()> {
    let pixi_dir = env_dir.join("pixi");
    let mirror = env_dir.join("exe").join(key);
    let Some(reqs) = spec.packages.get_mut("py") else { return Ok(()) };
    for r in reqs.iter_mut() {
        let PackageReq::Local { name, path, .. } = r else { continue };
        let legacy = legacy_copy(env_dir, key, name);
        let src = if legacy.is_dir() {
            Some(legacy.clone())
        } else if let Ok(real) = local_source(&mirror, "py", name, path) {
            Some(real)
        } else if Path::new(path).is_absolute() {
            local_source(Path::new("/"), "py", name, path).ok()
        } else {
            None
        };
        let Some(src) = src else {
            return Err(ManagerError::EnvError(format!(
                "installed program '{key}' depends on the local python package '{name}' \
                 (declared at '{path}'), but this environment holds no copy of its source. \
                 Reinstall the program from its source directory: mim install <dir>"
            )));
        };
        let rel = vendor_snapshot(&src, name, &pixi_dir)?;
        if src == legacy {
            let _ = std::fs::remove_dir_all(&legacy);
        }
        *r = PackageReq::Vendored { name: name.clone(), path: rel };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with_py_local(path: &str) -> EnvSpec {
        let j = format!(
            r#"{{"envspec_version":2,"morloc_version":"0","packages":{{"py":[{{"name":"mylib","source":"local","path":"{path}","editable":true}}]}}}}"#
        );
        EnvSpec::from_json(&j).unwrap()
    }

    fn write_pkg(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("pyproject.toml"), "[project]\nname='mylib'\n").unwrap();
        std::fs::write(dir.join("v.py"), body).unwrap();
    }

    fn vendored_path(spec: &EnvSpec) -> String {
        match &spec.packages["py"][0] {
            PackageReq::Vendored { path, .. } => path.clone(),
            other => panic!("expected Vendored, got {other:?}"),
        }
    }

    /// A package name becomes a directory under the pixi project, so one that
    /// is not a plain Python distribution name is refused before anything is
    /// written.
    #[test]
    fn vendor_refuses_a_name_that_is_not_a_distribution_name() {
        let root = tempfile::tempdir().unwrap();
        let pixi = tempfile::tempdir().unwrap();
        let pkg = root.path().join("mylib");
        write_pkg(&pkg, "x = 1\n");
        for bad in ["../escape", "a/b", "..", ".", "", "-x", "x-", "a b"] {
            assert!(vendor_snapshot(&pkg, bad, &pixi.path().join("pixi")).is_err(), "{bad:?}");
        }
        assert!(!pixi.path().join("pixi").exists());
        assert!(vendor_snapshot(&pkg, "My_lib.2-x", &pixi.path().join("pixi")).is_ok());
    }

    #[test]
    fn strip_removes_py_locals_only() {
        let j = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"local","path":"vendor/mylib","editable":true}],"rust":[{"name":"mycrate","source":"local","path":"vendor/mycrate","editable":false}]}}"#;
        let mut spec = EnvSpec::from_json(j).unwrap();
        assert!(has_py_locals(&spec));
        strip_py_locals(&mut spec);
        assert!(!has_py_locals(&spec));
        // Rust local remains (it is compiled at build, not stripped).
        assert!(spec.packages["rust"].iter().any(|r| matches!(r, PackageReq::Local { .. })));
    }

    #[test]
    fn vendor_copies_under_pixi_dir_and_derefs_symlinked_subdir() {
        let root = tempfile::tempdir().unwrap();
        let pixi = tempfile::tempdir().unwrap();
        // A package whose subpackage is a symlink to an external dir.
        let pkg = root.path().join("vendor/mylib");
        write_pkg(&pkg, "V=1\n");
        let ext = tempfile::tempdir().unwrap();
        std::fs::write(ext.path().join("sub.py"), "x=1\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(ext.path(), pkg.join("sub")).unwrap();

        let mut spec = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut spec, root.path(), pixi.path()).unwrap();
        let rel = vendored_path(&spec);
        assert!(rel.starts_with("local/mylib/"), "{rel}");
        assert_eq!(rel.len(), "local/mylib/".len() + HASH_LEN, "{rel}");
        let dest = pixi.path().join(&rel);
        assert!(dest.join("pyproject.toml").exists());
        // The symlinked subdir's contents were dereferenced and copied.
        assert!(dest.join("sub/sub.py").exists());
        // No leftover tmp dir.
        let leftovers: Vec<_> = std::fs::read_dir(pixi.path().join("local/mylib"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "atomic copy left a tmp dir behind");
        assert!(!has_py_locals(&spec));
    }

    #[test]
    fn vendor_is_content_addressed() {
        let root = tempfile::tempdir().unwrap();
        let pixi = tempfile::tempdir().unwrap();
        let pkg = root.path().join("vendor/mylib");
        write_pkg(&pkg, "V=1\n");
        let mut a = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut a, root.path(), pixi.path()).unwrap();
        // Unchanged source: the same snapshot is reused, not duplicated.
        let mut b = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut b, root.path(), pixi.path()).unwrap();
        assert_eq!(vendored_path(&a), vendored_path(&b));
        // The same content under another project root hashes the same.
        let root2 = tempfile::tempdir().unwrap();
        write_pkg(&root2.path().join("elsewhere/mylib"), "V=1\n");
        let mut c = spec_with_py_local("elsewhere/mylib");
        vendor_py_locals(&mut c, root2.path(), pixi.path()).unwrap();
        assert_eq!(vendored_path(&a), vendored_path(&c));
        // Changed source: a new snapshot beside the old one, which survives.
        std::fs::write(pkg.join("v.py"), "V=2\n").unwrap();
        let mut d = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut d, root.path(), pixi.path()).unwrap();
        assert_ne!(vendored_path(&a), vendored_path(&d));
        assert!(pixi.path().join(vendored_path(&a)).join("v.py").exists());
        assert_eq!(
            std::fs::read_to_string(pixi.path().join(vendored_path(&d)).join("v.py")).unwrap(),
            "V=2\n"
        );
        // Build artifacts do not travel or count.
        std::fs::create_dir_all(pkg.join("__pycache__")).unwrap();
        std::fs::write(pkg.join("__pycache__/v.pyc"), "junk").unwrap();
        let mut e = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut e, root.path(), pixi.path()).unwrap();
        assert_eq!(vendored_path(&d), vendored_path(&e));
        assert!(!pixi.path().join(vendored_path(&e)).join("__pycache__").exists());
    }

    #[test]
    fn sweep_removes_only_unreferenced_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let pixi = tempfile::tempdir().unwrap();
        let pkg = root.path().join("vendor/mylib");
        write_pkg(&pkg, "V=1\n");
        let mut a = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut a, root.path(), pixi.path()).unwrap();
        std::fs::write(pkg.join("v.py"), "V=2\n").unwrap();
        let mut b = spec_with_py_local("vendor/mylib");
        vendor_py_locals(&mut b, root.path(), pixi.path()).unwrap();
        // An in-flight temp dir is not the sweep's to remove.
        std::fs::create_dir_all(pixi.path().join("local/mylib/.tmp.999")).unwrap();

        // A snapshot within the grace period is not the sweep's to remove
        // either: it may belong to an install still writing its store entry.
        assert!(sweep_unreferenced_snapshots(pixi.path(), &[]).unwrap().is_empty());
        assert!(pixi.path().join(vendored_path(&a)).exists());

        let zero = std::time::Duration::ZERO;
        let removed = sweep_snapshots(pixi.path(), std::slice::from_ref(&b), zero).unwrap();
        assert_eq!(removed, vec![vendored_path(&a)]);
        assert!(!pixi.path().join(vendored_path(&a)).exists());
        assert!(pixi.path().join(vendored_path(&b)).exists());
        assert!(pixi.path().join("local/mylib/.tmp.999").exists());
        // Nothing referenced: the snapshot and its now-empty name dir go.
        sweep_snapshots(pixi.path(), &[], zero).unwrap();
        assert!(!pixi.path().join(vendored_path(&b)).exists());
    }

    #[test]
    fn repair_prefers_legacy_copy_then_mirror_then_fails() {
        let env = tempfile::tempdir().unwrap();
        // A legacy copy under <env_dir>/local/<key>/<name> with distinct content.
        write_pkg(&env.path().join("local/prog/mylib"), "LEGACY\n");
        let mut spec = spec_with_py_local("./vendor/mylib");
        repair_installed_py_locals(env.path(), "prog", &mut spec).unwrap();
        let rel = vendored_path(&spec);
        assert_eq!(
            std::fs::read_to_string(env.path().join("pixi").join(&rel).join("v.py")).unwrap(),
            "LEGACY\n"
        );
        // The legacy copy was consumed.
        assert!(!env.path().join("local/prog/mylib").exists());

        // No legacy copy: the source mirror under exe/<key>/ resolves the raw path.
        write_pkg(&env.path().join("exe/other/vendor/mylib"), "MIRROR\n");
        let mut spec = spec_with_py_local("./vendor/mylib");
        repair_installed_py_locals(env.path(), "other", &mut spec).unwrap();
        let rel = vendored_path(&spec);
        assert_eq!(
            std::fs::read_to_string(env.path().join("pixi").join(&rel).join("v.py")).unwrap(),
            "MIRROR\n"
        );

        // Neither: a clear error naming the program.
        let mut spec = spec_with_py_local("./vendor/mylib");
        let err = repair_installed_py_locals(env.path(), "gone", &mut spec).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("'gone'") && msg.contains("mim install"), "{msg}");
    }

    #[test]
    fn repair_uses_an_absolute_live_path_that_still_exists() {
        // A store entry a scratch-style resolve left with the real source path.
        let env = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        write_pkg(&src.path().join("mylib"), "LIVE\n");
        let mut spec = spec_with_py_local(&src.path().join("mylib").display().to_string());
        repair_installed_py_locals(env.path(), "prog", &mut spec).unwrap();
        let rel = vendored_path(&spec);
        assert_eq!(
            std::fs::read_to_string(env.path().join("pixi").join(&rel).join("v.py")).unwrap(),
            "LIVE\n"
        );
    }
}
