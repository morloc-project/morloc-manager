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

use std::path::PathBuf;
use std::process::Command;

use morloc_deps::envspec::{EnvSpec, LocalAnchor};
use morloc_deps::envstore::{EnvContext, Provenance, SolveInputs};
use morloc_deps::error::DepsError;
use morloc_deps::langsupport::LangSupport;
use morloc_deps::platform::conda_platform;

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
    ctx.sync(name, provenance, &resolved, &inputs)?;
    eprintln!("Environment dependencies synced for '{name}'.");
    Ok(())
}

/// `mim clean`: reset the environment's world to its installed baseline.
pub fn clean() -> Result<(), DepsError> {
    let ctx = env_context()?;
    let support = load_lang_support()?;
    let owned = solve_inputs()?;
    let inputs = owned.bind(&support);
    ctx.clean(&inputs)?;
    eprintln!("Environment reset to its installed baseline.");
    Ok(())
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
