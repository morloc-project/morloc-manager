//! ENV-2: an environment exists exactly when its configuration record does.
//! A `new` writes a marker first and holds a lock on it; if the `new` fails,
//! is interrupted or panics, its guard removes everything it created before
//! mim returns. A remnant left by a killed `new` is listed by `ls` and removed
//! by `rm` or by the next `new` of that name.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use serde::{Deserialize, Serialize};

use crate::config as cfg;
use crate::error::{ManagerError, Result};
use crate::types::{ContainerEngine, Scope};

const MARKER: &str = ".creating";

/// What a `new` in progress may have created besides its two directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    pid: u32,
    #[serde(default)]
    engine: Option<ContainerEngine>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

/// The state of a name with no configuration record.
#[derive(Debug, PartialEq, Eq)]
pub enum Remnant {
    /// Nothing is on disk for the name.
    None,
    /// A `new` that died left this behind.
    Incomplete,
    /// A `new` is creating it now.
    Busy,
    /// Files with no marker, from a mim older than the marker.
    Unmarked,
    /// A marker this user cannot open or lock.
    Unreadable,
}

fn marker_path(scope: Scope, name: &str) -> PathBuf {
    cfg::env_config_dir(scope, name).join(MARKER)
}

fn has_record(scope: Scope, name: &str) -> bool {
    cfg::env_config_path(scope, name).is_file() || cfg::env_config_path_legacy_json(scope, name).is_file()
}

fn try_lock(path: &Path, create: bool) -> Result<Option<Flock<File>>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)
        .map_err(|e| ManagerError::EnvError(format!("cannot open {}: {e}", path.display())))?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(Some(lock)),
        Err((_, Errno::EAGAIN)) => Ok(None),
        Err((_, e)) => Err(ManagerError::EnvError(format!("cannot lock {}: {e}", path.display()))),
    }
}

/// Classify `name` in `scope`. An environment with a record is `None`.
pub fn remnant(scope: Scope, name: &str) -> Remnant {
    if has_record(scope, name) {
        return Remnant::None;
    }
    let marker = marker_path(scope, name);
    if marker.is_file() {
        return match try_lock(&marker, false) {
            Ok(Some(_)) => Remnant::Incomplete,
            Ok(None) => Remnant::Busy,
            Err(_) => Remnant::Unreadable,
        };
    }
    let exists = |p: PathBuf| fs::symlink_metadata(p).is_ok();
    if exists(cfg::env_config_dir(scope, name)) || exists(cfg::env_data_dir(scope, name)) {
        Remnant::Unmarked
    } else {
        Remnant::None
    }
}

/// Every name in `scope` that has files on disk but no record.
pub fn list_remnants(scope: Scope) -> Vec<(String, Remnant)> {
    let mut names = std::collections::BTreeSet::new();
    for root in [cfg::config_dir(scope), cfg::data_dir(scope)] {
        if let Ok(entries) = fs::read_dir(root.join("environments")) {
            for e in entries.flatten() {
                if let Ok(n) = e.file_name().into_string() {
                    names.insert(n);
                }
            }
        }
    }
    names
        .into_iter()
        .map(|n| {
            let r = remnant(scope, &n);
            (n, r)
        })
        .filter(|(_, r)| *r != Remnant::None)
        .collect()
}

/// Remove what a `new` of `name` left behind: its engine objects as its
/// marker names them, then its data and configuration directories. Refused
/// while a `new` holds the marker.
pub fn remove_remnant(scope: Scope, name: &str) -> Result<()> {
    let path = marker_path(scope, name);
    // The lock first: a `new` that commits meanwhile has written its record,
    // which the check below then sees.
    let _lock = if path.is_file() {
        match try_lock(&path, false)? {
            Some(lock) => Some(lock),
            None => return Err(busy(name)),
        }
    } else {
        None
    };
    if has_record(scope, name) {
        return Err(ManagerError::EnvError(format!("'{name}' is an environment, not a remnant")));
    }
    let marker = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Marker>(&s).ok());
    remove_created(scope, name, marker.as_ref())
}

fn busy(name: &str) -> ManagerError {
    ManagerError::EnvError(format!("another `mim new` is creating '{name}' now"))
}

/// The engine objects a marker names, as (engine, kind, name).
fn engine_objects(marker: Option<&Marker>) -> Vec<(ContainerEngine, &'static str, String)> {
    let Some(Marker { engine: Some(engine), image, volume, .. }) = marker else { return Vec::new() };
    if !engine.is_oci() {
        return Vec::new();
    }
    let image = image.iter().map(|i| (*engine, "image", i.clone()));
    let volume = volume.iter().map(|v| (*engine, "volume", v.clone()));
    image.chain(volume).collect()
}

fn remove_created(scope: Scope, name: &str, marker: Option<&Marker>) -> Result<()> {
    for (engine, kind, object) in engine_objects(marker) {
        crate::environment::remove_engine_object(engine, kind, &object, || match kind {
            "image" => {
                crate::container::remove_image(engine, &object);
            }
            _ => {
                let _ = crate::container::volume_remove(engine, &object);
            }
        })?;
    }
    crate::environment::remove_tree(&cfg::env_data_dir(scope, name), name)?;
    crate::environment::remove_tree(&cfg::env_config_dir(scope, name), name)
}

/// The `new` must fail at its commit.
pub(crate) static INTERRUPTED: AtomicBool = AtomicBool::new(false);
/// A SIGINT arrived; it is raised again once the default is restored.
static SIGINT_SEEN: AtomicBool = AtomicBool::new(false);

extern "C" fn note_interrupt(_: nix::libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
    SIGINT_SEEN.store(true, Ordering::SeqCst);
}

/// SIGINT, deferred: a handler (reset to the default in every child at exec)
/// records it, the tty's SIGINT ends the children, and their failure unwinds
/// into the guard. Once the guard is done the signal is raised again, so mim
/// still dies of it. SIGTERM reaches only mim, so it keeps its default: a
/// `new` killed by it leaves a marked remnant.
struct DeferredSignals {
    previous: Option<SigAction>,
}

impl DeferredSignals {
    fn install() -> Self {
        let action = SigAction::new(SigHandler::Handler(note_interrupt), SaFlags::SA_RESTART, SigSet::empty());
        SIGINT_SEEN.store(false, Ordering::SeqCst);
        // SAFETY: the handler only stores to atomics.
        DeferredSignals { previous: unsafe { sigaction(Signal::SIGINT, &action) }.ok() }
    }
}

impl Drop for DeferredSignals {
    fn drop(&mut self) {
        if let Some(old) = self.previous.take() {
            // SAFETY: restores the disposition saved by `install`.
            let _ = unsafe { sigaction(Signal::SIGINT, &old) };
            if SIGINT_SEEN.swap(false, Ordering::SeqCst) {
                let _ = nix::sys::signal::raise(Signal::SIGINT);
            }
        }
    }
}

/// Held for the whole of a `new`. Dropped without `commit`, it removes what
/// the `new` created.
pub struct NewEnvGuard {
    scope: Scope,
    name: String,
    marker: Marker,
    lock: Option<Flock<File>>,
    _signals: DeferredSignals,
}

impl NewEnvGuard {
    /// Start creating `name`. A remnant of an earlier `new` is removed first;
    /// unmarked files from an older mim are refused.
    pub fn begin(scope: Scope, name: &str, engine: Option<ContainerEngine>) -> Result<Self> {
        let exists = || ManagerError::EnvError(format!("Environment '{name}' already exists."));
        if has_record(scope, name) {
            return Err(exists());
        }
        match remnant(scope, name) {
            Remnant::None => {}
            Remnant::Busy => return Err(busy(name)),
            Remnant::Unreadable => {
                return Err(ManagerError::EnvError(format!(
                    "cannot inspect what an earlier `mim new {name}` left in {}",
                    cfg::env_config_dir(scope, name).display()
                )))
            }
            Remnant::Incomplete => {
                eprintln!("Removing what an earlier `mim new {name}` left behind...");
                remove_remnant(scope, name)?;
            }
            Remnant::Unmarked => {
                return Err(ManagerError::EnvError(format!(
                    "files from an earlier `mim new {name}` remain in {} or {}. \
                     Remove them with `mim rm {name}`, then retry.",
                    cfg::env_config_dir(scope, name).display(),
                    cfg::env_data_dir(scope, name).display()
                )))
            }
        }
        let config_dir = cfg::env_config_dir(scope, name);
        fs::create_dir_all(&config_dir)
            .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", config_dir.display())))?;
        let oci = engine.filter(|e| e.is_oci());
        let marker = Marker {
            pid: std::process::id(),
            engine,
            image: oci.map(|_| crate::environment::env_image_tag(scope, name)),
            volume: oci.map(|_| crate::serve::prefix_volume(&cfg::env_data_dir(scope, name))),
        };
        let path = marker_path(scope, name);
        let Some(mut lock) = try_lock(&path, true)? else {
            return Err(busy(name));
        };
        if has_record(scope, name) {
            return Err(exists());
        }
        let text = serde_json::to_string(&marker).unwrap_or_default();
        lock.set_len(0)
            .and_then(|()| lock.write_all(text.as_bytes()))
            .map_err(|e| ManagerError::EnvError(format!("cannot write {}: {e}", path.display())))?;
        INTERRUPTED.store(false, Ordering::SeqCst);
        Ok(NewEnvGuard {
            scope,
            name: name.to_string(),
            marker,
            lock: Some(lock),
            _signals: DeferredSignals::install(),
        })
    }

    /// The record has been written: the environment exists. An interrupt
    /// received meanwhile fails the `new` instead.
    pub fn commit(mut self) -> Result<()> {
        if INTERRUPTED.load(Ordering::SeqCst) {
            return Err(ManagerError::EnvError("interrupted".to_string()));
        }
        if !has_record(self.scope, &self.name) {
            return Err(ManagerError::EnvError(format!(
                "the record of '{}' was not written",
                self.name
            )));
        }
        let _ = fs::remove_file(marker_path(self.scope, &self.name));
        self.lock = None;
        Ok(())
    }
}

impl Drop for NewEnvGuard {
    fn drop(&mut self) {
        if self.lock.is_none() {
            return;
        }
        let _ = fs::remove_file(cfg::env_config_path(self.scope, &self.name));
        match remove_created(self.scope, &self.name, Some(&self.marker)) {
            Ok(()) => eprintln!("Removed the partly created environment '{}'.", self.name),
            Err(e) => eprintln!(
                "Could not remove the partly created environment '{}' ({e}). \
                 `mim rm {}` removes it.",
                self.name, self.name
            ),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The interrupt flag is process-wide; these tests take turns with it.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    fn roots() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        cfg::test_roots::set(tmp.path());
        (serial, tmp)
    }

    fn write_record(name: &str) {
        let path = cfg::env_config_path(Scope::Local, name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "name: x\n").unwrap();
    }

    fn gone(name: &str) -> bool {
        !cfg::env_config_dir(Scope::Local, name).exists() && !cfg::env_data_dir(Scope::Local, name).exists()
    }

    /// ENV-2: a `new` that fails after creating its data dir leaves nothing.
    #[test]
    fn a_failed_new_leaves_nothing_behind() {
        let (_s, _t) = roots();
        let guard = NewEnvGuard::begin(Scope::Local, "e", None).unwrap();
        let data = cfg::env_data_dir(Scope::Local, "e");
        fs::create_dir_all(data.join("home")).unwrap();
        fs::write(data.join("home/.bashrc"), "x").unwrap();
        fs::write(cfg::env_config_dir(Scope::Local, "e").join("env.flags.yaml"), "x").unwrap();
        drop(guard);
        assert!(gone("e"));
        assert_eq!(remnant(Scope::Local, "e"), Remnant::None);
    }

    /// A committed `new` keeps everything but its marker; a commit with no
    /// record fails.
    #[test]
    fn a_committed_new_keeps_its_files_and_drops_its_marker() {
        let (_s, _t) = roots();
        let guard = NewEnvGuard::begin(Scope::Local, "f", None).unwrap();
        assert!(guard.commit().is_err());
        assert!(gone("f"));
        let guard = NewEnvGuard::begin(Scope::Local, "e", None).unwrap();
        fs::create_dir_all(cfg::env_data_dir(Scope::Local, "e")).unwrap();
        write_record("e");
        guard.commit().unwrap();
        assert!(cfg::env_data_dir(Scope::Local, "e").is_dir());
        assert!(!marker_path(Scope::Local, "e").exists());
        assert_eq!(remnant(Scope::Local, "e"), Remnant::None);
    }

    /// An interrupt during `new` fails it at the commit, and the guard
    /// removes the environment, record included.
    #[test]
    fn an_interrupted_new_is_removed_at_its_commit() {
        let (_s, _t) = roots();
        let guard = NewEnvGuard::begin(Scope::Local, "e", None).unwrap();
        write_record("e");
        INTERRUPTED.store(true, Ordering::SeqCst);
        let err = guard.commit().unwrap_err().to_string();
        INTERRUPTED.store(false, Ordering::SeqCst);
        assert!(err.contains("interrupted"), "{err}");
        assert!(gone("e"));
    }

    /// A `new` that died left a marker nobody holds: `ls` lists it, `rm`
    /// removes it, and the next `new` wipes it and starts clean.
    #[test]
    fn a_marked_remnant_is_listed_removed_and_replaced() {
        let (_s, _t) = roots();
        let dead = |name: &str| {
            fs::create_dir_all(cfg::env_config_dir(Scope::Local, name)).unwrap();
            fs::create_dir_all(cfg::env_data_dir(Scope::Local, name).join("pixi")).unwrap();
            let marker = Marker { pid: 1, engine: None, image: None, volume: None };
            fs::write(marker_path(Scope::Local, name), serde_json::to_string(&marker).unwrap()).unwrap();
        };
        dead("a");
        dead("b");
        assert_eq!(remnant(Scope::Local, "a"), Remnant::Incomplete);
        let listed: Vec<String> = list_remnants(Scope::Local).into_iter().map(|(n, _)| n).collect();
        assert_eq!(listed, vec!["a".to_string(), "b".to_string()]);
        remove_remnant(Scope::Local, "a").unwrap();
        assert!(gone("a"));
        let guard = NewEnvGuard::begin(Scope::Local, "b", None).unwrap();
        assert!(!cfg::env_data_dir(Scope::Local, "b").join("pixi").exists());
        drop(guard);
        assert!(gone("b"));
    }

    /// A marker whose lock is held belongs to a `new` still running: nothing
    /// touches it.
    #[test]
    fn a_remnant_whose_lock_is_held_is_left_alone() {
        let (_s, _t) = roots();
        let held = NewEnvGuard::begin(Scope::Local, "e", None).unwrap();
        fs::create_dir_all(cfg::env_data_dir(Scope::Local, "e")).unwrap();
        assert_eq!(remnant(Scope::Local, "e"), Remnant::Busy);
        assert!(remove_remnant(Scope::Local, "e").is_err());
        assert!(NewEnvGuard::begin(Scope::Local, "e", None).is_err());
        assert!(cfg::env_data_dir(Scope::Local, "e").is_dir());
        assert!(marker_path(Scope::Local, "e").is_file());
        drop(held);
    }

    /// Files with no marker come from an older mim: `new` refuses and names
    /// `mim rm`, which removes them.
    #[test]
    fn an_unmarked_remnant_is_refused_by_new_and_removed_by_rm() {
        let (_s, _t) = roots();
        fs::create_dir_all(cfg::env_data_dir(Scope::Local, "e").join("home")).unwrap();
        assert_eq!(remnant(Scope::Local, "e"), Remnant::Unmarked);
        let err = NewEnvGuard::begin(Scope::Local, "e", None).err().unwrap().to_string();
        assert!(err.contains("mim rm e"), "{err}");
        assert!(cfg::env_data_dir(Scope::Local, "e").is_dir());
        remove_remnant(Scope::Local, "e").unwrap();
        assert!(gone("e"));
    }

    /// ENV-2: a recorded environment is never a remnant: `new` refuses it, so
    /// a failing second `new` cannot delete it, and `rm` of a remnant spares
    /// it even with a stray marker.
    #[test]
    fn a_recorded_environment_is_never_cleaned_as_a_remnant() {
        let (_s, _t) = roots();
        write_record("e");
        fs::create_dir_all(cfg::env_data_dir(Scope::Local, "e")).unwrap();
        let err = NewEnvGuard::begin(Scope::Local, "e", None).err().unwrap().to_string();
        assert!(err.contains("already exists"), "{err}");
        fs::write(marker_path(Scope::Local, "e"), "{}").unwrap();
        assert!(remove_remnant(Scope::Local, "e").is_err());
        assert!(has_record(Scope::Local, "e"));
        assert!(cfg::env_data_dir(Scope::Local, "e").is_dir());
    }

    /// A container `new` records the image tag and prefix volume it will
    /// create, and a remnant's removal names both.
    #[test]
    fn a_container_remnant_names_its_image_and_volume() {
        let (_s, _t) = roots();
        let guard = NewEnvGuard::begin(Scope::Local, "e", Some(ContainerEngine::Podman)).unwrap();
        let marker: Marker =
            serde_json::from_str(&fs::read_to_string(marker_path(Scope::Local, "e")).unwrap()).unwrap();
        let tag = crate::environment::env_image_tag(Scope::Local, "e");
        let volume = crate::serve::prefix_volume(&cfg::env_data_dir(Scope::Local, "e"));
        assert_eq!(
            engine_objects(Some(&marker)),
            vec![
                (ContainerEngine::Podman, "image", tag),
                (ContainerEngine::Podman, "volume", volume),
            ]
        );
        let apptainer = Marker { engine: Some(ContainerEngine::Apptainer), ..marker.clone() };
        assert!(engine_objects(Some(&apptainer)).is_empty());
        drop(guard);
    }
}
