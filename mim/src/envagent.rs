//! The in-environment dependency agent, built into `mim`.
//!
//! It runs *inside* a morloc environment: the compiler invokes `mim` as the build
//! hook named by `MORLOC_BUILD_HOOK`, calling `mim sync --name <key> --spec
//! envspec.json --root <root>`; a user may run `mim sync`/`clean`/`spec` directly
//! in an entered shell. All three operate on that env's dependency world via the
//! shared `morloc-deps` kernel:
//!
//!   * `sync`  -- merge a program's declared deps, re-solve, install
//!   * `clean` -- reset the world to the installed baseline
//!   * `spec`  -- print the merged pixi manifest (no solve)
//!
//! These carry none of the manager's orchestration surface (env lifecycle,
//! containers, serve); they dispatch straight into `morloc-deps`. Keeping the
//! agent inside `mim` makes it version-coherent with the manager by construction
//! and keeps `mim` a single, freely relocatable binary.
//! Inputs come from the activated environment:
//!   * `MORLOC_STATE`/`MORLOC_HOME` -- the env data dir (holds `requirements/`, `pixi/`)
//!   * `morloc lang-support` -- the language-support table, from the compiler at
//!     `MORLOC_BIN` (else `morloc` on PATH), coherent with the driving compiler
//!   * `MORLOC_PIXI` (or `pixi` on PATH) -- the pixi solver
//!   * the host's conda platform tag, computed in-process
//!
//! It drives `morloc init` for on-demand shim builds via two env vars the
//! compiler reads (see [`ENV_STRICT_CONDA`], [`ENV_INIT_INCREMENTAL`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use morloc_deps::envspec::{EnvSpec, LocalAnchor};
use morloc_deps::envstore::{EnvContext, Provenance, SolveInputs};
use morloc_deps::error::DepsError;
use morloc_deps::langsupport::LangSupport;
use morloc_deps::platform::conda_platform;

/// Tells `morloc init` to select ONLY conda-prefix tools (never a stray host
/// python/R) and be fail-closed, so shims are ABI-coherent with the env interpreter.
pub(crate) const ENV_STRICT_CONDA: &str = "MORLOC_STRICT_CONDA";
/// Tells `morloc init` to skip the already-built runtime and language shims and
/// build only what is missing (a bare init rebuilds everything).
pub(crate) const ENV_INIT_INCREMENTAL: &str = "MORLOC_INIT_INCREMENTAL";

/// `mim sync`: merge a program spec into the environment's world spec and re-solve.
/// Invoked by the morloc compiler as the build hook when a program is built.
pub fn sync(
    name: &str,
    spec: &std::path::Path,
    root: Option<&std::path::Path>,
    installed: bool,
) -> Result<(), DepsError> {
    let ctx = env_context()?;
    let inputs = solve_inputs()?;
    let support = load_lang_support()?;
    let inputs = inputs.bind(&support);

    let json = std::fs::read_to_string(spec)
        .map_err(|e| DepsError::Env(format!("cannot read {}: {e}", spec.display())))?;
    // Resolve local (filesystem-path) deps to their live path (canonical real path
    // under the project root), keeping them editable, before storing -- so the
    // stored world spec is location-independent and pixi resolves them correctly.
    // A spec with no local deps is passed through unchanged.
    let mut es = EnvSpec::from_json(&json)?;
    let resolved = if es.has_local_deps() {
        let root = root.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        es.resolve_local_deps(&LocalAnchor::Live { root: &root })?;
        es.to_json()?
    } else {
        json
    };
    let provenance = if installed { Provenance::Installed } else { Provenance::Scratch };

    // Which languages this program's pools USE that have no morloc binding built
    // yet -- computed from the compiler-emitted `es.languages` (the actual pool
    // languages, NOT the transitive conda closure) against the per-language shim
    // markers. Checked BEFORE `ctx.sync` so an immutable env can refuse without
    // committing anything (a committed scratch spec would otherwise re-trigger the
    // refusal on every LATER build in the env, including unrelated ones).
    let home = morloc_home()?;
    let missing = declared_langs_missing_shims(&es, &home);
    if !missing.is_empty() && env_is_immutable() {
        return Err(DepsError::Env(format!(
            "building '{name}' needs a morloc binding for language(s) this environment does \
             not provide: {}. On-demand language provisioning is only available in a native \
             or pliable (writable) environment; a served or frozen image has a fixed language \
             set baked into its read-only runtime. Rebuild the image with these languages \
             pinned (e.g. `mim update --lang {}`).",
            missing.join(", "),
            missing.join(" --lang ")
        )));
    }

    let activation = match ctx.sync(name, provenance, &resolved, &inputs) {
        Ok(a) => a,
        // A solve failure in an env that pins an interpreter minor (to protect an
        // already-built shim's ABI) is often that pin conflicting with a
        // dependency wanting a different version. Add an actionable hint -- clearly
        // conditional, since the failure may be unrelated -- rather than surfacing
        // only pixi's generic "unavailable" message.
        Err(e) if ctx.has_abi_lock() => {
            return Err(DepsError::Env(format!(
                "{e}\n\nHint: if the failure above is an interpreter version conflict, note \
                 this environment pins its Python/R interpreter minor version to protect \
                 already-built language bindings; re-provision the environment to move it and \
                 rebuild the bindings: `mim update --env <env>`."
            )));
        }
        Err(e) => return Err(e),
    };

    // On-demand provisioning (mutable env): build the missing shims before the
    // pools that link them are compiled. `ctx.sync` provisioned the interpreters
    // and recorded their abi-lock; the marker stays absent until the build
    // succeeds, so a failed build is retried on the next `morloc make`.
    if !missing.is_empty() {
        eprintln!(
            "Provisioning morloc bindings for language(s): {}...",
            missing.join(", ")
        );
        // Serialize the shim build against concurrent makes on this env (a
        // separate lock acquisition from `ctx.sync`, which released its lock on
        // return) so two `morloc init`s cannot write the same shim artifacts at
        // once. `morloc init` is idempotent, so a second waiter is a no-op.
        let _build_lock = ctx.lock_env()?;
        build_new_language_shims(&activation, &home)?;
    }

    eprintln!("Environment dependencies synced for '{name}'.");
    Ok(())
}

/// The languages a program's pools USE (from the compiler-emitted `es.languages`)
/// whose morloc binding has not been built -- their `morloc init` marker is absent
/// under `<home>/opt/lang-configured/`. Keyed on the DECLARED pool languages, not
/// the solved pixi.lock, so a python/r-base pulled in transitively by a dependency
/// (with no py/r pool) never triggers a build or refusal. Reflects SHIM state, so
/// a failed build or an immutable-env refusal is re-detected on every retry.
/// Returns morloc short codes (py/r/cpp/julia), which are also valid `--lang` pins.
fn declared_langs_missing_shims(es: &EnvSpec, home: &Path) -> Vec<String> {
    let marker_dir = lang_marker_dir(home);
    es.languages
        .iter()
        .filter_map(|l| shim_marker_name(&l.lang).map(|m| (l.lang.clone(), m)))
        .filter(|(_, marker)| !marker_dir.join(marker).exists())
        .map(|(lang, _)| lang)
        .collect()
}

/// Map a morloc language code to the shim-marker filename `morloc init` writes
/// (the compiler's `DF.lsName`). `rust` has no langSetup shim (its marshaller is a
/// cargo build), so it maps to nothing. `julia` is normalized from the compiler's
/// `jl` at `EnvSpec` ingestion, so it arrives here as `julia`. The single source
/// of the marker filenames -- callers with a conda runtime code map it back first
/// (see [`runtime_to_morloc_lang`]).
fn shim_marker_name(lang: &str) -> Option<&'static str> {
    match lang {
        "py" => Some("python"),
        "r" => Some("R"),
        "cpp" => Some("C++"),
        "julia" => Some("Julia"),
        _ => None,
    }
}

/// Map a solved-runtime language code (from `pixi::runtime_languages`, e.g.
/// `python`) back to its morloc short code (`py`), so shim-marker lookups have one
/// key space. `rust` has no shim; `cpp` is not a conda runtime.
fn runtime_to_morloc_lang(runtime: &str) -> Option<&'static str> {
    match runtime {
        "python" => Some("py"),
        "r" => Some("r"),
        "julia" => Some("julia"),
        _ => None,
    }
}

/// The per-language shim-marker directory under the runtime home.
fn lang_marker_dir(home: &Path) -> PathBuf {
    home.join("opt").join("lang-configured")
}

/// The runtime home (`MORLOC_HOME`) where `morloc init` writes shims and their
/// markers. Distinct from the store root (`env_context` accepts `MORLOC_STATE`
/// too), so it is resolved separately and required: a managed `morloc make`
/// always exports it, and building/checking shims against the wrong home would
/// loop forever.
fn morloc_home() -> Result<PathBuf, DepsError> {
    std::env::var("MORLOC_HOME").map(PathBuf::from).map_err(|_| {
        DepsError::Env(
            "MORLOC_HOME is not set; on-demand language provisioning needs it to locate the \
             runtime where language bindings are built. Run inside a morloc environment."
                .to_string(),
        )
    })
}

/// Whether this environment is immutable (served or frozen): its language set is
/// fixed at build time and its runtime prefix is read-only, so on-demand
/// provisioning must be refused. Signalled by the manager via `MORLOC_IMMUTABLE`
/// (any non-empty, non-"0" value). A native or pliable (writable) container
/// environment leaves it unset.
fn env_is_immutable() -> bool {
    std::env::var("MORLOC_IMMUTABLE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Build the shims for on-demand-provisioned languages by re-running `morloc
/// init` incrementally and strict (see [`ENV_INIT_INCREMENTAL`],
/// [`ENV_STRICT_CONDA`]). The fresh activation puts the just-installed interpreter
/// on PATH (and CONDA_PREFIX, which strict mode requires); no FHS wrapping, since
/// the agent already runs inside the env's execution environment.
fn build_new_language_shims(activation: &[(String, String)], home: &Path) -> Result<(), DepsError> {
    let morloc = std::env::var("MORLOC_BIN").unwrap_or_else(|_| "morloc".to_string());
    let mut cmd = Command::new(&morloc);
    cmd.arg("init");
    crate::apply_activation(&mut cmd, activation);
    cmd.env("MORLOC_HOME", home);
    cmd.env(ENV_INIT_INCREMENTAL, "1");
    cmd.env(ENV_STRICT_CONDA, "1");
    let status = cmd
        .status()
        .map_err(|e| DepsError::Env(format!("could not run `{morloc} init`: {e}")))?;
    if !status.success() {
        return Err(DepsError::Env(
            "morloc init failed while building the shim for a newly provisioned language"
                .to_string(),
        ));
    }
    Ok(())
}

/// `mim clean`: reset the environment's world to its installed baseline. This is
/// the explicit garbage collector for on-demand languages: any language runtime
/// no installed program (or `--lang` pin) references is dropped from the solved
/// world. Pinned languages survive. When a language is dropped, its stale shim
/// marker is invalidated so a later on-demand re-add rebuilds cleanly.
pub fn clean() -> Result<(), DepsError> {
    let ctx = env_context()?;
    let support = load_lang_support()?;
    let owned = solve_inputs()?;
    let inputs = owned.bind(&support);
    let dropped = ctx.clean(&inputs)?;
    if !dropped.is_empty() {
        invalidate_shim_markers(&dropped);
        eprintln!("Dropped unused language runtime(s): {}.", dropped.join(", "));
    }
    eprintln!("Environment reset to its installed baseline.");
    Ok(())
}

/// Remove ONLY the dropped languages' shim markers under `<home>/opt/
/// lang-configured/`, so the next on-demand re-add rebuilds them against the
/// freshly solved interpreter while every still-present language keeps its valid
/// shim. `dropped` are runtime codes (python/r/julia). Best-effort. A missing
/// `MORLOC_HOME` (e.g. `mim clean` run outside an env) simply skips this.
fn invalidate_shim_markers(dropped: &[String]) {
    let Ok(home) = morloc_home() else { return };
    let dir = lang_marker_dir(&home);
    for lang in dropped {
        if let Some(marker) = runtime_to_morloc_lang(lang).and_then(shim_marker_name) {
            let _ = std::fs::remove_file(dir.join(marker));
        }
    }
}

/// `mim spec`: print the merged pixi manifest for the current declared world.
pub fn spec() -> Result<(), DepsError> {
    let ctx = env_context()?;
    let support = load_lang_support()?;
    let owned = solve_inputs()?;
    let inputs = owned.bind(&support);
    let specs = ctx.gather()?;
    print!("{}", ctx.rendered_manifest(&specs, &inputs)?);
    Ok(())
}

/// The solver context (pixi binary, platform, channels) shared by every command.
/// The lang-support table is bound separately via [`OwnedInputs::bind`] because it
/// borrows a table each command loads for itself.
struct OwnedInputs {
    pixi: PathBuf,
    platform: String,
    channels: Vec<String>,
}

impl OwnedInputs {
    fn bind<'a>(&'a self, support: &'a LangSupport) -> SolveInputs<'a> {
        SolveInputs {
            lang_support: support,
            pixi_bin: self.pixi.as_path(),
            platform: self.platform.as_str(),
            channels: self.channels.as_slice(),
        }
    }
}

fn solve_inputs() -> Result<OwnedInputs, DepsError> {
    Ok(OwnedInputs {
        pixi: pixi_bin(),
        platform: conda_platform(),
        channels: morloc_deps::pixi::default_channels(),
    })
}

/// The environment context. The requirements store lives under the mutable STATE
/// root (`MORLOC_STATE`); natively that equals `MORLOC_HOME`, but a container
/// mounts state separately, so prefer `MORLOC_STATE` and fall back to
/// `MORLOC_HOME`. The pixi project dir defaults to `<state>/pixi`; a container
/// bakes its env elsewhere and passes `MORLOC_PIXI_DIR`.
fn env_context() -> Result<EnvContext, DepsError> {
    let home = std::env::var("MORLOC_STATE")
        .or_else(|_| std::env::var("MORLOC_HOME"))
        .map_err(|_| {
            DepsError::Env(
                "neither MORLOC_STATE nor MORLOC_HOME is set; run `mim sync` inside a \
                 morloc environment"
                    .to_string(),
            )
        })?;
    let mut ctx = EnvContext::new(home);
    if let Ok(pixi_dir) = std::env::var("MORLOC_PIXI_DIR") {
        ctx = ctx.with_pixi_dir(pixi_dir);
    }
    Ok(ctx)
}

/// The pixi binary: `MORLOC_PIXI` if the environment names it, else `pixi` on PATH
/// (resolved by the OS at spawn).
fn pixi_bin() -> PathBuf {
    std::env::var("MORLOC_PIXI")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("pixi"))
}

/// The language-support table, obtained from the compiler. Prefer the exact
/// compiler that invoked us (`MORLOC_BIN`, exported by `morloc make` on the build
/// hook) so the table stays coherent with the driving compiler AND the reverse
/// call does not depend on `morloc` being on PATH; fall back to `morloc` on PATH
/// when run directly in an entered shell.
fn load_lang_support() -> Result<LangSupport, DepsError> {
    let morloc = std::env::var("MORLOC_BIN").unwrap_or_else(|_| "morloc".to_string());
    let out = Command::new(&morloc)
        .arg("lang-support")
        .output()
        .map_err(|e| DepsError::Env(format!("could not run `{morloc} lang-support`: {e}")))?;
    if !out.status.success() {
        return Err(DepsError::Env(format!(
            "`{morloc} lang-support` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    LangSupport::from_json(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_marker_name_maps_only_shim_bearing_langs() {
        assert_eq!(shim_marker_name("py"), Some("python"));
        assert_eq!(shim_marker_name("r"), Some("R"));
        assert_eq!(shim_marker_name("cpp"), Some("C++"));
        assert_eq!(shim_marker_name("julia"), Some("Julia"));
        // rust has no langSetup shim; an unknown code maps to nothing.
        assert_eq!(shim_marker_name("rust"), None);
        assert_eq!(shim_marker_name("nope"), None);
    }

    #[test]
    fn declared_langs_missing_shims_uses_declared_langs_and_markers() {
        let home = tempfile::tempdir().unwrap();
        // Only python's binding is built.
        let markers = lang_marker_dir(home.path());
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join("python"), "").unwrap();

        // A program declaring py + r + cpp + rust + julia (jl normalized to julia).
        let es = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"py"},{"lang":"r"},{"lang":"cpp"},{"lang":"rust"},{"lang":"jl"}]}"#,
        )
        .unwrap();

        // py is built (skipped); rust has no shim (skipped); r/cpp/julia are missing.
        assert_eq!(
            declared_langs_missing_shims(&es, home.path()),
            vec!["r".to_string(), "cpp".to_string(), "julia".to_string()]
        );
    }
}
