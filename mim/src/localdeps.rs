//! Container/install handling for local (filesystem-path) Python dependencies.
//!
//! The path/validation/rewrite logic lives in ONE place --
//! `morloc_deps::envspec::EnvSpec::resolve_local_deps` (shared by the scratch
//! `Live` anchor and the install `Relocated` anchor). This module holds only the
//! `mim`-side pieces: the pre-build strip, the has-locals check, and executing the
//! source-tree copies the resolver hands back for an install.
//!
//! Model recap: SCRATCH (`mim run morloc make`) keeps a live editable install
//! against `/work`; INSTALL copies the source into the state volume
//! (`<env_data>/local/<key>/<name>`, mounted everywhere) and installs it plain.
//! Rust local crates are compiled into the pool binary at build -- never copied.

use morloc_deps::envspec::{CopyJob, EnvSpec, PackageReq};

use crate::error::{ManagerError, Result};
use crate::provision::copy_dir_excluding;

/// Directory names not worth copying into the vendored snapshot.
const SKIP_DIRS: &[&str] = &["target", ".git", "__pycache__", ".venv", "node_modules"];

/// Whether the spec declares any Python local dependency (drives the install-time
/// copy step; a spec with none needs no relocation).
pub fn has_py_locals(spec: &EnvSpec) -> bool {
    spec.packages
        .get("py")
        .map(|reqs| reqs.iter().any(|r| matches!(r, PackageReq::Local { .. })))
        .unwrap_or(false)
}

/// Remove Python local deps from a spec. Used for the PRE-build install solve:
/// py locals are runtime-only (the pool build does not import them) and their
/// copy is only made after the build, so they are held back and provisioned by a
/// post-build re-solve.
pub fn strip_py_locals(spec: &mut EnvSpec) {
    if let Some(reqs) = spec.packages.get_mut("py") {
        reqs.retain(|r| !matches!(r, PackageReq::Local { .. }));
    }
}

/// Execute the copy jobs an install-time `resolve_local_deps(Relocated)` returned.
/// Each copy is ATOMIC (built into a sibling `.tmp` dir, then renamed over the
/// destination) so a crash mid-copy never leaves a half-populated dependency; and
/// it FOLLOWS symlinked subdirectories (a vendored package may symlink
/// subpackages) with a cycle guard.
pub fn perform_copy_jobs(jobs: &[CopyJob]) -> Result<()> {
    for job in jobs {
        let parent = job.dest.parent().ok_or_else(|| {
            ManagerError::EnvError(format!("invalid local-dep destination {}", job.dest.display()))
        })?;
        std::fs::create_dir_all(parent)
            .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", parent.display())))?;
        let file = job
            .dest
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("localdep");
        let tmp = parent.join(format!(".{file}.tmp.{}", std::process::id()));
        if tmp.exists() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        copy_dir_excluding(&job.src, &tmp, SKIP_DIRS, true)?;
        // Swap the fully-built tmp over any prior copy (a re-install refreshes the
        // source in place).
        if job.dest.exists() {
            std::fs::remove_dir_all(&job.dest).map_err(|e| {
                ManagerError::EnvError(format!("cannot replace {}: {e}", job.dest.display()))
            })?;
        }
        std::fs::rename(&tmp, &job.dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            ManagerError::EnvError(format!(
                "cannot install local dep into {}: {e}",
                job.dest.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use morloc_deps::envspec::LocalAnchor;

    fn spec_with_py_local(path: &str) -> EnvSpec {
        let j = format!(
            r#"{{"envspec_version":2,"morloc_version":"0","packages":{{"py":[{{"name":"mylib","source":"local","path":"{path}","editable":true}}]}}}}"#
        );
        EnvSpec::from_json(&j).unwrap()
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
    fn perform_copy_jobs_copies_and_derefs_symlinked_subdir() {
        let root = tempfile::tempdir().unwrap();
        let copy_root = tempfile::tempdir().unwrap();
        // A package whose subpackage is a symlink to an external dir.
        let pkg = root.path().join("vendor/mylib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("pyproject.toml"), "[project]\nname='mylib'\n").unwrap();
        let ext = tempfile::tempdir().unwrap();
        std::fs::write(ext.path().join("sub.py"), "x=1\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(ext.path(), pkg.join("sub")).unwrap();

        let mut spec = spec_with_py_local("vendor/mylib");
        let jobs = spec
            .resolve_local_deps(&LocalAnchor::Relocated {
                root: root.path(),
                copy_root: copy_root.path(),
                state_root: "/opt/morloc-state",
                key: "prog",
            })
            .unwrap();
        perform_copy_jobs(&jobs).unwrap();
        let dest = copy_root.path().join("local/prog/mylib");
        assert!(dest.join("pyproject.toml").exists());
        // The symlinked subdir's contents were dereferenced and copied.
        assert!(dest.join("sub/sub.py").exists());
        // No leftover tmp dir.
        let leftovers: Vec<_> = std::fs::read_dir(copy_root.path().join("local/prog"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "atomic copy left a tmp dir behind");
    }

    #[test]
    fn perform_copy_jobs_replaces_prior_copy() {
        let root = tempfile::tempdir().unwrap();
        let copy_root = tempfile::tempdir().unwrap();
        let pkg = root.path().join("vendor/mylib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("pyproject.toml"), "[project]\nname='mylib'\n").unwrap();
        std::fs::write(pkg.join("v.py"), "V=1\n").unwrap();
        // A stale prior copy with a file that must not survive the refresh.
        let dest = copy_root.path().join("local/prog/mylib");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("stale.py"), "old\n").unwrap();

        let mut spec = spec_with_py_local("vendor/mylib");
        let jobs = spec
            .resolve_local_deps(&LocalAnchor::Relocated {
                root: root.path(),
                copy_root: copy_root.path(),
                state_root: "/opt/morloc-state",
                key: "prog",
            })
            .unwrap();
        perform_copy_jobs(&jobs).unwrap();
        assert!(dest.join("v.py").exists());
        assert!(!dest.join("stale.py").exists(), "prior copy was not replaced");
    }
}
