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
//!   * `MORLOC_ENV_STORE` -- the env's store (holds `requirements/`, `pixi/`); see
//!     [`hook_context`]
//!   * `morloc lang-support` -- the language-support table, from the compiler at
//!     `MORLOC_BIN` (else `morloc` on PATH), coherent with the driving compiler
//!   * `MORLOC_PIXI` (or `pixi` on PATH) -- the pixi solver
//!   * the host's conda platform tag, computed in-process
//!
//! It drives `morloc init` for on-demand shim builds via two env vars the
//! compiler reads (see [`ENV_STRICT_CONDA`], [`ENV_INIT_INCREMENTAL`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use morloc_deps::abi;
use morloc_deps::envspec::EnvSpec;
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
    let hook = hook_context(|k| std::env::var(k).ok())?;
    let owned = solve_inputs()?;
    let support = load_lang_support()?;
    let json = std::fs::read_to_string(spec)
        .map_err(|e| DepsError::Env(format!("cannot read {}: {e}", spec.display())))?;
    let authority = bindings_authority(&support, |k| std::env::var(k).ok());
    sync_in(hook, &owned.bind(&support), name, &json, root, installed, authority.as_deref())
}

#[allow(clippy::too_many_arguments)]
fn sync_in(
    hook: HookContext,
    inputs: &SolveInputs,
    name: &str,
    json: &str,
    root: Option<&Path>,
    installed: bool,
    authority: Option<&str>,
) -> Result<(), DepsError> {
    let ctx = hook.ctx;
    let (key, provenance) = record_slot(name, installed, hook.foreign_state.as_deref());
    let installs = provenance == Provenance::Installed;
    let mut es = EnvSpec::from_json(json)?;
    // DEP-4, DEP-12
    let lock = ctx.lock_env()?;
    ctx.check_env_ids()?;
    let home = morloc_home()?;

    // Local (filesystem-path) deps never reach the store module-relative. An
    // installed program's Python locals are snapshotted into the pixi project
    // dir; every other local resolves to its live path (canonical real path
    // under the project root), keeping it editable. A spec with no local deps
    // is passed through unchanged. An install with a snapshot supersedes the
    // program's scratch builds: their live paths must not meet the snapshot in
    // one solve.
    let supersede = installs && es.has_py_locals();
    let resolved = if es.has_local_deps() {
        let root = root.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        if supersede {
            if env_is_immutable() {
                return Err(DepsError::Env(format!(
                    "installing '{name}' needs a snapshot of its local python dependencies \
                     in the environment, which a served or frozen image cannot take. Install \
                     it into the environment before serving or freezing."
                )));
            }
            crate::localdeps::vendor_py_locals(&mut es, &root, &ctx.pixi_dir())
                .map_err(|e| DepsError::Env(e.to_string()))?;
        }
        es.resolve_local_deps(&root)?;
        es.to_json()?
    } else {
        json.to_string()
    };

    // Which languages this program's pools USE that have no morloc binding built
    // yet -- computed from the compiler-emitted `es.languages` (the actual pool
    // languages, NOT the transitive conda closure) against the per-language shim
    // markers. Checked BEFORE `ctx.sync` so an immutable env can refuse without
    // committing anything (a committed scratch spec would otherwise re-trigger the
    // refusal on every LATER build in the env, including unrelated ones).
    let missing = declared_langs_missing_shims(&es, &home);
    let langs = shim_langs(&es);
    // IFC-10: whether the compiler's answer decided this immutable build.
    let mut judged = false;
    if let (Some(morloc), true) = (authority, env_is_immutable()) {
        // The ambient environment is the image's own; it is not scrubbed,
        // since scrubbing would take the image's interpreters off PATH.
        if langs.is_empty() {
            judged = true;
        } else {
            match check_bindings(&mut compiler_command(morloc, None, &home), &langs) {
                Ok(report) => {
                    refuse_unless_current(name, &report, &langs)?;
                    judged = true;
                }
                Err(e) => eprintln!("Warning: {e}; checking the bindings without the compiler"),
            }
        }
    }
    if !judged && !missing.is_empty() && env_is_immutable() {
        return Err(DepsError::Env(format!(
            "building '{name}' needs a morloc binding for language(s) this environment does \
             not provide: {}. On-demand language provisioning is only available in a native \
             or pliable (writable) environment; a served or frozen image has a fixed language \
             set baked into its read-only runtime. Rebuild the image with these languages \
             pinned (e.g. `mim modify --lang {}`).",
            missing.join(", "),
            missing.join(" --lang ")
        )));
    }

    // DEP-10: an immutable environment refuses before writing anything a sync
    // that would solve, or that needs a binding rebuilt for the current prefix.
    if env_is_immutable() {
        if ctx.sync_changes_world(&key, provenance, &resolved, supersede, inputs)? {
            return Err(DepsError::Env(format!(
                "building '{name}' needs packages this environment does not hold, and a \
                 served or frozen image cannot change its toolchain. Install the program \
                 into the environment before serving or freezing it."
            )));
        }
        if !judged {
            let stale = stale_declared_shims(&es, &home, &abi::meta_dir(&ctx.pixi_dir()));
            if !stale.is_empty() {
                return Err(stale_binding_refusal(name, &stale));
            }
        }
    }
    let activation = match ctx.sync(&lock, &key, provenance, &resolved, supersede, inputs) {
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

    if !judged {
        if let (Some(morloc), false) = (authority, env_is_immutable()) {
            match ensure_bindings(morloc, &activation, &home, &langs) {
                Ok(()) => judged = true,
                Err(BindingsError::Refused(e)) => return Err(e),
                Err(BindingsError::Unusable(e)) => {
                    eprintln!("Warning: {e}; checking the bindings without the compiler");
                }
            }
        }
    }
    if judged {
        crate::localdeps::sweep_store_or_warn(&ctx);
        drop(lock);
        eprintln!("Environment dependencies synced for '{name}'.");
        return Ok(());
    }

    // Now that the prefix is final, a binding the marker calls built may not
    // match the interpreter the prefix holds (moved by this or an earlier solve,
    // or the artifact gone). Such a binding fails only when a pool imports it,
    // so it is rebuilt here like a missing one: its marker goes, which is what
    // an incremental `morloc init` keys the rebuild on.
    let stale = stale_declared_shims(&es, &home, &abi::meta_dir(&ctx.pixi_dir()));
    if !stale.is_empty() && env_is_immutable() {
        return Err(stale_binding_refusal(name, &stale));
    }
    let mut missing = missing;
    for defect in &stale {
        eprintln!("Rebuilding a morloc binding: {defect}.");
        if let Some(marker) = abi::shim_marker_name(defect.lang()) {
            let _ = std::fs::remove_file(abi::lang_marker_dir(&home).join(marker));
        }
        if !missing.iter().any(|l| l == defect.lang()) {
            missing.push(defect.lang().to_string());
        }
    }

    // On-demand provisioning (mutable env): build the missing shims before the
    // pools that link them are compiled. `ctx.sync` provisioned the interpreters
    // and recorded their abi-lock; the marker stays absent until the build
    // succeeds, so a failed build is retried on the next `morloc make`.
    if !missing.is_empty() {
        eprintln!(
            "Provisioning morloc bindings for language(s): {}...",
            missing.join(", ")
        );
        let morloc = std::env::var("MORLOC_BIN").unwrap_or_else(|_| "morloc".to_string());
        build_new_language_shims(&morloc, &activation, &home)?;
    }

    crate::localdeps::sweep_store_or_warn(&ctx);
    drop(lock);
    eprintln!("Environment dependencies synced for '{name}'.");
    Ok(())
}

fn stale_binding_refusal(name: &str, stale: &[impl std::fmt::Display]) -> DepsError {
    DepsError::Env(format!(
        "building '{name}' needs morloc bindings this environment holds for another \
         interpreter version:\n  {}\nA served or frozen image cannot rebuild them. \
         Rebuild the environment: `mim update --reinit`.",
        stale.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n  ")
    ))
}

/// The languages a program's pools USE (from the compiler-emitted `es.languages`)
/// whose morloc binding has not been built -- their `morloc init` marker is absent
/// under `<home>/opt/lang-configured/`. Keyed on the DECLARED pool languages, not
/// the solved pixi.lock, so a python/r-base pulled in transitively by a dependency
/// (with no py/r pool) never triggers a build or refusal. Reflects SHIM state, so
/// a failed build or an immutable-env refusal is re-detected on every retry.
/// Returns morloc short codes (py/r/cpp), which are also valid `--lang` pins.
fn declared_langs_missing_shims(es: &EnvSpec, home: &Path) -> Vec<String> {
    let marker_dir = abi::lang_marker_dir(home);
    es.languages
        .iter()
        .filter_map(|l| abi::shim_marker_name(&l.lang).map(|m| (l.lang.clone(), m)))
        .filter(|(_, marker)| !marker_dir.join(marker).exists())
        .map(|(lang, _)| lang)
        .collect()
}

/// The bindings of the languages a program's pools USE that no longer match the
/// interpreter the prefix holds: the marker says built, but the artifact is gone
/// or tagged to another minor. Declared languages only, like
/// [`declared_langs_missing_shims`], so an interpreter a dependency pulled in
/// never triggers a rebuild for a program that does not use it.
fn stale_declared_shims(es: &EnvSpec, home: &Path, meta_dir: &Path) -> Vec<abi::ShimDefect> {
    abi::shim_defects(home, meta_dir)
        .into_iter()
        .filter(|d| !matches!(d, abi::ShimDefect::NotBuilt { .. }))
        .filter(|d| es.languages.iter().any(|l| l.lang == d.lang()))
        .collect()
}

/// The runtime home (`MORLOC_HOME`) where `morloc init` writes shims and their
/// markers. Distinct from the store root (`env_context` accepts `MORLOC_STATE`
/// too), so it is resolved separately and required: a managed `morloc make`
/// always exports it, and building/checking shims against the wrong home would
/// loop forever.
fn morloc_home() -> Result<PathBuf, DepsError> {
    std::env::var("MORLOC_HOME").ok().filter(|v| !v.is_empty()).map(PathBuf::from).ok_or_else(|| {
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
fn build_new_language_shims(morloc: &str, activation: &[(String, String)], home: &Path) -> Result<(), DepsError> {
    let mut cmd = compiler_command(morloc, Some(activation), home);
    cmd.arg("init");
    // PROV-5: unless the launcher chose a link dir, link nothing into the
    // user's own bin directory.
    if std::env::var_os("MORLOC_BIN_LINK_DIR").is_none() {
        cmd.env("MORLOC_BIN_LINK_DIR", "");
    }
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

/// A command running the compiler against the runtime `home`. With an
/// activation, the ambient environment is scrubbed and the activation
/// applied, as for a build; without one, the ambient environment is used as
/// it is. DEP-4: the hook is never re-entered.
fn compiler_command(morloc: &str, activation: Option<&[(String, String)]>, home: &Path) -> Command {
    let mut cmd = Command::new(morloc);
    if let Some(activation) = activation {
        morloc_deps::ambient::scrub(&mut cmd);
        crate::apply_activation(&mut cmd, activation);
    }
    cmd.env("MORLOC_HOME", home);
    cmd.env_remove("MORLOC_BUILD_HOOK");
    cmd
}

/// IFC-10: the compiler to ask whether bindings are current, when it can be
/// asked: it offers `morloc init --check` and is named by `MORLOC_BIN` (a
/// compiler found on PATH may not be the one that builds).
fn bindings_authority(support: &LangSupport, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if support.bindings_check < 1 {
        return None;
    }
    var("MORLOC_BIN").filter(|v| !v.is_empty())
}

/// The languages of a program's pools that have a binding (py, r, cpp).
fn shim_langs(es: &EnvSpec) -> Vec<String> {
    es.languages
        .iter()
        .filter(|l| abi::shim_marker_name(&l.lang).is_some())
        .map(|l| l.lang.clone())
        .collect()
}

/// IFC-10: the report `morloc init --check` prints.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct BindingsReport {
    runtime: RuntimeCheck,
    #[serde(default)]
    languages: Vec<LangCheck>,
}

#[derive(Debug, serde::Deserialize)]
struct RuntimeCheck {
    ok: bool,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, serde::Deserialize)]
struct LangCheck {
    lang: String,
    state: String,
    #[serde(default)]
    reason: String,
}

impl LangCheck {
    /// Whether an incremental init would make this binding current.
    fn rebuildable(&self) -> bool {
        self.state == "absent" || self.state == "stale"
    }
}

impl std::fmt::Display for LangCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason.is_empty() {
            write!(f, "{}: {}", self.lang, self.state)
        } else {
            write!(f, "{}: {} ({})", self.lang, self.state, self.reason)
        }
    }
}

/// Run `morloc init --check` for `langs` and read its report. An error means
/// the compiler could not be asked (it failed, or printed no report).
fn check_bindings(cmd: &mut Command, langs: &[String]) -> Result<BindingsReport, String> {
    cmd.arg("init").arg("--check");
    for l in langs {
        cmd.arg("--lang").arg(l);
    }
    let out = cmd.output().map_err(|e| format!("could not run `morloc init --check`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`morloc init --check` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_bindings_report(&String::from_utf8_lossy(&out.stdout))
}

pub(crate) fn parse_bindings_report(text: &str) -> Result<BindingsReport, String> {
    serde_json::from_str(text.trim()).map_err(|e| format!("`morloc init --check` printed no report: {e}"))
}

impl BindingsReport {
    /// The asked languages' entries that are not ok, with a `not reported`
    /// entry for any asked language the report leaves out.
    fn not_ok(&self, langs: &[String]) -> Vec<LangCheck> {
        let mut out: Vec<LangCheck> = self
            .languages
            .iter()
            .filter(|l| l.state != "ok" && langs.iter().any(|x| x == &l.lang))
            .map(|l| LangCheck { lang: l.lang.clone(), state: l.state.clone(), reason: l.reason.clone() })
            .collect();
        for lang in langs {
            if !self.languages.iter().any(|l| &l.lang == lang) {
                out.push(LangCheck { lang: lang.clone(), state: "not reported".into(), reason: String::new() });
            }
        }
        out
    }

    /// What is not current, for the runtime and the given languages, as lines.
    pub(crate) fn problems(&self, langs: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        if !self.runtime.ok {
            out.push(format!("runtime: {}", self.runtime.reason));
        }
        for l in &self.languages {
            if l.state != "ok" && langs.iter().any(|x| x == &l.lang) {
                out.push(l.to_string());
            }
        }
        for lang in langs {
            if !self.languages.iter().any(|l| &l.lang == lang) {
                out.push(format!("{lang}: not reported"));
            }
        }
        out
    }
}

/// Refuse a build in an immutable environment whose runtime or bindings are
/// not current: nothing can be rebuilt there.
fn refuse_unless_current(name: &str, report: &BindingsReport, langs: &[String]) -> Result<(), DepsError> {
    if !report.runtime.ok {
        return Err(runtime_refusal(name, &report.runtime.reason));
    }
    let not_ok = report.not_ok(langs);
    let absent: Vec<&LangCheck> = not_ok.iter().filter(|l| l.state == "absent").collect();
    if !absent.is_empty() {
        let langs: Vec<&str> = absent.iter().map(|l| l.lang.as_str()).collect();
        return Err(DepsError::Env(format!(
            "building '{name}' needs a morloc binding for language(s) this environment does \
             not provide: {}. A served or frozen image has a fixed language set baked into \
             its read-only runtime. Rebuild the image with these languages pinned (e.g. \
             `mim modify --lang {}`).",
            langs.join(", "),
            langs.join(" --lang ")
        )));
    }
    let other: Vec<&LangCheck> = not_ok.iter().filter(|l| l.state != "absent").collect();
    if !other.is_empty() {
        return Err(DepsError::Env(format!(
            "building '{name}' needs morloc bindings this environment cannot rebuild:\n  {}\n\
             A served or frozen image cannot rebuild them. Rebuild the environment: \
             `mim update --reinit`.",
            other.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n  ")
        )));
    }
    Ok(())
}

fn runtime_refusal(name: &str, reason: &str) -> DepsError {
    DepsError::Env(format!(
        "building '{name}' needs the environment's morloc runtime to match its compiler, \
         and it does not: {reason}. Rebuild the environment: `mim update --reinit`."
    ))
}

enum BindingsError {
    /// The compiler answered, and the build must stop.
    Refused(DepsError),
    /// The compiler could not be asked.
    Unusable(String),
}

/// IFC-10: in a mutable environment, ask the compiler which of `langs`'
/// bindings are not current, rebuild those with an incremental init, and ask
/// again. The runtime itself is never rebuilt here.
fn ensure_bindings(
    morloc: &str,
    activation: &[(String, String)],
    home: &Path,
    langs: &[String],
) -> Result<(), BindingsError> {
    if langs.is_empty() {
        return Ok(());
    }
    let ask = || check_bindings(&mut compiler_command(morloc, Some(activation), home), langs);
    let report = ask().map_err(BindingsError::Unusable)?;
    if !report.runtime.ok {
        return Err(BindingsError::Refused(DepsError::Env(format!(
            "the environment's morloc runtime does not match its compiler: {}. Rebuild the \
             environment: `mim update --reinit`.",
            report.runtime.reason
        ))));
    }
    let not_ok = report.not_ok(langs);
    let unbuildable: Vec<&LangCheck> = not_ok.iter().filter(|l| !l.rebuildable()).collect();
    if !unbuildable.is_empty() {
        return Err(BindingsError::Refused(DepsError::Env(format!(
            "morloc bindings cannot be built in this environment:\n  {}",
            unbuildable.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n  ")
        ))));
    }
    let needed: Vec<&LangCheck> = not_ok.iter().filter(|l| l.rebuildable()).collect();
    if needed.is_empty() {
        return Ok(());
    }
    eprintln!(
        "Provisioning morloc bindings for language(s): {}...",
        needed.iter().map(|l| l.lang.as_str()).collect::<Vec<_>>().join(", ")
    );
    build_new_language_shims(morloc, activation, home).map_err(BindingsError::Refused)?;
    let after = ask().map_err(BindingsError::Unusable)?;
    let still = after.not_ok(langs);
    if after.runtime.ok && still.is_empty() {
        return Ok(());
    }
    let mut lines: Vec<String> = still.iter().map(|l| l.to_string()).collect();
    if !after.runtime.ok {
        lines.insert(0, format!("runtime: {}", after.runtime.reason));
    }
    Err(BindingsError::Refused(DepsError::Env(format!(
        "morloc init ran, but these morloc bindings are still not current:\n  {}",
        lines.join("\n  ")
    ))))
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
    let lock = ctx.lock_env()?;
    let dropped = ctx.clean(&lock, &inputs)?;
    if !dropped.is_empty() {
        invalidate_shim_markers(&dropped);
        eprintln!("Dropped unused language runtime(s): {}.", dropped.join(", "));
    }
    drop(lock);
    eprintln!("Environment reset to its installed baseline.");
    Ok(())
}

/// Remove ONLY the dropped languages' shim markers under `<home>/opt/
/// lang-configured/`, so the next on-demand re-add rebuilds them against the
/// freshly solved interpreter while every still-present language keeps its valid
/// shim. `dropped` are runtime codes (python/r). Best-effort. A missing
/// `MORLOC_HOME` (e.g. `mim clean` run outside an env) simply skips this.
fn invalidate_shim_markers(dropped: &[String]) {
    let Ok(home) = morloc_home() else { return };
    let dir = abi::lang_marker_dir(&home);
    for lang in dropped {
        if let Some(marker) = abi::runtime_to_morloc_lang(lang).and_then(abi::shim_marker_name) {
            let _ = std::fs::remove_file(dir.join(marker));
        }
    }
}

/// `mim spec`: print the merged pixi manifest the next `sync` solves.
pub fn spec() -> Result<(), DepsError> {
    let ctx = env_context()?;
    let support = load_lang_support()?;
    let owned = solve_inputs()?;
    let inputs = owned.bind(&support);
    let _lock = ctx.lock_env()?;
    let specs = ctx.sync_world()?;
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

/// The hook's view of its environment: the environment's store, and the build's
/// state when that is not the store.
struct HookContext {
    ctx: EnvContext,
    foreign_state: Option<PathBuf>,
}

/// DEP-13: the compiler's state: `MORLOC_STATE`, else `MORLOC_HOME`, else
/// `$HOME/.local/share/morloc`, an empty value counting as unset.
pub(crate) fn compiler_state(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    set("MORLOC_STATE")
        .or_else(|| set("MORLOC_HOME"))
        .map(PathBuf::from)
        .or_else(|| set("HOME").map(|h| Path::new(&h).join(".local/share/morloc")))
}

/// DEP-11: the store is `MORLOC_ENV_STORE`, never the build's state, and it
/// must exist. The pixi project defaults to `<store>/pixi`; a container passes
/// `MORLOC_PIXI_DIR`. The build's state is the compiler's: `MORLOC_STATE`, else
/// `MORLOC_HOME`, else `$HOME/.local/share/morloc`, an empty value counting as
/// unset. States are compared and keyed by their canonical path.
fn hook_context(var: impl Fn(&str) -> Option<String>) -> Result<HookContext, DepsError> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    let store = set("MORLOC_ENV_STORE").map(PathBuf::from).ok_or_else(|| {
        DepsError::Env(
            "MORLOC_ENV_STORE is not set, so the environment this build belongs to is \
             unknown; nothing was changed. Run the build inside `mim run` or `mim shell`."
                .to_string(),
        )
    })?;
    let canon_store = std::fs::canonicalize(&store).map_err(|e| {
        DepsError::Env(format!(
            "the environment store {} named by MORLOC_ENV_STORE cannot be used ({e}); \
             nothing was changed. If the environment was renamed or removed, start it \
             again with `mim run` or `mim shell`.",
            store.display()
        ))
    })?;
    let state = compiler_state(&var);
    let foreign_state = match state.map(|s| std::fs::canonicalize(&s).unwrap_or(s)) {
        Some(s) if s == canon_store => None,
        Some(s) => Some(s),
        None => Some(PathBuf::new()),
    };
    let mut ctx = EnvContext::new(&store);
    if let Some(pixi_dir) = set("MORLOC_PIXI_DIR") {
        ctx = ctx.with_pixi_dir(pixi_dir);
    }
    Ok(HookContext { ctx, foreign_state })
}

fn env_context() -> Result<EnvContext, DepsError> {
    hook_context(|k| std::env::var(k).ok()).map(|h| h.ctx)
}

/// DEP-13: where a sync records its spec. A build under another state is a
/// scratch build named for its program and that state.
fn record_slot(name: &str, installed: bool, foreign_state: Option<&Path>) -> (String, Provenance) {
    use sha2::{Digest, Sha256};
    match foreign_state {
        None if installed => (name.to_string(), Provenance::Installed),
        None => (name.to_string(), Provenance::Scratch),
        Some(state) => {
            let digest = Sha256::digest(state.as_os_str().as_encoded_bytes());
            let tag = crate::provision::hex_lower(&digest[..6]);
            (format!("{name}@{tag}"), Provenance::Scratch)
        }
    }
}

/// The pixi binary: `MORLOC_PIXI` if the environment names it, else `pixi` on PATH
/// (resolved by the OS at spawn).
fn pixi_bin() -> PathBuf {
    pixi_bin_from(|k| std::env::var(k).ok())
}

/// IFC-9: `MORLOC_PIXI`, an empty value counting as unset, else `pixi`.
fn pixi_bin_from(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    var("MORLOC_PIXI").filter(|v| !v.is_empty()).map_or_else(|| PathBuf::from("pixi"), PathBuf::from)
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

    /// A stub compiler: `init --check` prints `$STATE/report` (or `report.after`
    /// once an init has run); a bare `init` records itself and succeeds.
    fn stub_compiler(dir: &Path, before: &str, after: &str) -> String {
        std::fs::write(dir.join("report"), before).unwrap();
        std::fs::write(dir.join("report.after"), after).unwrap();
        let exe = dir.join("morloc");
        let d = dir.display();
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho \"$*\" >> {d}/calls\n\
                 case \"$*\" in\n  *--check*) if [ -e {d}/ran ]; then cat {d}/report.after; else cat {d}/report; fi ;;\n\
                 init) touch {d}/ran ;;\n  *) exit 9 ;;\nesac\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe.display().to_string()
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls")).unwrap_or_default().lines().map(String::from).collect()
    }

    const OK: &str = r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"ok","reason":""}]}"#;
    const STALE: &str = r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"stale","reason":"built for another interpreter"}]}"#;
    const ABSENT: &str = r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"absent","reason":""}]}"#;
    const TORN: &str = r#"{"bindings_check":1,"runtime":{"ok":false,"reason":"the last runtime install did not finish"},"languages":[]}"#;
    const NO_PY: &str = r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"unavailable","reason":"python3 is not on PATH"}]}"#;

    #[test]
    fn a_compiler_is_asked_only_when_it_offers_the_check_and_is_named() {
        let mut support = LangSupport::from_json(r#"{"toolchain":[],"languages":{}}"#).unwrap();
        let bin = |k: &str| (k == "MORLOC_BIN").then(|| "/x/morloc".to_string());
        assert_eq!(bindings_authority(&support, bin), None);
        support.bindings_check = 1;
        assert_eq!(bindings_authority(&support, bin), Some("/x/morloc".to_string()));
        assert_eq!(bindings_authority(&support, |_| None), None);
        assert_eq!(bindings_authority(&support, |_| Some(String::new())), None);
    }

    #[test]
    fn the_compilers_report_is_read_as_written() {
        let r = parse_bindings_report(STALE).unwrap();
        assert!(r.runtime.ok);
        assert_eq!(r.languages[0].lang, "py");
        assert!(r.languages[0].rebuildable());
        assert!(parse_bindings_report("not json").is_err());
    }

    #[test]
    fn a_stale_binding_is_rebuilt_once_and_checked_again() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), STALE, OK);
        assert!(ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]).is_ok());
        let c = calls(tmp.path());
        assert_eq!(c.iter().filter(|l| l.as_str() == "init").count(), 1, "{c:?}");
        assert_eq!(c.iter().filter(|l| l.contains("--check")).count(), 2, "{c:?}");
        assert!(c[0].contains("--lang py"), "{c:?}");
    }

    #[test]
    fn current_bindings_run_no_init() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), OK, OK);
        assert!(ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]).is_ok());
        assert!(calls(tmp.path()).iter().all(|l| l.contains("--check")));
    }

    #[test]
    fn a_binding_still_stale_after_init_is_an_error_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), ABSENT, STALE);
        match ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]) {
            Err(BindingsError::Refused(e)) => assert!(e.to_string().contains("another interpreter"), "{e}"),
            _ => panic!("a binding still stale after init was accepted"),
        }
        assert_eq!(calls(tmp.path()).iter().filter(|l| l.as_str() == "init").count(), 1);
    }

    #[test]
    fn a_runtime_that_does_not_match_is_refused_without_rebuilding() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), TORN, OK);
        match ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]) {
            Err(BindingsError::Refused(e)) => assert!(e.to_string().contains("mim update --reinit"), "{e}"),
            _ => panic!("a torn runtime was accepted"),
        }
        assert!(calls(tmp.path()).iter().all(|l| l.contains("--check")));
    }

    #[test]
    fn a_binding_that_cannot_be_built_is_refused_without_rebuilding() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), NO_PY, OK);
        assert!(matches!(
            ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]),
            Err(BindingsError::Refused(_))
        ));
        assert!(calls(tmp.path()).iter().all(|l| l.contains("--check")));
    }

    #[test]
    fn a_compiler_that_cannot_answer_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), "garbage", OK);
        assert!(matches!(
            ensure_bindings(&morloc, &[], tmp.path(), &["py".to_string()]),
            Err(BindingsError::Unusable(_))
        ));
    }

    #[test]
    fn an_immutable_environment_refuses_by_the_bindings_state() {
        let absent = parse_bindings_report(ABSENT).unwrap();
        assert!(refuse_unless_current("p", &absent, &["py".to_string()]).unwrap_err().to_string().contains("mim modify --lang py"));
        let stale = parse_bindings_report(STALE).unwrap();
        assert!(refuse_unless_current("p", &stale, &["py".to_string()]).unwrap_err().to_string().contains("mim update --reinit"));
        let torn = parse_bindings_report(TORN).unwrap();
        assert!(refuse_unless_current("p", &torn, &["py".to_string()]).unwrap_err().to_string().contains("did not finish"));
        assert!(refuse_unless_current("p", &parse_bindings_report(OK).unwrap(), &["py".to_string()]).is_ok());
    }

    #[test]
    fn only_the_programs_languages_are_judged() {
        let report = parse_bindings_report(
            r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"ok","reason":""},{"lang":"r","state":"unavailable","reason":"R is not on PATH"}]}"#,
        )
        .unwrap();
        assert!(refuse_unless_current("p", &report, &["py".to_string()]).is_ok());
        assert!(refuse_unless_current("p", &report, &["cpp".to_string()]).is_err(), "an unreported language is not current");
    }

    #[test]
    fn a_program_with_no_bound_languages_asks_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let morloc = stub_compiler(tmp.path(), NO_PY, NO_PY);
        assert!(ensure_bindings(&morloc, &[], tmp.path(), &[]).is_ok());
        assert!(calls(tmp.path()).is_empty());
    }

    #[test]
    fn a_rebuild_is_judged_on_the_prefixs_languages_only() {
        let report = parse_bindings_report(
            r#"{"bindings_check":1,"runtime":{"ok":true},"languages":[{"lang":"py","state":"ok","reason":""},{"lang":"r","state":"unavailable","reason":"R is not on PATH"}]}"#,
        )
        .unwrap();
        assert!(report.problems(&["py".to_string()]).is_empty());
        assert_eq!(report.problems(&["r".to_string()]).len(), 1);
        assert_eq!(report.problems(&["cpp".to_string()]), vec!["cpp: not reported".to_string()]);
        assert_eq!(parse_bindings_report(TORN).unwrap().problems(&[]).len(), 1);
    }

    #[test]
    fn the_unactivated_check_leaves_the_ambient_path_alone() {
        let cmd = compiler_command("morloc", None, Path::new("/home"));
        assert!(cmd.get_envs().all(|(k, _)| k != "PATH"));
        assert!(cmd.get_envs().any(|(k, v)| k == "MORLOC_BUILD_HOOK" && v.is_none()));
    }

    #[test]
    fn declared_langs_missing_shims_uses_declared_langs_and_markers() {
        let home = tempfile::tempdir().unwrap();
        // Only python's binding is built.
        let markers = abi::lang_marker_dir(home.path());
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join("python"), "").unwrap();

        // A program declaring py + r + cpp + rust.
        let es = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"py"},{"lang":"r"},{"lang":"cpp"},{"lang":"rust"}]}"#,
        )
        .unwrap();

        // py is built (skipped); rust has no shim (skipped); r/cpp are missing.
        assert_eq!(
            declared_langs_missing_shims(&es, home.path()),
            vec!["r".to_string(), "cpp".to_string()]
        );
    }

    #[test]
    fn stale_declared_shims_covers_only_bindings_the_program_uses() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let meta = tmp.path().join("conda-meta");
        std::fs::create_dir_all(&meta).unwrap();
        // The prefix holds python 3.12; the binding was built for 3.13.
        std::fs::write(meta.join("python-3.12.9-0.json"), r#"{"name":"python","version":"3.12.9"}"#).unwrap();
        let markers = abi::lang_marker_dir(&home);
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join("python"), "").unwrap();
        std::fs::write(home.join("opt/pymorloc.cpython-313-x86_64-linux-gnu.so"), "").unwrap();

        let py = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"py"}]}"#,
        )
        .unwrap();
        let stale = stale_declared_shims(&py, &home, &meta);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].lang(), "py");

        // A C++ program does not care that the python binding is stale.
        let cpp = EnvSpec::from_json(
            r#"{"envspec_version":2,"morloc_version":"0","languages":[{"lang":"cpp"}]}"#,
        )
        .unwrap();
        assert!(stale_declared_shims(&cpp, &home, &meta).is_empty());

        // An unbuilt binding is the missing-marker path's business, not this one's.
        std::fs::remove_file(markers.join("python")).unwrap();
        assert!(stale_declared_shims(&py, &home, &meta).is_empty());
    }

    /// An empty MORLOC_PIXI is unset, like every variable the hook reads.
    #[test]
    fn an_empty_pixi_variable_means_pixi_on_path() {
        assert_eq!(pixi_bin_from(|_| Some(String::new())), PathBuf::from("pixi"));
        assert_eq!(pixi_bin_from(|_| None), PathBuf::from("pixi"));
        assert_eq!(pixi_bin_from(|_| Some("/x/pixi".to_string())), PathBuf::from("/x/pixi"));
    }

    fn vars(pairs: &[(&str, &Path)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.display().to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn the_hook_store_is_the_environments_not_the_states() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("env");
        let private = d.path().join("private");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&private).unwrap();
        let pixi = d.path().join("pixi");

        let hook = hook_context(vars(&[
            ("MORLOC_ENV_STORE", &store),
            ("MORLOC_STATE", &private),
            ("MORLOC_HOME", &store),
            ("MORLOC_PIXI_DIR", &pixi),
        ]))
        .unwrap();
        assert_eq!(hook.ctx.env_home(), store.as_path());
        assert_eq!(hook.ctx.pixi_dir(), pixi);
        assert_eq!(hook.foreign_state.as_deref(), Some(private.as_path()));

        // The environment's own state, spelled differently, is not foreign.
        let spelled = d.path().join("private/../env");
        let own = hook_context(vars(&[("MORLOC_ENV_STORE", &store), ("MORLOC_STATE", &spelled)])).unwrap();
        assert_eq!(own.foreign_state, None);

        // Natively MORLOC_STATE is unset and the state is MORLOC_HOME.
        let native = hook_context(vars(&[("MORLOC_ENV_STORE", &store), ("MORLOC_HOME", &store)])).unwrap();
        assert_eq!(native.foreign_state, None);
        assert_eq!(native.ctx.pixi_dir(), store.join("pixi"));
    }

    #[test]
    fn a_hook_without_the_store_marker_refuses() {
        let d = tempfile::tempdir().unwrap();
        let err = hook_context(vars(&[("MORLOC_STATE", d.path()), ("MORLOC_HOME", d.path())]))
            .err()
            .expect("a hook without MORLOC_ENV_STORE must refuse");
        assert!(err.to_string().contains("MORLOC_ENV_STORE"), "{err}");
    }

    #[test]
    fn a_hook_whose_store_is_gone_refuses_and_creates_nothing() {
        let d = tempfile::tempdir().unwrap();
        let gone = d.path().join("renamed-away");
        let err = hook_context(vars(&[("MORLOC_ENV_STORE", &gone), ("MORLOC_STATE", &gone)]))
            .err()
            .expect("a hook whose store does not exist must refuse");
        assert!(err.to_string().contains(&gone.display().to_string()), "{err}");
        assert!(!gone.exists());
    }

    #[test]
    fn one_state_has_one_scratch_key_however_it_is_spelled() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("env");
        let private = d.path().join("private");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&private).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let slash = PathBuf::from(format!("{}/", private.display()));

        let key = |state: &Path| {
            let hook = hook_context(vars(&[("MORLOC_ENV_STORE", &store), ("MORLOC_STATE", state)])).unwrap();
            record_slot("prog", true, hook.foreign_state.as_deref()).0
        };
        assert_eq!(key(&private), key(&slash));
        assert_eq!(key(&private), key(&link));
    }

    #[test]
    fn a_build_with_no_state_set_uses_the_compilers_default_state() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("env");
        std::fs::create_dir_all(&store).unwrap();
        let home = d.path().join("home");
        let default_state = home.join(".local/share/morloc");
        std::fs::create_dir_all(&default_state).unwrap();

        let hook = hook_context(vars(&[("MORLOC_ENV_STORE", &store), ("HOME", &home)])).unwrap();
        assert_eq!(hook.foreign_state, Some(default_state.canonicalize().unwrap()));

        // When that default is the store, the build is the environment's own.
        let own = hook_context(vars(&[("MORLOC_ENV_STORE", &default_state), ("HOME", &home)])).unwrap();
        assert_eq!(own.foreign_state, None);
    }

    /// DEP-12: the id check comes before any write, the vendored snapshot of
    /// a program's Python locals included.
    #[test]
    fn a_refused_install_writes_nothing_into_the_project() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("env");
        let shared = d.path().join("shared");
        let owner = EnvContext::new(d.path().join("owner")).with_pixi_dir(&shared);
        owner.adopt_env_ids(&owner.lock_env_creating().unwrap()).unwrap();
        let mine = EnvContext::new(&store);
        mine.adopt_env_ids(&mine.lock_env_creating().unwrap()).unwrap();
        let listing = || {
            let mut v: Vec<_> = std::fs::read_dir(&shared).unwrap().map(|e| e.unwrap().file_name()).collect();
            v.sort();
            v
        };
        let before = listing();

        let root = d.path().join("prog");
        std::fs::create_dir_all(root.join("src/mylib")).unwrap();
        std::fs::write(root.join("src/mylib/__init__.py"), "V=1\n").unwrap();
        let json = r#"{"envspec_version":2,"morloc_version":"0","packages":{"py":[{"name":"mylib","source":"local","path":"./src/mylib","editable":true}]}}"#;
        let support = LangSupport::from_json(
            r#"{"schema_version":"1.0","morloc_version":"0.99.0","languages":{"py":{"runtime":{"package":"python","version":">=3.10,<3.14","default":"3.12"}}}}"#,
        )
        .unwrap();
        let channels = morloc_deps::pixi::default_channels();
        let inputs = SolveInputs { lang_support: &support, pixi_bin: Path::new("/nonexistent/pixi"), platform: "linux-64", channels: &channels };
        let hook = HookContext { ctx: EnvContext::new(&store).with_pixi_dir(&shared), foreign_state: None };

        let err = sync_in(hook, &inputs, "prog", json, Some(&root), true, None).unwrap_err();
        assert!(err.to_string().contains("different environments"), "{err}");
        assert_eq!(listing(), before);
        assert!(EnvContext::new(&store).gather().unwrap().is_empty());
    }

    #[test]
    fn a_build_under_another_state_is_recorded_as_scratch() {
        let state = Path::new("/somewhere/state");
        assert_eq!(record_slot("prog", true, None), ("prog".to_string(), Provenance::Installed));
        assert_eq!(record_slot("prog", false, None), ("prog".to_string(), Provenance::Scratch));

        let (key, prov) = record_slot("prog", true, Some(state));
        assert_eq!(prov, Provenance::Scratch);
        assert_ne!(key, "prog");
        assert!(key.starts_with("prog@"), "{key}");
        assert!(!key.contains('/'), "{key}");
        assert_eq!(record_slot("prog", false, Some(state)).0, key, "one key per program and state");
        assert_ne!(record_slot("prog", true, Some(Path::new("/elsewhere"))).0, key);
    }

    #[test]
    fn the_environment_lock_is_the_one_the_hook_takes() {
        use nix::fcntl::{Flock, FlockArg};
        let d = tempfile::tempdir().unwrap();
        let data_dir = d.path().join("env");
        let private = d.path().join("private");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&private).unwrap();

        let manager = EnvContext::new(&data_dir);
        let _held = manager.lock_env().unwrap();

        let hook = hook_context(vars(&[
            ("MORLOC_ENV_STORE", &data_dir),
            ("MORLOC_STATE", &private),
            ("MORLOC_PIXI_DIR", &data_dir.join("pixi")),
        ]))
        .unwrap();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(hook.ctx.solve_lock_path())
            .unwrap();
        assert!(
            Flock::lock(file, FlockArg::LockExclusiveNonblock).is_err(),
            "the hook under a private state must wait on the manager's lock"
        );
    }
}
