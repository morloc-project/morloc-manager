//! The environment's persisted dependency-spec store.
//!
//! Each program contributes its compiler-produced `envspec.json` to
//! `<env_home>/requirements/{installed,scratch}/...`. The union of these is the
//! environment's declared dependency world; `installed/` alone is the canonical
//! baseline that `clean` resets to. Scratch holds the specs of programs that were
//! built (`morloc make`) but not installed.
//!
//! An installed program occupies `installed/<program>.json`: the environment owns
//! one `exe/<program>` per name, so the name is a unique slot. A scratch build is a
//! `morloc make` anywhere on the filesystem and its name is NOT unique -- `-o nexus`
//! is the overwhelmingly common default -- so it occupies
//! `scratch/<world-digest>/<program>.json`, keyed on the declared world itself. See
//! [`EnvContext::write_spec`].
//!
//! Specs are stored as the raw JSON the compiler emitted -- storing verbatim
//! avoids a second (Rust) serializer for a schema the compiler already owns.

use std::fs::File;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use sha2::{Digest, Sha256};

use crate::envspec::EnvSpec;
use crate::error::{DepsError, Result};
use crate::langsupport::LangSupport;

/// The runtime ingredients a solve needs that the environment context cannot
/// supply on its own: morloc's language-support table, the pixi binary, the
/// conda platform tag, and the channel priority list. Callers assemble these
/// (the manager from its config/provisioning, the in-env agent from its
/// environment) and hand them to `sync`/`clean`.
pub struct SolveInputs<'a> {
    pub lang_support: &'a LangSupport,
    pub pixi_bin: &'a Path,
    pub platform: &'a str,
    pub channels: &'a [String],
}

/// The result of a solve: the captured toolchain activation env-map, and whether
/// a pixi solve actually ran (`false` = served from the solve-cache unchanged).
pub struct SolveOutcome {
    pub activation: Vec<(String, String)>,
    pub solved: bool,
}

/// The env-level requirements artifacts captured before a rebuild so a failed
/// rebuild can roll them back. `None` = the file was absent at snapshot time.
/// Produced by [`EnvContext::snapshot_requirements`], consumed by
/// [`EnvContext::restore_requirements`].
pub struct RequirementsSnapshot {
    toolchain: Option<String>,
    extras: Option<String>,
}

/// Locates an environment's on-disk layout. Constructed from the env's home
/// directory: the manager derives that from its config, the in-env agent from
/// its environment variables. Deliberately agnostic to how the env was located.
pub struct EnvContext {
    env_home: PathBuf,
    /// Explicit pixi project dir. `None` = `<env_home>/pixi` (the native
    /// layout). A container bakes its pixi env at a fixed path distinct from the
    /// state root, so it supplies the location explicitly.
    pixi_dir: Option<PathBuf>,
}

/// The environment's exclusive lock (DEP-4), for the store it was taken on.
/// Functions that mutate an environment take one, so a caller cannot reach
/// them without it.
pub struct EnvLock {
    _flock: Flock<File>,
    home: PathBuf,
}

/// Whether a program's spec belongs to the canonical installed baseline or a
/// transient scratch build. `clean` keeps only `Installed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Installed,
    Scratch,
}

impl Provenance {
    fn dir_name(self) -> &'static str {
        match self {
            Provenance::Installed => "installed",
            Provenance::Scratch => "scratch",
        }
    }
}

/// The scratch sub-directory for a declared world: the first 16 hex digits of the
/// SHA-256 of its canonical JSON. Truncated because it only has to separate the
/// worlds present in one environment, and the full digest makes for an unreadable
/// path.
///
/// It digests the spec re-serialized through [`EnvSpec`] rather than the raw text,
/// so formatting alone cannot split one world across two slots. What CAN split one
/// is a difference the type still carries: list order, or the `morloc_version`
/// stamp after a compiler upgrade. That only ever yields an extra slot, which
/// over-states the environment's world -- the direction that keeps a built program
/// working.
fn world_digest(spec: &EnvSpec) -> Result<String> {
    let canonical = spec.to_json()?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(crate::cert::hex_lower(&digest[..8]))
}

impl EnvContext {
    pub fn new(env_home: impl AsRef<Path>) -> Self {
        EnvContext { env_home: env_home.as_ref().to_path_buf(), pixi_dir: None }
    }

    /// Override the pixi project dir (default `<env_home>/pixi`).
    pub fn with_pixi_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.pixi_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn env_home(&self) -> &Path {
        &self.env_home
    }

    /// `<env_home>/requirements`
    pub fn requirements_dir(&self) -> PathBuf {
        self.env_home.join("requirements")
    }

    fn provenance_dir(&self, p: Provenance) -> PathBuf {
        self.requirements_dir().join(p.dir_name())
    }

    /// Persist a program's compiler-produced envspec JSON under the given
    /// provenance. The spec is validated before it is written so a malformed
    /// spec never enters the store.
    ///
    /// An INSTALLED program occupies `installed/<program>.json`: the environment
    /// owns one `exe/<program>`, so its name is a unique slot.
    ///
    /// A SCRATCH build occupies `scratch/<world-digest>/<program>.json`, keyed on
    /// the declared world it is reporting. Its name cannot key it: the name is the
    /// `-o`/`--name` value, defaulting to the source basename, and unrelated
    /// programs share one constantly (`-o nexus` above all). Keyed by name, the
    /// second program to build overwrites the first's declared dependencies, and
    /// the next solve then evicts packages the first still needs -- from a prefix it
    /// may be running out of. Keyed by world, two programs declaring different
    /// worlds hold two slots and the gathered union covers both, while rebuilding
    /// either with unchanged dependencies rewrites its own slot in place.
    ///
    /// A build whose dependencies CHANGE lands in a new slot and leaves its previous
    /// world behind, to be reclaimed by `clean`. That is deliberate: a scratch build
    /// is a program somewhere on the filesystem that mim cannot see, so mim cannot
    /// tell "this program dropped a dependency" from "a different program built
    /// under the same name". Retaining the old world over-states the environment,
    /// which keeps a built program working; dropping it would break one.
    pub fn write_spec(&self, lock: &EnvLock, program: &str, provenance: Provenance, json: &str) -> Result<()> {
        self.owns(lock)?;
        let spec = EnvSpec::from_json(json)?;
        write_replacing(&self.spec_path(program, provenance, &spec)?, json)
    }

    /// The slot a program's spec occupies under `provenance`.
    fn spec_path(&self, program: &str, provenance: Provenance, spec: &EnvSpec) -> Result<PathBuf> {
        validate_program_name(program)?;
        let dir = match provenance {
            Provenance::Installed => self.provenance_dir(provenance),
            Provenance::Scratch => self.provenance_dir(provenance).join(world_digest(spec)?),
        };
        Ok(dir.join(format!("{program}.json")))
    }

    /// Every slot holding `program` under `provenance`.
    fn named_paths(&self, program: &str, provenance: Provenance) -> Result<Vec<PathBuf>> {
        validate_program_name(program)?;
        Ok(json_paths(&self.provenance_dir(provenance))?
            .into_iter()
            .filter(|p| p.file_stem().and_then(|s| s.to_str()) == Some(program))
            .collect())
    }

    /// Remove EVERY spec stored under `program` in a provenance tier, whichever
    /// declared world keyed it. Installing a program supersedes its scratch builds,
    /// which sit in per-world slots, so superseding must sweep them all rather than
    /// guess a world.
    pub fn remove_named(&self, lock: &EnvLock, program: &str, provenance: Provenance) -> Result<()> {
        self.owns(lock)?;
        for path in self.named_paths(program, provenance)? {
            remove_spec_file(&path)?;
        }
        Ok(())
    }

    /// Delete every scratch spec: the `clean` reset to the installed baseline.
    pub fn clear_scratch(&self, lock: &EnvLock) -> Result<()> {
        self.owns(lock)?;
        let dir = self.provenance_dir(Provenance::Scratch);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DepsError::Env(format!("cannot clear {}: {e}", dir.display()))),
        }
    }

    /// The installed baseline: the env's toolchain pins plus every spec under
    /// `installed/`, ordered by program name for deterministic downstream
    /// rendering. `clean` re-solves exactly this set, so the pins survive it.
    pub fn gather_installed(&self) -> Result<Vec<EnvSpec>> {
        self.gather_installed_with(None)
    }

    fn gather_installed_with(&self, change: Option<&Change>) -> Result<Vec<EnvSpec>> {
        let mut specs = self.tier(Provenance::Installed, change)?;
        if let Some(toolchain) = self.read_toolchain()? {
            specs.push(toolchain);
        }
        Ok(specs)
    }

    /// The scratch builds' specs, ordered by slot.
    pub fn gather_scratch(&self) -> Result<Vec<EnvSpec>> {
        self.tier(Provenance::Scratch, None)
    }

    /// One provenance tier's specs in gather order, as they would be after
    /// `change` were committed.
    fn tier(&self, provenance: Provenance, change: Option<&Change>) -> Result<Vec<EnvSpec>> {
        let dir = self.provenance_dir(provenance);
        let mut paths = json_paths(&dir)?;
        if let Some(c) = change {
            paths.retain(|p| !c.drop.contains(p) && *p != c.path);
            if c.path.starts_with(&dir) {
                paths.push(c.path.clone());
                paths.sort_by(spec_order);
            }
        }
        paths
            .iter()
            .map(|path| match change {
                Some(c) if *path == c.path => Ok(c.spec.clone()),
                _ => read_spec(path),
            })
            .collect()
    }

    /// What `sync` solves: the declared world plus the shim-ABI pin. `mim spec`
    /// renders the same set, so it prints the manifest the next sync solves.
    pub fn sync_world(&self) -> Result<Vec<EnvSpec>> {
        self.sync_world_with(None)
    }

    fn sync_world_with(&self, change: Option<&Change>) -> Result<Vec<EnvSpec>> {
        let mut specs = self.gather_with(change)?;
        self.append_abi_lock(&mut specs)?;
        Ok(specs)
    }

    /// The full declared world: the installed baseline then `scratch`, each
    /// ordered by program name. The total order matters: the union feeds a
    /// solve-cache marker compared as text, so a non-deterministic order churns.
    pub fn gather(&self) -> Result<Vec<EnvSpec>> {
        self.gather_with(None)
    }

    fn gather_with(&self, change: Option<&Change>) -> Result<Vec<EnvSpec>> {
        let mut specs = self.gather_installed_with(change)?;
        specs.extend(self.tier(Provenance::Scratch, change)?);
        Ok(specs)
    }

    /// The stored spec JSON of an installed program.
    pub fn read_installed(&self, program: &str) -> Result<String> {
        let path = self.provenance_dir(Provenance::Installed).join(format!("{program}.json"));
        std::fs::read_to_string(&path)
            .map_err(|e| DepsError::Env(format!("cannot read {}: {e}", path.display())))
    }

    /// `<env_home>/requirements/toolchain.json` -- the env's initialization pins
    /// (from `--lang`). Env-level toolchain configuration, deliberately kept
    /// OUTSIDE the per-program `installed/`/`scratch/` dirs so it is not mistaken
    /// for a program (e.g. it never appears in `program_names`).
    fn toolchain_path(&self) -> PathBuf {
        self.requirements_dir().join("toolchain.json")
    }

    /// Persist the env's toolchain-pin spec (a languages-only EnvSpec), validated
    /// before writing. Folded into every gather so the in-env agent re-solves
    /// with the same pins the manager used.
    pub fn write_toolchain(&self, json: &str) -> Result<()> {
        EnvSpec::from_json(json)?;
        self.write_requirements_file(&self.toolchain_path(), json)
    }

    /// The env's toolchain-pin spec, if one has been persisted.
    fn read_toolchain(&self) -> Result<Option<EnvSpec>> {
        read_optional_spec(&self.toolchain_path())
    }

    /// The languages PINNED as this environment's persistent baseline (the
    /// `--lang` toolchain spec), sorted and deduplicated. These are kept
    /// installed even when no program uses them; a language present in the solved
    /// world but NOT here was provisioned on demand.
    pub fn pinned_languages(&self) -> Result<Vec<String>> {
        let mut langs: Vec<String> = match self.read_toolchain()? {
            Some(spec) => spec.languages.into_iter().map(|l| l.lang).collect(),
            None => Vec::new(),
        };
        langs.sort();
        langs.dedup();
        Ok(langs)
    }

    /// Create the requirements dir and write `json` to a file directly under it
    /// (the toolchain pin, the abi lock). Callers own any pre-write validation.
    fn write_requirements_file(&self, path: &Path, json: &str) -> Result<()> {
        let dir = self.requirements_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| DepsError::Env(format!("cannot create {}: {e}", dir.display())))?;
        std::fs::write(path, json)
            .map_err(|e| DepsError::Env(format!("cannot write {}: {e}", path.display())))
    }

    /// `<env_home>/requirements/abi-lock.json` -- the interpreter minor-version
    /// pins that protect the shim ABI. Kept OUTSIDE `installed/`/`scratch/` (like
    /// `toolchain.json`) so it is never mistaken for a program.
    fn abi_lock_path(&self) -> PathBuf {
        self.requirements_dir().join("abi-lock.json")
    }

    /// Append the shim-ABI pin to a solve set, if one is recorded. Called only by
    /// the in-env solve entry points (`sync`/`clean`), so a make-time or clean
    /// re-solve holds the interpreter minor the shims were built against. It is
    /// deliberately NOT folded into `gather`/`gather_installed`: the manager's
    /// provision solve reads those, and provision is allowed to move the
    /// interpreter (it rebuilds the shims), so the pin must not gate it.
    fn append_abi_lock(&self, specs: &mut Vec<EnvSpec>) -> Result<()> {
        if let Some(abi_lock) = self.read_abi_lock()? {
            specs.push(abi_lock);
        }
        Ok(())
    }

    /// Drop the recorded shim-ABI pin. A rebuild that re-solves from scratch and
    /// rebuilds every binding (`mim update --reinit`) owes the interpreters no
    /// loyalty, so the pin goes before the solve and is re-recorded after it.
    pub fn clear_abi_lock(&self) -> Result<()> {
        remove_if_present(&self.abi_lock_path())
    }

    /// Forget the last successful in-env solve (its world digest and cached
    /// activation), so the next `sync` re-solves and re-captures instead of
    /// fast-pathing past a prefix that was rebuilt underneath it.
    pub fn clear_solve_cache(&self) -> Result<()> {
        remove_if_present(&self.solved_digest_path())?;
        remove_if_present(&self.activation_cache_path())
    }

    /// The env's shim-ABI pin spec, if one has been recorded.
    fn read_abi_lock(&self) -> Result<Option<EnvSpec>> {
        read_optional_spec(&self.abi_lock_path())
    }

    /// Whether this env pins an interpreter minor to protect a built shim's ABI.
    /// A solve failure in such an env may be the pin conflicting with a dependency
    /// that needs a different interpreter version, which callers surface as an
    /// actionable hint.
    pub fn has_abi_lock(&self) -> bool {
        self.abi_lock_path().exists()
    }

    /// `<env_home>/requirements/extras.json` -- the env-level user conda extras
    /// (utilities added via `mim modify --conda-packages`), a plain list of
    /// conda match-specs. Kept OUTSIDE `installed/`/`scratch/` (like
    /// `toolchain.json`/`abi-lock.json`) so it is never mistaken for a program.
    /// Folded into `rendered_manifest`, so the in-env agent's make-time re-solve
    /// keeps these packages the manager installed instead of pruning them.
    fn conda_extras_path(&self) -> PathBuf {
        self.requirements_dir().join("extras.json")
    }

    /// Persist the env's conda extras. An empty list removes the file, so clearing
    /// the extras (e.g. `mim modify` with a shorter list) truly clears them from
    /// later solves rather than leaving a stale set behind.
    pub fn write_conda_extras(&self, extras: &[String]) -> Result<()> {
        if extras.is_empty() {
            return remove_if_present(&self.conda_extras_path());
        }
        let json = serde_json::to_string(extras)
            .map_err(|e| DepsError::Env(format!("cannot serialize conda extras: {e}")))?;
        self.write_requirements_file(&self.conda_extras_path(), &json)
    }

    /// The env's conda extras, or an empty list if none are recorded.
    pub fn read_conda_extras(&self) -> Result<Vec<String>> {
        let path = self.conda_extras_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| DepsError::Env(format!("cannot parse {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(DepsError::Env(format!("cannot read {}: {e}", path.display()))),
        }
    }

    /// Snapshot the env-level requirements artifacts a rebuild's prime step
    /// overwrites -- the `--lang` toolchain pins and the conda extras -- so a
    /// failed `mim modify`/`update` can restore them, exactly as the cert files are
    /// snapshotted. The abi-lock and per-program `installed/` specs are NOT primed
    /// from the (mutated) config, so they are not part of the snapshot.
    pub fn snapshot_requirements(&self) -> RequirementsSnapshot {
        RequirementsSnapshot {
            toolchain: std::fs::read_to_string(self.toolchain_path()).ok(),
            extras: std::fs::read_to_string(self.conda_extras_path()).ok(),
        }
    }

    /// Restore the requirements artifacts captured by [`snapshot_requirements`]:
    /// each file is rewritten to its snapshotted content, or removed if it was
    /// absent when the snapshot was taken (so a modify that ADDED extras/pins rolls
    /// all the way back to none).
    pub fn restore_requirements(&self, snap: &RequirementsSnapshot) -> Result<()> {
        restore_or_remove(&self.toolchain_path(), snap.toolchain.as_deref())?;
        restore_or_remove(&self.conda_extras_path(), snap.extras.as_deref())?;
        Ok(())
    }

    /// Record the shim-ABI pin from this env's freshly solved conda prefix: pin
    /// each interpreter (python, r-base) to the MINOR version `morloc init` just
    /// built its shims against. Folded into every later in-env solve (see
    /// `append_abi_lock`), so a dependency that would bump an interpreter minor
    /// fails the solve legibly rather than silently breaking the shim ABI.
    ///
    /// Called by the manager's provision after its solve, by `clean` after a
    /// re-solve, and by every successful `sync` (DEP-5). A re-provision is free to move
    /// the interpreter; the recorded pin gates the agent's later make-time solves
    /// (see `append_abi_lock`). If the env has no ABI-relevant interpreter, any
    /// stale lock is removed.
    ///
    /// `morloc_version` is stamped into the recorded spec (the env's provisioned
    /// version tag). `support` supplies morloc's supported interpreter windows: an
    /// interpreter whose solved version falls outside its window (e.g. a python
    /// pulled transitively by a non-py program, which conda resolves to a release
    /// morloc bans) is NOT pinned -- pinning it would fold an unsatisfiable interval
    /// into every later solve. See `abi::abi_lock_spec`.
    pub fn record_abi_lock(&self, morloc_version: &str, support: &LangSupport) -> Result<()> {
        let meta = crate::abi::meta_dir(&self.pixi_dir());
        let windows = support.runtime_windows();
        match crate::abi::abi_lock_spec(&meta, morloc_version, &windows) {
            Some(spec) => {
                let json = serde_json::to_string(&spec)
                    .map_err(|e| DepsError::Env(format!("cannot serialize abi lock: {e}")))?;
                if std::fs::read_to_string(self.abi_lock_path()).ok().as_deref() == Some(json.as_str()) {
                    return Ok(());
                }
                self.write_requirements_file(&self.abi_lock_path(), &json)
            }
            None => remove_if_present(&self.abi_lock_path()),
        }
    }

    /// The pixi project directory (holds `pixi.toml`/`pixi.lock` and the solved
    /// prefix): the explicit override if set, else `<env_home>/pixi`.
    pub fn pixi_dir(&self) -> PathBuf {
        self.pixi_dir
            .clone()
            .unwrap_or_else(|| self.env_home.join("pixi"))
    }

    /// The per-env solve lock. Kept at the `env_home` root (never under
    /// `requirements/`, so `gather` cannot mistake it for a program). All
    /// processes that mutate this env share its home -- in a container the agent's
    /// `env_home` is the host env dir via bind mount, so the same inode (hence the
    /// same `flock`) is seen across the host/container boundary.
    pub fn solve_lock_path(&self) -> PathBuf {
        self.env_home.join(".solve.lock")
    }

    /// The environment id recorded in the store (DEP-12).
    pub fn store_id_path(&self) -> PathBuf {
        self.requirements_dir().join("env-id")
    }

    /// The environment id recorded in the pixi project (DEP-12).
    pub fn pixi_id_path(&self) -> PathBuf {
        self.pixi_dir().join(".morloc-env-id")
    }

    /// DEP-12: give the store an id if it has none and copy it into the pixi
    /// project. Only the manager calls this: it alone holds the data dir that
    /// contains both, so it alone knows they belong together.
    pub fn adopt_env_ids(&self, lock: &EnvLock) -> Result<()> {
        self.owns(lock)?;
        let id = match read_trimmed(&self.store_id_path())? {
            Some(id) => id,
            None => {
                let id = fresh_env_id(&self.env_home);
                write_replacing(&self.store_id_path(), &format!("{id}\n"))?;
                id
            }
        };
        if read_trimmed(&self.pixi_id_path())?.as_deref() != Some(id.as_str()) {
            write_replacing(&self.pixi_id_path(), &format!("{id}\n"))?;
        }
        Ok(())
    }

    /// DEP-12: refuse unless the store and the pixi project hold one id.
    pub fn check_env_ids(&self) -> Result<()> {
        let store = read_trimmed(&self.store_id_path())?;
        let pixi = read_trimmed(&self.pixi_id_path())?;
        match (store, pixi) {
            (Some(a), Some(b)) if a == b => Ok(()),
            (Some(_), Some(_)) => Err(DepsError::Env(format!(
                "the requirements store {} and the pixi project {} belong to different \
                 environments; nothing was changed. A build sees its environment's store \
                 through MORLOC_ENV_STORE, which `mim run` and `mim shell` set.",
                self.requirements_dir().display(),
                self.pixi_dir().display()
            ))),
            _ => Err(DepsError::Env(format!(
                "the requirements store {} or the pixi project {} has no environment id; \
                 nothing was changed. `mim update` records it, as does starting the \
                 environment with `mim run` or `mim shell` as a user who can write it.",
                self.requirements_dir().display(),
                self.pixi_dir().display()
            ))),
        }
    }

    /// The lock must be this store's.
    fn owns(&self, lock: &EnvLock) -> Result<()> {
        if lock.home == self.env_home {
            Ok(())
        } else {
            Err(DepsError::Env(format!(
                "the lock on {} does not cover {}; nothing was changed",
                lock.home.display(),
                self.env_home.display()
            )))
        }
    }

    /// DEP-4: take the environment's lock on the file its store holds now,
    /// blocking until it is free. A first non-blocking attempt lets us tell the
    /// user we are waiting rather than pausing silently.
    fn lock_solve(&self) -> Result<Flock<File>> {
        self.lock_from(self.open_lock_file()?)
    }

    /// Lock `file`, and the store's lock file again whenever the one locked is
    /// no longer the one at its path.
    fn lock_from(&self, mut file: File) -> Result<Flock<File>> {
        let path = self.solve_lock_path();
        loop {
            let lock_err = |e: Errno| DepsError::Env(format!("cannot lock {}: {e}", path.display()));
            let guard = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(guard) => guard,
                Err((file, Errno::EAGAIN)) => {
                    eprintln!("Waiting for another dependency sync on this environment to finish...");
                    Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, e)| lock_err(e))?
                }
                Err((_, e)) => return Err(lock_err(e)),
            };
            if is_current(&guard, &path) {
                return Ok(guard);
            }
            file = self.open_lock_file()?;
        }
    }

    fn open_lock_file(&self) -> Result<File> {
        let path = self.solve_lock_path();
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => DepsError::Env(format!(
                    "the environment store {} does not exist; nothing was changed. If the \
                     environment was renamed or removed, start it again with `mim run` or \
                     `mim shell`.",
                    self.env_home.display()
                )),
                _ => DepsError::Env(format!("cannot open solve lock {}: {e}", path.display())),
            })
    }

    /// Render the given specs to a pixi manifest (the merged, backend-neutral
    /// requirement set lowered to `pixi.toml` text). No solve, no I/O. Fails on a
    /// cross-program conda channel conflict for one package.
    pub fn rendered_manifest(&self, specs: &[EnvSpec], inputs: &SolveInputs) -> Result<String> {
        Ok(crate::pixi::render_manifest(&self.requirement_set(specs, inputs)?))
    }

    /// Build this env's declared world (the `RequirementSet`) from the gathered specs
    /// + language support + the persisted conda extras, via the ONE shared
    /// `pixi::build_requirement_set` the manager also uses (`build_env_requirements`).
    /// The single builder is what keeps the manager and agent from diverging on how
    /// the world is assembled -- the two-writer drift behind the byte-identity bugs.
    fn requirement_set(
        &self,
        specs: &[EnvSpec],
        inputs: &SolveInputs,
    ) -> Result<crate::pixi::RequirementSet> {
        crate::pixi::build_requirement_set(
            &crate::pixi::PixiManifestInput {
                platform: inputs.platform,
                channels: inputs.channels,
                specs,
                lang_support: inputs.lang_support,
            },
            &self.read_conda_extras()?,
        )
    }

    /// Render the given specs to a pixi manifest and solve it into this env's
    /// pixi prefix, returning the captured activation env-map. Does NOT persist
    /// specs or rebuild the morloc runtime -- callers own those steps.
    ///
    /// Materialize the declared world into this env's pixi prefix.
    ///
    /// DEP-6: the solve is skipped when the declared world's digest
    /// (`requirement_digest`) and the `pixi.toml` on disk both match the last
    /// successful solve here, the prefix exists and the activation is cached. A
    /// manifest some other writer put there since is solved again.
    ///
    /// When the world DID change, `try_locked` picks the strategy:
    /// - `true` (sync): try `pixi install --locked` FIRST -- if the existing lock still
    ///   satisfies the manifest (e.g. only the abi-lock narrowed an already-locked
    ///   interpreter), the manager's TESTED versions are reused with ZERO drift; only
    ///   if the lock cannot satisfy the world do we re-solve (`solved = true`).
    /// - `false` (clean): always re-solve. Clean's job is to DROP an unreferenced
    ///   language; installing the old lock would keep it, so the lock must be rebuilt.
    ///
    /// `solved` is `true` only when a real re-solve ran (a `--locked` install reuses
    /// the lock, so the world is unchanged -> `false`); `clean` re-records the
    /// abi-lock only then.
    pub fn solve_world(
        &self,
        specs: &[EnvSpec],
        inputs: &SolveInputs,
        try_locked: bool,
    ) -> Result<SolveOutcome> {
        let reqs = self.requirement_set(specs, inputs)?;
        let digest = crate::pixi::requirement_digest(&reqs);
        let pixi_dir = self.pixi_dir();
        let cache_path = self.activation_cache_path();
        let digest_path = self.solved_digest_path();
        let prefix = crate::abi::conda_prefix(&pixi_dir);
        // DEP-6: the recorded digest names the world and the manifest that solve
        // wrote; a manifest another writer replaced since is solved again.
        let on_disk = std::fs::read_to_string(pixi_dir.join("pixi.toml")).ok();
        let unchanged = match (std::fs::read_to_string(&digest_path), on_disk) {
            (Ok(prev), Some(manifest)) => prev == solved_digest(&digest, &manifest),
            _ => false,
        };
        if unchanged && prefix.is_dir() {
            if let Some(activation) = read_activation_cache(&cache_path) {
                return Ok(SolveOutcome { activation, solved: false });
            }
        }
        // Invalidate the cached activation BEFORE touching the prefix. Otherwise an
        // interrupted materialize (SIGKILL mid install) that has already written the
        // new digest would leave a half-installed prefix that still passes `is_dir()`
        // plus a STALE cache -- so the next build would cache-hit with the wrong
        // activation. Written last (after success), it is absent until the prefix is
        // whole, forcing a corrective re-materialize.
        let _ = std::fs::remove_file(&cache_path);
        let _ = std::fs::remove_file(&digest_path);
        let mut written = crate::pixi::render_manifest(&reqs);
        crate::pixi::write_manifest(&pixi_dir, &written)?;
        // In-env agent path (`mim sync`): the built image already sets the CA env vars,
        // which the pixi subprocess inherits, so no override is needed. No FHS
        // wrapping: the in-env agent already runs inside its execution environment.
        let mut solved = if try_locked
            && crate::pixi::install_locked(&pixi_dir, inputs.pixi_bin, None, None)?
        {
            false
        } else {
            crate::pixi::solve(&pixi_dir, inputs.pixi_bin, None, None)?;
            true
        };
        // An interpreter the solve pulled in that nothing declares (a conda extra
        // depending on python) is a language this environment now has: `morloc
        // init` will build its binding, which needs the binder dependencies and an
        // interpreter inside morloc's window. Declare it and solve again. The
        // digest recorded below stays the declared world's, so the next make still
        // fast-paths; the pin recorded after this solve is what carries the
        // adoption forward.
        let locked = crate::pixi::locked_packages(&pixi_dir, inputs.platform)?;
        let pulled = crate::abi::undeclared_shim_runtimes(&locked, specs, inputs.lang_support);
        if !pulled.is_empty() {
            eprintln!("{}", adoption_note(&pulled));
            let mut adopted = specs.to_vec();
            adopted.push(adopted_language_spec(&pulled, &inputs.lang_support.morloc_version));
            let reqs = self.requirement_set(&adopted, inputs)?;
            written = crate::pixi::render_manifest(&reqs);
            crate::pixi::write_manifest(&pixi_dir, &written)?;
            crate::pixi::solve(&pixi_dir, inputs.pixi_bin, None, None)?;
            solved = true;
        }
        // Keep the host-readable record mirror in step with the prefix that was
        // just installed, so a host reading this environment does not describe a
        // world it has moved on from. A failure here leaves the prefix correct and
        // only its description behind, which is not worth failing a build over, but
        // it is worth saying: the stale copy is what the host would then report.
        if let Err(e) = crate::abi::refresh_conda_meta_mirror(&prefix, &pixi_dir) {
            eprintln!(
                "Warning: the environment was installed, but its host-readable record \
                 copy could not be updated ({e}). Until the next successful install, \
                 tools reading this environment from outside it describe the previous \
                 set of packages."
            );
        }
        // The world just changed, so a package new to this env may ship paths that
        // a case-folding filesystem cannot keep apart. Checked before the
        // activation is cached, so a refused prefix leaves no usable state.
        crate::casefold::check_pixi_dir(&pixi_dir)?;
        let activation =
            crate::pixi::capture_activation(&pixi_dir, inputs.pixi_bin, None, None)?;
        write_activation_cache(&cache_path, &activation);
        // Record the digest of the world we just materialized so the next make can
        // fast-path. Written only after success, so a failed build is retried.
        let _ = std::fs::write(&digest_path, solved_digest(&digest, &written));
        Ok(SolveOutcome { activation, solved })
    }

    /// The record of the last successful solve: the declared world's digest and
    /// the digest of the manifest it wrote (DEP-6).
    fn solved_digest_path(&self) -> PathBuf {
        self.pixi_dir().join(".morloc-solved-digest")
    }

    /// The cached toolchain activation env-map file for the current solve.
    fn activation_cache_path(&self) -> PathBuf {
        self.pixi_dir().join(".morloc-activation.json")
    }

    /// The morloc languages whose RUNTIME is present in the solved world (read
    /// from `pixi.lock`). Empty before the first solve. Used by `clean` to detect
    /// which languages a re-solve DROPPED (so the agent can invalidate their stale
    /// shim markers).
    fn runtime_language_set(&self, platform: &str) -> Result<Vec<String>> {
        let locked = crate::pixi::locked_packages(&self.pixi_dir(), platform)?;
        Ok(crate::pixi::runtime_languages(&locked)
            .into_iter()
            .map(|(lang, _version)| lang.to_string())
            .collect())
    }

    /// DEP-5: merge a program's spec into the declared world under
    /// `provenance`, replacing its scratch builds when `supersede_scratch`, and
    /// solve. The store changes only after the solve succeeds, so a failed or
    /// interrupted sync leaves it as it was. The world solved is the one the
    /// store then holds, in gather order.
    pub fn sync(
        &self,
        lock: &EnvLock,
        program: &str,
        provenance: Provenance,
        spec_json: &str,
        supersede_scratch: bool,
        inputs: &SolveInputs,
    ) -> Result<Vec<(String, String)>> {
        self.owns(lock)?;
        self.check_env_ids()?;
        let spec = EnvSpec::from_json(spec_json)?;
        let change = Change {
            path: self.spec_path(program, provenance, &spec)?,
            spec,
            drop: if supersede_scratch { self.named_paths(program, Provenance::Scratch)? } else { Vec::new() },
        };
        let specs = self.sync_world_with(Some(&change))?;
        // Prefer the manager's tested lock (`--locked`); re-solve only if it cannot
        // satisfy this program's world.
        let outcome = self.solve_world(&specs, inputs, true)?;
        self.commit(&change, spec_json)?;
        // DEP-5: the pin follows the prefix; one this sync cannot record, the
        // next records.
        if let Err(e) = self.record_abi_lock(inputs.lang_support.morloc_version.as_str(), inputs.lang_support) {
            eprintln!("Warning: could not record the interpreter version pin: {e}");
        }
        Ok(outcome.activation)
    }

    /// Write a sync's spec and remove the specs it supersedes. With removals,
    /// a note naming them and the spec's digest is written first, so a commit
    /// a crash interrupts is finished by the next lock (DEP-5).
    fn commit(&self, change: &Change, spec_json: &str) -> Result<()> {
        if change.drop.is_empty() {
            return write_replacing(&change.path, spec_json);
        }
        let rel = |p: &Path| p.strip_prefix(self.requirements_dir()).unwrap_or(p).display().to_string();
        let mut note = format!("{}\n{}\n", rel(&change.path), crate::cert::hex_lower(&Sha256::digest(spec_json)));
        for path in &change.drop {
            note.push_str(&format!("{}\n", rel(path)));
        }
        write_replacing(&self.pending_commit_path(), &note)?;
        write_replacing(&change.path, spec_json)?;
        self.finish_commit()
    }

    fn pending_commit_path(&self) -> PathBuf {
        self.requirements_dir().join("pending-commit")
    }

    /// Finish a commit whose spec was written: remove the specs it supersedes.
    /// A note whose spec was never written is dropped with nothing removed.
    fn finish_commit(&self) -> Result<()> {
        let Some(note) = read_trimmed(&self.pending_commit_path())? else {
            return Ok(());
        };
        let mut lines = note.lines();
        let (Some(spec), Some(digest)) = (lines.next(), lines.next()) else {
            return remove_if_present(&self.pending_commit_path());
        };
        let dir = self.requirements_dir();
        let written = std::fs::read(dir.join(spec))
            .map(|bytes| crate::cert::hex_lower(&Sha256::digest(bytes)) == digest)
            .unwrap_or(false);
        if written {
            for path in lines {
                remove_spec_file(&dir.join(path))?;
            }
        }
        remove_if_present(&self.pending_commit_path())
    }

    /// The names (file stems) of the installed programs only (the canonical
    /// baseline), sorted. Excludes transient scratch builds -- unlike
    /// `program_names`, which unions both tiers.
    pub fn installed_program_names(&self) -> Result<Vec<String>> {
        let mut names = program_stems(&self.provenance_dir(Provenance::Installed))?;
        names.sort();
        Ok(names)
    }

    /// The names (file stems) of every program with a stored spec, sorted and
    /// deduplicated. Used to attribute a conflict.
    pub fn program_names(&self) -> Result<Vec<String>> {
        let mut names = program_stems(&self.provenance_dir(Provenance::Installed))?;
        names.extend(program_stems(&self.provenance_dir(Provenance::Scratch))?);
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Reset the declared world to the installed baseline: solve the installed
    /// programs (plus the pinned toolchain), then drop every scratch spec.
    /// Unlike `sync`, this does NOT append the abi-lock: clean is the one path
    /// allowed to DROP an unreferenced on-demand language,
    /// so it re-solves free of the old interpreter pins, then re-records the pin
    /// for whatever interpreters survive (pruning pins for dropped ones).
    /// Pinned (`--lang`) languages live in the toolchain spec and so survive;
    /// on-demand languages no program references are dropped. Returns the
    /// languages whose runtime was dropped, so the agent can invalidate their
    /// stale shim markers.
    pub fn clean(&self, lock: &EnvLock, inputs: &SolveInputs) -> Result<Vec<String>> {
        self.owns(lock)?;
        self.check_env_ids()?;
        let mut specs = self.gather_installed()?;
        let before = self.runtime_language_set(inputs.platform)?;
        // Keep the abi-lock pin ONLY for languages still referenced by an
        // installed program or a `--lang` pin, so their shims stay valid; drop it
        // for languages no program references so those can leave the solved world.
        // (Appending the full pin unconditionally would force every once-added
        // interpreter to stay -- no cleanup; dropping it unconditionally could
        // move a still-referenced interpreter and break its shim.)
        let referenced: std::collections::BTreeSet<String> = specs
            .iter()
            .flat_map(|s| s.languages.iter().map(|l| l.lang.clone()))
            .collect();
        if let Some(mut abi) = self.read_abi_lock()? {
            abi.languages.retain(|l| referenced.contains(&l.lang));
            if !abi.languages.is_empty() {
                specs.push(abi);
            }
        }
        // Force a real re-solve (never `--locked`): clean's purpose is to DROP an
        // unreferenced language, and installing the existing lock would keep it.
        let outcome = self.solve_world(&specs, inputs, false)?;
        self.clear_scratch(lock)?;
        let dropped = if outcome.solved {
            self.record_abi_lock(inputs.lang_support.morloc_version.as_str(), inputs.lang_support)?;
            let after = self.runtime_language_set(inputs.platform)?;
            before.into_iter().filter(|l| !after.contains(l)).collect()
        } else {
            Vec::new()
        };
        Ok(dropped)
    }

    /// The environment's lock (DEP-4). The store must exist: a build hook must
    /// not make one for an environment that was removed.
    /// A commit a crash interrupted is finished first (DEP-5), once the store
    /// and the project are known to be one environment's (DEP-12).
    pub fn lock_env(&self) -> Result<EnvLock> {
        let lock = self.lock_env_as_is()?;
        if self.check_env_ids().is_ok() {
            self.finish_commit().map_err(|e| {
                DepsError::Env(format!(
                    "{e}; an interrupted change to the requirements store could not be \
                     finished. Removing {} and running `mim clean --uninstalled` resets the store.",
                    self.pending_commit_path().display()
                ))
            })?;
        }
        Ok(lock)
    }

    /// The environment's lock, with the store left exactly as found: for
    /// removing or moving the whole environment.
    pub fn lock_env_as_is(&self) -> Result<EnvLock> {
        Ok(EnvLock { _flock: self.lock_solve()?, home: self.env_home.clone() })
    }

    /// The environment's lock, making its store first if it has none: for the
    /// manager, which creates environments.
    pub fn lock_env_creating(&self) -> Result<EnvLock> {
        std::fs::create_dir_all(&self.env_home)
            .map_err(|e| DepsError::Env(format!("cannot create {}: {e}", self.env_home.display())))?;
        self.lock_env()
    }
}

/// A spec write a sync commits once its solve succeeds: the program's slot
/// and the slots it supersedes.
struct Change {
    path: PathBuf,
    spec: EnvSpec,
    drop: Vec<PathBuf>,
}

/// Whether `file` is still the file at `path`.
fn is_current(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Read + parse an optional on-disk spec file (the toolchain pin, the abi lock):
/// a missing file is `None`, a present one is validated.
fn read_optional_spec(path: &Path) -> Result<Option<EnvSpec>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(EnvSpec::from_json(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DepsError::Env(format!("cannot read {}: {e}", path.display()))),
    }
}

/// The cached toolchain activation env-map, if present and parseable. A missing,
/// unreadable, or corrupt cache yields `None` (forcing a fresh solve), never an
/// error -- the cache is an optimization, not a source of truth.
fn read_activation_cache(path: &Path) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut activation = serde_json::from_str::<Vec<(String, String)>>(&text).ok()?;
    if crate::pixi::activated_twice(&activation) {
        return None;
    }
    crate::pixi::normalize_activation(&mut activation);
    Some(activation)
}

/// Persist the activation env-map beside the solved prefix. Best-effort: a write
/// failure must not fail the solve (the cache is an optimization); the next
/// solve simply re-captures.
fn write_activation_cache(path: &Path, activation: &[(String, String)]) {
    if let Ok(json) = serde_json::to_string(activation) {
        let _ = std::fs::write(path, json);
    }
}

/// Rewrite `path` to `content`, or remove it if `content` is `None` (the file was
/// absent when snapshotted). Creates the parent dir on write. The rollback
/// counterpart of a snapshot capture.
fn restore_or_remove(path: &Path, content: Option<&str>) -> Result<()> {
    match content {
        Some(text) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    DepsError::Env(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(path, text)
                .map_err(|e| DepsError::Env(format!("cannot write {}: {e}", path.display())))
        }
        None => remove_if_present(path),
    }
}

/// Write `text` to `path` through a sibling temporary file and a rename, so a
/// reader sees the old content or the new, never part of it.
fn write_replacing(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DepsError::Env(format!("cannot create {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            DepsError::Env(format!("cannot write {}: {e}", path.display()))
        })
}

/// The solve record: the declared world's digest and a digest of the manifest
/// text that solve wrote.
fn solved_digest(world: &str, manifest: &str) -> String {
    let m = crate::cert::hex_lower(&Sha256::digest(manifest.as_bytes()));
    format!("{world}\nmanifest {m}\n")
}

/// A file's trimmed content; `None` when it is absent or empty.
fn read_trimmed(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => Ok(None),
        Ok(t) => Ok(Some(t.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DepsError::Env(format!("cannot read {}: {e}", path.display()))),
    }
}

/// A new environment id: a digest of this process, the store path and the
/// clock, which no other environment's id repeats.
fn fresh_env_id(env_home: &Path) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!("{}\0{}\0{nanos}", std::process::id(), env_home.display());
    crate::cert::hex_lower(&Sha256::digest(seed.as_bytes())[..16])
}

/// Remove a file, tolerating its absence (an uninstall, or clearing a stale lock).
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DepsError::Env(format!("cannot remove {}: {e}", path.display()))),
    }
}

/// A languages-only spec declaring `langs` (morloc short codes) with no version
/// constraint, so the language clamp bounds each to morloc's supported window
/// and its binder dependencies join the world. Built for interpreters a solve
/// pulled in without a declaration; see `abi::undeclared_shim_runtimes`.
pub fn adopted_language_spec(langs: &[String], morloc_version: &str) -> EnvSpec {
    EnvSpec::from_languages(
        morloc_version,
        langs
            .iter()
            .map(|lang| crate::envspec::LangReq { lang: lang.clone(), constraint: None, std: None })
            .collect(),
    )
}

/// What the user is told when a solve adopts interpreters no program declared.
pub fn adoption_note(langs: &[String]) -> String {
    let names: Vec<&str> = langs.iter().map(|l| crate::abi::language_display(l)).collect();
    format!(
        "Note: the solve pulled in {} without a program declaring it; adopting it as \
         an environment language so it stays within morloc's supported range and \
         its morloc binding can be built.",
        names.join(", ")
    )
}

/// Remove one stored spec file, then its directory once empty -- an empty
/// store directory reads exactly like an absent one everywhere here. A
/// missing file is not an error.
fn remove_spec_file(path: &Path) -> Result<()> {
    remove_if_present(path)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

/// Every spec file under `dir`: the name-only slots (`<name>.json`) plus the
/// world-keyed ones one level down (`<world-digest>/<name>.json`). Sorted by
/// program name, then by path, so the gather order is deterministic (it feeds the
/// merged requirement set) and reads in program order. A missing directory yields
/// an empty list.
fn json_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_json(dir, true, &mut files)?;
    files.sort_by(spec_order);
    Ok(files)
}

/// Gather order: by program name, then by path.
fn spec_order(a: &PathBuf, b: &PathBuf) -> std::cmp::Ordering {
    let stem = |p: &PathBuf| p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    stem(a).cmp(&stem(b)).then_with(|| a.cmp(b))
}

/// Append the `*.json` files directly under `dir` to `out`, descending one level
/// into subdirectories when `descend` is set. World slots nest exactly one level,
/// so this never recurses further.
fn collect_json(dir: &Path, descend: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(DepsError::Env(format!("cannot read {}: {e}", dir.display()))),
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            if descend {
                collect_json(&path, false, out)?;
            }
        } else if path.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

/// Read and parse one stored spec.
fn read_spec(path: &Path) -> Result<EnvSpec> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| DepsError::Env(format!("cannot read {}: {e}", path.display())))?;
    EnvSpec::from_json(&text)
}

/// The `.json` file stems (program names) directly under `dir`.
fn program_stems(dir: &Path) -> Result<Vec<String>> {
    Ok(json_paths(dir)?
        .into_iter()
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
        .collect())
}

/// A program name is used as a bare file stem in the store, so it must not
/// contain path separators or traversal segments -- otherwise a crafted name
/// could write outside the store.
fn validate_program_name(program: &str) -> Result<()> {
    let bad = program.is_empty()
        || program == "."
        || program == ".."
        || program.contains('/')
        || program.contains('\\')
        || program.contains(std::path::MAIN_SEPARATOR);
    if bad {
        return Err(DepsError::Env(format!(
            "invalid program name '{program}': must be non-empty and contain no path separators"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid specs (envspec_version 1); the program name is carried by the
    // file name, not the spec body.
    fn spec(langs: &str) -> String {
        format!(
            r#"{{"envspec_version":2,"morloc_version":"0.0.0","languages":[{langs}]}}"#
        )
    }

    fn lk(ctx: &EnvContext) -> EnvLock {
        ctx.lock_env_creating().unwrap()
    }

    fn ctx() -> (tempfile::TempDir, EnvContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = EnvContext::new(dir.path());
        (dir, ctx)
    }

    #[test]
    fn gather_unions_installed_and_scratch_in_stable_order() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "bravo", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "alpha", Provenance::Installed, &spec(r#"{"lang":"cpp"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "zeta", Provenance::Scratch, &spec(r#"{"lang":"r"}"#)).unwrap();

        let all = ctx.gather().unwrap();
        assert_eq!(all.len(), 3);
        // installed (alpha, bravo by file-name order) then scratch (zeta)
        assert_eq!(all[0].languages[0].lang, "cpp"); // alpha
        assert_eq!(all[1].languages[0].lang, "py"); // bravo
        assert_eq!(all[2].languages[0].lang, "r"); // zeta

        let installed = ctx.gather_installed().unwrap();
        assert_eq!(installed.len(), 2);
    }

    #[test]
    fn clear_scratch_resets_to_installed_baseline() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "keep", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "temp", Provenance::Scratch, &spec(r#"{"lang":"r"}"#)).unwrap();
        assert_eq!(ctx.gather().unwrap().len(), 2);

        ctx.clear_scratch(&lk(&ctx)).unwrap();
        let after = ctx.gather().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].languages[0].lang, "py");
    }

    #[test]
    fn scratch_builds_declaring_different_worlds_do_not_overwrite_each_other() {
        let (_d, ctx) = ctx();
        // Two unrelated programs, both built as `nexus` (the -o default), declaring
        // different worlds. Keyed on the name alone the second erases the first, and
        // the next solve drops what the first still needs.
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"r"}"#)).unwrap();

        let mut langs: Vec<String> =
            ctx.gather().unwrap().iter().map(|s| s.languages[0].lang.clone()).collect();
        langs.sort();
        assert_eq!(langs, vec!["py".to_string(), "r".to_string()]);
        // Still one program: the two slots are two declared worlds of one NAME.
        assert_eq!(ctx.program_names().unwrap(), vec!["nexus".to_string()]);
    }

    #[test]
    fn rebuilding_an_unchanged_world_rewrites_one_slot() {
        let (_d, ctx) = ctx();
        // The common case: the same program rebuilt over and over. Its world is
        // unchanged, so it must not accumulate a slot per build.
        for _ in 0..5 {
            ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py"}"#)).unwrap();
        }
        assert_eq!(ctx.gather().unwrap().len(), 1);
    }

    #[test]
    fn install_supersedes_every_scratch_world_of_that_name() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"r"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Installed, &spec(r#"{"lang":"cpp"}"#)).unwrap();

        ctx.remove_named(&lk(&ctx), "nexus", Provenance::Scratch).unwrap();
        let all = ctx.gather().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].languages[0].lang, "cpp");
        // A superseded name leaves no empty world directories behind.
        assert!(std::fs::read_dir(ctx.requirements_dir().join("scratch"))
            .map(|d| d.count() == 0)
            .unwrap_or(true));
    }

    #[test]
    fn clear_scratch_drops_every_world_slot() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"r"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "keep", Provenance::Installed, &spec(r#"{"lang":"cpp"}"#)).unwrap();
        ctx.clear_scratch(&lk(&ctx)).unwrap();

        let all = ctx.gather().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].languages[0].lang, "cpp");
    }

    #[test]
    fn gather_is_empty_when_no_store_exists() {
        let (_d, ctx) = ctx();
        assert!(ctx.gather().unwrap().is_empty());
        assert!(ctx.gather_installed().unwrap().is_empty());
    }

    #[test]
    fn write_spec_rejects_malformed_json() {
        let (_d, ctx) = ctx();
        let r = ctx.write_spec(&lk(&ctx), "bad", Provenance::Scratch, "{ not json");
        assert!(r.is_err());
    }

    #[test]
    fn toolchain_folds_into_baseline_but_is_not_a_program() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "prog", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_toolchain(&spec(r#"{"lang":"py","constraint":"==3.12"}"#)).unwrap();

        // the pins are part of the installed baseline (so `clean` keeps them)...
        assert_eq!(ctx.gather_installed().unwrap().len(), 2);
        assert_eq!(ctx.gather().unwrap().len(), 2);
        // ...but they are NOT a program: never surfaced in conflict attribution.
        assert_eq!(ctx.program_names().unwrap(), vec!["prog".to_string()]);
    }

    #[test]
    fn abi_lock_gates_only_the_in_env_solve_not_the_gather() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "prog", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        std::fs::create_dir_all(ctx.requirements_dir()).unwrap();
        std::fs::write(
            ctx.abi_lock_path(),
            spec(r#"{"lang":"py","constraint":"3.12.*"}"#),
        )
        .unwrap();

        // The gather the MANAGER's provision reads must NOT include the pin (so a
        // re-provision is free to move the interpreter and rebuild the shims).
        assert_eq!(ctx.gather_installed().unwrap().len(), 1);
        assert_eq!(ctx.gather().unwrap().len(), 1);
        // The in-env solve set (sync/clean) DOES include it.
        let mut solve_set = ctx.gather_installed().unwrap();
        ctx.append_abi_lock(&mut solve_set).unwrap();
        assert_eq!(solve_set.len(), 2);
        // ...but the pin is NOT a program: never surfaced in conflict attribution.
        assert_eq!(ctx.program_names().unwrap(), vec!["prog".to_string()]);
        // `mim spec` renders the sync world, pin included.
        assert_eq!(ctx.sync_world().unwrap().len(), 2);
    }

    #[test]
    fn conda_extras_round_trip_and_clear() {
        let (_d, ctx) = ctx();
        assert!(ctx.read_conda_extras().unwrap().is_empty());
        ctx.write_conda_extras(&["nvim".into(), "bat>=0.24".into()]).unwrap();
        assert_eq!(
            ctx.read_conda_extras().unwrap(),
            vec!["nvim".to_string(), "bat>=0.24".to_string()]
        );
        // An empty list removes the file, so a shrunk extras list truly clears
        // (a later solve must not re-add a package the user dropped).
        ctx.write_conda_extras(&[]).unwrap();
        assert!(ctx.read_conda_extras().unwrap().is_empty());
        assert!(!ctx.conda_extras_path().exists());
    }

    #[test]
    fn requirements_snapshot_restores_toolchain_and_extras() {
        // A failed `mim modify` must roll the requirements store back to exactly its
        // pre-modify state: files that existed are restored to their old content,
        // and files the modify ADDED are removed (not left with the new values).
        let (_d, ctx) = ctx();
        // Pre-modify state: a toolchain pin exists, no extras yet.
        ctx.write_toolchain(&spec(r#"{"lang":"py","constraint":"==3.12"}"#)).unwrap();
        let snap = ctx.snapshot_requirements();

        // The (failed) modify's prime step overwrites toolchain and adds extras.
        ctx.write_toolchain(&spec(r#"{"lang":"py","constraint":"==3.13"}"#)).unwrap();
        ctx.write_conda_extras(&["nvim".into()]).unwrap();

        // Rollback restores the old toolchain and clears the added extras.
        ctx.restore_requirements(&snap).unwrap();
        assert_eq!(ctx.pinned_languages().unwrap(), vec!["py".to_string()]);
        assert_eq!(
            ctx.read_toolchain().unwrap().unwrap().languages[0].constraint.as_deref(),
            Some("==3.12")
        );
        assert!(ctx.read_conda_extras().unwrap().is_empty());
        assert!(!ctx.conda_extras_path().exists());
    }

    /// A support table whose py binder pulls numpy, so a user extra can COLLIDE
    /// with an injected dep -- the case that exposes any manager/agent drift.
    fn support_with_numpy() -> LangSupport {
        LangSupport::from_json(
            r#"{"schema_version":"1.0","morloc_version":"0.99.0","languages":{
                "py":{"runtime":{"package":"python","version":">=3.10,<3.14","default":"3.12"},
                      "requires":[{"package":"numpy","constraint":">=1.22,<3","optional":false}]}}}"#,
        )
        .unwrap()
    }

    #[test]
    fn extras_render_identically_in_manager_and_agent_paths() {
        // This is the regression guard for the whole fix: the in-env agent
        // (rendered_manifest, reading extras.json) and the manager (resolve then
        // add_conda_specs) must produce a byte-identical manifest, INCLUDING a
        // constraint that collides with an injected binder dep -- otherwise the
        // make-time solve-cache text compare fails and every build re-solves.
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "prog", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        let extras = vec!["numpy>=1.20".to_string(), "nvim".to_string()];
        ctx.write_conda_extras(&extras).unwrap();

        let support = support_with_numpy();
        let channels = crate::pixi::default_channels();
        let pixi_bin = std::path::Path::new("/nonexistent/pixi"); // render never invokes pixi
        let inputs = SolveInputs {
            lang_support: &support,
            pixi_bin,
            platform: "linux-64",
            channels: &channels,
        };
        let specs = ctx.gather_installed().unwrap();

        // AGENT: render from the store (installed specs) + extras.json.
        let agent = ctx.rendered_manifest(&specs, &inputs).unwrap();

        // MANAGER: resolve the same specs, then fold the same extras via the same
        // helper AFTER resolve (mirrors build_env_requirements), and render.
        let mut req = crate::pixi::resolve_requirements(&crate::pixi::PixiManifestInput {
            platform: "linux-64",
            channels: &channels,
            specs: &specs,
            lang_support: &support,
        })
        .unwrap();
        req.add_conda_specs(&extras).unwrap();
        let manager = crate::pixi::render_manifest(&req);

        assert_eq!(agent, manager, "manager and agent must render byte-identical manifests");
        // The collision merged into one numpy entry; the fresh extra is present.
        assert_eq!(agent.matches("\"numpy\" =").count(), 1, "numpy must appear once:\n{agent}");
        assert!(agent.contains("\"nvim\""), "nvim extra must be present:\n{agent}");
    }

    /// A minimal support table whose python runtime window is morloc's real one
    /// (`>=3.10,<3.14`), so the abi-lock recorder's window filter is exercised.
    fn support() -> LangSupport {
        LangSupport::from_json(
            r#"{"schema_version":"1.0","morloc_version":"0.99.0","languages":{
                "py":{"runtime":{"package":"python","version":">=3.10,<3.14","default":"3.12"}}}}"#,
        )
        .unwrap()
    }

    #[test]
    fn record_abi_lock_pins_solved_interpreter_and_clears_when_absent() {
        let (_d, ctx) = ctx();
        let prefix = crate::abi::conda_prefix(&ctx.pixi_dir());
        let meta = prefix.join("conda-meta");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("python-3.12.5-0.json"), r#"{"name":"python","version":"3.12.5"}"#).unwrap();

        ctx.record_abi_lock("0.99.0", &support()).unwrap();
        let mut solve_set = Vec::new();
        ctx.append_abi_lock(&mut solve_set).unwrap();
        assert_eq!(solve_set.len(), 1);
        assert_eq!(solve_set[0].languages[0].lang, "py");
        assert_eq!(solve_set[0].languages[0].constraint.as_deref(), Some(">=3.12,<3.13"));

        // No interpreter in the prefix -> a subsequent record clears the stale lock.
        std::fs::remove_dir_all(&meta).unwrap();
        std::fs::create_dir_all(&meta).unwrap();
        ctx.record_abi_lock("0.99.0", &support()).unwrap();
        let mut after = Vec::new();
        ctx.append_abi_lock(&mut after).unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn record_abi_lock_declares_python_outside_supported_window_unpinned() {
        // A python 3.14 in the prefix (transitively pulled, banned by the window)
        // must NOT be pinned -- otherwise every later solve folds in an
        // unsatisfiable `>=3.10,<3.14` AND `>=3.14,<3.15` and pixi fails
        // cryptically -- but it IS declared, so the next solve clamps it into the
        // window instead of leaving it where no binding can be built.
        let (_d, ctx) = ctx();
        let prefix = crate::abi::conda_prefix(&ctx.pixi_dir());
        let meta = prefix.join("conda-meta");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("python-3.14.0-0.json"), r#"{"name":"python","version":"3.14.0"}"#).unwrap();

        ctx.record_abi_lock("0.99.0", &support()).unwrap();
        let mut solve_set = Vec::new();
        ctx.append_abi_lock(&mut solve_set).unwrap();
        assert_eq!(solve_set.len(), 1);
        assert_eq!(solve_set[0].languages[0].lang, "py");
        assert_eq!(solve_set[0].languages[0].constraint, None);
    }

    #[test]
    fn solve_lock_is_exclusive() {
        let (_d, ctx) = ctx();
        // Hold the env's solve lock.
        let guard = ctx.lock_solve().unwrap();
        // A second exclusive acquire on the same lockfile (a distinct open
        // descriptor, as a concurrent process would use) must fail non-blocking
        // while the first is held.
        let path = ctx.solve_lock_path();
        let open = || std::fs::OpenOptions::new().create(true).write(true).open(&path).unwrap();
        match Flock::lock(open(), FlockArg::LockExclusiveNonblock) {
            Ok(_) => panic!("second lock granted while the first is held"),
            Err((_, e)) => assert_eq!(e, Errno::EAGAIN),
        }
        // Releasing the first lets the next acquire succeed.
        drop(guard);
        assert!(Flock::lock(open(), FlockArg::LockExclusiveNonblock).is_ok());
    }

    #[test]
    fn write_spec_rejects_path_traversal_names() {
        let (_d, ctx) = ctx();
        let good = spec(r#"{"lang":"py"}"#);
        for name in ["../escape", "sub/child", "..", ".", ""] {
            assert!(
                ctx.write_spec(&lk(&ctx), name, Provenance::Scratch, &good).is_err(),
                "expected '{name}' to be rejected"
            );
        }
        // a plain name is accepted
        assert!(ctx.write_spec(&lk(&ctx), "ok-name", Provenance::Scratch, &good).is_ok());
    }

    #[test]
    fn write_spec_rejects_stale_schema_version() {
        let (_d, ctx) = ctx();
        // A pre-1 schema version is stale; the store must not accept a spec it
        // cannot understand.
        let r = ctx.write_spec(
            &lk(&ctx),
            "old",
            Provenance::Scratch,
            r#"{"envspec_version":0,"morloc_version":"0.0.0"}"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn remove_and_promote_semantics() {
        let (_d, ctx) = ctx();
        ctx.write_spec(&lk(&ctx), "p", Provenance::Scratch, &spec(r#"{"lang":"py"}"#)).unwrap();
        // promote: write to installed, drop from scratch
        ctx.write_spec(&lk(&ctx), "p", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.remove_named(&lk(&ctx), "p", Provenance::Scratch).unwrap();
        assert_eq!(ctx.gather().unwrap().len(), 1);
        assert_eq!(ctx.gather_installed().unwrap().len(), 1);
        // removing a missing spec is a no-op
        ctx.remove_named(&lk(&ctx), "p", Provenance::Scratch).unwrap();
    }

    #[test]
    fn pinned_languages_reads_only_the_toolchain_spec() {
        let (_d, ctx) = ctx();
        // An installed program's languages are NOT pins (they are on-demand).
        ctx.write_spec(&lk(&ctx), "prog", Provenance::Installed, &spec(r#"{"lang":"r"}"#)).unwrap();
        assert!(ctx.pinned_languages().unwrap().is_empty());
        // The --lang toolchain spec IS the pinned baseline.
        ctx.write_toolchain(&spec(r#"{"lang":"py"},{"lang":"cpp"}"#)).unwrap();
        assert_eq!(
            ctx.pinned_languages().unwrap(),
            vec!["cpp".to_string(), "py".to_string()] // sorted
        );
    }

    #[test]
    fn has_abi_lock_reflects_the_lock_file() {
        let (_d, ctx) = ctx();
        assert!(!ctx.has_abi_lock());
        std::fs::create_dir_all(ctx.requirements_dir()).unwrap();
        std::fs::write(ctx.abi_lock_path(), spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        assert!(ctx.has_abi_lock());
    }

    #[test]
    fn activation_cache_round_trips_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("act.json");
        assert!(read_activation_cache(&path).is_none()); // missing -> None
        let env = vec![("PATH".to_string(), "/x".to_string()), ("CC".to_string(), "gcc".to_string())];
        write_activation_cache(&path, &env);
        assert_eq!(read_activation_cache(&path), Some(env));
        // corrupt content -> None (cache is an optimization, never an error)
        std::fs::write(&path, "not json").unwrap();
        assert!(read_activation_cache(&path).is_none());
    }

    #[test]
    fn a_cached_bare_compiler_is_pinned_on_read() {
        let prefix = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(prefix.path().join("bin")).unwrap();
        std::fs::write(prefix.path().join("bin/x-cc"), "").unwrap();
        let p = prefix.path().display().to_string();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("act.json");
        let env = vec![("CONDA_PREFIX".to_string(), p.clone()), ("CC".to_string(), "x-cc".to_string())];
        write_activation_cache(&path, &env);
        let read = read_activation_cache(&path).unwrap();
        assert!(read.contains(&("CC".to_string(), format!("{p}/bin/x-cc"))), "{read:?}");
    }

    /// A pixi stand-in: any install copies the manifest into the prefix as
    /// `solved.toml`, so a test can see which manifest the prefix was solved
    /// from; shell-hook activates that prefix.
    fn fake_pixi(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("bin/pixi");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             for a in \"$@\"; do case \"$prev\" in --manifest-path) m=\"$a\";; esac; prev=\"$a\"; done\n\
             d=$(dirname \"$m\"); p=\"$d/.pixi/envs/default\"\n\
             case \"$1\" in\n\
             install) echo x >> \"$(dirname \"$0\")/installs\";\n\
               mkdir -p \"$p/conda-meta\" \"$p/bin\" && touch \"$p/bin/cc\" \"$p/bin/c++\" && chmod +x \"$p/bin/cc\" \"$p/bin/c++\" && cp \"$m\" \"$p/solved.toml\" && printf 'environments: {}\\n' > \"$d/pixi.lock\";;\n\
             shell-hook) echo \"export CONDA_PREFIX='$p' CC='$p/bin/cc' CXX='$p/bin/c++'\";;\n\
             *) exit 9;;\n\
             esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn solved_manifest(ctx: &EnvContext) -> String {
        std::fs::read_to_string(crate::abi::conda_prefix(&ctx.pixi_dir()).join("solved.toml")).unwrap()
    }

    fn adopted(ctx: &EnvContext) {
        let lock = ctx.lock_env_creating().unwrap();
        ctx.adopt_env_ids(&lock).unwrap();
    }

    fn failing_pixi(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("bin/pixi");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn installs(pixi: &Path) -> usize {
        std::fs::read_to_string(pixi.parent().unwrap().join("installs")).map(|t| t.lines().count()).unwrap_or(0)
    }

    fn texts(specs: &[EnvSpec]) -> Vec<String> {
        specs.iter().map(|s| s.to_json().unwrap()).collect()
    }

    /// DEP-5: a sync whose solve fails, or that dies before it commits, leaves
    /// the store as it was, superseded scratch builds included.
    #[test]
    fn a_failed_solve_leaves_the_store_as_it_was() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Installed, &spec(r#"{"lang":"py"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        let pixi = failing_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let before = texts(&ctx.gather().unwrap());
        let lock = ctx.lock_env().unwrap();
        let err = ctx
            .sync(&lock, "nexus", Provenance::Installed, &spec(r#"{"lang":"py","constraint":"3.11.*"}"#), true, &inputs)
            .unwrap_err();
        assert!(err.to_string().contains("pixi"), "the solve itself must fail: {err}");
        assert_eq!(texts(&ctx.gather().unwrap()), before);

        assert!(ctx.clean(&lock, &inputs).is_err());
        assert_eq!(texts(&ctx.gather().unwrap()), before, "a failed clean keeps the scratch builds");
    }

    #[test]
    fn an_install_supersedes_the_programs_scratch_builds_once_it_solves() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "other", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let lock = ctx.lock_env().unwrap();
        ctx.sync(&lock, "nexus", Provenance::Installed, &spec(r#"{"lang":"py"}"#), true, &inputs).unwrap();
        assert_eq!(texts(&ctx.gather_installed().unwrap()), texts(&[EnvSpec::from_json(&spec(r#"{"lang":"py"}"#)).unwrap()]));
        assert_eq!(ctx.program_names().unwrap(), vec!["nexus", "other"]);
        assert_eq!(ctx.gather_scratch().unwrap().len(), 1);
    }

    /// The world a sync solves is the one the store holds after it commits, so
    /// the same build again takes the fast path.
    #[test]
    fn a_repeated_sync_takes_the_fast_path() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        ctx.write_spec(&lk(&ctx), "zeta", Provenance::Installed, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "alpha", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        ctx.write_spec(&lk(&ctx), "omega", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.12.*"}"#)).unwrap();
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let lock = ctx.lock_env().unwrap();
        for (name, prov) in [("mid", Provenance::Scratch), ("beta", Provenance::Installed)] {
            ctx.sync(&lock, name, prov, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
            let n = installs(&pixi);
            ctx.sync(&lock, name, prov, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
            assert_eq!(installs(&pixi), n, "{name} solved again");
        }
    }

    /// A scratch build made after an install of the same name is part of the
    /// world: the install superseded only the builds before it.
    #[test]
    fn a_later_scratch_build_of_an_installed_program_is_in_the_world() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &spec(r#"{"lang":"py","constraint":"3.11.*"}"#)).unwrap();
        let lock = ctx.lock_env().unwrap();
        let vendored = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"vendored","path":"/s/mylib"}]}}"#;
        ctx.sync(&lock, "nexus", Provenance::Installed, vendored, true, &inputs).unwrap();
        assert!(ctx.gather_scratch().unwrap().is_empty());

        let later = spec(r#"{"lang":"py","constraint":"3.12.*"}"#);
        ctx.sync(&lock, "nexus", Provenance::Scratch, &later, false, &inputs).unwrap();
        assert_eq!(texts(&ctx.gather_scratch().unwrap()), texts(&[EnvSpec::from_json(&later).unwrap()]));
    }

    /// DEP-5: a commit a crash interrupted after its spec was written is
    /// finished by the next lock; one interrupted before is dropped. DEP-12:
    /// not before the ids agree, and not by a lock that takes the store as is.
    #[test]
    fn an_interrupted_commit_is_finished_by_the_next_lock() {
        let (_d, ctx) = ctx();
        let old = spec(r#"{"lang":"py","constraint":"3.11.*"}"#);
        let new = spec(r#"{"lang":"py"}"#);
        let note = |ctx: &EnvContext| {
            let scratch = ctx.named_paths("nexus", Provenance::Scratch).unwrap();
            let rel = scratch[0].strip_prefix(ctx.requirements_dir()).unwrap().display().to_string();
            let digest = crate::cert::hex_lower(&Sha256::digest(new.as_bytes()));
            std::fs::write(ctx.pending_commit_path(), format!("installed/nexus.json\n{digest}\n{rel}\n")).unwrap();
        };

        ctx.write_spec(&lk(&ctx), "nexus", Provenance::Scratch, &old).unwrap();
        note(&ctx);
        drop(lk(&ctx));
        assert!(ctx.pending_commit_path().exists(), "no ids: the note waits");
        adopted(&ctx);
        drop(lk(&ctx));
        assert_eq!(ctx.gather_scratch().unwrap().len(), 1, "the spec was never written: nothing is removed");
        assert!(!ctx.pending_commit_path().exists());
        let id = ctx.store_id_path();
        std::fs::remove_file(&id).unwrap();

        note(&ctx);
        std::fs::create_dir_all(ctx.requirements_dir().join("installed")).unwrap();
        std::fs::write(ctx.requirements_dir().join("installed/nexus.json"), &new).unwrap();
        drop(lk(&ctx));
        assert_eq!(ctx.gather_scratch().unwrap().len(), 1, "no ids: the store is left alone");
        assert!(ctx.pending_commit_path().exists());

        let as_is = ctx.lock_env_as_is().unwrap();
        ctx.adopt_env_ids(&as_is).unwrap();
        drop(as_is);
        drop(ctx.lock_env_as_is().unwrap());
        assert_eq!(ctx.gather_scratch().unwrap().len(), 1, "a lock taken as is finishes nothing");

        drop(lk(&ctx));
        assert!(ctx.gather_scratch().unwrap().is_empty());
        assert!(!ctx.pending_commit_path().exists());
    }

    /// The interpreter pin follows the prefix, so a sync that fast-paths
    /// records a pin a solved-then-interrupted sync never did.
    #[test]
    fn a_fast_path_records_a_missing_interpreter_pin() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let lock = ctx.lock_env().unwrap();
        ctx.sync(&lock, "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
        let meta = crate::abi::conda_prefix(&ctx.pixi_dir()).join("conda-meta");
        std::fs::write(meta.join("python-3.12.9-0.json"), r#"{"name":"python","version":"3.12.9"}"#).unwrap();
        let _ = std::fs::remove_file(ctx.abi_lock_path());
        let n = installs(&pixi);

        ctx.sync(&lock, "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
        assert_eq!(installs(&pixi), n, "the fixture must take the fast path");
        assert!(ctx.has_abi_lock());
    }

    #[test]
    fn a_lock_for_another_environment_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let a = EnvContext::new(d.path().join("a"));
        let b = EnvContext::new(d.path().join("b"));
        adopted(&a);
        adopted(&b);
        let lock_a = a.lock_env().unwrap();
        assert!(b.adopt_env_ids(&lock_a).is_err());
        let support = support();
        let pixi = fake_pixi(d.path());
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        assert!(b.sync(&lock_a, "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).is_err());
        assert!(b.clean(&lock_a, &inputs).is_err());
        assert!(b.gather().unwrap().is_empty());
    }

    #[test]
    fn only_the_managers_lock_creates_the_store() {
        let d = tempfile::tempdir().unwrap();
        let gone = EnvContext::new(d.path().join("gone"));
        assert!(gone.lock_env().is_err());
        assert!(!d.path().join("gone").exists());
        assert!(gone.lock_env_creating().is_ok());
        assert!(d.path().join("gone").is_dir());
    }

    /// DEP-4: a waiter whose lock file was removed while it waited (the
    /// environment removed and created again) locks the file now there.
    #[test]
    fn a_waiter_locks_the_stores_current_lock_file() {
        use nix::fcntl::{Flock, FlockArg};
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("env");
        let ctx = EnvContext::new(&home);
        std::fs::create_dir_all(&home).unwrap();
        let opened_before = ctx.open_lock_file().unwrap();
        std::fs::rename(&home, d.path().join("removed")).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let held = ctx.lock_from(opened_before).unwrap();
        let current = std::fs::OpenOptions::new().write(true).open(home.join(".solve.lock")).unwrap();
        assert!(
            Flock::lock(current, FlockArg::LockExclusiveNonblock).is_err(),
            "the waiter must hold the lock file the store has now"
        );
        drop(held);
    }

    #[test]
    fn a_manifest_changed_after_the_digest_is_solved_again() {
        let (d, ctx) = ctx();
        adopted(&ctx);
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        ctx.sync(&ctx.lock_env().unwrap(), "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
        let rendered = std::fs::read_to_string(ctx.pixi_dir().join("pixi.toml")).unwrap();
        assert_eq!(solved_manifest(&ctx), rendered);

        // Another writer replaces the manifest and solves it; the recorded
        // digest still names this world.
        let other = "[workspace]\nname = \"other\"\n";
        std::fs::write(ctx.pixi_dir().join("pixi.toml"), other).unwrap();
        std::fs::write(crate::abi::conda_prefix(&ctx.pixi_dir()).join("solved.toml"), other).unwrap();

        ctx.sync(&ctx.lock_env().unwrap(), "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap();
        assert_eq!(solved_manifest(&ctx), rendered, "the unchanged world must not fast-path onto another manifest");
    }

    #[test]
    fn a_sync_refuses_a_project_with_another_id() {
        let d = tempfile::tempdir().unwrap();
        let shared = d.path().join("env/pixi");
        let env = EnvContext::new(d.path().join("env"));
        adopted(&env);
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("pixi.toml"), "keep\n").unwrap();

        // A store adopted for its own project, then pointed at the shared one.
        let other = EnvContext::new(d.path().join("other"));
        adopted(&other);
        let other = EnvContext::new(d.path().join("other")).with_pixi_dir(&shared);

        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let err = other.sync(&other.lock_env().unwrap(), "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap_err();
        assert!(err.to_string().contains("belong to different environments"), "{err}");
        assert!(other.gather().unwrap().is_empty(), "a refused sync records no spec");
        assert_eq!(std::fs::read_to_string(shared.join("pixi.toml")).unwrap(), "keep\n");
        assert!(other.clean(&other.lock_env().unwrap(), &inputs).is_err());
        assert_eq!(std::fs::read_to_string(shared.join("pixi.toml")).unwrap(), "keep\n");
    }

    #[test]
    fn a_sync_refuses_a_missing_id() {
        let (d, ctx) = ctx();
        let pixi = fake_pixi(d.path());
        let support = support();
        let channels = crate::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: &pixi, platform: "linux-64", channels: &channels };
        let err = ctx.sync(&ctx.lock_env().unwrap(), "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).unwrap_err();
        assert!(err.to_string().contains("no environment id"), "{err}");
        assert!(ctx.gather().unwrap().is_empty());
        assert!(!ctx.pixi_dir().join("pixi.toml").exists());

        // Only the project's id: a store that was never this project's.
        adopted(&ctx);
        std::fs::remove_file(ctx.store_id_path()).unwrap();
        assert!(ctx.sync(&ctx.lock_env().unwrap(), "prog", Provenance::Scratch, &spec(r#"{"lang":"py"}"#), false, &inputs).is_err());
        assert!(ctx.clean(&ctx.lock_env().unwrap(), &inputs).is_err());
        assert!(ctx.gather().unwrap().is_empty());
    }

    #[test]
    fn the_manager_gives_store_and_project_one_id() {
        let (_d, ctx) = ctx();
        adopted(&ctx);
        let id = std::fs::read_to_string(ctx.store_id_path()).unwrap();
        assert!(!id.trim().is_empty());
        assert_eq!(std::fs::read_to_string(ctx.pixi_id_path()).unwrap(), id);

        // Adopting again keeps the id, and repairs a project holding another.
        std::fs::write(ctx.pixi_id_path(), "stranger\n").unwrap();
        adopted(&ctx);
        assert_eq!(std::fs::read_to_string(ctx.store_id_path()).unwrap(), id);
        assert_eq!(std::fs::read_to_string(ctx.pixi_id_path()).unwrap(), id);

        // Two environments never share an id.
        let (_e, other) = self::ctx();
        adopted(&other);
        assert_ne!(std::fs::read_to_string(other.store_id_path()).unwrap(), id);
    }
}
