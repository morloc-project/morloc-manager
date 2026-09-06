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
fn spec_from_exposure(ex: &ExposureConfig) -> Option<crate::ServeSpec> {
    if ex.is_empty() {
        return None;
    }
    Some(crate::ServeSpec::new(
        ex.mcp.clone(),
        ex.api.clone(),
        ex.eval.as_ref().map(|e| e.allow.join(",")),
    ))
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
/// The base is the environment's own image and cannot be anything else: it
/// carries pixi to install the toolchain, the activation wrapper every process
/// goes through, and the compiler a sandboxed eval forks.
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
    verbose: bool,
) -> Result<()> {
    if !Path::new(v_data_dir).is_dir() {
        return Err(ManagerError::FreezeError(format!(
            "Data directory does not exist: {v_data_dir}"
        )));
    }
    let modules = scan_modules(&format!("{v_data_dir}/fdb"));
    let programs = scan_programs(&format!("{v_data_dir}/exe"));
    if programs.is_empty() {
        return Err(ManagerError::FreezeError(
            "No morloc programs are installed. Compile and install with 'morloc make --install' before freezing.".to_string()
        ));
    }

    // Validate the programs in the environment as it actually runs: the runtime
    // and the toolchain are mounts, not image layers, so a validation without
    // them probes an empty directory.
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
        stage_into_context(Path::new(v_data_dir), &context, rel)?;
    }

    let exposure = config::read_exposure(scope, env_name).unwrap_or_default();
    let cmd = match spec_from_exposure(&exposure) {
        // The image does not waive eval's token requirement. Whether this
        // container is reachable is the operator's decision, made outside it with
        // a published port or a network, and the image cannot see that decision;
        // it can only see that eval is expensive and unbounded by what the author
        // declared. An operator who wants open eval sets MORLOC_EVAL_ALLOW_NO_AUTH.
        Some(spec) => crate::build_router_command(
            crate::serve::CONTAINER_MORLOC_STATE,
            DEPLOY_HTTP_PORT,
            "0.0.0.0",
            &spec,
            false,
            false,
        ),
        None => Vec::new(),
    };
    let optional_state: Vec<String> = paths
        .iter()
        .filter(|p| OPTIONAL_STATE.contains(&p.as_str()))
        .cloned()
        .collect();
    let labels = deploy_labels(env_name, &ver, &programs, &modules, &exposure);

    let dockerfile = context.join("Dockerfile");
    let text = crate::dockerfile::generate_deploy_dockerfile(
        &crate::dockerfile::DeployDockerfileInput {
            base_image: env_image,
            cmd: &cmd,
            optional_state: &optional_state,
            http_port: DEPLOY_HTTP_PORT,
            // Podman's OCI output format drops HEALTHCHECK and warns.
            healthcheck: engine == ContainerEngine::Docker,
            labels: &labels,
        },
    );
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
    if let Some(path) = save_to {
        eprintln!("Saving {tag} to {path}...");
        crate::container::save_image(engine, tag, path)
            .map_err(|e| ManagerError::FreezeError(format!("could not save {tag}: {e}")))?;
        eprintln!("Wrote {path} (load it elsewhere with `{} load -i {path}`)", engine.name());
    }
    Ok(())
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
    programs: &[ProgramEntry],
    modules: &[ModuleEntry],
    exposure: &ExposureConfig,
) -> Vec<(String, String)> {
    let join = |xs: Vec<String>| xs.join(",");
    let mut labels = vec![
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

fn scan_programs(exe_dir: &str) -> Vec<ProgramEntry> {
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
    }
    match serde_json::from_slice::<ManifestStub>(&bytes) {
        Ok(stub) => stub.commands.into_iter().map(|c| c.name).collect(),
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
        let ex = ExposureConfig {
            mcp: vec!["dna".to_string()],
            api: vec!["util".to_string()],
            eval: None,
        };
        let spec = spec_from_exposure(&ex).expect("a spec");
        let cmd = crate::build_router_command(
            crate::serve::CONTAINER_MORLOC_STATE,
            DEPLOY_HTTP_PORT,
            "0.0.0.0",
            &spec,
            false,
            false,
        );
        assert!(cmd.windows(2).any(|w| w == ["--mcp", "dna"]), "{cmd:?}");
        assert!(cmd.windows(2).any(|w| w == ["--api", "util"]), "{cmd:?}");
        // A container's loopback is its own, so a published port only reaches a
        // service bound to all interfaces.
        assert!(cmd.windows(2).any(|w| w == ["--http-host", "0.0.0.0"]), "{cmd:?}");
        // And nothing waives authentication on that bind: the nexus refuses to
        // start until the operator supplies a token or overrides the command.
        assert!(!cmd.iter().any(|a| a == "--allow-no-auth"), "{cmd:?}");
        // Nor eval's own requirement. Whether this container is reachable is
        // decided outside it, so the image cannot waive on the operator's behalf.
        assert!(!cmd.iter().any(|a| a == "--eval-allow-no-auth"), "{cmd:?}");
    }

    #[test]
    fn an_environment_that_exposed_nothing_gets_no_default_command() {
        assert!(spec_from_exposure(&ExposureConfig::default()).is_none());
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
        let ex = ExposureConfig {
            mcp: vec!["dna".to_string()],
            api: Vec::new(),
            eval: Some(EvalExposure { allow: vec!["dna".to_string()] }),
        };
        let labels = deploy_labels("dev", &Version::new(0, 101, 0), &programs, &modules, &ex);
        let get = |k: &str| {
            labels
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("morloc.environment").as_deref(), Some("dev"));
        assert_eq!(get("morloc.programs").as_deref(), Some("dna"));
        assert_eq!(get("morloc.modules").as_deref(), Some("root-py"));
        assert_eq!(get("morloc.mcp").as_deref(), Some("dna"));
        assert_eq!(get("morloc.eval").as_deref(), Some("dna"));
        // An adapter nothing was exposed on is absent rather than empty.
        assert_eq!(get("morloc.api"), None);
        assert!(get("org.opencontainers.image.version").is_some());
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
}
