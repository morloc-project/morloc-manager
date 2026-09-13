use std::fs;
use std::path::Path;

use chrono::Utc;
use sha2::{Digest, Sha256};
use crate::config;
use crate::error::{ManagerError, Result};
use crate::types::*;

/// Parts of an environment a deployment artifact cannot do without.
const REQUIRED_PARTS: [&str; 4] = ["runtime", "exe", "pixi/pixi.toml", "pixi/pixi.lock"];

/// Parts that travel when the environment has them. Module sources and the
/// install prefix for local native dependencies are on a pool's search path,
/// so they are not merely nice to have -- they are absent only when the
/// environment never had them.
const OPTIONAL_STATE: [&str; 3] = ["fdb", "src", "modules"];

/// Port the deployment image's router listens on. A fixed, documented port is
/// what makes `docker run -p <host>:8080 <image>` work without reading the
/// image; the host side is the operator's to choose.
pub const DEPLOY_HTTP_PORT: u16 = 8080;

/// What an environment declared it serves, or `None` when it declared nothing.
/// An environment that exposed nothing still freezes: the image is then a
/// command line rather than a server, which is a real use and better than
/// inventing a default command that would publish whatever was installed.
fn spec_from_exposure(ex: &ViewSet) -> Option<crate::ServeSpec> {
    if ex.is_empty() {
        return None;
    }
    Some(crate::ServeSpec::new(
        ex.mcp.clone(),
        ex.api.clone(),
        ex.eval.as_ref().map(|e| e.allow.join(",")),
    ))
}

/// Which deployment image to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// The environment whole: compiler, toolchain, pixi. Can eval and build.
    Full,
    /// The programs and what runs them. Cannot eval or build.
    Slim,
}

impl Flavor {
    pub fn label(self) -> &'static str {
        match self {
            Flavor::Full => "full",
            Flavor::Slim => "slim",
        }
    }
}

/// What a slim image needs to know about the environment beyond its data dir:
/// the base its image was built on, and the system packages layered onto it.
pub struct SlimBase<'a> {
    pub base_image: &'a str,
    pub system_packages: &'a [String],
}

/// Build a self-contained deployment image from an environment.
///
/// The image is the environment with its mounted halves baked in: the runtime
/// copied, the toolchain reinstalled from the environment's own lock, the
/// programs and the module sources behind them, and the exposed set compiled
/// into the default command. Nothing is left in the working directory, because
/// the artifact is a tag in the engine's image store -- moved by pushing it to
/// a registry, by `save_to`, or by rebuilding it.
///
/// The full image's base is the environment's own image and cannot be
/// anything else: it carries pixi to install the toolchain, the activation
/// wrapper every process goes through, and the compiler a sandboxed eval
/// forks. A slim image (`slim` given) uses that image only as a build stage
/// and starts its final stage from the environment's base; see
/// [`crate::dockerfile::generate_slim_dockerfile`].
#[allow(clippy::too_many_arguments)]
pub fn freeze_environment(
    scope: Scope,
    env_name: &str,
    ver: Version,
    engine: ContainerEngine,
    env_image: &str,
    v_data_dir: &str,
    tag: &str,
    save_to: Option<&str>,
    slim: Option<SlimBase<'_>>,
    force: bool,
    verbose: bool,
) -> Result<()> {
    let flavor = if slim.is_some() { Flavor::Slim } else { Flavor::Full };
    if !Path::new(v_data_dir).is_dir() {
        return Err(ManagerError::FreezeError(format!(
            "Data directory does not exist: {v_data_dir}"
        )));
    }
    let modules = scan_modules(&format!("{v_data_dir}/fdb"));
    let programs = scan_programs(&format!("{v_data_dir}/exe"));
    // An environment with nothing installed still freezes -- as a base to run
    // programs in later, or to hand someone the toolchain -- once the author
    // has confirmed that is what they mean. The programs directory has to
    // exist to be copied, whether or not anything is in it.
    fs::create_dir_all(Path::new(v_data_dir).join("exe"))
        .map_err(|e| ManagerError::FreezeError(format!("cannot create the programs directory: {e}")))?;

    let exposure = config::read_views(scope, env_name).unwrap_or_default();
    if flavor == Flavor::Slim {
        if let Some(eval) = &exposure.eval {
            return Err(ManagerError::FreezeError(format!(
                "the environment exposes eval ({}), and a slim image cannot evaluate: it \
                 carries no compiler. Remove the view (`mim view rm --eval`) or freeze \
                 without --slim.",
                eval.allow.join(",")
            )));
        }
    }
    check_programs(v_data_dir, &programs, force)?;

    // Validate the programs in the environment as it actually runs: the runtime
    // and the toolchain are mounts, not image layers, so a validation without
    // them probes an empty directory. HOME is under the state mount too, and is
    // created host-side so a program touching it does not hit ENOENT.
    let _ = config::ensure_env_home(v_data_dir);
    let (bind_mounts, volumes) = crate::base_mounts(v_data_dir);
    crate::serve::validate_programs(engine, env_image, &programs, bind_mounts, volumes, verbose)?;

    let paths = frozen_paths(Path::new(v_data_dir))?;
    for rel in &paths {
        check_readable_recursive(&Path::new(v_data_dir).join(rel))?;
    }

    // A build context holding exactly what travels. The environment directory
    // cannot be handed to the engine as-is: it also holds caches, logs, a build
    // context of its own and the environment's home, all of which would be sent
    // to the daemon and baked into a layer.
    let context = Path::new(v_data_dir).join(FREEZE_CONTEXT_SUBDIR);
    let _ = fs::remove_dir_all(&context);
    fs::create_dir_all(&context)
        .map_err(|e| ManagerError::FreezeError(format!("cannot create the build context: {e}")))?;
    eprintln!("Staging the environment into a build context...");
    for rel in &paths {
        if flavor == Flavor::Slim && rel == "runtime" {
            stage_runtime_without_build_inputs(Path::new(v_data_dir), &context)?;
        } else {
            stage_into_context(Path::new(v_data_dir), &context, rel)?;
        }
    }

    let cmd = deploy_command(&exposure);
    let optional_state: Vec<String> = paths
        .iter()
        .filter(|p| OPTIONAL_STATE.contains(&p.as_str()))
        .cloned()
        .collect();
    let labels = deploy_labels(env_name, &ver, flavor, &programs, &modules, &exposure);
    // Podman's OCI output format drops HEALTHCHECK and warns.
    let healthcheck = engine == ContainerEngine::Docker;

    let dockerfile = context.join("Dockerfile");
    let text = match &slim {
        None => crate::dockerfile::generate_deploy_dockerfile(
            &crate::dockerfile::DeployDockerfileInput {
                base_image: env_image,
                cmd: &cmd,
                optional_state: &optional_state,
                http_port: DEPLOY_HTTP_PORT,
                healthcheck,
                labels: &labels,
            },
        ),
        Some(base) => {
            let plan = write_prune_lists(Path::new(v_data_dir), &context)?;
            eprintln!(
                "Cutting {} build-only packages from the toolchain ({} kept)",
                plan.removed.len(),
                plan.kept.len()
            );
            if verbose {
                eprintln!("  removed: {}", plan.removed.join(" "));
            }
            let cert_file = crate::cert::stage_into_context(scope, env_name, &context)?;
            let extras = crate::dockerfile::BuildExtras {
                system_packages: base.system_packages.to_vec(),
            };
            crate::dockerfile::generate_slim_dockerfile(
                &crate::dockerfile::SlimDockerfileInput {
                    env_image,
                    base_image: base.base_image,
                    extras: &extras,
                    cert_file: cert_file.as_deref(),
                    cmd: &cmd,
                    optional_state: &optional_state,
                    http_port: DEPLOY_HTTP_PORT,
                    labels: &labels,
                    healthcheck,
                },
            )
        }
    };
    fs::write(&dockerfile, &text)
        .map_err(|e| ManagerError::FreezeError(format!("cannot write the Dockerfile: {e}")))?;

    eprintln!("Building the deployment image {tag}...");
    let cfg = crate::container::BuildConfig {
        dockerfile: dockerfile.to_string_lossy().to_string(),
        context: context.to_string_lossy().to_string(),
        tag: tag.to_string(),
        build_args: Vec::new(),
        extra_flags: Vec::new(),
    };
    let status = crate::container::container_build_visible(engine, &cfg);
    // The context is large (the runtime alone is around a hundred megabytes) and
    // is worth nothing once the image exists, so it goes whether or not the
    // build succeeded.
    let _ = fs::remove_dir_all(&context);
    if !status.success() {
        return Err(ManagerError::FreezeError(format!(
            "the deployment image build failed (see the output above). The environment \
             itself is untouched; nothing was frozen into '{tag}'."
        )));
    }

    eprintln!("Built {tag}");
    if flavor == Flavor::Slim {
        // The image stands alone, so it is checked with nothing mounted: the
        // launchers must find the nexus, and the nexus and every pool binary
        // must resolve their libraries from what the cut left behind.
        check_linkage(engine, tag, verbose)?;
        crate::serve::validate_programs(engine, tag, &programs, Vec::new(), Vec::new(), verbose)?;
    }
    if let Some(path) = save_to {
        eprintln!("Saving {tag} to {path}...");
        crate::container::save_image(engine, tag, path)
            .map_err(|e| ManagerError::FreezeError(format!("could not save {tag}: {e}")))?;
        eprintln!("Wrote {path} (load it elsewhere with `{} load -i {path}`)", engine.name());
    }
    eprintln!();
    for line in run_hints(engine, tag, flavor, &programs, !cmd.is_empty()) {
        eprintln!("{line}");
    }
    Ok(())
}

/// Directories that are the working state of a tool -- a version control
/// store, a compiler's output, an interpreter's bytecode cache, an editor's
/// notes -- and never an input to a running program. An installed program is
/// a mirror of its project directory, and the compiler's install filter drops
/// only `.git`, so one of these under `exe/<name>/` means the project's
/// `.morlocignore` does not name it and every freeze would carry it.
const TOOL_STATE_DIRS: &[&str] = &[
    ".git",
    ".claude",
    ".stack-work",
    ".cargo",
    ".pixi",
    ".venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".ipynb_checkpoints",
    "__pycache__",
    "node_modules",
    "target",
];

/// A single file this big inside a program is worth a question: pools are a
/// few megabytes, so it is data or build output.
const LARGE_FILE_BYTES: u64 = 50 << 20;

/// A program this big in total is worth the same question.
const HEAVY_PROGRAM_BYTES: u64 = 100 << 20;

/// What a walk of the installed programs found, before anything expensive
/// runs. Names are program names; paths are relative to `exe/<name>/`.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProgramAudit {
    /// Bytes per program, in program order.
    sizes: Vec<(String, u64)>,
    /// Tool-state directories present, sorted.
    tool_state: Vec<(String, String)>,
    /// Files past `LARGE_FILE_BYTES`, sorted.
    large_files: Vec<(String, String, u64)>,
}

/// Walk `exe/<name>/` for each program. A tool-state directory is recorded
/// and not entered -- it may be a build tree of gigabytes, and the point of
/// finding it is to stop quickly -- so a program's size excludes it. Symlinks
/// are not followed: a link out of the tree is copied as a file by the
/// staging step, so it is sized as one here.
fn audit_programs(exe_dir: &Path, names: &[String]) -> ProgramAudit {
    let mut audit = ProgramAudit::default();
    for name in names {
        let root = exe_dir.join(name);
        let mut total = 0u64;
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                if meta.is_dir() {
                    let file_name = entry.file_name();
                    if TOOL_STATE_DIRS.iter().any(|d| file_name.to_str() == Some(d)) {
                        audit.tool_state.push((name.clone(), rel));
                        continue;
                    }
                    stack.push(path);
                } else {
                    total += meta.len();
                    if meta.len() > LARGE_FILE_BYTES {
                        audit.large_files.push((name.clone(), rel, meta.len()));
                    }
                }
            }
        }
        audit.sizes.push((name.clone(), total));
    }
    audit.tool_state.sort();
    audit.large_files.sort();
    audit
}

/// Whether to go on after the audit.
#[derive(Debug)]
enum Verdict {
    Proceed,
    /// Nothing is wrong, but something deserves a look; the caller asks,
    /// prefacing the question with this.
    Confirm(String),
    Refuse(String),
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// Sizes always, and every large file, so what the image will weigh is on the
/// screen before the build rather than in `docker images` after it.
fn audit_report(audit: &ProgramAudit) -> Vec<String> {
    if audit.sizes.is_empty() {
        return vec!["Installed programs to freeze: none".to_string()];
    }
    let mut lines = vec!["Installed programs to freeze:".to_string()];
    for (name, bytes) in &audit.sizes {
        lines.push(format!("  {name:<20} {}", megabytes(*bytes)));
    }
    for (name, rel, bytes) in &audit.large_files {
        lines.push(format!("  large file: {name}/{rel} ({})", megabytes(*bytes)));
    }
    for (name, rel) in &audit.tool_state {
        lines.push(format!("  tool state: {name}/{rel} (not sized)"));
    }
    lines
}

/// Tool state is a refusal: it is never something a program reads, it is
/// always something the project should have ignored, and the fix belongs in
/// the project rather than in a list the freeze keeps. Weight is a question,
/// because a program may legitimately carry a data file, and only its author
/// knows. A terminal is asked; a script has no one to ask and is refused
/// unless it said `--force`, which answers both.
fn audit_verdict(audit: &ProgramAudit, force: bool, interactive: bool) -> Verdict {
    if force {
        return Verdict::Proceed;
    }
    if audit.sizes.is_empty() {
        let why = "No morloc programs are installed, so the image will hold the environment \
                   and nothing to run in it. Install one with 'mim install <dir>' first, or \
                   continue to freeze the environment alone.";
        return if interactive {
            Verdict::Confirm(why.to_string())
        } else {
            Verdict::Refuse(format!(
                "{why} This is not a terminal, so nobody can confirm; pass --force to freeze \
                 an environment with no programs."
            ))
        };
    }
    if !audit.tool_state.is_empty() {
        let mut msg = String::from(
            "installed programs carry tool-state directories that would be frozen into the image:\n",
        );
        for (name, rel) in &audit.tool_state {
            msg.push_str(&format!("  exe/{name}/{rel}\n"));
        }
        msg.push_str(
            "A program is installed as a mirror of its project directory. Name these in the \
             project's .morlocignore (one pattern per line, e.g. `target/`), reinstall the \
             program, and freeze again. To freeze them anyway, pass --force.",
        );
        return Verdict::Refuse(msg);
    }
    let heavy = audit.sizes.iter().any(|(_, b)| *b > HEAVY_PROGRAM_BYTES)
        || !audit.large_files.is_empty();
    if !heavy {
        return Verdict::Proceed;
    }
    if interactive {
        return Verdict::Confirm(
            "A program is installed as a mirror of its project directory, so the sizes \
             above are what the image will carry. Trim a program with its .morlocignore \
             and reinstall, or continue as is."
                .to_string(),
        );
    }
    Verdict::Refuse(format!(
        "an installed program is larger than {} or holds a file larger than {}, and this \
         is not a terminal, so nobody can confirm it. A program is installed as a mirror \
         of its project directory; trim it with the project's .morlocignore and reinstall, \
         or pass --force to freeze it as is.",
        megabytes(HEAVY_PROGRAM_BYTES),
        megabytes(LARGE_FILE_BYTES)
    ))
}

/// Refuse or ask before any container runs, so a project that needs its
/// `.morlocignore` fixed learns that in a second rather than after validation
/// and a staging copy.
fn check_programs(v_data_dir: &str, programs: &[ProgramEntry], force: bool) -> Result<()> {
    use std::io::{self, IsTerminal, Write};
    let names: Vec<String> = programs.iter().map(|p| p.name.clone()).collect();
    let audit = audit_programs(&Path::new(v_data_dir).join("exe"), &names);
    for line in audit_report(&audit) {
        eprintln!("{line}");
    }
    match audit_verdict(&audit, force, io::stdin().is_terminal()) {
        Verdict::Proceed => Ok(()),
        Verdict::Refuse(msg) => Err(ManagerError::FreezeError(msg)),
        Verdict::Confirm(why) => {
            eprintln!("{why}");
            eprint!("Continue? [y/N] ");
            io::stderr().flush().ok();
            let mut answer = String::new();
            io::stdin().read_line(&mut answer).ok();
            if matches!(answer.trim(), "y" | "yes" | "Y" | "YES") {
                Ok(())
            } else {
                Err(ManagerError::FreezeError("aborted; nothing was frozen.".to_string()))
            }
        }
    }
}

/// How to run the image just built, with the tag spelled out. An engine
/// resolves a bare name to `:latest`, and a name it does not hold locally to
/// a registry pull, so a tag typed from memory fails in ways that do not
/// mention the image that exists. Engine flags come before the image, where
/// an engine reads them; anything after it is the container's command.
fn run_hints(
    engine: ContainerEngine,
    tag: &str,
    flavor: Flavor,
    programs: &[ProgramEntry],
    serves: bool,
) -> Vec<String> {
    let exe = engine.name();
    let mut lines = vec![
        "To use the image:".to_string(),
        format!("  {exe} run -it --rm {tag} /bin/bash"),
    ];
    // A slim image has no compiler to list programs with; the label does.
    match flavor {
        Flavor::Full => lines.push(format!("  {exe} run --rm {tag} morloc list --programs")),
        Flavor::Slim => lines.push(format!(
            "  {exe} inspect -f '{{{{index .Config.Labels \"morloc.programs\"}}}}' {tag}"
        )),
    }
    // One example, on a launcher the validation above just ran.
    if let Some(first) = programs.first() {
        lines.push(format!("  {exe} run --rm {tag} {} --help", first.name));
    }
    if serves {
        lines.push(format!(
            "  {exe} run --rm -p {DEPLOY_HTTP_PORT}:{DEPLOY_HTTP_PORT} {tag}    # serve the exposed set"
        ));
    } else {
        lines.push(
            "  (no views are declared, so the image has no default command; \
             `mim view add <program> --as mcp,api` before freezing makes it serve)"
                .to_string(),
        );
    }
    lines
}

/// The default command a deployment image serves under: the router over the
/// environment's declared set, or nothing at all when it declared nothing.
///
/// The image serves without a bearer token, and the nexus says so at startup.
/// Inside a container the bind address carries no information about exposure --
/// it is always the wildcard, because a container's loopback is its own -- so
/// refusing on that basis would fire identically whether a port was published
/// or not. What can reach a container is decided outside it, by a published
/// port or a network or a gateway, and that is where access control belongs.
///
/// Eval is the exception and keeps its requirement. The image cannot see the
/// operator's exposure decision, but it can see that eval runs expressions the
/// caller writes rather than the functions the author exported, and that one
/// call can rebuild a pool. An operator who wants it open sets
/// MORLOC_EVAL_ALLOW_NO_AUTH.
fn deploy_command(exposure: &ViewSet) -> Vec<String> {
    match spec_from_exposure(exposure) {
        Some(spec) => crate::build_router_command(
            crate::serve::CONTAINER_MORLOC_STATE,
            DEPLOY_HTTP_PORT,
            "0.0.0.0",
            &spec,
            true,
            false,
        ),
        None => Vec::new(),
    }
}

/// Where a freeze stages its build context, under the environment's own data
/// dir so it shares a filesystem with what it copies and never crosses a mount.
const FREEZE_CONTEXT_SUBDIR: &str = "freeze-build";

/// Copy one frozen path into the build context, preserving its shape. A file
/// keeps its parent directory, so `pixi/pixi.lock` lands at `pixi/pixi.lock`
/// and the generated `COPY pixi/ ...` finds it.
fn stage_into_context(root: &Path, context: &Path, rel: &str) -> Result<()> {
    let from = root.join(rel);
    let to = context.join(rel);
    if from.is_dir() {
        return crate::provision::copy_dir_excluding(&from, &to, &[], false)
            .map_err(|e| ManagerError::FreezeError(format!("cannot stage {rel}: {e}")));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ManagerError::FreezeError(format!("cannot stage {rel}: {e}")))?;
    }
    fs::copy(&from, &to)
        .map_err(|e| ManagerError::FreezeError(format!("cannot stage {rel}: {e}")))?;
    Ok(())
}

/// Top-level runtime directories that exist to build programs, not to run
/// them: the C/C++ headers (with a precompiled header that is most of the
/// runtime's weight) and the Rust source libmorloc and the nexus were built
/// from. A slim image leaves them out.
const RUNTIME_BUILD_INPUTS: [&str; 2] = ["include", "rust"];

/// Stage `runtime/` without its build inputs. The exclusion is by top-level
/// name only: a directory called `include` deeper in the tree is a binding's
/// own and travels.
fn stage_runtime_without_build_inputs(root: &Path, context: &Path) -> Result<()> {
    let from = root.join("runtime");
    let to = context.join("runtime");
    let entries = fs::read_dir(&from)
        .map_err(|e| ManagerError::FreezeError(format!("cannot read runtime: {e}")))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if RUNTIME_BUILD_INPUTS.contains(&name.as_str()) {
            continue;
        }
        stage_into_context(&from, &to, &name)?;
    }
    Ok(())
}

/// Compute the cut from the environment's own package records and write the
/// two lists the slim Dockerfile consumes into the build context. The records
/// come from the host-readable mirror the environment keeps beside its lock
/// (the prefix itself is engine storage), and the roots from the manifest the
/// same lock was solved from.
fn write_prune_lists(v_data_dir: &Path, context: &Path) -> Result<morloc_deps::prune::PrunePlan> {
    use morloc_deps::prune;
    let pixi_dir = v_data_dir.join("pixi");
    let manifest = fs::read_to_string(pixi_dir.join("pixi.toml"))
        .map_err(|e| ManagerError::FreezeError(format!("cannot read pixi.toml: {e}")))?;
    let roots = prune::manifest_dependencies(&manifest);
    let meta_dir = morloc_deps::abi::meta_dir(&pixi_dir);
    let records = prune::read_records(&meta_dir);
    if records.is_empty() {
        return Err(ManagerError::FreezeError(format!(
            "no package records at {}; the environment's toolchain has not been \
             materialized. Run 'mim update' first.",
            meta_dir.display()
        )));
    }
    let plan = prune::plan_prune(&roots, &records).map_err(ManagerError::FreezeError)?;
    let mut files = Vec::new();
    for f in &plan.removed_files {
        files.extend_from_slice(f.as_bytes());
        files.push(0);
    }
    fs::write(context.join(crate::dockerfile::PRUNE_FILES), files)
        .map_err(|e| ManagerError::FreezeError(format!("cannot write the prune list: {e}")))?;
    let mut records_text = plan.removed_records.join("\n");
    if !records_text.is_empty() {
        records_text.push('\n');
    }
    fs::write(context.join(crate::dockerfile::PRUNE_RECORDS), records_text)
        .map_err(|e| ManagerError::FreezeError(format!("cannot write the prune list: {e}")))?;
    Ok(plan)
}

/// The shell that checks a slim image's dynamic linkage, run inside it. The
/// nexus, libmorloc and every compiled pool must resolve against what the
/// cut left. The language bindings are left out: they are loaded by an
/// interpreter that already holds the interpreter's own library, so `ldd`
/// on them reports that library missing whether or not anything is wrong.
/// LD_LIBRARY_PATH is what the nexus exports to pools at run time.
fn linkage_check_script() -> String {
    let mh = crate::serve::CONTAINER_MORLOC_HOME;
    let state = crate::serve::CONTAINER_MORLOC_STATE;
    format!(
        "export LD_LIBRARY_PATH={mh}/lib; \
         for f in {mh}/bin/morloc-nexus {mh}/lib/libmorloc.so $(find {state}/exe -name 'pool-*.out'); do \
           [ -e \"$f\" ] && ldd \"$f\" | sed \"s|^|$f: |\"; \
         done; true"
    )
}

/// Fail the freeze if anything in the slim image cannot resolve a library.
fn check_linkage(engine: ContainerEngine, tag: &str, verbose: bool) -> Result<()> {
    eprintln!("Checking dynamic linkage in {tag}...");
    let cfg = crate::container::RunConfig {
        command: Some(vec!["sh".to_string(), "-c".to_string(), linkage_check_script()]),
        ..crate::container::RunConfig::new(tag)
    };
    let (status, stdout, stderr) = crate::container::container_run_quiet(engine, &cfg);
    if verbose {
        eprintln!("{stdout}");
    }
    if !status.success() {
        return Err(ManagerError::FreezeError(format!(
            "the linkage check could not run in {tag}: {}",
            stderr.lines().take(5).collect::<Vec<_>>().join("\n")
        )));
    }
    let missing = morloc_deps::abi::unresolved_libs(&stdout);
    if missing.is_empty() {
        return Ok(());
    }
    Err(ManagerError::FreezeError(format!(
        "the slim image is missing shared libraries the programs need:\n  {}\n\
         The cut removed something a program links against. Freeze without --slim \
         and report this.",
        missing.join("\n  ")
    )))
}

/// What the image says about itself.
///
/// A frozen image leaves the manager's world entirely: nothing tracks it, and
/// whoever meets it next may have neither the environment it came from nor any
/// record of what went into it. So its provenance travels inside it, where
/// `docker inspect` will find it, rather than in a file beside it that can be
/// separated from it.
fn deploy_labels(
    env_name: &str,
    ver: &Version,
    flavor: Flavor,
    programs: &[ProgramEntry],
    modules: &[ModuleEntry],
    exposure: &ViewSet,
) -> Vec<(String, String)> {
    let join = |xs: Vec<String>| xs.join(",");
    let mut labels = vec![
        ("morloc.flavor".to_string(), flavor.label().to_string()),
        (
            "org.opencontainers.image.created".to_string(),
            Utc::now().to_rfc3339(),
        ),
        (
            "org.opencontainers.image.version".to_string(),
            ver.show(),
        ),
        (
            "org.opencontainers.image.title".to_string(),
            format!("morloc {env_name}"),
        ),
        ("morloc.environment".to_string(), env_name.to_string()),
        ("morloc.version".to_string(), ver.show()),
        (
            "morloc.programs".to_string(),
            join(programs.iter().map(|p| p.name.clone()).collect()),
        ),
    ];
    if !modules.is_empty() {
        labels.push((
            "morloc.modules".to_string(),
            join(modules.iter().map(|m| m.name.clone()).collect()),
        ));
    }
    if !exposure.mcp.is_empty() {
        labels.push(("morloc.mcp".to_string(), join(exposure.mcp.clone())));
    }
    if !exposure.api.is_empty() {
        labels.push(("morloc.api".to_string(), join(exposure.api.clone())));
    }
    if let Some(eval) = &exposure.eval {
        labels.push(("morloc.eval".to_string(), join(eval.allow.clone())));
    }
    labels
}

/// The parts of an environment a deployment artifact carries, as paths relative
/// to the environment data dir.
///
/// An environment is an image plus three host-side pieces, and a frozen artifact
/// has to carry the two the image does not hold. `runtime` is MORLOC_HOME: the
/// nexus, libmorloc, the language bindings, and the launcher of every installed
/// program -- all built after the image was, so none of it is in a layer.
/// `pixi.toml` and `pixi.lock` are the toolchain: the solved prefix itself is
/// engine storage rather than a directory, so what travels is the lock that
/// reproduces it. `exe` holds the programs being deployed, which is the point.
///
/// `fdb`, `src` and `modules` travel when present: module sources are what a
/// sandboxed eval reads, and `modules` is the install prefix for any local
/// native dependency a pool links against at run time.
///
/// Anything required and absent is an error naming it. A missing piece used to
/// be skipped in silence, which produced an artifact that looked whole, failed
/// its image build several steps later on a `COPY` of a path that was never
/// written, and gave no hint that the environment was the problem.
pub(crate) fn frozen_paths(v_data_dir: &Path) -> Result<Vec<String>> {
    let mut missing: Vec<&str> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for rel in REQUIRED_PARTS {
        if v_data_dir.join(rel).exists() {
            paths.push(rel.to_string());
        } else {
            missing.push(rel);
        }
    }
    if !missing.is_empty() {
        return Err(ManagerError::FreezeError(format!(
            "environment at '{}' is missing {} a deployment artifact cannot do without:\n  {}\n\
             Provision the environment first with 'mim update --env <env>'.",
            v_data_dir.display(),
            if missing.len() == 1 { "something" } else { "things" },
            missing.join("\n  ")
        )));
    }
    paths.extend(
        OPTIONAL_STATE
            .iter()
            .filter(|rel| v_data_dir.join(rel).exists())
            .map(|rel| rel.to_string()),
    );
    Ok(paths)
}

// ======================================================================
// Internal: scanning installed state
// ======================================================================

pub(crate) fn scan_modules(fdb_dir: &str) -> Vec<ModuleEntry> {
    let fdb_path = Path::new(fdb_dir);
    if !fdb_path.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(fdb_path) else {
        return Vec::new();
    };

    #[derive(serde::Deserialize)]
    struct ModuleStub {
        name: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        morloc_version: Option<String>,
        #[serde(default)]
        built_with_morloc: Option<String>,
    }

    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with(".module")
        })
        .filter_map(|e| {
            let bytes = fs::read(e.path()).ok()?;
            let stub: ModuleStub = serde_json::from_slice(&bytes).ok()?;
            let digest = Sha256::digest(&bytes);
            let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            Some(ModuleEntry {
                name: stub.name,
                version: stub.version,
                sha256,
                morloc_version: stub.morloc_version,
                built_with_morloc: stub.built_with_morloc,
            })
        })
        .collect()
}

pub(crate) fn scan_programs(exe_dir: &str) -> Vec<ProgramEntry> {
    let exe_path = Path::new(exe_dir);
    if !exe_path.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(exe_path) else {
        return Vec::new();
    };
    // Each installed program is a subdirectory exe/<name>/ with a manifest.json.
    entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            if !dir.is_dir() {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            // Installed layout: exe/<name>/<name>-build/manifest.json.
            let manifest = dir.join(format!("{}-build", name)).join("manifest.json");
            if !manifest.is_file() {
                return None;
            }
            let commands = parse_manifest_commands(&manifest);
            Some(ProgramEntry { name, commands })
        })
        .collect()
}

/// The commands a caller can invoke on a program. A manifest also lists the
/// terminal actions the compiler synthesizes for `@render` and `@with`, which
/// are reachable only as a flag on their parent and are marked `internal`.
fn parse_manifest_commands(path: &Path) -> Vec<String> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct ManifestStub {
        #[serde(default)]
        commands: Vec<ManifestStubCmd>,
    }
    #[derive(serde::Deserialize)]
    struct ManifestStubCmd {
        name: String,
        #[serde(default)]
        internal: bool,
    }
    match serde_json::from_slice::<ManifestStub>(&bytes) {
        Ok(stub) => stub
            .commands
            .into_iter()
            .filter(|c| !c.internal)
            .map(|c| c.name)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Walk a directory tree and verify every file is readable by the current user.
fn check_readable_recursive(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| {
        ManagerError::FreezeError(format!("Cannot read directory {}: {e}", dir.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            ManagerError::FreezeError(format!(
                "Cannot read entry in {}: {e}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            check_readable_recursive(&path)?;
        } else if fs::File::open(&path).is_err() {
            return Err(ManagerError::FreezeError(format!(
                "Unreadable file: {}. Fix permissions or remove before freezing.",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment data dir holding `present`, as a container environment
    /// lays one out: the runtime under `runtime/`, never at the root.
    fn env_dir(present: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for rel in present {
            let path = tmp.path().join(rel);
            if rel.contains('.') {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "").unwrap();
            } else {
                std::fs::create_dir_all(&path).unwrap();
            }
        }
        tmp
    }

    const WHOLE: [&str; 4] = ["runtime", "exe", "pixi/pixi.toml", "pixi/pixi.lock"];

    #[test]
    fn a_deployment_image_serves_the_declared_set() {
        let ex = ViewSet {
            mcp: vec!["dna".to_string()],
            api: vec!["util".to_string()],
            eval: None,
        };
        let cmd = deploy_command(&ex);
        assert!(cmd.windows(2).any(|w| w == ["--mcp", "dna"]), "{cmd:?}");
        assert!(cmd.windows(2).any(|w| w == ["--api", "util"]), "{cmd:?}");
        // A container's loopback is its own, so a published port only reaches a
        // service bound to all interfaces.
        assert!(cmd.windows(2).any(|w| w == ["--http-host", "0.0.0.0"]), "{cmd:?}");
        // The image serves without a token, because inside a container the bind
        // address says nothing about who can reach the process.
        assert!(cmd.iter().any(|a| a == "--allow-no-auth"), "{cmd:?}");
        // Eval is the exception and keeps its requirement: the image cannot see
        // the operator's exposure decision, but it can see that eval is
        // expensive and unbounded by what the author declared.
        assert!(!cmd.iter().any(|a| a == "--eval-allow-no-auth"), "{cmd:?}");
    }

    /// The image leaves the manager's world under a name the operator typed,
    /// and an engine resolves a bare name to `:latest` and a missing local
    /// image to a registry pull. The hints spell the full tag and put the
    /// engine's flags before it, where an engine reads them.
    #[test]
    fn run_hints_name_the_tag_and_the_programs() {
        let programs = vec![
            ProgramEntry { name: "pacman".to_string(), commands: vec!["play".to_string()] },
        ];
        let served = run_hints(ContainerEngine::Podman, "pacman:v1", Flavor::Full, &programs, true);
        let text = served.join("\n");
        assert!(text.contains("podman run -it --rm pacman:v1 /bin/bash"), "{text}");
        assert!(text.contains("podman run --rm pacman:v1 morloc list --programs"), "{text}");
        assert!(text.contains("podman run --rm pacman:v1 pacman --help"), "{text}");
        assert!(
            text.contains(&format!("podman run --rm -p {DEPLOY_HTTP_PORT}:{DEPLOY_HTTP_PORT} pacman:v1")),
            "{text}"
        );

        // With nothing exposed the image has no default command, and a hint to
        // serve it would start a container that exits at once.
        let cli_only = run_hints(ContainerEngine::Docker, "pacman:v1", Flavor::Full, &programs, false);
        let text = cli_only.join("\n");
        assert!(text.contains("docker run --rm pacman:v1 pacman --help"), "{text}");
        assert!(!text.contains("-p "), "{text}");
    }

    #[test]
    fn an_environment_with_no_views_gets_no_default_command() {
        assert!(spec_from_exposure(&ViewSet::default()).is_none());
        assert!(deploy_command(&ViewSet::default()).is_empty());
    }

    #[test]
    fn labels_say_what_the_image_holds() {
        // The image leaves the manager's world, so whoever meets it next may
        // have neither the environment nor any record of what went in.
        let programs = vec![ProgramEntry {
            name: "dna".to_string(),
            commands: vec!["revcomp".to_string()],
        }];
        let modules = vec![ModuleEntry {
            name: "root-py".to_string(),
            version: None,
            sha256: "abc".to_string(),
            morloc_version: None,
            built_with_morloc: None,
        }];
        let ex = ViewSet {
            mcp: vec!["dna".to_string()],
            api: Vec::new(),
            eval: Some(EvalCapability { allow: vec!["dna".to_string()] }),
        };
        let labels = deploy_labels("dev", &Version::new(0, 101, 0), Flavor::Slim, &programs, &modules, &ex);
        let get = |k: &str| {
            labels
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("morloc.environment").as_deref(), Some("dev"));
        assert_eq!(get("morloc.flavor").as_deref(), Some("slim"));
        assert_eq!(get("morloc.programs").as_deref(), Some("dna"));
        assert_eq!(get("morloc.modules").as_deref(), Some("root-py"));
        assert_eq!(get("morloc.mcp").as_deref(), Some("dna"));
        assert_eq!(get("morloc.eval").as_deref(), Some("dna"));
        // An adapter nothing was exposed on is absent rather than empty.
        assert_eq!(get("morloc.api"), None);
        assert!(get("org.opencontainers.image.version").is_some());
    }

    fn project(root: &Path, name: &str, files: &[(&str, usize)]) {
        for (rel, size) in files {
            let p = root.join("exe").join(name).join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![b'x'; *size]).unwrap();
        }
    }

    /// A program is installed as a mirror of its project directory, and the
    /// compiler's install filter drops only `.git`. Whatever tooling left
    /// beside the sources -- a cargo `target/`, a Python cache, an editor's
    /// notes -- is in `exe/<name>/` and would travel with every freeze.
    #[test]
    fn the_audit_finds_tool_state_and_weight() {
        let root = tempfile::tempdir().unwrap();
        project(
            root.path(),
            "todo",
            &[
                ("src/lib.py", 10),
                ("target/release/big.bin", 10),
                ("__pycache__/lib.cpython-313.pyc", 10),
                ("todo-build/pools/py/pool.py", 10),
            ],
        );
        project(root.path(), "atlas", &[("data/genome.fa", LARGE_FILE_BYTES as usize + 1)]);
        let audit = audit_programs(&root.path().join("exe"), &["todo".to_string(), "atlas".to_string()]);
        assert_eq!(
            audit.tool_state,
            vec![
                ("todo".to_string(), "__pycache__".to_string()),
                ("todo".to_string(), "target".to_string()),
            ]
        );
        assert_eq!(audit.large_files.len(), 1);
        assert_eq!(audit.large_files[0].0, "atlas");
        assert_eq!(audit.large_files[0].1, "data/genome.fa");
        // A tool-state directory is reported, not walked: it may be a
        // multi-gigabyte build tree, and the answer is to refuse quickly.
        assert_eq!(audit.sizes.iter().find(|(n, _)| n == "todo").unwrap().1, 20);
        let text = audit_report(&audit).join("\n");
        assert!(text.contains("todo/target") && text.contains("not sized"), "{text}");
    }

    /// Tool state is refused outright, with the fix named where it lives: the
    /// project's `.morlocignore`, followed by a reinstall. `--force` overrides.
    #[test]
    fn tool_state_refuses_the_freeze_unless_forced() {
        let audit = ProgramAudit {
            sizes: vec![("todo".to_string(), 40)],
            tool_state: vec![("todo".to_string(), "target".to_string())],
            large_files: Vec::new(),
        };
        match audit_verdict(&audit, false, true) {
            Verdict::Refuse(msg) => {
                assert!(msg.contains("todo"), "{msg}");
                assert!(msg.contains("target"), "{msg}");
                assert!(msg.contains(".morlocignore"), "{msg}");
                assert!(msg.contains("--force"), "{msg}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(audit_verdict(&audit, true, true), Verdict::Proceed));
    }

    /// Weight is a question, not a refusal: a program may legitimately carry a
    /// data file. A terminal gets asked; a script has to say `--force`.
    #[test]
    fn weight_asks_a_terminal_and_refuses_a_script() {
        let heavy = ProgramAudit {
            sizes: vec![("atlas".to_string(), HEAVY_PROGRAM_BYTES + 1)],
            tool_state: Vec::new(),
            large_files: Vec::new(),
        };
        assert!(matches!(audit_verdict(&heavy, false, true), Verdict::Confirm(_)));
        match audit_verdict(&heavy, false, false) {
            Verdict::Refuse(msg) => assert!(msg.contains("--force"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(audit_verdict(&heavy, true, false), Verdict::Proceed));

        let big_file = ProgramAudit {
            sizes: vec![("atlas".to_string(), 1)],
            tool_state: Vec::new(),
            large_files: vec![("atlas".to_string(), "data/genome.fa".to_string(), LARGE_FILE_BYTES + 1)],
        };
        assert!(matches!(audit_verdict(&big_file, false, true), Verdict::Confirm(_)));

        let light = ProgramAudit {
            sizes: vec![("todo".to_string(), 40)],
            tool_state: Vec::new(),
            large_files: Vec::new(),
        };
        assert!(matches!(audit_verdict(&light, false, false), Verdict::Proceed));
    }

    /// An environment with nothing installed is still worth freezing -- as a
    /// base, or to hand someone the toolchain -- but not by accident.
    #[test]
    fn no_programs_is_a_question_not_a_refusal() {
        let empty = ProgramAudit::default();
        match audit_verdict(&empty, false, true) {
            Verdict::Confirm(why) => assert!(why.contains("No morloc programs"), "{why}"),
            other => panic!("expected a question, got {other:?}"),
        }
        match audit_verdict(&empty, false, false) {
            Verdict::Refuse(msg) => assert!(msg.contains("--force"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(audit_verdict(&empty, true, false), Verdict::Proceed));
        assert_eq!(audit_report(&empty), vec!["Installed programs to freeze: none"]);
    }

    #[test]
    fn the_audit_report_shows_every_size_and_every_large_file() {
        let audit = ProgramAudit {
            sizes: vec![("atlas".to_string(), 300 << 20), ("todo".to_string(), 1 << 20)],
            tool_state: Vec::new(),
            large_files: vec![("atlas".to_string(), "data/genome.fa".to_string(), 299 << 20)],
        };
        let text = audit_report(&audit).join("\n");
        assert!(text.contains("atlas") && text.contains("300.0 MB"), "{text}");
        assert!(text.contains("todo") && text.contains("1.0 MB"), "{text}");
        assert!(text.contains("data/genome.fa") && text.contains("299.0 MB"), "{text}");
    }

    /// A slim image has no `morloc` to list programs with; the label is how
    /// whoever holds the image learns what is in it.
    #[test]
    fn slim_hints_read_the_label_instead_of_running_the_compiler() {
        let programs = vec![ProgramEntry { name: "dna".to_string(), commands: vec![] }];
        let text = run_hints(ContainerEngine::Docker, "dna:v1-slim", Flavor::Slim, &programs, false).join("\n");
        assert!(text.contains("docker inspect -f '{{index .Config.Labels \"morloc.programs\"}}' dna:v1-slim"), "{text}");
        assert!(!text.contains("morloc list"), "{text}");
    }

    /// The headers and the Rust source built the runtime; nothing runs them.
    #[test]
    fn a_slim_runtime_leaves_its_build_inputs_behind() {
        let root = tempfile::tempdir().unwrap();
        for rel in [
            "runtime/bin/morloc-nexus",
            "runtime/lib/libmorloc.so",
            "runtime/opt/pymorloc/include/x.h",
            "runtime/include/morloc.h",
            "runtime/rust/Cargo.toml",
        ] {
            let p = root.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        }
        let ctx = root.path().join("ctx");
        stage_runtime_without_build_inputs(root.path(), &ctx).unwrap();
        assert!(ctx.join("runtime/bin/morloc-nexus").is_file());
        assert!(ctx.join("runtime/lib/libmorloc.so").is_file());
        // Only the top-level build inputs go; a binding's own include dir stays.
        assert!(ctx.join("runtime/opt/pymorloc/include/x.h").is_file());
        assert!(!ctx.join("runtime/include").exists());
        assert!(!ctx.join("runtime/rust").exists());
    }

    /// The lists the Dockerfile consumes: NUL-separated paths (a path may hold
    /// anything but NUL) and one record name per line.
    #[test]
    fn prune_lists_are_written_from_the_mirror_and_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let pixi = root.path().join("pixi");
        let mirror = pixi.join(morloc_deps::abi::CONDA_META_MIRROR);
        std::fs::create_dir_all(&mirror).unwrap();
        std::fs::write(
            pixi.join("pixi.toml"),
            "[workspace]\nname = \"x\"\n\n[dependencies]\n\"python\" = \"*\"\n\"rust\" = \"*\"\n",
        )
        .unwrap();
        std::fs::write(
            mirror.join("python-3.13.1-h0.json"),
            r#"{"name":"python","depends":["libgcc"],"files":["bin/python3"]}"#,
        )
        .unwrap();
        std::fs::write(
            mirror.join("libgcc-14.2-h0.json"),
            r#"{"name":"libgcc","depends":[],"files":["lib/libgcc_s.so.1"]}"#,
        )
        .unwrap();
        std::fs::write(
            mirror.join("rust-1.83-h0.json"),
            r#"{"name":"rust","depends":["libgcc"],"files":["bin/rustc","bin/cargo"]}"#,
        )
        .unwrap();
        let ctx = root.path().join("ctx");
        std::fs::create_dir_all(&ctx).unwrap();
        let plan = write_prune_lists(root.path(), &ctx).unwrap();
        assert_eq!(plan.removed, vec!["rust"]);
        let files = std::fs::read(ctx.join(crate::dockerfile::PRUNE_FILES)).unwrap();
        assert_eq!(files, b"bin/cargo\0bin/rustc\0conda-meta/rust-1.83-h0.json\0");
        let records = std::fs::read_to_string(ctx.join(crate::dockerfile::PRUNE_RECORDS)).unwrap();
        assert_eq!(records, "rust-1.83-h0.json\n");
    }

    #[test]
    fn a_slim_freeze_needs_materialized_records() {
        let root = tempfile::tempdir().unwrap();
        let pixi = root.path().join("pixi");
        std::fs::create_dir_all(&pixi).unwrap();
        std::fs::write(pixi.join("pixi.toml"), "[dependencies]\n\"python\" = \"*\"\n").unwrap();
        let ctx = root.path().join("ctx");
        std::fs::create_dir_all(&ctx).unwrap();
        let err = write_prune_lists(root.path(), &ctx).unwrap_err().to_string();
        assert!(err.contains("mim update"), "{err}");
    }

    /// The bindings are deliberately not checked: an interpreter extension
    /// resolves the interpreter's library from the process that loads it.
    #[test]
    fn the_linkage_check_covers_the_nexus_the_library_and_the_pools() {
        let script = linkage_check_script();
        assert!(script.contains("/opt/morloc/bin/morloc-nexus"), "{script}");
        assert!(script.contains("/opt/morloc/lib/libmorloc.so"), "{script}");
        assert!(script.contains("-name 'pool-*.out'"), "{script}");
        assert!(!script.contains("rmorloc") && !script.contains("pymorloc"), "{script}");
        assert!(script.contains("LD_LIBRARY_PATH=/opt/morloc/lib"), "{script}");
    }

    #[test]
    fn a_whole_environment_carries_its_runtime_and_its_lock() {
        let tmp = env_dir(&WHOLE);
        let got = frozen_paths(tmp.path()).unwrap();
        // The runtime is the half the image does not hold, and the lock is what
        // reproduces the toolchain where the artifact lands.
        assert!(got.contains(&"runtime".to_string()), "{got:?}");
        assert!(got.contains(&"pixi/pixi.lock".to_string()), "{got:?}");
        assert!(got.contains(&"exe".to_string()), "{got:?}");
    }

    #[test]
    fn optional_parts_travel_only_when_present() {
        let bare = env_dir(&WHOLE);
        let got = frozen_paths(bare.path()).unwrap();
        assert!(!got.contains(&"fdb".to_string()), "{got:?}");

        let mut with_extras: Vec<&str> = WHOLE.to_vec();
        with_extras.extend(["fdb", "src", "modules"]);
        let full = env_dir(&with_extras);
        let got = frozen_paths(full.path()).unwrap();
        for rel in ["fdb", "src", "modules"] {
            assert!(got.contains(&rel.to_string()), "{rel} missing from {got:?}");
        }
    }

    #[test]
    fn a_missing_runtime_is_named_not_skipped() {
        // The old behaviour: absent directories were dropped from the archive
        // without a word, and the artifact failed its image build later on a
        // path nobody had been told was never written.
        let tmp = env_dir(&["exe", "pixi/pixi.toml", "pixi/pixi.lock"]);
        let err = frozen_paths(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("runtime"), "{err}");
    }

    #[test]
    fn a_missing_lock_is_named() {
        let tmp = env_dir(&["runtime", "exe", "pixi/pixi.toml"]);
        let err = frozen_paths(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("pixi/pixi.lock"), "{err}");
    }

    #[test]
    fn every_missing_part_is_reported_at_once() {
        // One round trip per missing piece is a bad way to learn what an
        // environment lacks.
        let tmp = env_dir(&["exe"]);
        let err = frozen_paths(tmp.path()).unwrap_err().to_string();
        for rel in ["runtime", "pixi/pixi.toml", "pixi/pixi.lock"] {
            assert!(err.contains(rel), "{rel} missing from: {err}");
        }
    }

    /// A `--' @render` or `--' @with` directive makes the compiler synthesize a
    /// command that is not callable in its own right; a client selects it with a
    /// flag on its parent. Counting those reports more commands than the program
    /// has.
    #[test]
    fn a_synthesized_terminal_is_not_a_callable_command() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("manifest.json");
        std::fs::write(
            &manifest,
            r#"{"name":"todo","commands":[
                 {"name":"list","internal":false},
                 {"name":"mlcp_list_draw","internal":true},
                 {"name":"add","internal":false}
               ]}"#,
        )
        .unwrap();
        assert_eq!(parse_manifest_commands(&manifest), vec!["list", "add"]);
    }

    /// A manifest predating the `internal` field names only callable commands.
    #[test]
    fn a_command_without_the_field_is_callable() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("manifest.json");
        std::fs::write(&manifest, r#"{"name":"p","commands":[{"name":"only"}]}"#).unwrap();
        assert_eq!(parse_manifest_commands(&manifest), vec!["only"]);
    }
}
