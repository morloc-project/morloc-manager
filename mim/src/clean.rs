//! `mim clean`: remove what an environment regenerates on demand (ENV-12).
//!
//! Runs inside the environment, against its state root (`MORLOC_STATE`) and
//! runtime prefix (`MORLOC_HOME`). The host command delegates here.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::error::{ManagerError, Result};

/// Files younger than this may belong to a build still in flight.
pub const RECENT: Duration = Duration::from_secs(3600);

/// The cargo lock files of one profile directory of a target dir.
const CARGO_LOCKS: [&str; 3] = [".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"];

pub struct Roots {
    pub state: PathBuf,
    pub home: PathBuf,
}

impl Roots {
    fn rust_build(&self) -> PathBuf {
        self.state.join("cache").join("rust-build")
    }
}

#[derive(Default)]
pub struct Category {
    pub label: &'static str,
    pub paths: Vec<PathBuf>,
    /// Pool builds left in place because they are recent.
    pub kept_recent: usize,
}

/// `pool_<16 hex>` at the start of a target-dir entry name, followed by the
/// end of the name, `-` (cargo's metadata hash) or `.` (an extension).
fn pool_key(name: &str) -> Option<&str> {
    let key = name.get(..21)?;
    let hex = key.strip_prefix("pool_")?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match name.as_bytes().get(21) {
        None | Some(b'-') | Some(b'.') => Some(key),
        _ => None,
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(p).and_then(|m| m.modified()).ok()
}

fn is_recent(t: Option<SystemTime>, now: SystemTime) -> bool {
    match t {
        Some(t) => now.duration_since(t).map(|d| d < RECENT).unwrap_or(true),
        None => false,
    }
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

fn name_of(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// The profile directories of the shared cargo target dir (`release/`, ...).
fn profile_dirs(rust_build: &Path) -> Vec<PathBuf> {
    entries(rust_build).into_iter().filter(|p| p.is_dir()).collect()
}

/// Exclusive locks on every profile directory's cargo lock files, so that no
/// cargo build runs while they are held. Taken without blocking and released
/// in full before each retry, so a cargo build holding one and waiting for
/// another can never deadlock against this.
pub struct CargoGuard {
    _locks: Vec<Flock<File>>,
}

pub fn lock_cargo(rust_build: &Path) -> Result<CargoGuard> {
    let mut announced = false;
    loop {
        match try_lock_cargo(rust_build)? {
            Some(guard) => return Ok(guard),
            None => {
                if !announced {
                    eprintln!("Waiting for a running cargo build to finish...");
                    announced = true;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn try_lock_cargo(rust_build: &Path) -> Result<Option<CargoGuard>> {
    let mut locks = Vec::new();
    for profile in profile_dirs(rust_build) {
        for lock in CARGO_LOCKS {
            let path = profile.join(lock);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| ManagerError::EnvError(format!("cannot open {}: {e}", path.display())))?;
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(g) => locks.push(g),
                Err((_, Errno::EAGAIN)) => return Ok(None),
                Err((_, e)) => {
                    return Err(ManagerError::EnvError(format!("cannot lock {}: {e}", path.display())))
                }
            }
        }
    }
    Ok(Some(CargoGuard { _locks: locks }))
}

/// What `clean` removes, given the cargo locks are held. `crates` adds the
/// compiled dependency cache to the pool outputs.
pub fn plan(roots: &Roots, crates: bool, now: SystemTime) -> Vec<Category> {
    let mut pools = Category { label: "Rust pool build outputs", ..Default::default() };
    let mut deps = Category { label: "compiled crate cache", ..Default::default() };

    for profile in profile_dirs(&roots.rust_build()) {
        // Every entry of the profile dir and of its subdirectories, grouped
        // by pool key; `None` holds everything that is not a pool's.
        let mut by_key: BTreeMap<Option<String>, Vec<PathBuf>> = BTreeMap::new();
        for top in entries(&profile) {
            let name = name_of(&top);
            if CARGO_LOCKS.contains(&name) {
                continue;
            }
            if let Some(k) = pool_key(name) {
                by_key.entry(Some(k.to_string())).or_default().push(top);
            } else if top.is_dir() && !top.is_symlink() {
                let inner = entries(&top);
                for p in &inner {
                    let k = pool_key(name_of(p)).map(str::to_string);
                    if k.is_some() || crates {
                        by_key.entry(k).or_default().push(p.clone());
                    }
                }
            } else if crates {
                by_key.entry(None).or_default().push(top);
            }
        }
        for (key, paths) in by_key {
            match key {
                Some(_) if paths.iter().any(|p| is_recent(mtime(p), now)) => pools.kept_recent += 1,
                Some(_) => pools.paths.extend(paths),
                None => deps.paths.extend(paths),
            }
        }
    }

    let mut temps = Category { label: "stale temporary files", ..Default::default() };
    // `morloc eval` work dirs left empty in the runtime's tmp/.
    for p in entries(&roots.home.join("tmp")) {
        if name_of(&p).starts_with("morloc-eval-")
            && p.is_dir()
            && !p.is_symlink()
            && fs::read_dir(&p).map(|mut d| d.next().is_none()).unwrap_or(false)
            && !is_recent(mtime(&p), now)
        {
            temps.paths.push(p);
        }
    }
    // Staging files of an environment cargo-lock merge that was killed.
    for p in entries(&roots.state.join("cache")) {
        let name = name_of(&p);
        let staged = name
            .strip_prefix("rust-env.v2.lock")
            .and_then(|r| r.strip_suffix(".tmp"))
            .is_some_and(|mid| !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit() || b == b'-'));
        if staged && p.is_file() && !is_recent(mtime(&p), now) {
            temps.paths.push(p);
        }
    }

    let mut out = vec![pools];
    if crates {
        out.push(deps);
    }
    out.push(temps);
    out
}

/// Disk bytes under `paths`, counting a hard-linked file once.
pub fn disk_usage(paths: &[PathBuf]) -> u64 {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        let Ok(m) = fs::symlink_metadata(&p) else { continue };
        if m.nlink() > 1 && !m.is_dir() && !seen.insert((m.dev(), m.ino())) {
            continue;
        }
        total += m.blocks() * 512;
        if m.is_dir() {
            stack.extend(entries(&p));
        }
    }
    total
}

fn remove(p: &Path) -> std::io::Result<()> {
    let m = fs::symlink_metadata(p)?;
    if m.is_dir() {
        // An eval dir that gained a file since planning stays.
        if name_of(p).starts_with("morloc-eval-") {
            return fs::remove_dir(p);
        }
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// Remove (or with `dry_run`, list) the regenerable files of the environment
/// at `roots`, holding the cargo locks throughout.
pub fn clean_files(roots: &Roots, crates: bool, dry_run: bool) -> Result<()> {
    let _guard = lock_cargo(&roots.rust_build())?;
    let cats = plan(roots, crates, SystemTime::now());
    let mut total = 0u64;
    let mut failures = Vec::new();
    for cat in &cats {
        let bytes = disk_usage(&cat.paths);
        total += bytes;
        if !dry_run {
            for p in &cat.paths {
                if let Err(e) = remove(p) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        failures.push(format!("{}: {e}", p.display()));
                    }
                }
            }
        }
        let verb = if dry_run { "Would remove" } else { "Removed" };
        eprintln!("{verb} {}: {} entries, {}", cat.label, cat.paths.len(), human(bytes));
        if cat.kept_recent > 0 {
            eprintln!("  kept {} pool build(s) from the last hour", cat.kept_recent);
        }
    }
    eprintln!("{}: {}", if dry_run { "Total" } else { "Freed" }, human(total));
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ManagerError::EnvError(format!(
            "could not remove {} path(s):\n  {}",
            failures.len(),
            failures.join("\n  ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn age(p: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        File::open(p).unwrap().set_modified(t).unwrap();
    }

    const OLD: u64 = 2 * 3600;
    const P1: &str = "pool_00112233445566aa";
    const P2: &str = "pool_ffeeddccbbaa9988";

    /// A state root with one old pool, one fresh pool, a dependency crate and
    /// the files `clean` must never touch.
    fn fixture() -> (tempfile::TempDir, Roots) {
        let t = tempfile::tempdir().unwrap();
        let state = t.path().join("state");
        let home = t.path().join("home");
        let rel = state.join("cache/rust-build/release");
        for p in [P1, P2] {
            let files = [
                rel.join(p),
                rel.join(format!("{p}.d")),
                rel.join(format!("deps/{p}-0123456789abcdef")),
                rel.join(format!("deps/{p}-0123456789abcdef.d")),
                rel.join(format!("build/{p}-fedcba9876543210/output")),
                rel.join(format!(".fingerprint/{p}-0123456789abcdef/bin-{p}")),
            ];
            for f in &files {
                touch(f);
                if p == P1 {
                    age(f, OLD);
                    let dir = f.parent().unwrap();
                    if pool_key(name_of(dir)).is_some() {
                        age(dir, OLD);
                    }
                }
            }
        }
        for f in [
            "deps/librustmorloc-1111222233334444.rlib",
            "deps/serde-5555666677778888.d",
            "build/serde-9999aaaabbbbcccc/output",
            "librustmorloc.d",
        ] {
            touch(&rel.join(f));
            age(&rel.join(f), OLD);
        }
        touch(&rel.join(".cargo-lock"));
        touch(&state.join("cache/rust-build/CACHEDIR.TAG"));
        touch(&state.join("cache/rust-env.v2.lock"));
        (t, Roots { state, home })
    }

    fn rel(r: &Roots) -> PathBuf {
        r.rust_build().join("release")
    }

    #[test]
    fn clean_removes_old_pool_outputs_and_keeps_the_crate_cache() {
        let (_t, r) = fixture();
        clean_files(&r, false, false).unwrap();
        let rel = rel(&r);
        assert!(!rel.join(P1).exists());
        assert!(!rel.join(format!("deps/{P1}-0123456789abcdef")).exists());
        assert!(!rel.join(format!("build/{P1}-fedcba9876543210")).exists());
        assert!(!rel.join(format!(".fingerprint/{P1}-0123456789abcdef")).exists());
        assert!(rel.join("deps/librustmorloc-1111222233334444.rlib").exists());
        assert!(rel.join("build/serde-9999aaaabbbbcccc/output").exists());
        assert!(rel.join("librustmorloc.d").exists());
        assert!(rel.join(".cargo-lock").exists());
        assert!(r.state.join("cache/rust-env.v2.lock").exists());
    }

    #[test]
    fn clean_keeps_a_pool_built_in_the_last_hour() {
        let (_t, r) = fixture();
        let cats = plan(&r, true, SystemTime::now());
        assert_eq!(cats[0].kept_recent, 1);
        clean_files(&r, true, false).unwrap();
        let rel = rel(&r);
        assert!(rel.join(P2).exists());
        assert!(rel.join(format!("deps/{P2}-0123456789abcdef")).exists());
    }

    #[test]
    fn clean_crates_removes_the_crate_cache_but_not_locks_or_pins() {
        let (_t, r) = fixture();
        clean_files(&r, true, false).unwrap();
        let rel = rel(&r);
        assert!(!rel.join("deps/librustmorloc-1111222233334444.rlib").exists());
        assert!(!rel.join("build/serde-9999aaaabbbbcccc").exists());
        assert!(!rel.join("librustmorloc.d").exists());
        for lock in CARGO_LOCKS {
            assert!(rel.join(lock).exists(), "{lock}");
        }
        assert!(r.rust_build().join("CACHEDIR.TAG").exists());
        assert!(r.state.join("cache/rust-env.v2.lock").exists());
    }

    #[test]
    fn clean_dry_run_removes_nothing() {
        let (_t, r) = fixture();
        clean_files(&r, true, true).unwrap();
        assert!(rel(&r).join(P1).exists());
        assert!(rel(&r).join("deps/librustmorloc-1111222233334444.rlib").exists());
    }

    #[test]
    fn clean_waits_while_cargo_holds_a_lock() {
        let (_t, r) = fixture();
        let path = rel(&r).join(".cargo-build-lock");
        touch(&path);
        let held = Flock::lock(File::open(&path).unwrap(), FlockArg::LockExclusive).unwrap();
        assert!(try_lock_cargo(&r.rust_build()).unwrap().is_none());
        drop(held);
        assert!(try_lock_cargo(&r.rust_build()).unwrap().is_some());
    }

    #[test]
    fn clean_removes_only_old_empty_eval_dirs_and_killed_lock_merges() {
        let (_t, r) = fixture();
        let tmp = r.home.join("tmp");
        let empty_old = tmp.join("morloc-eval--0001");
        let empty_new = tmp.join("morloc-eval--0002");
        let full_old = tmp.join("morloc-eval--0003");
        for d in [&empty_old, &empty_new, &full_old] {
            fs::create_dir_all(d).unwrap();
        }
        touch(&full_old.join("main.loc"));
        age(&empty_old, OLD);
        age(&full_old, OLD);
        let merge_old = r.state.join("cache/rust-env.v2.lock980404-0.tmp");
        let merge_new = r.state.join("cache/rust-env.v2.lock980405-0.tmp");
        touch(&merge_old);
        touch(&merge_new);
        age(&merge_old, OLD);
        clean_files(&r, false, false).unwrap();
        assert!(!empty_old.exists());
        assert!(empty_new.exists());
        assert!(full_old.exists());
        assert!(!merge_old.exists());
        assert!(merge_new.exists());
    }

    #[test]
    fn pool_keys_match_only_pool_crates() {
        assert_eq!(pool_key("pool_3e088d4d61494df9"), Some("pool_3e088d4d61494df9"));
        assert_eq!(pool_key("pool_3e088d4d61494df9.d"), Some("pool_3e088d4d61494df9"));
        assert_eq!(pool_key("pool_3e088d4d61494df9-0a1b2c3d4e5f6a7b"), Some("pool_3e088d4d61494df9"));
        assert_eq!(pool_key("pool_3e088d4d61494df9x"), None);
        assert_eq!(pool_key("pool_3e088d4d6149zzzz"), None);
        assert_eq!(pool_key("libpool_3e088d4d61494df9"), None);
        assert_eq!(pool_key("pool"), None);
    }

    #[test]
    fn disk_usage_counts_a_hard_link_once() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::write(&a, vec![0u8; 64 * 1024]).unwrap();
        let b = t.path().join("b");
        fs::hard_link(&a, &b).unwrap();
        assert_eq!(disk_usage(&[a.clone(), b]), disk_usage(&[a]));
    }
}
