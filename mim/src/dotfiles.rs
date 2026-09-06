//! Dotfiles an environment's home is seeded with, and the record that makes the
//! seeding reversible.
//!
//! Copying a directory of dotfiles into a home is destructive in one direction
//! only: afterwards the home holds a mix of files the manager wrote and files
//! the user made, with nothing to tell them apart. So every copy also writes a
//! manifest -- the source it came from, each relative path, and the SHA-256 of
//! the bytes as written. Removal consults that record and takes back only what
//! is still byte-for-byte what was put there. A file the user has since edited
//! is theirs, and is kept.
//!
//! The manifest lives at the root of the environment's state directory, which is
//! the tree bind-mounted into the container, so it is readable from inside the
//! environment as well as from the host.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config as cfg;
use crate::error::{ManagerError, Result};
use crate::types::ContainerEngine;

/// Manifest filename, at the root of the env state dir (`<data_dir>`).
const MANIFEST_FILE: &str = "dotfiles.json";

/// One file placed into the environment's home by a dotfiles copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotfileEntry {
    /// Path relative to the environment's home.
    pub path: String,
    /// SHA-256 of the bytes as written, so a later edit by the user is
    /// distinguishable from the untouched copy.
    pub sha256: String,
}

/// The record of the most recent dotfiles copy into an environment's home.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DotfilesManifest {
    /// Canonicalized host directory the files came from. Recorded so `info` can
    /// report it; the manager reads that directory and never writes to it.
    pub source: String,
    /// Every file written, relative to the home.
    pub files: Vec<DotfileEntry>,
    /// Directories the copy CREATED, relative to the home, parents before
    /// children. Directories that already existed are not listed, so removal
    /// never prunes one the user made.
    pub dirs: Vec<String>,
}

/// What a removal actually did. Files the user edited are kept, so a removal is
/// legitimately partial and the caller reports which files stayed and why.
#[derive(Debug)]
pub struct ForgetReport {
    /// Files taken back (still byte-identical to what was copied).
    pub removed: usize,
    /// Files left in place because their contents changed after the copy.
    pub kept: Vec<String>,
    /// The source directory the removed copy came from.
    pub source: String,
}

/// Path of the manifest for an environment.
pub fn manifest_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MANIFEST_FILE)
}

/// The recorded dotfiles copy, or `None` when the environment has none. An
/// unreadable or malformed manifest reads as `None`: the record exists to make
/// removal safe, and a record that cannot be trusted must not drive deletions.
pub fn read_manifest(data_dir: &Path) -> Option<DotfilesManifest> {
    cfg::read_config::<DotfilesManifest>(&manifest_path(data_dir)).ok()
}

/// Copy a directory of dotfiles into the environment's home, recording what was
/// written so it can be taken back later.
///
/// A previous copy is retracted first, so switching from one dotfiles directory
/// to another does not silently accumulate files from the old one. Files the
/// user edited survive that retraction and are then overwritten only if the new
/// source also provides them -- which is the documented behavior of the copy.
pub fn apply(engine: Option<ContainerEngine>, data_dir: &Path, src: &str) -> Result<()> {
    if !matches!(engine, Some(e) if e.is_oci()) {
        return Err(not_supported());
    }
    let src_path = Path::new(src);
    if !src_path.is_dir() {
        return Err(ManagerError::EnvError(format!(
            "--dotfiles path is not a directory: {}",
            src_path.display()
        )));
    }
    let home = cfg::ensure_env_home(data_dir);
    // Retract the previous copy before laying down the new one; a file both sets
    // provide is removed and rewritten, which is the same end state as an
    // overwrite.
    if read_manifest(data_dir).is_some() {
        let prior = forget(data_dir)?;
        // Files edited since the last copy survive the retraction. Say so: they
        // are about to be either overwritten by the new set or left behind by it,
        // and both are surprising if the user has forgotten they edited them.
        if !prior.kept.is_empty() {
            eprintln!(
                "Kept {} file(s) edited since the previous copy from {}: {}",
                prior.kept.len(),
                prior.source,
                prior.kept.join(", ")
            );
        }
    }
    let canonical = fs::canonicalize(src_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| src.to_string());
    let mut manifest = DotfilesManifest { source: canonical, ..Default::default() };
    copy_recording(src_path, &home, Path::new(""), &mut manifest)?;
    cfg::write_config(&manifest_path(data_dir), &manifest)?;
    eprintln!(
        "Copied {} dotfile(s) from {} into {}",
        manifest.files.len(),
        src_path.display(),
        home.display()
    );
    Ok(())
}

/// Take back a recorded dotfiles copy: remove every file still byte-identical to
/// what was written, prune the directories the copy created if they are now
/// empty, and drop the manifest.
///
/// A file whose contents changed after the copy is the user's and is kept. That
/// makes removal partial by design, which is why the report names what stayed.
pub fn forget(data_dir: &Path) -> Result<ForgetReport> {
    let Some(manifest) = read_manifest(data_dir) else {
        return Err(ManagerError::EnvError(
            "this environment has no recorded dotfiles copy, so there is nothing to \
             remove. An environment seeded before dotfiles were tracked has no record \
             of which files came from where; edit its home directly."
                .to_string(),
        ));
    };
    let home = cfg::env_home_dir(data_dir);
    let mut removed = 0usize;
    let mut kept = Vec::new();
    for entry in &manifest.files {
        // The manifest is written by this module and holds relative paths only,
        // but it sits in a directory the user can reach, so re-check rather than
        // trust it: an absolute or climbing path is refused, never joined.
        let rel = Path::new(&entry.path);
        if rel.is_absolute() || rel.components().any(|c| c.as_os_str() == "..") {
            return Err(ManagerError::EnvError(format!(
                "dotfiles manifest names a path outside the environment home \
                 ('{}'); refusing to remove anything. Delete {} to reset the record.",
                entry.path,
                manifest_path(data_dir).display()
            )));
        }
        let path = home.join(rel);
        match fs::read(&path) {
            // Gone already: nothing to take back.
            Err(_) => {}
            Ok(bytes) if hex_sha256(&bytes) == entry.sha256 => {
                fs::remove_file(&path).map_err(|e| {
                    ManagerError::EnvError(format!("cannot remove {}: {e}", path.display()))
                })?;
                removed += 1;
            }
            Ok(_) => kept.push(entry.path.clone()),
        }
    }
    // Prune created directories deepest-first, and only while empty, so a
    // directory holding a kept file (or anything the user added) survives.
    let mut dirs: Vec<&String> = manifest.dirs.iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(Path::new(d).components().count()));
    for dir in dirs {
        let path = home.join(dir);
        if path.is_dir() && fs::read_dir(&path).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(&path);
        }
    }
    let manifest_file = manifest_path(data_dir);
    if manifest_file.exists() {
        fs::remove_file(&manifest_file).map_err(|e| {
            ManagerError::EnvError(format!("cannot remove {}: {e}", manifest_file.display()))
        })?;
    }
    Ok(ForgetReport { removed, kept, source: manifest.source })
}

/// The single rejection for `--dotfiles` on a non-OCI backend: apptainer inherits
/// the host `$HOME` and native uses the real host home, so neither consults the
/// env-owned home this flag populates. Every dotfiles path returns this, so the
/// message has one source of truth.
pub fn not_supported() -> ManagerError {
    ManagerError::EnvError(
        "--dotfiles applies only to docker/podman environments; apptainer \
         inherits the host $HOME and the native backend uses your real home"
            .to_string(),
    )
}

// --- internals ---------------------------------------------------------------

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Copy `src` into `dst`, appending every file written and every directory
/// created to `manifest`. `rel` is the path of `dst` relative to the home.
///
/// Symlink handling matches the general-purpose copy this replaced: a symlink to
/// a file is followed and its content copied; a symlink to a directory is
/// skipped, since a dotfiles directory that links to `$HOME` would otherwise
/// pull in the whole home (or cycle).
fn copy_recording(
    src: &Path,
    dst: &Path,
    rel: &Path,
    manifest: &mut DotfilesManifest,
) -> Result<()> {
    // Record the directory only when this copy is what brings it into being, so
    // removal never prunes a directory that predates it.
    if !dst.exists() {
        if rel.as_os_str().is_empty() {
            // The home itself is the environment's, not this copy's.
        } else {
            manifest.dirs.push(rel.to_string_lossy().into_owned());
        }
    }
    fs::create_dir_all(dst)
        .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", dst.display())))?;
    let entries = fs::read_dir(src)
        .map_err(|e| ManagerError::EnvError(format!("cannot read {}: {e}", src.display())))?;
    // Directory order is filesystem-defined; sort so a manifest is reproducible
    // and its `dirs` list stays parents-before-children.
    let mut names: Vec<_> = entries.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for entry in names {
        let from = entry.path();
        let name = entry.file_name();
        let to = dst.join(&name);
        let child_rel = rel.join(&name);
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        if is_symlink && from.is_dir() {
            continue;
        }
        if !is_symlink && from.is_dir() {
            copy_recording(&from, &to, &child_rel, manifest)?;
        } else {
            let bytes = fs::read(&from).map_err(|e| {
                ManagerError::EnvError(format!("cannot read {}: {e}", from.display()))
            })?;
            fs::write(&to, &bytes).map_err(|e| {
                ManagerError::EnvError(format!(
                    "cannot copy {} -> {}: {e}",
                    from.display(),
                    to.display()
                ))
            })?;
            manifest.files.push(DotfileEntry {
                path: child_rel.to_string_lossy().into_owned(),
                sha256: hex_sha256(&bytes),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The relative paths a manifest records, as a set for order-free comparison.
    fn recorded_paths(manifest: &DotfilesManifest) -> BTreeSet<String> {
        manifest.files.iter().map(|e| e.path.clone()).collect()
    }

    /// Build a dotfiles source directory and an env data dir, and copy one into
    /// the other's home. Returns (env data dir, source dir, home).
    fn seeded() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let env = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join(".bashrc"), b"export A=1\n").unwrap();
        fs::create_dir_all(src.path().join(".config/nvim")).unwrap();
        fs::write(src.path().join(".config/nvim/init.lua"), b"-- vim\n").unwrap();
        apply(Some(ContainerEngine::Podman), env.path(), &src.path().to_string_lossy()).unwrap();
        let home = cfg::env_home_dir(env.path());
        (env, src, home)
    }

    #[test]
    fn apply_records_every_file_and_created_directory() {
        let (env, src, home) = seeded();
        assert_eq!(fs::read(home.join(".bashrc")).unwrap(), b"export A=1\n");
        assert_eq!(fs::read(home.join(".config/nvim/init.lua")).unwrap(), b"-- vim\n");

        let m = read_manifest(env.path()).expect("manifest written");
        assert_eq!(m.source, fs::canonicalize(src.path()).unwrap().to_string_lossy());
        assert_eq!(
            recorded_paths(&m),
            [".bashrc".to_string(), ".config/nvim/init.lua".to_string()]
                .into_iter()
                .collect()
        );
        // Parents before children, and the home itself is never listed.
        assert_eq!(m.dirs, vec![".config".to_string(), ".config/nvim".to_string()]);
    }

    #[test]
    fn forget_removes_untouched_files_and_prunes_created_dirs() {
        let (env, _src, home) = seeded();
        let report = forget(env.path()).unwrap();
        assert_eq!(report.removed, 2);
        assert!(report.kept.is_empty());
        assert!(!home.join(".bashrc").exists());
        assert!(!home.join(".config/nvim/init.lua").exists());
        assert!(!home.join(".config/nvim").exists(), "created dir pruned");
        assert!(!home.join(".config").exists(), "created parent pruned");
        assert!(home.is_dir(), "the home itself survives");
        assert!(!manifest_path(env.path()).exists(), "manifest dropped");
    }

    #[test]
    fn forget_keeps_a_file_the_user_edited() {
        let (env, _src, home) = seeded();
        fs::write(home.join(".bashrc"), b"export A=1\nexport MINE=2\n").unwrap();

        let report = forget(env.path()).unwrap();
        assert_eq!(report.kept, vec![".bashrc".to_string()]);
        assert_eq!(report.removed, 1, "the untouched file still goes");
        assert_eq!(
            fs::read(home.join(".bashrc")).unwrap(),
            b"export A=1\nexport MINE=2\n",
            "the user's edit is preserved verbatim"
        );
        assert!(!home.join(".config/nvim/init.lua").exists());
    }

    #[test]
    fn forget_leaves_a_directory_holding_a_kept_file() {
        let (env, _src, home) = seeded();
        fs::write(home.join(".config/nvim/init.lua"), b"-- mine\n").unwrap();
        forget(env.path()).unwrap();
        assert!(home.join(".config/nvim/init.lua").is_file());
        assert!(home.join(".config/nvim").is_dir(), "non-empty dir survives");
    }

    #[test]
    fn forget_never_prunes_a_directory_the_copy_did_not_create() {
        let env = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        // The user already has a .config with their own file in it.
        let home = cfg::ensure_env_home(env.path());
        fs::create_dir_all(home.join(".config")).unwrap();
        fs::write(home.join(".config/mine.toml"), b"user\n").unwrap();
        fs::create_dir_all(src.path().join(".config")).unwrap();
        fs::write(src.path().join(".config/theirs.toml"), b"copied\n").unwrap();

        apply(Some(ContainerEngine::Podman), env.path(), &src.path().to_string_lossy()).unwrap();
        let m = read_manifest(env.path()).unwrap();
        assert!(m.dirs.is_empty(), ".config already existed, so it is not recorded");

        forget(env.path()).unwrap();
        assert!(!home.join(".config/theirs.toml").exists());
        assert!(home.join(".config/mine.toml").is_file(), "user file untouched");
        assert!(home.join(".config").is_dir());
    }

    #[test]
    fn reapplying_retracts_the_previous_set() {
        let (env, _src, home) = seeded();
        // A second dotfiles directory that shares one file and drops another.
        let next = tempfile::tempdir().unwrap();
        fs::write(next.path().join(".bashrc"), b"export B=2\n").unwrap();
        fs::write(next.path().join(".vimrc"), b"set nu\n").unwrap();

        apply(Some(ContainerEngine::Podman), env.path(), &next.path().to_string_lossy()).unwrap();
        assert_eq!(fs::read(home.join(".bashrc")).unwrap(), b"export B=2\n", "overwritten");
        assert!(home.join(".vimrc").is_file(), "new file added");
        assert!(
            !home.join(".config/nvim/init.lua").exists(),
            "a file only the old set provided does not linger"
        );
        let m = read_manifest(env.path()).unwrap();
        assert_eq!(
            recorded_paths(&m),
            [".bashrc".to_string(), ".vimrc".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn reapplying_forgets_only_the_old_manifest() {
        // The new manifest must describe the NEW set alone -- if the retraction
        // ran after the copy, or the manifests merged, removal would later miss
        // files or try to remove ones that were never written.
        let (env, _src, _home) = seeded();
        let next = tempfile::tempdir().unwrap();
        fs::write(next.path().join(".bashrc"), b"export B=2\n").unwrap();
        apply(Some(ContainerEngine::Podman), env.path(), &next.path().to_string_lossy()).unwrap();

        let m = read_manifest(env.path()).unwrap();
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.source, fs::canonicalize(next.path()).unwrap().to_string_lossy());
        // The recorded hash is of the NEW bytes, so removal recognizes them.
        let report = forget(env.path()).unwrap();
        assert_eq!(report.removed, 1);
        assert!(report.kept.is_empty());
    }

    #[test]
    fn reapplying_does_not_clobber_an_edit_it_does_not_replace() {
        let (env, _src, home) = seeded();
        fs::write(home.join(".config/nvim/init.lua"), b"-- mine\n").unwrap();
        let next = tempfile::tempdir().unwrap();
        fs::write(next.path().join(".vimrc"), b"set nu\n").unwrap();

        apply(Some(ContainerEngine::Podman), env.path(), &next.path().to_string_lossy()).unwrap();
        assert_eq!(
            fs::read(home.join(".config/nvim/init.lua")).unwrap(),
            b"-- mine\n",
            "an edited file the new set does not provide is left alone"
        );
    }

    #[test]
    fn forget_without_a_manifest_is_an_honest_error() {
        let env = tempfile::tempdir().unwrap();
        let err = forget(env.path()).unwrap_err();
        assert!(err.to_string().contains("no recorded dotfiles copy"), "got: {err}");
    }

    #[test]
    fn forget_refuses_a_manifest_that_escapes_the_home() {
        let env = tempfile::tempdir().unwrap();
        cfg::ensure_env_home(env.path());
        let m = DotfilesManifest {
            source: "/somewhere".to_string(),
            files: vec![DotfileEntry {
                path: "../../../etc/passwd".to_string(),
                sha256: "0".repeat(64),
            }],
            dirs: Vec::new(),
        };
        cfg::write_config(&manifest_path(env.path()), &m).unwrap();
        let err = forget(env.path()).unwrap_err();
        assert!(err.to_string().contains("outside the environment home"), "got: {err}");
        // Refused wholesale: the manifest is left for the user to inspect.
        assert!(manifest_path(env.path()).exists());
    }

    #[test]
    fn apply_skips_a_symlinked_directory_but_follows_a_symlinked_file() {
        let env = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"do not recurse\n").unwrap();
        fs::write(outside.path().join("real.txt"), b"content\n").unwrap();
        // A link to a directory would otherwise pull in the whole target tree.
        std::os::unix::fs::symlink(outside.path(), src.path().join(".linkdir")).unwrap();
        // A link to a file is followed and copied as content.
        std::os::unix::fs::symlink(outside.path().join("real.txt"), src.path().join(".linkfile"))
            .unwrap();

        apply(Some(ContainerEngine::Podman), env.path(), &src.path().to_string_lossy()).unwrap();
        let home = cfg::env_home_dir(env.path());
        assert!(!home.join(".linkdir").exists(), "symlinked dir skipped");
        assert_eq!(fs::read(home.join(".linkfile")).unwrap(), b"content\n");
        let m = read_manifest(env.path()).unwrap();
        assert_eq!(recorded_paths(&m), [".linkfile".to_string()].into_iter().collect());
    }

    #[test]
    fn apply_rejects_non_oci_backends() {
        let env = tempfile::tempdir().unwrap();
        assert!(apply(None, env.path(), "/whatever").is_err());
        assert!(apply(Some(ContainerEngine::Apptainer), env.path(), "/whatever").is_err());
    }

    #[test]
    fn apply_rejects_a_source_that_is_not_a_directory() {
        let env = tempfile::tempdir().unwrap();
        let err = apply(Some(ContainerEngine::Podman), env.path(), "/no/such/dotfiles")
            .unwrap_err();
        assert!(err.to_string().contains("not a directory"), "got: {err}");
    }
}
