use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::container::{
    container_run, container_run_quiet, container_stop, container_remove, engine_executable,
    exit_code_to_int, RunConfig,
};
use crate::error::{ManagerError, Result};
use crate::types::*;

use sha2::Digest;

/// Serve an environment by bind-mounting its data directory into the container.
pub fn serve_environment(
    engine: ContainerEngine,
    verbose: bool,
    image: &str,
    data_dir: &str,
    container_name: &str,
    ports: &[(u16, u16)],
    publish_host: Option<&str>,
    network: Option<&str>,
    extra_flags: &[String],
    shm_size: &Option<String>,
    user_env: &[(String, String)],
    command: &[String],
    mount_home: Option<&str>,
) -> Result<()> {
    if matches!(engine, ContainerEngine::Apptainer) {
        // Apptainer already runs in the host netns; `network`/`publish_host`
        // don't apply -- the nexus `--http-host` in `command` is the bind.
        let _ = network;
        // Apptainer shares the host network namespace (no `-p` mapping), so
        // host exposure is decided by the nexus `--http-host` in `command`,
        // not by `publish_host`.
        return serve_apptainer_instance(
            verbose,
            image,
            data_dir,
            container_name,
            ports,
            extra_flags,
            user_env,
            command,
        );
    }

    // Clean up any existing dead container with this name (silently)
    let _ = crate::container::container_remove_quiet(engine, container_name);

    if network == Some("host") {
        eprintln!(
            "Starting serve container {container_name} on the host network..."
        );
    } else {
        let port_str: Vec<String> = ports
            .iter()
            .map(|(h, c)| format!("{h}:{c}"))
            .collect();
        eprintln!(
            "Starting serve container {container_name} on ports {}...",
            port_str.join(", ")
        );
    }

    let suffix = crate::selinux::volume_suffix(crate::selinux::detect_selinux());
    let cfg = serve_run_config(
        image, data_dir, container_name, ports, publish_host, network, extra_flags, shm_size,
        user_env, command, mount_home, suffix,
    )?;

    if verbose {
        let exe = engine_executable(engine);
        let extra = crate::container::engine_specific_run_flags_io(engine);
        let args = crate::container::build_run_args(engine, &extra, &cfg);
        let quoted: Vec<String> = args.iter().map(|a| {
            if a.contains(' ') { format!("'{a}'") } else { a.clone() }
        }).collect();
        eprintln!("[mim] {exe} {}", quoted.join(" "));
    }

    let (status, _stdout, run_err) = container_run(engine, &cfg);
    if !status.success() {
        // `container_run` may have left a partially-created container behind
        // (e.g., port conflict after container creation). Clean it up so the
        // next `start` doesn't fail on a name collision.
        let _ = crate::container::container_remove_quiet(engine, container_name);

        // Detect port conflict and provide a friendlier error message
        let lower = run_err.to_lowercase();
        if lower.contains("address already in use") || lower.contains("port is already allocated")
            || lower.contains("pasta failed")
        {
            // Try to extract the port number from the error
            let port_hint = ports.first()
                .map(|(h, _)| format!(" Port {h} is already in use."))
                .unwrap_or_default();
            return Err(ManagerError::EnvError(format!(
                "{port_hint}\n  \
                 Another container or process is using this port.\n  \
                 Use '-p <other-port>:8080' to choose a different host port, or\n  \
                 check running containers with 'mim status'."
            )));
        }

        return Err(ManagerError::EngineError {
            engine,
            code: exit_code_to_int(status),
            stderr: run_err,
        });
    }

    // Verify container reached running state
    thread::sleep(Duration::from_secs(1));
    let exe = engine_executable(engine);
    let insp_output = Command::new(exe)
        .args(["inspect", "--format", "{{.State.Status}}", container_name])
        .output();
    match insp_output {
        Ok(o) if o.status.success() => {
            let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if state == "running" {
                eprintln!("Container {container_name} started");
                eprintln!("  Logs:   mim logs");
                eprintln!("  Stop:   mim stop");
                eprintln!("  Status: mim status");
                Ok(())
            } else {
                let log_output = Command::new(exe).args(["logs", container_name]).output();
                let logs = log_output
                    .map(|o| {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        format!("{stdout}{stderr}")
                    })
                    .unwrap_or_default();
                let _ = container_remove(engine, container_name);
                Err(ManagerError::EngineError {
                    engine,
                    code: 1,
                    stderr: format!("Container failed to start (state: {state}):\n{logs}"),
                })
            }
        }
        _ => Err(ManagerError::EngineError {
            engine,
            code: 1,
            stderr: "Failed to inspect container state".to_string(),
        }),
    }
}

pub fn stop_serve_container(engine: ContainerEngine, verbose: bool, name: &str) -> Result<()> {
    if matches!(engine, ContainerEngine::Apptainer) {
        // Existence check is more useful as an error than as a precondition
        // here: the instance might already be gone from a SIGKILL etc.
        let running = apptainer_list_serve_instances();
        if !running.iter().any(|c| c.name == name) {
            return Err(ManagerError::EnvError(format!(
                "No serve instance running for '{name}'"
            )));
        }
        if verbose {
            let exe = engine_executable(engine);
            eprintln!("[mim] {exe} instance stop {name}");
        }
        return apptainer_instance_stop(name);
    }
    if !crate::container::container_exists(engine, name) {
        return Err(ManagerError::EnvError(format!(
            "No serve container running for '{name}'"
        )));
    }
    if verbose {
        let exe = engine_executable(engine);
        eprintln!("[mim] {exe} stop {name}");
    }
    let (status, err) = container_stop(engine, name);
    let _ = crate::container::container_remove_quiet(engine, name);
    if !status.success() {
        return Err(ManagerError::EngineError {
            engine,
            code: exit_code_to_int(status),
            stderr: err,
        });
    }
    Ok(())
}

/// Build the serve container name for an environment.
/// Format: morloc-serve-<username>-<envname>
/// The kernel-exported hostname (trimmed), falling back to `localhost`. Read
/// from `/proc/sys/kernel/hostname` -- cheaper than spawning `/bin/hostname`
/// and avoids dragging in a new `nix` feature.
pub fn system_hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// The current user's login name, for namespacing serve containers.
fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn serve_container_name(env_name: &str) -> String {
    format!("{}{env_name}", serve_container_prefix())
}

/// The prefix used to filter all serve containers for the current user.
pub fn serve_container_prefix() -> String {
    format!("morloc-serve-{}-", current_user())
}

/// Extract the environment name from a serve container name.
pub fn env_name_from_container(container_name: &str) -> &str {
    let prefix = serve_container_prefix();
    container_name.strip_prefix(&prefix).unwrap_or(container_name)
}

#[derive(serde::Serialize)]
pub struct ServeContainerInfo {
    pub name: String,
    pub env: String,
    pub ports: String,
    pub status: String,
    /// Adapter/mode label from the runtime serve-record ("mcp+api", "mcp",
    /// "api", "mcp+eval", ...); "-" when no record is found. Filled by `status`.
    #[serde(default)]
    pub mode: String,
    /// Served-module summary from the runtime serve-record; "-" when unknown.
    #[serde(default)]
    pub modules: String,
    /// Base URL from the runtime serve-record (MCP at /mcp, API at /call/...);
    /// "-" when unknown. Fixes host-net port-blindness (comes from the record,
    /// not `docker ps`).
    #[serde(default)]
    pub url: String,
}

/// Query running serve containers and return structured info.
pub fn query_serve_containers(engine: ContainerEngine, verbose: bool) -> Result<Vec<ServeContainerInfo>> {
    if matches!(engine, ContainerEngine::Apptainer) {
        if verbose {
            let exe = engine_executable(engine);
            eprintln!("[mim] {exe} instance list --json");
        }
        return Ok(apptainer_list_serve_instances());
    }
    let exe = engine_executable(engine);
    let fmt = "{{.Names}}\t{{.Status}}\t{{.Ports}}";
    let prefix = serve_container_prefix();
    let filter = format!("name={prefix}");
    if verbose {
        eprintln!("[mim] {exe} ps -a --filter {filter} --format '{fmt}'");
    }
    let output = Command::new(exe)
        .args([
            "ps", "-a", "--filter", &filter, "--format", fmt,
        ])
        // Use /tmp as cwd to avoid podman "cannot chdir" failures when the
        // current directory is inaccessible (e.g. another user's home).
        .current_dir("/tmp")
        .output()
        .map_err(|e| ManagerError::EngineError {
            engine,
            code: 1,
            stderr: format!("Failed to list containers: {e}"),
        })?;
    if !output.status.success() {
        return Err(ManagerError::EngineError {
            engine,
            code: exit_code_to_int(output.status),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ps_serve_lines(&text))
}

/// Parse the tab-separated `Names\tStatus\tPorts` output of `docker/podman ps`
/// into serve-container records.
///
/// Ports is optional: host-network containers have no port mapping, so podman
/// emits an empty final field, and trimming the whole blob strips the last
/// line's trailing tab. Requiring only Names+Status keeps such containers (any
/// line's, not just the first) from being silently dropped.
fn parse_ps_serve_lines(text: &str) -> Vec<ServeContainerInfo> {
    let mut result = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let name = parts[0];
            let status = parts[1];
            let ports = parts.get(2).copied().unwrap_or("");
            let env = env_name_from_container(name);
            result.push(ServeContainerInfo {
                name: name.to_string(),
                env: env.to_string(),
                ports: if ports.is_empty() { "-".to_string() } else { ports.to_string() },
                status: status.to_string(),
                mode: "-".to_string(),
                modules: "-".to_string(),
                url: "-".to_string(),
            });
        }
    }
    result
}

/// Find running serve container names for the current user.
pub fn find_running_serve_containers(engine: ContainerEngine) -> Vec<String> {
    if matches!(engine, ContainerEngine::Apptainer) {
        return apptainer_list_serve_instances()
            .into_iter()
            .map(|c| c.name)
            .collect();
    }
    let exe = engine_executable(engine);
    let filter = format!("name={}", serve_container_prefix());
    let output = Command::new(exe)
        .args(["ps", "--filter", &filter, "--format", "{{.Names}}"])
        .current_dir("/tmp")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The container a served environment runs in.
///
/// A served pliable environment needs the same three-way mount the run path
/// uses. The image carries neither the morloc runtime nor the conda toolchain
/// -- `morloc init` builds the runtime into `<env>/runtime` and pixi solves the
/// toolchain into its own volume, both after the image is built -- so serving
/// with only the state mount leaves the container without `morloc-nexus` on
/// PATH and without an interpreter for any pool.
#[allow(clippy::too_many_arguments)]
fn serve_run_config(
    image: &str,
    data_dir: &str,
    container_name: &str,
    ports: &[(u16, u16)],
    publish_host: Option<&str>,
    network: Option<&str>,
    extra_flags: &[String],
    shm_size: &Option<String>,
    user_env: &[(String, String)],
    command: &[String],
    mount_home: Option<&str>,
    selinux_suffix: &str,
) -> Result<RunConfig> {
    let mut cfg = RunConfig::new(image);
    cfg.read_only = true;
    cfg.remove_after = false;
    cfg.name = Some(container_name.to_string());
    cfg.ports = ports.to_vec();
    cfg.publish_host = publish_host.map(str::to_string);
    cfg.network = network.map(str::to_string);
    let mh = CONTAINER_MORLOC_HOME;
    let (binds, volumes) = crate::base_mounts(data_dir);
    cfg.bind_mounts = binds;
    cfg.volumes = volumes;
    // A host-mounted home shadows the env-owned one for served daemons too, so a
    // program reading `~/.config` sees the same home as `mim shell`.
    cfg.bind_mounts.extend(home_mount(mount_home)?);
    // On an SELinux host a bind mount is readable from the container only once
    // relabelled; the run path asks for that on every mount it makes, and these
    // are the same directories.
    cfg.selinux_suffix = selinux_suffix.to_string();
    // Docker/podman run as the host UID without mounting the host $HOME; pool
    // daemons may touch $HOME (matplotlib config, R tempdir), so oci_base_env
    // points it at a writable, mounted target ($MORLOC_STATE/home). Create it on
    // the host bind-mount side so those writes do not hit ENOENT.
    let _ = crate::config::ensure_env_home(data_dir);
    cfg.env = oci_base_env(mh);
    // A served container is IMMUTABLE: its runtime prefix is read-only (see
    // `cfg.read_only = true` above) and its language set is fixed at image-build
    // time. Mark it so the in-env build hook refuses on-demand language
    // provisioning (which would need to write the runtime) with an actionable
    // "rebuild the image with --lang X" message. The pliable `run` path (writable,
    // persistent runtime mount) is NOT marked, so on-demand works there.
    cfg.env.push(("MORLOC_IMMUTABLE".to_string(), "1".to_string()));
    cfg.env.extend(user_env.iter().cloned());
    cfg.command = Some(command.to_vec());
    cfg.shm_size = shm_size.clone();
    cfg.extra_flags = vec!["-d".to_string()];
    cfg.extra_flags.extend(extra_flags.iter().cloned());
    Ok(cfg)
}

// ======================================================================
// Program validation
// ======================================================================

/// Run `--help` for each installed program inside a container image to
/// verify that pool processes start correctly (e.g. all imports resolve).
pub fn validate_programs(
    engine: ContainerEngine,
    image: &str,
    programs: &[ProgramEntry],
    bind_mounts: Vec<(String, String)>,
    volumes: Vec<(String, String)>,
    verbose: bool,
) -> Result<()> {
    if programs.is_empty() {
        return Ok(());
    }
    eprintln!("Validating installed programs...");
    let suffix = crate::selinux::volume_suffix(crate::selinux::detect_selinux());
    let mut any_failed = false;
    for prog in programs {
        let cfg =
            program_help_config(image, &prog.name, bind_mounts.clone(), volumes.clone(), suffix);
        if verbose {
            let exe = engine_executable(engine);
            let extra = crate::container::engine_specific_run_flags_io(engine);
            let args = crate::container::build_run_args(engine, &extra, &cfg);
            eprintln!("[mim] {exe} {}", args.join(" "));
        }
        let (status, _stdout, stderr) = container_run_quiet(engine, &cfg);
        if status.success() {
            let n = prog.commands.len();
            eprintln!("  [ok] {} ({} commands)", prog.name, n);
        } else {
            let snippet: String = stderr.lines().take(5).collect::<Vec<_>>().join("\n    ");
            eprintln!("  [FAIL] {}: {}", prog.name, snippet);
            any_failed = true;
        }
    }
    if any_failed {
        return Err(ManagerError::FreezeError(
            "Some programs failed validation (see errors above)".to_string(),
        ));
    }
    Ok(())
}

/// A one-shot container in the environment, running `command`: the same
/// mounts, env and entrypoint every other process in the environment gets.
/// `selinux_suffix` is the caller's relabel decision for the bind mounts.
pub(crate) fn env_run_config(
    image: &str,
    command: Vec<String>,
    bind_mounts: Vec<(String, String)>,
    volumes: Vec<(String, String)>,
    selinux_suffix: &str,
) -> RunConfig {
    RunConfig {
        bind_mounts,
        volumes,
        command: Some(command),
        env: oci_base_env(CONTAINER_MORLOC_HOME),
        selinux_suffix: selinux_suffix.to_string(),
        ..RunConfig::new(image)
    }
}

/// The container a program is validated in. A launcher is `exec morloc-nexus
/// ...`, found through PATH, and MORLOC_HOME/bin is on PATH only because
/// `oci_base_env` puts it there -- the base image cannot bake it, since in a
/// pliable environment it is a run-time mount.
pub(crate) fn program_help_config(
    image: &str,
    program: &str,
    bind_mounts: Vec<(String, String)>,
    volumes: Vec<(String, String)>,
    selinux_suffix: &str,
) -> RunConfig {
    let exe_path = format!("{CONTAINER_MORLOC_HOME}/bin/{program}");
    env_run_config(
        image,
        vec![exe_path, "--help".to_string()],
        bind_mounts,
        volumes,
        selinux_suffix,
    )
}

// ======================================================================
// Container constants
// ======================================================================

/// In-container MORLOC_HOME: the IMMUTABLE runtime prefix (bin/lib/include),
/// image-baked by `morloc init` and NEVER bind-mounted, so a state mount cannot
/// shadow it (the mount-shadow bug the three-way split fixes).
pub const CONTAINER_MORLOC_HOME: &str = "/opt/morloc";

/// In-container MORLOC_STATE: the MUTABLE state root (exe/, fdb/, installed
/// modules, logs). This is the ONLY thing bind-mounted from the host env dir, so
/// installs/builds persist without covering the runtime.
pub const CONTAINER_MORLOC_STATE: &str = "/opt/morloc-state";

/// In-container HOME for docker/podman: an env-owned, base-independent home so
/// the identity is the same regardless of the base image's /etc/passwd. It is an
/// in-image symlink to the mounted, writable env home under MORLOC_STATE
/// (`/opt/morloc-state/home`), where dotfiles land. Apptainer keeps the host
/// $HOME and does not use this.
pub const CONTAINER_HOME: &str = "/home/morloc";

/// Bind mount for a host-mounted `$HOME` (`EnvironmentConfig::mount_home`), or
/// no mount when the environment owns its home. The target is the state-relative
/// home (`<state>/home`), NOT `CONTAINER_HOME`: the latter is an image-baked
/// symlink to the former, so binding the resolved target keeps the mount
/// independent of how an engine treats a symlinked mount point. It nests inside
/// the state mount, which docker and podman both apply in destination order.
///
/// Errors (rather than mounting) when the recorded directory has gone: docker
/// and podman silently CREATE a missing bind source as a root-owned empty
/// directory, which would hand the user an empty, unwritable home in place of
/// the one they persisted.
pub fn home_mount(mount_home: Option<&str>) -> Result<Vec<(String, String)>> {
    let Some(src) = mount_home else {
        return Ok(Vec::new());
    };
    if !std::path::Path::new(src).is_dir() {
        return Err(ManagerError::EnvError(format!(
            "the environment's host home '{src}' is missing. Restore that directory, \
             or drop the mount with `mim modify --env <env> --mount-home none`."
        )));
    }
    Ok(vec![(
        src.to_string(),
        format!("{CONTAINER_MORLOC_STATE}/home"),
    )])
}

/// Fixed in-container mount point + working directory for the host cwd on the
/// run/shell/capture paths. A constant (not the literal host path) so the work
/// dir is deterministic regardless of the host user or path: no host-named tree
/// appears under `/`, and it cannot collide with CONTAINER_HOME when the host
/// user happens to be named `morloc`. Safe now that pool artifacts are
/// path-independent (they resolve relative to the manifest, not a baked cwd).
/// Matches the image WORKDIR.
pub const CONTAINER_WORK: &str = "/work";

/// Fixed in-image path to the `libnss_wrapper.so` interposer. The Dockerfile
/// symlinks the apt-installed library (whose real path is arch-dependent) here,
/// so the entrypoint can LD_PRELOAD a stable, single-quote-free path. Used to
/// synthesize a passwd/group entry for the host UID under `--userns=keep-id`,
/// which otherwise has no /etc/passwd entry.
pub const CONTAINER_NSS_WRAPPER_LIB: &str = "/usr/local/lib/morloc-nss-wrapper.so";

/// Fixed path of the `libnss-extrausers` identity file (the module reads this
/// exact location; it is not configurable). Dev images point `nsswitch.conf` at
/// the `extrausers` NSS source and the entrypoint writes the host UID's
/// passwd/group entry here. Unlike nss_wrapper (an `LD_PRELOAD` interposer that
/// the loader strips from setuid binaries), extrausers is a real NSS module, so
/// setuid `sudo` resolves the entry -- which passwordless `sudo` in the dev
/// container requires. The file is world-writable so the (non-root, host-UID)
/// entrypoint can append to it.
pub const CONTAINER_EXTRAUSERS_DIR: &str = "/var/lib/extrausers";

/// System PATH tail appended after the morloc + toolchain dirs for every
/// in-container invocation (run, serve, apptainer).
pub const CONTAINER_PATH_TAIL: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Where the morloc compiler + the Rust source are COPYed in the
/// requirement-derived image (see dockerfile.rs). Must be on PATH so the
/// `morloc` compiler resolves at build/run time. (morloc-nexus is NOT here: it
/// is built from source by `morloc init` into `$MORLOC_HOME/bin`.)
pub const CONTAINER_RUNTIME_BIN: &str = "/opt/morloc-runtime";

/// The pixi-solved conda environment's bin inside the image (WORKDIR `/env`,
/// pixi's default environment). Provides the language toolchain (python, R,
/// ...); PATH is the load-bearing part of pixi activation.
pub const CONTAINER_PIXI_ENV_BIN: &str = "/env/.pixi/envs/default/bin";

/// The pixi PROJECT dir inside the image (holds `pixi.toml`/`pixi.lock` and the
/// solved prefix). Distinct from the state root, so the in-env agent is told
/// this location explicitly (MORLOC_PIXI_DIR) rather than deriving it.
pub const CONTAINER_PIXI_DIR: &str = "/env";

/// The pixi binary inside the image (PIXI_HOME=/opt/pixi; not on the run PATH).
pub const CONTAINER_PIXI_BIN: &str = "/opt/pixi/bin/pixi";

/// In-container mount point for the conda package cache, pointed at by
/// `PIXI_CACHE_DIR` and backed by [`PIXI_CACHE_VOLUME`].
///
/// It is deliberately not under the state mount. Package payloads are unpacked
/// here byte for byte, case-colliding names included, and then copied into the
/// prefix; on a host share that folds letter case one file of every such pair is
/// lost during unpacking, and the copy into the prefix then fails on a source
/// that is no longer there.
pub const CONTAINER_PIXI_CACHE: &str = "/opt/morloc-pixi-cache";

/// The engine volume backing [`CONTAINER_PIXI_CACHE`]. Conda packages are
/// content-addressed, so one cache serves every environment on the machine; it
/// is not per-environment and removing an environment does not remove it.
pub const PIXI_CACHE_VOLUME: &str = "morloc-pixi-cache";

/// The engine volume holding an environment's solved conda prefix, mounted over
/// `<CONTAINER_PIXI_DIR>/.pixi`.
///
/// Only the prefix moves off the host. `pixi.toml` and `pixi.lock` stay on the
/// bind mount beside it, because the host solves the environment (with the
/// host's CA bundle) and reads the lock back; the prefix itself is a Linux tree
/// that only in-container processes ever execute.
///
/// The name carries the environment's directory name so `volume ls` is readable,
/// plus a digest of its full path so two environments cannot share a volume --
/// environment names may contain characters a volume name may not, and are
/// unique only within a scope.
pub fn prefix_volume(env_dir: &Path) -> String {
    let path = env_dir.to_string_lossy();
    let readable: String = env_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect();
    format!("morloc-env-{readable}-{:.8x}-pixi", sha2::Sha256::digest(path.as_bytes()))
}

/// Dev environments only: the ghcup bin dir baked into the dev image
/// (`GHCUP_INSTALL_BASE_PREFIX=/opt`), holding `ghcup`/`stack`. Placed on the run
/// PATH so an interactive dev shell can build the compiler, not just run it. On a
/// release image the dir is absent, so the PATH entry is a harmless no-op.
pub const CONTAINER_GHCUP_BIN: &str = "/opt/.ghcup/bin";

/// The shell lines that activate the mounted conda toolchain for a container
/// process: set CONDA_PREFIX, run pixi's shell-hook (for PATH), then source the
/// conda activate.d scripts. shell-hook alone does NOT export $CC/$AR, which the
/// pool builds require, so the activate.d sourcing is mandatory. Shared by the
/// image ENTRYPOINT and the env-materialize step so the two cannot drift.
pub fn conda_activate_lines() -> [String; 3] {
    let prefix = format!("{CONTAINER_PIXI_DIR}/.pixi/envs/default");
    [
        format!("export CONDA_PREFIX={prefix}"),
        format!(
            "eval \"$({CONTAINER_PIXI_BIN} shell-hook --manifest-path {CONTAINER_PIXI_DIR}/pixi.toml --shell bash 2>/dev/null)\" || true"
        ),
        "for f in \"$CONDA_PREFIX/etc/conda/activate.d/\"*.sh; do [ -r \"$f\" ] && . \"$f\"; done".to_string(),
    ]
}

/// The in-container PATH for a requirement-derived image: installed program
/// launchers + shims + the init-built morloc-nexus (`$MORLOC_HOME/bin`), the
/// morloc compiler, the pixi toolchain, then the system tail. Single source of
/// truth for every in-container PATH.
pub fn container_path(mh: &str) -> String {
    // CONTAINER_GHCUP_BIN exists only in a dev image (a harmless no-op otherwise),
    // so an interactive dev shell has `stack`/`ghc` on PATH.
    format!("{mh}/bin:{CONTAINER_RUNTIME_BIN}:{CONTAINER_PIXI_ENV_BIN}:{CONTAINER_GHCUP_BIN}:{CONTAINER_PATH_TAIL}")
}

/// Base env for a docker/podman in-container process. MORLOC_HOME is the baked
/// runtime prefix; MORLOC_STATE is the mounted, writable state root; HOME is the
/// env-owned CONTAINER_HOME (an in-image symlink to the mounted env home under
/// state), so pool daemons/tools have a writable home. `morloc-nexus` finds
/// `libmorloc.so` via its own baked `bin/../lib` RUNPATH (the runtime is no
/// longer shadowed), so no `LD_LIBRARY_PATH` override is needed. Callers extend
/// this with phase-specific vars (CARGO_HOME/MORLOC_BIN_LINK_DIR, user_env).
/// Apptainer mounts the host $HOME and uses its own env, so it does not use this.
pub fn oci_base_env(mh: &str) -> Vec<(String, String)> {
    vec![
        ("MORLOC_HOME".to_string(), mh.to_string()),
        ("MORLOC_STATE".to_string(), CONTAINER_MORLOC_STATE.to_string()),
        ("HOME".to_string(), CONTAINER_HOME.to_string()),
        ("PATH".to_string(), container_path(mh)),
        // Off the state mount: see CONTAINER_PIXI_CACHE.
        ("PIXI_CACHE_DIR".to_string(), CONTAINER_PIXI_CACHE.to_string()),
        // A UTF-8 locale (C.UTF-8 is built into the base image's glibc) so the
        // compiler and programs can emit non-ASCII to stdout; under the default C
        // locale that fails with "commitBuffer: invalid argument".
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
    ]
}

/// The OCI managed-environment markers appended for an in-container `morloc make`:
/// the `MORLOC_ENV` gate the compiler's dependency callback checks; the build hook
/// (`MORLOC_BUILD_HOOK` = the staged mim agent, run as `mim sync ...`) and
/// `MORLOC_BIN` (the staged compiler) it needs to provision, both under
/// `CONTAINER_RUNTIME_BIN`; plus the baked pixi location (`/env`, distinct from the
/// state root). Shared so `mim info` reports exactly the markers a run
/// exports.
pub fn oci_managed_markers() -> Vec<(String, String)> {
    vec![
        ("MORLOC_ENV".to_string(), "container".to_string()),
        ("MORLOC_BUILD_HOOK".to_string(), format!("{CONTAINER_RUNTIME_BIN}/mim")),
        ("MORLOC_BIN".to_string(), format!("{CONTAINER_RUNTIME_BIN}/morloc")),
        ("MORLOC_PIXI".to_string(), CONTAINER_PIXI_BIN.to_string()),
        ("MORLOC_PIXI_DIR".to_string(), CONTAINER_PIXI_DIR.to_string()),
    ]
}

// ======================================================================
// Apptainer instance backend
// ======================================================================

/// Start a long-running morloc serve instance under Apptainer.
///
/// Apptainer has no NAT; host_port and container_port must match. If they
/// differ, return a clear error rather than silently rewriting either side.
/// All other RunConfig flags translate per the table in build_apptainer_args.
fn serve_apptainer_instance(
    verbose: bool,
    image: &str,
    data_dir: &str,
    instance_name: &str,
    ports: &[(u16, u16)],
    extra_flags: &[String],
    user_env: &[(String, String)],
    command: &[String],
) -> Result<()> {
    // Stop any leftover instance with this name from a previous run.
    let _ = apptainer_instance_stop(instance_name);

    for (h, c) in ports {
        if h != c {
            return Err(ManagerError::EnvError(format!(
                "Apptainer uses host networking; port mapping -p {h}:{c} cannot be \
                 rewritten. Either configure the nexus to bind to port {h} directly, \
                 or use -p {c}:{c} (matching host:container)."
            )));
        }
    }

    let port_str: Vec<String> = ports
        .iter()
        .map(|(h, c)| format!("{h}:{c}"))
        .collect();
    eprintln!(
        "Starting serve instance {instance_name} on ports {}...",
        port_str.join(", ")
    );
    eprintln!(
        "[mim] note: apptainer uses host networking; nexus binds directly \
         to host:{}",
        ports.first().map(|(h, _)| *h).unwrap_or(8080)
    );

    let mh = CONTAINER_MORLOC_HOME;

    let exe = engine_executable(ContainerEngine::Apptainer);
    let mut argv: Vec<String> = vec!["instance".to_string(), "start".to_string()];
    // The same three-way mount the OCI path uses. Apptainer has no volumes, so
    // the conda prefix comes straight from the host dir under `<env>/pixi`, which
    // is where it lives on the Linux-only filesystems Apptainer runs on.
    let (binds, _volumes) = crate::base_mounts(data_dir);
    for (src, dest) in binds {
        argv.push("--bind".to_string());
        argv.push(format!("{src}:{dest}"));
    }
    argv.push("--env".to_string());
    argv.push(format!("PATH={}", container_path(mh)));
    argv.push("--env".to_string());
    argv.push(format!("MORLOC_HOME={mh}"));
    argv.push("--env".to_string());
    argv.push(format!("MORLOC_STATE={CONTAINER_MORLOC_STATE}"));
    for (k, v) in user_env {
        argv.push("--env".to_string());
        argv.push(format!("{k}={v}"));
    }
    for f in extra_flags {
        argv.push(f.clone());
    }
    argv.push(image.to_string());
    argv.push(instance_name.to_string());
    // After the instance name, args are forwarded to the image's startscript,
    // which dispatches on them (a router serve, an `mcp --http-port` serve, or
    // a user's custom %startscript). The caller supplies the full command.
    argv.extend(command.iter().cloned());

    if verbose {
        let quoted: Vec<String> = argv
            .iter()
            .map(|a| if a.contains(' ') { format!("'{a}'") } else { a.clone() })
            .collect();
        eprintln!("[mim] {exe} {}", quoted.join(" "));
    }

    let status = Command::new(exe)
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| ManagerError::EnvError(format!("Failed to spawn apptainer: {e}")))?;
    if !status.success() {
        return Err(ManagerError::EngineError {
            engine: ContainerEngine::Apptainer,
            code: exit_code_to_int(status),
            stderr: "apptainer instance start failed (see output above)".to_string(),
        });
    }

    eprintln!("Instance {instance_name} started");
    eprintln!("  Logs:   mim logs");
    eprintln!("  Stop:   mim stop");
    eprintln!("  Status: mim status");
    Ok(())
}

/// Stop a running Apptainer instance by name. Returns Ok(()) when no such
/// instance exists -- mirrors the pre-emptive cleanup model used by the OCI
/// path.
pub fn apptainer_instance_stop(name: &str) -> Result<()> {
    let exe = engine_executable(ContainerEngine::Apptainer);
    let _ = Command::new(exe)
        .args(["instance", "stop", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// List Apptainer instances whose names begin with the morloc serve prefix.
/// Parses `apptainer instance list --json`.
pub fn apptainer_list_serve_instances() -> Vec<ServeContainerInfo> {
    let exe = engine_executable(ContainerEngine::Apptainer);
    let output = Command::new(exe)
        .args(["instance", "list", "--json"])
        .current_dir("/tmp")
        .output();
    let Ok(o) = output else {
        return Vec::new();
    };
    if !o.status.success() {
        return Vec::new();
    }
    // Apptainer's JSON is `{"instances": [{"instance": "name", ...}, ...]}`.
    let text = String::from_utf8_lossy(&o.stdout);
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let prefix = serve_container_prefix();
    let mut result = Vec::new();
    if let Some(arr) = parsed.get("instances").and_then(|v| v.as_array()) {
        for entry in arr {
            let name = entry
                .get("instance")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !name.starts_with(&prefix) {
                continue;
            }
            let env = env_name_from_container(&name).to_string();
            result.push(ServeContainerInfo {
                name,
                env,
                ports: "-".to_string(),
                status: "Up".to_string(),
                mode: "-".to_string(),
                modules: "-".to_string(),
                url: "-".to_string(),
            });
        }
    }
    result
}

/// Read the Apptainer per-instance log file. Apptainer has no `logs`
/// subcommand; logs live at
/// `~/.apptainer/instances/logs/<host>/<user>/<instance>/<instance>.{out,err}`.
///
/// `follow` is honored on a best-effort basis: if true, we tail -f the .out
/// file (mirroring the OCI path which interleaves stdout/stderr to stdout).
pub fn apptainer_logs(instance_name: &str, follow: bool) -> Result<()> {
    let host = system_hostname();
    let user = current_user();
    let home = dirs::home_dir().ok_or_else(|| {
        ManagerError::EnvError("Cannot determine home directory".to_string())
    })?;
    let log_dir = home
        .join(".apptainer/instances/logs")
        .join(host)
        .join(&user)
        .join(instance_name);
    let out_path = log_dir.join(format!("{instance_name}.out"));
    let err_path = log_dir.join(format!("{instance_name}.err"));

    if !out_path.exists() && !err_path.exists() {
        return Err(ManagerError::EnvError(format!(
            "No log files found for instance '{instance_name}'. \
             Expected under {}",
            log_dir.display()
        )));
    }

    if follow {
        // Use `tail -F` for follow mode; works regardless of file rotation.
        let mut argv = vec!["-F".to_string()];
        if out_path.exists() {
            argv.push(out_path.to_string_lossy().to_string());
        }
        if err_path.exists() {
            argv.push(err_path.to_string_lossy().to_string());
        }
        let status = Command::new("tail")
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| ManagerError::EnvError(format!("Failed to spawn tail: {e}")))?;
        if !status.success() {
            return Err(ManagerError::EngineError {
                engine: ContainerEngine::Apptainer,
                code: exit_code_to_int(status),
                stderr: "tail failed".to_string(),
            });
        }
        return Ok(());
    }

    // Non-follow: dump out then err to stdout (stderr-as-stdout merge mirrors
    // what the OCI path does via stderr-as-stdout redirection).
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if let Ok(buf) = fs::read(&out_path) {
        let _ = lock.write_all(&buf);
    }
    if let Ok(buf) = fs::read(&err_path) {
        let _ = lock.write_all(&buf);
    }
    Ok(())
}

/// Dump the router-captured per-daemon stderr logs to stdout as a snapshot.
/// These live at `$MORLOC_HOME/logs/*.err` in the container, which is the host
/// path `<env_data_dir>/logs/*.err` via the bind mount. The runtime router
/// writes them when it spawns pool daemons, so they carry startup crashes that
/// the engine's own container logs may not surface. Best-effort and
/// engine-independent: silently does nothing when the dir is absent or empty.
pub fn dump_err_files(logs_dir: &Path) -> Result<()> {
    let entries = match fs::read_dir(logs_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    let mut err_files: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "err").unwrap_or(false))
        .collect();
    err_files.sort();
    if err_files.is_empty() {
        return Ok(());
    }
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    for p in &err_files {
        // Header so multiple daemons' logs stay distinguishable.
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = writeln!(lock, "==> {name} <==");
        if let Ok(buf) = fs::read(p) {
            let _ = lock.write_all(&buf);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bind in a served container is a host directory the run path also
    /// mounts, and on an SELinux host each is readable only once relabelled.
    /// The suffix the caller decides on must land on every bind mount and on
    /// no engine volume, which is engine storage and cannot be relabelled.
    #[test]
    fn a_served_container_relabels_every_bind_mount_and_no_volume() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_string_lossy().to_string();
        let cmd = vec!["morloc-nexus".to_string(), "router".to_string()];
        let cfg = serve_run_config(
            "img", &data_dir, "morloc-serve-dev", &[(8080, 8080)], None, None, &[], &None, &[],
            &cmd, None, ":z",
        )
        .unwrap();
        let args = crate::container::build_run_args(
            ContainerEngine::Docker,
            &crate::container::engine_specific_run_flags(ContainerEngine::Docker),
            &cfg,
        );
        let mounts: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "-v")
            .map(|w| &w[1])
            .collect();
        assert_eq!(mounts.len(), cfg.bind_mounts.len() + cfg.volumes.len(), "{args:?}");
        for (host, container) in &cfg.bind_mounts {
            let want = format!("{host}:{container}:z");
            assert!(mounts.contains(&&want), "{want} missing from {mounts:?}");
        }
        for (volume, container) in &cfg.volumes {
            let want = format!("{volume}:{container}");
            assert!(mounts.contains(&&want), "{want} missing from {mounts:?}");
        }
        // The same env every container process gets, plus the immutability mark.
        let path = cfg.env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap();
        assert!(path.split(':').any(|d| d == format!("{CONTAINER_MORLOC_HOME}/bin")), "{path}");
        assert!(cfg.env.contains(&("MORLOC_IMMUTABLE".to_string(), "1".to_string())));
        assert!(cfg.read_only);
        assert_eq!(cfg.command.as_deref(), Some(&cmd[..]));
    }

    /// A launcher is `exec morloc-nexus ...`, resolved through PATH, and the
    /// base image does not bake MORLOC_HOME/bin onto PATH: that directory is a
    /// mount that exists only at run time. The env every other container
    /// process gets supplies it; a validation run without it reports
    /// `morloc-nexus: not found` for a program that works everywhere else.
    #[test]
    fn validation_runs_a_program_with_the_runtime_on_path() {
        let binds = vec![("/host/env/runtime".to_string(), CONTAINER_MORLOC_HOME.to_string())];
        let cfg = program_help_config("img", "dna", binds, Vec::new(), ":z");
        let path = cfg
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .expect("validation sets PATH");
        assert!(
            path.split(':').any(|d| d == format!("{CONTAINER_MORLOC_HOME}/bin")),
            "{path}"
        );
        assert_eq!(
            cfg.command.as_deref(),
            Some(&[format!("{CONTAINER_MORLOC_HOME}/bin/dna"), "--help".to_string()][..])
        );
        assert_eq!(cfg.selinux_suffix, ":z");
        // The image's entrypoint is the activation wrapper every process goes
        // through; bypassing it validates a container nothing else runs in.
        assert!(!cfg.extra_flags.iter().any(|f| f == "--entrypoint"), "{:?}", cfg.extra_flags);
    }

    #[test]
    fn prefix_volume_names_are_engine_legal_and_readable() {
        let name = prefix_volume(Path::new("/home/z/.local/share/morloc/environments/latest"));
        assert!(name.starts_with("morloc-env-latest-"), "{name}");
        assert!(name.ends_with("-pixi"), "{name}");
        // docker/podman accept [a-zA-Z0-9][a-zA-Z0-9_.-]*
        let mut chars = name.chars();
        assert!(chars.next().unwrap().is_ascii_alphanumeric());
        assert!(chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'));
    }

    #[test]
    fn prefix_volume_is_stable_and_per_environment() {
        let a = Path::new("/data/environments/latest");
        let b = Path::new("/data/other-scope/environments/latest");
        assert_eq!(prefix_volume(a), prefix_volume(a));
        // Same directory name under a different root: the digest keeps the two
        // environments from sharing one prefix.
        assert_ne!(prefix_volume(a), prefix_volume(b));
    }

    #[test]
    fn a_volume_name_survives_an_environment_name_a_volume_may_not_hold() {
        // Environment names allow any Unicode alphanumeric; volume names do not.
        let name = prefix_volume(Path::new("/data/environments/pruebas-nino"));
        assert!(name.is_ascii(), "{name}");
    }

    // A previous whole-output `.trim()` stripped the trailing tab from the last
    // `ps` line, so the last host-network container (empty Ports) split into two
    // fields and was dropped by a `parts.len() >= 3` guard. Both containers here
    // have empty Ports; both must survive regardless of position.
    #[test]
    fn empty_ports_container_not_dropped_by_trailing_tab() {
        // Exactly what `podman ps --format '{{.Names}}\t{{.Status}}\t{{.Ports}}'`
        // emits for two host-network serve containers (trailing tab per line).
        let raw = "morloc-serve-z-dev\tUp 36 hours\t\nmorloc-serve-z-latest\tUp 22 minutes\t\n";
        let got = parse_ps_serve_lines(raw);
        let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["morloc-serve-z-dev", "morloc-serve-z-latest"]);
        assert_eq!(got[1].status, "Up 22 minutes");
        assert_eq!(got[1].ports, "-"); // empty Ports rendered as "-"
    }
}
