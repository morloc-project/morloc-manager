use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;

use crate::error::ManagerError;
type MimResult<T> = crate::error::Result<T>;
use crate::types::ContainerEngine;

// ======================================================================
// Configuration records
// ======================================================================

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub image: String,
    pub bind_mounts: Vec<(String, String)>,
    /// Engine-managed volumes as `(volume name, container path)`. Unlike a bind
    /// mount these live in the engine's own storage, so their filesystem is the
    /// engine's rather than the host's -- on macOS that is the Linux filesystem
    /// inside the engine VM, which preserves letter case and is not reached
    /// through a file-sharing layer. Applied by the OCI engines only; Apptainer
    /// has no equivalent and leaves the underlying bind mount showing through.
    pub volumes: Vec<(String, String)>,
    pub ports: Vec<(u16, u16)>,
    /// Host interface to publish ports on (`docker -p <host>:H:C`). `None`
    /// binds all interfaces (0.0.0.0); `Some("127.0.0.1")` restricts to
    /// loopback so the service is not reachable off the host.
    pub publish_host: Option<String>,
    /// Container network mode (`docker --network <net>`). `Some("host")` shares
    /// the host network namespace so a `--http-host 127.0.0.1` bind lands on the
    /// host's loopback, unreachable by sibling containers; `-p` is then invalid
    /// and suppressed. `None` uses the engine default (bridge).
    pub network: Option<String>,
    pub env: Vec<(String, String)>,
    /// Variables whose values must not appear on the engine's command line
    /// (SERVE-4): named in the argv, valued in the engine process's environment
    /// (`engine_process_env`). Only for names the engine CLI itself ignores.
    pub secret_env: Vec<(String, String)>,
    pub read_only: bool,
    pub interactive: bool,
    pub remove_after: bool,
    pub name: Option<String>,
    pub shm_size: Option<String>,
    pub command: Option<Vec<String>>,
    pub work_dir: Option<String>,
    pub selinux_suffix: String,
    /// RUN-1: the container runs with SELinux labelling off, so a directory
    /// unsafe to relabel (the user's home) is mounted unrelabelled.
    pub label_disable: bool,
    /// RUN-1: a server drops every capability and cannot gain privileges.
    pub hardened: bool,
    pub extra_flags: Vec<String>,
    /// The architecture the image was built for, passed as `--platform` so the
    /// engine never guesses from the host or from `DOCKER_DEFAULT_PLATFORM`.
    /// `None` leaves the choice to the engine. Apptainer has no equivalent and
    /// ignores it.
    pub platform: Option<crate::arch::Arch>,
}

impl RunConfig {
    pub fn new(image: &str) -> Self {
        Self {
            image: image.to_string(),
            bind_mounts: Vec::new(),
            volumes: Vec::new(),
            ports: Vec::new(),
            publish_host: None,
            network: None,
            env: Vec::new(),
            secret_env: Vec::new(),
            read_only: false,
            interactive: false,
            remove_after: true,
            name: None,
            shm_size: None,
            command: None,
            work_dir: None,
            selinux_suffix: String::new(),
            label_disable: false,
            hardened: false,
            extra_flags: Vec::new(),
            platform: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub dockerfile: String,
    pub context: String,
    pub tag: String,
    pub build_args: Vec<(String, String)>,
    /// User-supplied build-engine flags (from env.flags.yaml `build` section
    /// plus any one-shot CLI overrides). Forwarded verbatim into the build
    /// argv after `--build-arg` pairs, before the context.
    pub extra_flags: Vec<String>,
    /// The architecture to build for, passed as `--platform`: it selects the
    /// base image's manifest entry and the emulator every RUN step executes
    /// under. `None` leaves it to the engine.
    pub platform: Option<crate::arch::Arch>,
}



// ======================================================================
// Engine detection
// ======================================================================

/// Returns the executable name for the given engine. For Apptainer, runtime-
/// detects `apptainer` (preferred) then `singularity`; result is cached for
/// the lifetime of the process.
pub fn engine_executable(engine: ContainerEngine) -> &'static str {
    match engine {
        ContainerEngine::Docker => "docker",
        ContainerEngine::Podman => "podman",
        ContainerEngine::Apptainer => apptainer_executable(),
    }
}

/// Cached selection of the Apptainer binary. Resolved once per process.
fn apptainer_executable() -> &'static str {
    static EXE: OnceLock<&'static str> = OnceLock::new();
    EXE.get_or_init(|| {
        if has_on_path("apptainer") {
            "apptainer"
        } else if has_on_path("singularity") {
            "singularity"
        } else {
            // Fall through to "apptainer"; the actual exec will fail loudly
            // with a "command not found" the user can act on. Better than a
            // silent compile-time decision.
            "apptainer"
        }
    })
}


/// Internal: argv-shape used by the dispatch in build_run_args.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgStyle {
    /// Docker/Podman `run` semantics.
    Oci,
    /// Apptainer `exec`/`shell` semantics.
    Apptainer,
}

fn argstyle(engine: ContainerEngine) -> ArgStyle {
    match engine {
        ContainerEngine::Docker | ContainerEngine::Podman => ArgStyle::Oci,
        ContainerEngine::Apptainer => ArgStyle::Apptainer,
    }
}

/// Check $PATH for an executable named `exe`. Reads PATH directly rather than
/// shelling out: avoids the cost of a process spawn and makes the check
/// hermetic against shell aliases.
fn has_on_path(exe: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    for dir in path.split(':') {
        let candidate = Path::new(dir).join(exe);
        if let Ok(meta) = candidate.metadata() {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return true;
            }
        }
    }
    false
}

// ======================================================================
// Operations
// ======================================================================

pub fn container_run(engine: ContainerEngine, cfg: &RunConfig) -> MimResult<(ExitStatus, String, String)> {
    let args = run_argv(engine, cfg)?;
    Ok(run_process_env(engine_executable(engine), &args, &engine_process_env(engine, cfg)))
}

/// The variables the engine process itself must carry so the container sees
/// `cfg.secret_env`: as named for docker/podman `-e NAME`, and for Apptainer
/// under the prefixes it forwards without a flag.
pub fn engine_process_env(engine: ContainerEngine, cfg: &RunConfig) -> Vec<(String, String)> {
    match argstyle(engine) {
        ArgStyle::Oci => cfg.secret_env.clone(),
        ArgStyle::Apptainer => apptainer_forwarded(&cfg.secret_env),
    }
}

/// `vars` as Apptainer forwards them into a container: `APPTAINERENV_`, or
/// `SINGULARITYENV_` when the binary is SingularityCE (`apptainer_executable`).
pub fn apptainer_forwarded(vars: &[(String, String)]) -> Vec<(String, String)> {
    vars.iter()
        .flat_map(|(k, v)| {
            [("APPTAINERENV_", k, v), ("SINGULARITYENV_", k, v)]
                .map(|(p, k, v)| (format!("{p}{k}"), v.clone()))
        })
        .collect()
}

/// Like `container_run` but captures both stdout and stderr (no streaming).
pub fn container_run_quiet(engine: ContainerEngine, cfg: &RunConfig) -> MimResult<(ExitStatus, String, String)> {
    let args = run_argv(engine, cfg)?;
    Ok(run_process_quiet_env(engine_executable(engine), &args, &engine_process_env(engine, cfg)))
}

pub fn container_run_passthrough(
    engine: ContainerEngine,
    verbose: bool,
    shell: bool,
    cfg: &RunConfig,
) -> MimResult<ExitStatus> {
    let exe = engine_executable(engine);
    let args = run_argv(engine, cfg)?;

    if verbose || shell {
        let quoted: Vec<String> = args
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("'{a}'")
                } else {
                    a.clone()
                }
            })
            .collect();
        eprintln!("[mim] {exe} {}", quoted.join(" "));
    }

    Command::new(exe)
        .args(&args)
        .envs(engine_process_env(engine, cfg))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_or_else(|e| Ok(report_spawn_failure(exe, e)), Ok)
}

/// Build a container image with all output (stdout+stderr) redirected to stderr.
/// Use for IO () commands where stdout must stay clean.
pub fn container_build_visible(engine: ContainerEngine, cfg: &BuildConfig) -> ExitStatus {
    let exe = engine_executable(engine);
    let args = build_build_args(cfg);
    run_process_to_stderr(exe, &args)
}




pub fn image_exists_locally(engine: ContainerEngine, image: &str) -> bool {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            Command::new(exe)
                .args(["image", "inspect", image])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        // For Apptainer, "image" is a .sif file path on disk. A file's
        // existence is the same as the image being available.
        ArgStyle::Apptainer => Path::new(image).is_file(),
    }
}

/// Run `image inspect` and return the stderr if it fails.
/// Returns None on success, Some(stderr) on failure.
pub fn image_inspect_stderr(engine: ContainerEngine, image: &str) -> Option<String> {
    let exe = engine_executable(engine);
    let output = Command::new(exe)
        .args(["image", "inspect", image])
        .stdout(Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        None
    } else {
        Some(String::from_utf8_lossy(&output.stderr).to_string())
    }
}



pub fn container_stop(engine: ContainerEngine, name_or_id: &str) -> (ExitStatus, String) {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (code, _, err) = run_process(exe, &["stop".to_string(), name_or_id.to_string()]);
            (code, err)
        }
        // For Apptainer, `mim run` is one-shot; there is nothing
        // to stop. Long-running instances are managed in serve.rs via
        // `apptainer instance stop`. Return a successful no-op so callers
        // doing pre-emptive cleanup (e.g. remove_environment) succeed.
        ArgStyle::Apptainer => (no_op_exit_status(), String::new()),
    }
}

pub fn container_remove(engine: ContainerEngine, name_or_id: &str) -> ExitStatus {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (code, _, _) = run_process(
                exe,
                &["rm".to_string(), "-f".to_string(), name_or_id.to_string()],
            );
            code
        }
        ArgStyle::Apptainer => no_op_exit_status(),
    }
}

/// Quiet container removal: suppresses stderr (for pre-emptive cleanup).
pub fn container_remove_quiet(engine: ContainerEngine, name_or_id: &str) -> ExitStatus {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (code, _, _) = run_process_quiet(
                exe,
                &["rm".to_string(), "-f".to_string(), name_or_id.to_string()],
            );
            code
        }
        ArgStyle::Apptainer => no_op_exit_status(),
    }
}

/// What an engine said about whether an object exists. `Unknown` means the
/// engine could not answer (daemon down, socket permissions, no session), which
/// must never be read as "absent" by code that deletes or forgets things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Present,
    Absent,
    Unknown(String),
}

/// Classify an `<engine> <kind> inspect` outcome. Docker says "No such
/// container/image/volume" or "No such object"; podman says "no such ..." or
/// "... not known". An engine binary that is not installed holds nothing.
fn classify_inspect(outcome: io::Result<std::process::Output>) -> Probe {
    match outcome {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Probe::Absent,
        Err(e) => Probe::Unknown(e.to_string()),
        Ok(out) if out.status.success() => Probe::Present,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
            if err.contains("no such") || err.contains("not known") {
                Probe::Absent
            } else {
                Probe::Unknown(err.trim().to_string())
            }
        }
    }
}

/// Ask an OCI engine whether an object of `kind` (`container`, `image`,
/// `volume`) named `name` exists. Apptainer has none of these engine objects.
pub fn probe(engine: ContainerEngine, kind: &str, name: &str) -> Probe {
    match argstyle(engine) {
        ArgStyle::Oci => classify_inspect(
            Command::new(engine_executable(engine))
                .args([kind, "inspect", name])
                .stdin(Stdio::null())
                .output(),
        ),
        ArgStyle::Apptainer => Probe::Absent,
    }
}

/// Check whether a container with this name exists (running or stopped).
pub fn container_exists(engine: ContainerEngine, name: &str) -> bool {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            Command::new(exe)
                .args(["container", "inspect", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        // For Apptainer, persistent containers live as instances and are
        // queried via serve.rs. The generic container_exists check on the
        // OCI side is only meaningful in the OCI world, so report false
        // here -- callers should go through serve::query_serve_containers
        // for the Apptainer path.
        ArgStyle::Apptainer => false,
    }
}

/// Remove an engine-managed volume. Quiet and best-effort: a volume that was
/// never created (an environment removed before it was ever materialized) is not
/// an error. Apptainer has no volumes, so this is a no-op there.
pub fn volume_remove(engine: ContainerEngine, name: &str) -> ExitStatus {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (code, _, _) = run_process_quiet(
                exe,
                &["volume".to_string(), "rm".to_string(), "-f".to_string(), name.to_string()],
            );
            code
        }
        ArgStyle::Apptainer => no_op_exit_status(),
    }
}

/// Copy the contents of volume `from` into a fresh volume `to`, through a
/// throwaway container of `image`. Neither engine can rename a volume, so this
/// is how a volume follows its environment. `to` is mounted at `to_mount`, a
/// path the image prepares with the mode a new volume should inherit; `from`
/// is mounted read-only elsewhere. Runs as the host user like every other
/// materialize step, so the copy keeps the ownership the prefix was written
/// with.
pub fn volume_copy(
    engine: ContainerEngine,
    image: &str,
    platform: Option<crate::arch::Arch>,
    from: &str,
    to: &str,
    to_mount: &str,
) -> Result<(), String> {
    if argstyle(engine) != ArgStyle::Oci {
        return Err("apptainer has no volumes to copy".to_string());
    }
    let from_mount = "/mnt/morloc-volume-from";
    let cfg = RunConfig {
        volumes: vec![
            (from.to_string(), format!("{from_mount}:ro")),
            (to.to_string(), to_mount.to_string()),
        ],
        platform,
        remove_after: true,
        command: Some(vec![
            "cp".to_string(),
            "-a".to_string(),
            format!("{from_mount}/."),
            format!("{to_mount}/"),
        ]),
        ..RunConfig::new(image)
    };
    let (status, _, stderr) = container_run_quiet(engine, &cfg).map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(stderr.trim().to_string())
    }
}

/// Give a local image a second tag. The image keeps its old tag; drop that with
/// `remove_image`, which only untags while another tag still references it.
pub fn tag_image(engine: ContainerEngine, from: &str, to: &str) -> Result<(), String> {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (status, _, stderr) =
                run_process_quiet(exe, &["tag".to_string(), from.to_string(), to.to_string()]);
            if status.success() {
                Ok(())
            } else {
                Err(stderr.trim().to_string())
            }
        }
        ArgStyle::Apptainer => Err("apptainer has no image store to tag in".to_string()),
    }
}

/// Write an image to a tarball with the engine's own `save`, so an artifact can
/// cross to a machine with no registry between them. The result is what
/// `docker load` / `podman load` reads back: every layer, including the base.
pub fn save_image(engine: ContainerEngine, tag: &str, path: &str) -> Result<(), String> {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (status, _, stderr) = run_process(
                exe,
                &[
                    "save".to_string(),
                    "-o".to_string(),
                    path.to_string(),
                    tag.to_string(),
                ],
            );
            if status.success() {
                Ok(())
            } else {
                Err(stderr.trim().to_string())
            }
        }
        // Apptainer images are already files; there is nothing to export.
        ArgStyle::Apptainer => Err("apptainer has no image store to save from".to_string()),
    }
}

pub fn remove_image(engine: ContainerEngine, tag: &str) -> bool {
    match argstyle(engine) {
        ArgStyle::Oci => {
            let exe = engine_executable(engine);
            let (status, _, _) = run_process(exe, &["rmi".to_string(), tag.to_string()]);
            status.success()
        }
        // For Apptainer the "image" is a .sif file on disk. Removing the
        // image is just deleting the file. Treat tag as a path here -- the
        // env-cleanup caller in environment.rs already has the right path
        // because it comes from EnvironmentConfig::layered_sif.
        ArgStyle::Apptainer => {
            std::fs::remove_file(Path::new(tag)).is_ok()
        }
    }
}

/// A successful status for engine no-ops.
fn no_op_exit_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

// ======================================================================
// CLI argument construction
// ======================================================================

pub fn build_run_args(
    engine: ContainerEngine,
    extra_engine_flags: &[String],
    cfg: &RunConfig,
) -> Vec<String> {
    match argstyle(engine) {
        ArgStyle::Oci => build_oci_run_args(engine, extra_engine_flags, cfg),
        ArgStyle::Apptainer => build_apptainer_args(extra_engine_flags, cfg),
    }
}

/// Today's `docker run`-style argv builder. Behavior is unchanged from before
/// the Apptainer addition.
fn build_oci_run_args(
    engine: ContainerEngine,
    extra_engine_flags: &[String],
    cfg: &RunConfig,
) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    args.extend(extra_engine_flags.iter().cloned());

    if cfg.remove_after {
        args.push("--rm".to_string());
    }
    if cfg.read_only {
        args.push("--read-only".to_string());
        // Docker does not auto-mount a tmpfs at /tmp when --read-only is used
        // (podman does). Pool daemons need a writable /tmp for temp files.
        if engine == ContainerEngine::Docker {
            args.push("--tmpfs".to_string());
            args.push("/tmp".to_string());
        }
    }
    // Always attach stdin so piped input works; only allocate a TTY for
    // interactive (shell) sessions.
    args.push("-i".to_string());
    if cfg.interactive {
        args.push("-t".to_string());
    }
    if let Some(ref n) = cfg.name {
        args.push("--name".to_string());
        args.push(n.clone());
    }
    if let Some(ref s) = cfg.shm_size {
        args.push("--shm-size".to_string());
        args.push(s.clone());
    }
    if let Some(ref w) = cfg.work_dir {
        args.push("-w".to_string());
        args.push(w.clone());
    }
    for (host, container) in &cfg.bind_mounts {
        args.push("-v".to_string());
        args.push(bind_spec(host, container, &cfg.selinux_suffix, cfg.label_disable));
    }
    // After the bind mounts, so a volume whose target nests inside one is applied
    // over it. Both engines order mounts by destination depth regardless, but
    // emitting them in this order keeps the command line readable. No SELinux
    // suffix: relabelling applies to host paths, not engine-managed storage.
    for (volume, container) in &cfg.volumes {
        args.push("-v".to_string());
        args.push(format!("{volume}:{container}"));
    }
    if let Some(ref net) = cfg.network {
        args.push("--network".to_string());
        args.push(net.clone());
    }
    // Under host networking the container shares the host netns and the nexus
    // binds host ports directly, so `-p` is invalid (docker rejects it) and
    // unnecessary. Skip publishing in that mode.
    if cfg.network.as_deref() != Some("host") {
        for (host_port, container_port) in &cfg.ports {
            args.push("-p".to_string());
            match &cfg.publish_host {
                Some(ip) => args.push(format!("{ip}:{host_port}:{container_port}")),
                None => args.push(format!("{host_port}:{container_port}")),
            }
        }
    }
    for (key, val) in &cfg.env {
        args.push("-e".to_string());
        args.push(format!("{key}={val}"));
    }
    for (key, _) in &cfg.secret_env {
        args.push("-e".to_string());
        args.push(key.clone());
    }
    if cfg.label_disable {
        args.push("--security-opt".to_string());
        args.push("label=disable".to_string());
    }
    if cfg.hardened {
        args.push("--cap-drop=ALL".to_string());
        args.push("--security-opt".to_string());
        args.push("no-new-privileges".to_string());
    }
    args.extend(cfg.extra_flags.iter().cloned());
    push_platform(&mut args, cfg.platform);
    args.push(cfg.image.clone());
    if let Some(ref cmd) = cfg.command {
        args.extend(cmd.iter().cloned());
    }
    args
}

/// A `-v` bind: `host:target[:opts]`, where a target written `path:ro` and the
/// SELinux relabel `:z` share one comma-separated option field. A container
/// with labelling disabled relabels nothing.
fn bind_spec(host: &str, container: &str, selinux_suffix: &str, label_disable: bool) -> String {
    let (target, ro) = match container.strip_suffix(":ro") {
        Some(t) => (t, true),
        None => (container, false),
    };
    let relabel = selinux_suffix.trim_start_matches(':');
    let mut opts: Vec<&str> = Vec::new();
    if ro {
        opts.push("ro");
    }
    if !relabel.is_empty() && !label_disable {
        opts.push(relabel);
    }
    if opts.is_empty() {
        format!("{host}:{target}")
    } else {
        format!("{host}:{target}:{}", opts.join(","))
    }
}

/// `--platform` goes after the user's flags: the engine takes the last
/// occurrence, so the image's own architecture always wins over a stray one in
/// a flag file or `-x`.
fn push_platform(args: &mut Vec<String>, platform: Option<crate::arch::Arch>) {
    if let Some(a) = platform {
        args.push("--platform".to_string());
        args.push(a.oci_platform().to_string());
    }
}

/// Apptainer/Singularity argv builder. Translates RunConfig to `apptainer
/// exec`, `apptainer shell`, or `apptainer run` semantics. Image is the path
/// to a local .sif file.
///
/// Subcommand selection:
/// * `interactive=true` + cmd=Some(["/bin/bash"]) => `shell` (matches the
///   interactive `shell` UX in run_with_config).
/// * cmd=Some(other) => `exec` (run the command directly, bypassing any
///   runscript).
/// * cmd=None => `run` (invoke the image's runscript).
///
/// Flag translation (see plan):
/// * `-v` => `--bind` (selinux suffixes preserved)
/// * `-e` => `--env`
/// * `-w` => `--pwd`
/// * `--rm`, `-i`/`-t`, `--name`, `--read-only`, `--tmpfs`, `--user`,
///   `--userns=keep-id` => dropped (Apptainer semantics make them
///   redundant or nonsensical).
/// * `--shm-size` => dropped silently. Apptainer shares host /dev/shm.
/// * `-p H:C` with H==C => dropped silently. Apptainer uses host network.
/// * `-p H:C` with H!=C => dropped here with a warning; callers that care
///   about exact port mapping (e.g. serve) should validate before reaching
///   this function.
fn build_apptainer_args(extra_engine_flags: &[String], cfg: &RunConfig) -> Vec<String> {
    let is_shell = cfg.interactive
        && cfg
            .command
            .as_ref()
            .map(|c| c.as_slice() == ["/bin/bash"])
            .unwrap_or(false);

    let subcommand = if is_shell {
        "shell"
    } else if cfg.command.is_some() {
        "exec"
    } else {
        "run"
    };
    let mut args = vec![subcommand.to_string()];
    args.extend(extra_engine_flags.iter().cloned());

    if let Some(ref w) = cfg.work_dir {
        args.push("--pwd".to_string());
        args.push(w.clone());
    }
    for (host, container) in &cfg.bind_mounts {
        // No SELinux suffix: `--bind` accepts only `ro`/`rw` as options and
        // aborts on anything else, and an unprivileged Apptainer process keeps
        // the user's own context, so nothing needs relabelling.
        args.push("--bind".to_string());
        args.push(format!("{host}:{container}"));
    }
    for (key, val) in &cfg.env {
        args.push("--env".to_string());
        args.push(format!("{key}={val}"));
    }

    // Port mapping: Apptainer shares the host network namespace. H==C is a
    // no-op (the inside-container port is the host port). H!=C cannot be
    // expressed; we surface the impossibility once per invocation and drop
    // the flag rather than silently rewrite.
    for (host_port, container_port) in &cfg.ports {
        if host_port != container_port {
            warn_dropped(&format!(
                "-p {host_port}:{container_port}: apptainer uses host networking; \
                 H!=C port mapping is not supported. Bind the container service \
                 directly to host port {host_port} or change --port to a matching pair."
            ));
        }
        // No flag emitted either way.
    }

    if cfg.shm_size.is_some() {
        // Drop silently per plan: Apptainer shares host /dev/shm so
        // --shm-size has no analog and the user's intent (large SHM) is
        // already satisfied.
    }

    args.extend(cfg.extra_flags.iter().cloned());
    args.push(cfg.image.clone());
    if !is_shell {
        if let Some(ref cmd) = cfg.command {
            args.extend(cmd.iter().cloned());
        }
    }
    args
}

/// Print a warning to stderr once per (process, message). Used by the
/// Apptainer translation layer to surface dropped flags without spamming
/// repeated invocations.
fn warn_dropped(message: &str) {
    use std::sync::Mutex;
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let mut guard = seen.lock().unwrap();
    if guard.insert(message.to_string()) {
        eprintln!("[mim] note: {message}");
    }
}

/// How a docker daemon maps the container's users to the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerUsers {
    /// Container uids are host uids.
    Rootful,
    /// The daemon runs as the caller: container root is the caller.
    Rootless,
    /// A rootful daemon with userns-remap: container uids are shifted.
    UsernsRemap,
}

/// RUN-1: read `docker info --format '{{json .SecurityOptions}}'`, a JSON list
/// of `name=<option>[,key=value...]` entries.
pub fn parse_docker_security_options(json: &str) -> Result<DockerUsers, String> {
    let opts: Vec<String> = serde_json::from_str(json.trim())
        .map_err(|e| format!("unreadable security options {:?}: {e}", json.trim()))?;
    let names: Vec<&str> =
        opts.iter().filter_map(|o| o.split(',').next()?.strip_prefix("name=")).collect();
    Ok(if names.contains(&"rootless") {
        DockerUsers::Rootless
    } else if names.contains(&"userns") {
        DockerUsers::UsernsRemap
    } else {
        DockerUsers::Rootful
    })
}

fn probe_docker_users() -> Result<DockerUsers, String> {
    let out = Command::new(engine_executable(ContainerEngine::Docker))
        .args(["info", "--format", "{{json .SecurityOptions}}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    parse_docker_security_options(&String::from_utf8_lossy(&out.stdout))
}

fn docker_users() -> MimResult<DockerUsers> {
    static USERS: OnceLock<Result<DockerUsers, String>> = OnceLock::new();
    USERS.get_or_init(probe_docker_users).clone().map_err(|why| {
        ManagerError::EnvError(format!(
            "cannot tell whether `docker` is rootless: `docker info` gave no security \
             options ({why}). If `docker` is podman's compatibility command, use the \
             podman engine instead."
        ))
    })
}

/// RUN-1: the flags that make the container's user the caller, so files it
/// writes belong to them. `users` is the docker daemon's mapping.
pub fn identity_flags(engine: ContainerEngine, users: DockerUsers, uid: u32, gid: u32) -> Vec<String> {
    let as_caller = || vec!["--user".to_string(), format!("{uid}:{gid}")];
    match engine {
        ContainerEngine::Podman if uid == 0 => Vec::new(),
        ContainerEngine::Podman => vec!["--userns=keep-id".to_string()],
        ContainerEngine::Docker => match users {
            DockerUsers::Rootless => Vec::new(),
            DockerUsers::Rootful if uid == 0 => Vec::new(),
            DockerUsers::Rootful => as_caller(),
            DockerUsers::UsernsRemap => {
                let mut flags = vec!["--userns=host".to_string()];
                if uid != 0 {
                    flags.extend(as_caller());
                }
                flags
            }
        },
        // Apptainer runs as the calling user and ignores --user/--userns.
        ContainerEngine::Apptainer => Vec::new(),
    }
}

fn engine_specific_run_flags_io(engine: ContainerEngine) -> MimResult<Vec<String>> {
    let users = match engine {
        ContainerEngine::Docker => docker_users()?,
        _ => DockerUsers::Rootful,
    };
    Ok(identity_flags(engine, users, nix::unistd::getuid().as_raw(), nix::unistd::getgid().as_raw()))
}

/// The argv that runs `image` to hand every file under host directory `dir`
/// back to the caller (uid `uid`, gid `gid`), for a daemon mapping `users`.
pub fn reclaim_argv(users: DockerUsers, image: &str, dir: &str, selinux_suffix: &str, uid: u32, gid: u32) -> Vec<String> {
    let (flags, owner) = match users {
        DockerUsers::Rootless => (Vec::new(), "0:0".to_string()),
        DockerUsers::Rootful => (vec!["--user".to_string(), "0:0".to_string()], format!("{uid}:{gid}")),
        DockerUsers::UsernsRemap => (
            vec!["--userns=host".to_string(), "--user".to_string(), "0:0".to_string()],
            format!("{uid}:{gid}"),
        ),
    };
    let cfg = RunConfig {
        bind_mounts: vec![(dir.to_string(), "/reclaim".to_string())],
        selinux_suffix: selinux_suffix.to_string(),
        command: Some(vec!["chown".to_string(), "-R".to_string(), owner, "/reclaim".to_string()]),
        ..RunConfig::new(image)
    };
    build_run_args(ContainerEngine::Docker, &flags, &cfg)
}

/// RUN-1: files an earlier docker run wrote as another host uid (a subuid,
/// under rootless docker) are handed back to the caller, so they can be
/// deleted.
pub fn reclaim_ownership(image: &str, dir: &str, selinux_suffix: &str) -> MimResult<()> {
    let users = docker_users()?;
    let args = reclaim_argv(
        users,
        image,
        dir,
        selinux_suffix,
        nix::unistd::getuid().as_raw(),
        nix::unistd::getgid().as_raw(),
    );
    let (status, _, stderr) = run_process_quiet(engine_executable(ContainerEngine::Docker), &args);
    if status.success() {
        Ok(())
    } else {
        Err(ManagerError::EnvError(format!(
            "could not hand the files under {dir} back to you: {}",
            stderr.trim()
        )))
    }
}

/// Whether anything under `path` belongs to a uid other than the caller's.
fn holds_foreign_files(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = nix::unistd::getuid();
    if me.is_root() {
        return false;
    }
    let me = me.as_raw();
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.uid() != me {
            return true;
        }
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
    false
}

/// RUN-1: hand `dir` back to the caller if an earlier docker run left files
/// in it owned by another uid.
pub fn reclaim_if_foreign(image: &str, dir: &Path) -> MimResult<()> {
    if !holds_foreign_files(dir) {
        return Ok(());
    }
    let suffix = crate::selinux::volume_suffix(crate::selinux::detect_selinux());
    reclaim_ownership(image, &dir.to_string_lossy(), suffix)
}

/// The engine argv that runs `cfg`, identity flags included. Every container
/// mim runs goes through here.
pub fn run_argv(engine: ContainerEngine, cfg: &RunConfig) -> MimResult<Vec<String>> {
    Ok(build_run_args(engine, &engine_specific_run_flags_io(engine)?, cfg))
}

/// Pure version for testing.
#[cfg(test)]
pub fn engine_specific_run_flags(engine: ContainerEngine) -> Vec<String> {
    match engine {
        ContainerEngine::Podman => vec!["--userns=keep-id".to_string()],
        ContainerEngine::Docker => Vec::new(),
        ContainerEngine::Apptainer => Vec::new(),
    }
}

pub fn build_build_args(cfg: &BuildConfig) -> Vec<String> {
    let mut args = vec![
        "build".to_string(),
        "-f".to_string(),
        cfg.dockerfile.clone(),
        "-t".to_string(),
        cfg.tag.clone(),
    ];
    for (key, val) in &cfg.build_args {
        args.push("--build-arg".to_string());
        args.push(format!("{key}={val}"));
    }
    args.extend(cfg.extra_flags.iter().cloned());
    push_platform(&mut args, cfg.platform);
    args.push(cfg.context.clone());
    args
}

// ======================================================================
// Apptainer build operations
// ======================================================================







// ======================================================================
// Process execution
// ======================================================================

/// Run a process with both stdout and stderr redirected to our stderr.
/// Returns only the exit status. Use for IO () commands where mim's
/// stdout must stay clean but the user should see all container output.
fn run_process_to_stderr(exe: &str, args: &[String]) -> ExitStatus {
    let mut child = match Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return report_spawn_failure(exe, e),
    };
    // Pump child stdout -> our stderr
    if let Some(mut child_stdout) = child.stdout.take() {
        let stderr = io::stderr();
        let _ = io::copy(&mut child_stdout, &mut stderr.lock());
    }
    child.wait().unwrap_or_else(|e| report_spawn_failure(exe, e))
}

/// Print why an engine command could not be run and return the failed status
/// of `spawn_failure`, for helpers that hand back only a status.
fn report_spawn_failure(exe: &str, e: io::Error) -> ExitStatus {
    let (status, _, msg) = spawn_failure(exe, e);
    eprintln!("{msg}");
    status
}

/// Run a process with stderr streamed live to the terminal.
/// Returns (exit_status, captured_stdout, "").
fn run_process(exe: &str, args: &[String]) -> (ExitStatus, String, String) {
    run_process_env(exe, args, &[])
}

fn run_process_env(exe: &str, args: &[String], env: &[(String, String)]) -> (ExitStatus, String, String) {
    let output = Command::new(exe)
        .args(args)
        .envs(env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output();
    match output {
        Ok(output) => (
            output.status,
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::new(),
        ),
        Err(e) => spawn_failure(exe, e),
    }
}

/// The outcome reported for an engine command that could not be started: a
/// failed status (127, the shell's "cannot run") carrying the reason, so the
/// caller's ordinary failure handling reports it.
fn spawn_failure(exe: &str, e: io::Error) -> (ExitStatus, String, String) {
    use std::os::unix::process::ExitStatusExt;
    (ExitStatus::from_raw(127 << 8), String::new(), format!("failed to execute {exe}: {e}"))
}

/// Run a process with all output captured (no streaming).
/// Used when stderr must be parsed (e.g., for error classification).
fn run_process_quiet(exe: &str, args: &[String]) -> (ExitStatus, String, String) {
    run_process_quiet_env(exe, args, &[])
}

fn run_process_quiet_env(exe: &str, args: &[String], env: &[(String, String)]) -> (ExitStatus, String, String) {
    let output = Command::new(exe).args(args).envs(env.iter().cloned()).stdin(Stdio::null()).output();
    match output {
        Ok(output) => (
            output.status,
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ),
        Err(e) => spawn_failure(exe, e),
    }
}

// ======================================================================
// Helpers
// ======================================================================

pub fn exit_code_to_int(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {

    /// RUN-1: the daemon's mapping, read from real `docker info` output.
    #[test]
    fn docker_security_options_tell_rootless_from_userns_remap() {
        let rootless = r#"["name=seccomp,profile=builtin","name=rootless","name=cgroupns"]"#;
        let remap = r#"["name=apparmor","name=seccomp,profile=builtin","name=userns","name=cgroupns"]"#;
        let rootful = "[\"name=apparmor\",\"name=seccomp,profile=builtin\",\"name=cgroupns\"]\n";
        assert_eq!(parse_docker_security_options(rootless), Ok(DockerUsers::Rootless));
        assert_eq!(parse_docker_security_options(remap), Ok(DockerUsers::UsernsRemap));
        assert_eq!(parse_docker_security_options(rootful), Ok(DockerUsers::Rootful));
        assert!(parse_docker_security_options("").is_err());
        assert!(parse_docker_security_options("<no value>").is_err());
    }

    /// RUN-1: files a container writes belong to the caller. Rootless docker
    /// already runs as the caller, userns-remap is lifted, and podman is
    /// unchanged.
    #[test]
    fn the_container_user_is_the_caller_for_every_daemon_mapping() {
        use DockerUsers::*;
        let d = |users, uid| identity_flags(ContainerEngine::Docker, users, uid, 100);
        assert_eq!(d(Rootless, 1000), Vec::<String>::new());
        assert_eq!(d(Rootless, 0), Vec::<String>::new());
        assert_eq!(d(Rootful, 1000), vec!["--user", "1000:100"]);
        assert_eq!(d(Rootful, 0), Vec::<String>::new());
        assert_eq!(d(UsernsRemap, 1000), vec!["--userns=host", "--user", "1000:100"]);
        assert_eq!(d(UsernsRemap, 0), vec!["--userns=host"]);
        for users in [Rootless, Rootful, UsernsRemap] {
            assert_eq!(identity_flags(ContainerEngine::Podman, users, 1000, 100), vec!["--userns=keep-id"]);
            assert!(identity_flags(ContainerEngine::Podman, users, 0, 0).is_empty());
            assert!(identity_flags(ContainerEngine::Apptainer, users, 1000, 100).is_empty());
        }
    }

    /// RUN-1: every container mim runs takes its identity flags from
    /// `run_argv`; nothing builds a run argv around it.
    #[test]
    fn every_container_run_goes_through_run_argv() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut files = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let body = match text.find("\nmod tests {") {
                Some(i) => &text[..i],
                None => &text[..],
            };
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            for (n, line) in body.lines().enumerate() {
                let builds = line.contains("engine_specific_run_flags_io(") || line.contains("build_run_args(");
                if builds && name != "container.rs" {
                    offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:#?}");
    }

    /// RUN-1: the repair runs as a user that can chown and hands the files to
    /// whoever the caller is on the host.
    #[test]
    fn reclaiming_files_chowns_them_to_the_host_caller() {
        let argv = |users| reclaim_argv(users, "img", "/d", "", 1000, 100);
        let rootless = argv(DockerUsers::Rootless);
        assert!(!rootless.iter().any(|a| a == "--user"), "{rootless:?}");
        assert!(rootless.ends_with(&["img", "chown", "-R", "0:0", "/reclaim"].map(String::from)), "{rootless:?}");
        let remap = argv(DockerUsers::UsernsRemap);
        assert!(remap.iter().any(|a| a == "--userns=host"), "{remap:?}");
        assert!(remap.windows(2).any(|w| w[0] == "--user" && w[1] == "0:0"), "{remap:?}");
        assert!(remap.ends_with(&["chown", "-R", "1000:100", "/reclaim"].map(String::from)), "{remap:?}");
        assert!(argv(DockerUsers::Rootful).windows(2).any(|w| w[0] == "-v" && w[1] == "/d:/reclaim"));
    }

    /// A read-only bind and the SELinux relabel share one option field, and
    /// a container with labelling disabled relabels nothing.
    #[test]
    fn a_bind_joins_read_only_and_relabel_options() {
        assert_eq!(bind_spec("/h", "/c", "", false), "/h:/c");
        assert_eq!(bind_spec("/h", "/c", ":z", false), "/h:/c:z");
        assert_eq!(bind_spec("/h", "/c:ro", "", false), "/h:/c:ro");
        assert_eq!(bind_spec("/h", "/c:ro", ":z", false), "/h:/c:ro,z");
        assert_eq!(bind_spec("/h", "/c", ":z", true), "/h:/c");
        let mut cfg = RunConfig::new("img");
        cfg.bind_mounts = vec![("/home/u".to_string(), "/work".to_string())];
        cfg.selinux_suffix = ":z".to_string();
        cfg.label_disable = true;
        let args = build_run_args(ContainerEngine::Podman, &[], &cfg);
        assert!(args.windows(2).any(|w| w[0] == "-v" && w[1] == "/home/u:/work"), "{args:?}");
        assert!(args.windows(2).any(|w| w[0] == "--security-opt" && w[1] == "label=disable"), "{args:?}");
    }
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn failed(stderr: &str) -> io::Result<std::process::Output> {
        Ok(std::process::Output {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// An engine binary that cannot be started is a failed command, not an
    /// exit of the whole process.
    #[test]
    fn a_missing_engine_binary_is_a_failure_not_an_exit() {
        let (status, _, err) = run_process_quiet("/nonexistent/podman", &["ps".to_string()]);
        assert!(!status.success());
        assert!(err.contains("/nonexistent/podman"), "{err}");
        let (status, _, _) = run_process("/nonexistent/podman", &["ps".to_string()]);
        assert!(!status.success());
    }

    /// SERVE-4: a secret reaches the container by name; its value is in no
    /// engine argument, only in the environment of the engine process.
    #[test]
    fn a_secret_variable_never_appears_on_the_engine_command_line() {
        let mut cfg = RunConfig::new("img");
        cfg.env = vec![("MORLOC_HOME".to_string(), "/opt/morloc".to_string())];
        cfg.secret_env = vec![("MORLOC_MCP_TOKEN".to_string(), "s3cret-value".to_string())];
        for engine in [ContainerEngine::Docker, ContainerEngine::Podman, ContainerEngine::Apptainer] {
            let args = build_run_args(engine, &engine_specific_run_flags(engine), &cfg);
            assert!(!args.iter().any(|a| a.contains("s3cret-value")), "{engine:?}: {args:?}");
            let env = engine_process_env(engine, &cfg);
            assert!(env.iter().any(|(_, v)| v == "s3cret-value"), "{engine:?}: {env:?}");
        }
        let docker = build_run_args(ContainerEngine::Docker, &[], &cfg);
        assert!(docker.windows(2).any(|w| w[0] == "-e" && w[1] == "MORLOC_MCP_TOKEN"), "{docker:?}");
        assert_eq!(
            engine_process_env(ContainerEngine::Apptainer, &cfg),
            vec![
                ("APPTAINERENV_MORLOC_MCP_TOKEN".to_string(), "s3cret-value".to_string()),
                ("SINGULARITYENV_MORLOC_MCP_TOKEN".to_string(), "s3cret-value".to_string()),
            ]
        );
    }

    /// Only an engine's own "no such object" answer means absent; an engine
    /// that cannot answer is unknown, never absent.
    #[test]
    fn an_engine_that_cannot_answer_is_not_taken_for_absence() {
        assert_eq!(classify_inspect(failed("Error: No such container: x")), Probe::Absent);
        assert_eq!(classify_inspect(failed("Error: no such volume x")), Probe::Absent);
        assert_eq!(classify_inspect(failed("Error: x: image not known")), Probe::Absent);
        assert_eq!(
            classify_inspect(Err(io::Error::from(io::ErrorKind::NotFound))),
            Probe::Absent
        );
        let down = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.";
        assert!(matches!(classify_inspect(failed(down)), Probe::Unknown(_)));
        let perm = "permission denied while trying to connect to the Docker daemon socket";
        assert!(matches!(classify_inspect(failed(perm)), Probe::Unknown(_)));
        assert!(matches!(
            classify_inspect(Err(io::Error::from(io::ErrorKind::PermissionDenied))),
            Probe::Unknown(_)
        ));
    }
}
