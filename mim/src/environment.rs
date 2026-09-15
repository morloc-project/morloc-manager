use std::fs;
use std::process::Command;

use serde::Serialize;

use crate::config;
use crate::container::{self, engine_executable, image_exists_locally};
use crate::error::{ManagerError, Result};
use crate::serve;
use crate::types::*;

// ======================================================================
// Public types
// ======================================================================


/// Info returned by list_environments.
#[derive(Serialize)]
pub struct EnvInfo {
    pub name: String,
    pub morloc_version: Option<Version>,
    pub is_default: bool,
    /// A dev environment (built from a mounted source tree). For these the
    /// version above is the stdlib base, not the compiler.
    pub is_dev: bool,
    /// The image architecture of a docker/podman env; `None` for the other
    /// backends, which build for and run on the host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<crate::arch::Arch>,
}

// ======================================================================
// Image resolution
// ======================================================================










/// Detect the morloc version by running `morloc --version` inside the image.
/// For docker/podman this uses `<engine> run --rm <ref>`; for apptainer it
/// uses `apptainer exec <sif-path>`.
pub fn detect_morloc_version(engine: ContainerEngine, image: &str) -> Result<Version> {
    let exe = engine_executable(engine);
    let argv: Vec<&str> = match engine {
        ContainerEngine::Docker | ContainerEngine::Podman => {
            vec!["run", "--rm", image, "morloc", "--version"]
        }
        ContainerEngine::Apptainer => vec!["exec", image, "morloc", "--version"],
    };
    let output = Command::new(exe)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| ManagerError::EnvError(format!("Failed to run container: {e}")))?;

    if !output.status.success() {
        return Err(ManagerError::EnvError(format!(
            "Image '{image}' does not have a working morloc binary: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let ver_out = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Version::from_command_output(&ver_out).ok_or_else(|| {
        ManagerError::EnvError(format!(
            "Could not parse morloc version from image '{image}' output: {ver_out}"
        ))
    })
}


// ======================================================================
// Core operations
// ======================================================================

/// Create or update an environment.
///
/// When `is_new` is true: validates name uniqueness, creates data directories.
/// Validate that an environment name contains only allowed characters.
pub fn validate_env_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ManagerError::EnvError(format!(
            "Invalid environment name '{name}': must contain only alphanumeric characters, hyphens, underscores, or dots"
        )));
    }
    Ok(())
}




/// Remove an environment and all its data. `engine` is the container engine for
/// container-backed environments, or `None` for native environments (which have
/// no serve container or image to clean up, only on-disk files).
pub fn remove_environment(engine: Option<ContainerEngine>, scope: Scope, name: &str) -> Result<()> {
    let ec = config::read_env_config(scope, name)
        .map_err(|_| ManagerError::EnvironmentNotFound(name.to_string()))?;

    // Stop and remove any running serve container and built image. Native
    // environments have neither, so this whole block is skipped for them.
    if let Some(engine) = engine {
        // Stop and remove any running serve container for this environment
        // before removing its image. If we skipped this, the serve container
        // would keep running and be unreachable through mim.
        let serve_name = serve::serve_container_name(name);
        if container::container_exists(engine, &serve_name) {
            let _ = container::container_stop(engine, &serve_name);
            let _ = container::container_remove_quiet(engine, &serve_name);
        }

        // Remove built Dockerfile layer image
        if let Some(ref img) = ec.built_image {
            if image_exists_locally(engine, img) {
                container::remove_image(engine, img);
            }
        }

        // The solved conda prefix lives in an engine volume rather than under the
        // env data dir, so deleting the directory tree below would leave it
        // behind -- gigabytes with nothing left to reference them.
        let _ = container::volume_remove(
            engine,
            &serve::prefix_volume(&config::env_data_dir(scope, name)),
        );
    }

    // Remove config directory
    let cfg_dir = config::env_config_dir(scope, name);
    if cfg_dir.is_dir() {
        let _ = fs::remove_dir_all(&cfg_dir);
    }

    // Remove data directory
    let data_dir = config::env_data_dir(scope, name);
    if data_dir.is_dir() {
        let _ = fs::remove_dir_all(&data_dir);
    }

    // If a config's default tag pointed at this name AND no env of that name
    // survives in any scope, clear the tag. The name-still-resolves guard avoids
    // over-clearing when a same-named env remains in the other scope (e.g. a
    // local `default` is removed while a system `default` — the common case now
    // that no-name `new` uses the constant name `default` — still exists).
    let name_orphaned = config::find_env_scope(name).is_err();
    if name_orphaned {
        for cfg_scope in [Scope::Local, Scope::System] {
            let cfg_path = config::config_path(cfg_scope);
            if let Ok(cfg) = config::read_config::<Config>(&cfg_path) {
                if cfg.default_env.as_deref() == Some(name) {
                    let new_cfg = Config {
                        default_env: None,
                        ..cfg
                    };
                    let _ = config::write_config(&cfg_path, &new_cfg);
                }
            }
        }
    }

    Ok(())
}

/// The tag of the requirement-derived image an environment builds for itself.
/// Derived from the name, so a rename re-tags it.
pub fn env_image_tag(name: &str) -> String {
    format!("localhost/morloc-env:{name}")
}

/// Why an environment cannot be renamed right now, checked before any side
/// effect. A native environment is refused outright: its conda prefix, its
/// MORLOC_HOME and every installed program's launcher and manifest carry the
/// absolute host path that includes the name, and none of them can be moved in
/// place.
pub fn validate_rename(scope: Scope, old: &str, new: &str, ec: &EnvironmentConfig) -> Result<()> {
    validate_env_name(new)?;
    if new == old {
        return Err(ManagerError::EnvError(format!(
            "environment '{old}' already has that name; nothing to rename."
        )));
    }
    if config::env_config_dir(scope, new).exists() || config::env_data_dir(scope, new).exists() {
        return Err(ManagerError::EnvError(format!(
            "an environment named '{new}' already exists; pick another name, or remove \
             it first with 'mim rm {new}'."
        )));
    }
    if ec.backend.is_native() {
        return Err(ManagerError::EnvError(format!(
            "environment '{old}' uses the native backend, whose toolchain, runtime and \
             installed programs are built at absolute paths that include its name; it \
             cannot be renamed in place. Create the environment again under the new \
             name with 'mim new {new} --engine native' and reinstall its programs."
        )));
    }
    Ok(())
}

/// Rename a container-backed environment: its config and data directories move,
/// its record is rewritten under the new name, and every engine-side object
/// derived from the name follows -- the built image is re-tagged and the solved
/// conda prefix is copied into the volume named for the new data directory,
/// since neither docker nor podman can rename a volume. Paths inside the
/// container are the same for every environment, so nothing in the moved trees
/// needs rewriting. A default that pointed at the old name follows it.
///
/// Every step that can fail is undone in reverse if a later one does, so a
/// failed rename leaves the environment exactly as it was. The caller has
/// already checked that nothing is serving it.
pub fn rename_environment(
    scope: Scope,
    old: &str,
    new: &str,
    ec: &EnvironmentConfig,
) -> Result<EnvironmentConfig> {
    validate_rename(scope, old, new, ec)?;
    let old_data = config::env_data_dir(scope, old);
    let new_data = config::env_data_dir(scope, new);
    let old_cfg = config::env_config_dir(scope, old);
    let new_cfg = config::env_config_dir(scope, new);
    let engine = ec.backend.container_engine().filter(|e| e.is_oci());
    let mut new_ec = ec.clone();
    new_ec.name = new.to_string();

    // Each completed step pushes its reversal; a failure runs them last-first.
    let mut undo: Vec<Box<dyn FnOnce()>> = Vec::new();
    let fail = |undo: Vec<Box<dyn FnOnce()>>, e: ManagerError| -> Result<EnvironmentConfig> {
        for step in undo.into_iter().rev() {
            step();
        }
        Err(e)
    };

    // The solved prefix, keyed on the data directory path (see
    // `serve::prefix_volume`). Copied first: it is the slow step and the one
    // most likely to fail, and until the directories move nothing else has
    // changed.
    let mut old_volume: Option<String> = None;
    let mut old_tag: Option<String> = None;
    if let Some(engine) = engine {
        let from = serve::prefix_volume(&old_data);
        if container::volume_exists(engine, &from) {
            let Some(image) = ec
                .built_image
                .as_deref()
                .filter(|img| image_exists_locally(engine, img))
            else {
                return Err(ManagerError::EnvError(format!(
                    "environment '{old}' has a solved conda prefix but no built image to \
                     copy it with; rebuild first with 'mim update --env {old}'."
                )));
            };
            let to = serve::prefix_volume(&new_data);
            eprintln!("Copying the solved conda prefix of '{old}' to its new volume...");
            let mount = format!("{}/.pixi", serve::CONTAINER_PIXI_DIR);
            let platform = ec.oci_arch()?;
            if let Err(msg) = container::volume_copy(engine, image, platform, &from, &to, &mount) {
                let _ = container::volume_remove(engine, &to);
                return Err(ManagerError::EnvError(format!(
                    "could not copy the conda prefix volume of '{old}':\n{msg}"
                )));
            }
            let to_undo = to.clone();
            undo.push(Box::new(move || {
                let _ = container::volume_remove(engine, &to_undo);
            }));
            old_volume = Some(from);
        }
        // Only the name-derived tag follows the name; a custom tag stays valid.
        if ec.built_image.as_deref() == Some(env_image_tag(old).as_str()) {
            let from = env_image_tag(old);
            let to = env_image_tag(new);
            if image_exists_locally(engine, &from) {
                if let Err(msg) = container::tag_image(engine, &from, &to) {
                    return fail(
                        undo,
                        ManagerError::EnvError(format!(
                            "could not re-tag the image of '{old}' as {to}:\n{msg}"
                        )),
                    );
                }
                let to_undo = to.clone();
                undo.push(Box::new(move || {
                    container::remove_image(engine, &to_undo);
                }));
                old_tag = Some(from);
            }
            new_ec.built_image = Some(to);
        }
    }

    // The directories. Each rename is within one parent, so it never crosses
    // a filesystem.
    if let Err(e) = fs::rename(&old_cfg, &new_cfg) {
        return fail(
            undo,
            ManagerError::EnvError(format!(
                "cannot move {} to {}: {e}",
                old_cfg.display(),
                new_cfg.display()
            )),
        );
    }
    {
        let (from, to) = (new_cfg.clone(), old_cfg.clone());
        undo.push(Box::new(move || {
            let _ = fs::rename(&from, &to);
        }));
    }
    if old_data.is_dir() {
        if let Err(e) = fs::rename(&old_data, &new_data) {
            return fail(
                undo,
                ManagerError::EnvError(format!(
                    "cannot move {} to {}: {e}",
                    old_data.display(),
                    new_data.display()
                )),
            );
        }
        let (from, to) = (new_data.clone(), old_data.clone());
        undo.push(Box::new(move || {
            let _ = fs::rename(&from, &to);
        }));
    }

    // A recorded host path (an apptainer image, a mounted home, a CA bundle)
    // that sat inside the trees that just moved has moved with them.
    let mut recorded: Vec<&mut String> = Vec::new();
    recorded.extend(new_ec.base_sif.as_mut());
    recorded.extend(new_ec.layered_sif.as_mut());
    recorded.extend(new_ec.mount_home.as_mut());
    recorded.extend(new_ec.cert_bundle.as_mut());
    recorded.extend(new_ec.dev.as_mut().map(|d| &mut d.source));
    recorded.extend(new_ec.local_runtime.as_mut().map(|l| &mut l.source));
    for p in recorded {
        let moved = relocate_path(p, &old_cfg, &new_cfg)
            .or_else(|| relocate_path(p, &old_data, &new_data));
        if let Some(m) = moved {
            *p = m;
        }
    }
    if let Err(e) = config::write_env_config(scope, new, &new_ec) {
        return fail(undo, e);
    }

    // Committed. What remains is cleanup of the old identity and the pointers
    // to it; none of it can un-rename the environment, so failures here are
    // reported, not unwound.
    // The serve record moved with the config dir and names a container that
    // no longer exists (the caller refused a live serve).
    config::remove_serve_runtime(scope, new);
    if let Some(engine) = engine {
        if let Some(v) = old_volume {
            if !container::volume_remove(engine, &v).success() {
                eprintln!("Warning: could not remove the old conda prefix volume {v}.");
            }
        }
        if let Some(t) = old_tag {
            if !container::remove_image(engine, &t) {
                eprintln!("Warning: could not drop the old image tag {t}.");
            }
        }
    }
    rename_default_pointers(scope, old, new);
    Ok(new_ec)
}

/// `path` with the `old_root` prefix replaced by `new_root`, or `None` when
/// the path does not lie under `old_root`.
fn relocate_path(path: &str, old_root: &std::path::Path, new_root: &std::path::Path) -> Option<String> {
    std::path::Path::new(path)
        .strip_prefix(old_root)
        .ok()
        .map(|rest| new_root.join(rest).to_string_lossy().into_owned())
}

/// Whether the default recorded in the `cfg_scope` config named the environment
/// that was just renamed (in `env_scope`), and so should now name it by its new
/// name. A local pointer resolves local-first, so it named this environment when
/// the environment was local, or when it was system-scope and no local
/// environment of the old name shadows it (`old_resolves` is false). A system
/// pointer only ever names a system environment.
fn default_pointer_follows(cfg_scope: Scope, env_scope: Scope, old_resolves: bool) -> bool {
    match cfg_scope {
        Scope::Local => env_scope == Scope::Local || !old_resolves,
        Scope::System => env_scope == Scope::System,
    }
}

/// Repoint every default that named the renamed environment. Best-effort: a
/// config that cannot be rewritten is reported, since the environment has
/// already moved.
fn rename_default_pointers(env_scope: Scope, old: &str, new: &str) {
    let old_resolves = config::find_env_scope(old).is_ok();
    for cfg_scope in [Scope::Local, Scope::System] {
        if !default_pointer_follows(cfg_scope, env_scope, old_resolves) {
            continue;
        }
        let cfg_path = config::config_path(cfg_scope);
        let Ok(cfg) = config::read_config::<Config>(&cfg_path) else { continue };
        if cfg.default_env.as_deref() != Some(old) {
            continue;
        }
        let new_cfg = Config {
            default_env: Some(new.to_string()),
            ..cfg
        };
        if config::write_config(&cfg_path, &new_cfg).is_err() {
            eprintln!(
                "Warning: could not update the default in {}; it still names '{old}'. \
                 Set it again with: mim modify --env {new} --set-default",
                cfg_path.display()
            );
        }
    }
}

/// List environments in the given scope.
pub fn list_environments(scope: Scope, default_env: Option<&str>) -> Vec<EnvInfo> {
    let names = config::list_env_names(scope);
    let mut result = Vec::new();
    for name in names {
        if let Ok(ec) = config::read_env_config(scope, &name) {
            result.push(EnvInfo {
                name: name.clone(),
                is_dev: ec.is_dev(),
                arch: ec.oci_arch().ok().flatten(),
                morloc_version: ec.morloc_version,
                is_default: default_env == Some(name.as_str()),
            });
        }
    }
    result
}

/// Tag an environment as the default by writing default_env to the given
/// write_scope config.
pub fn set_default_environment(name: &str, write_scope: Scope) -> Result<()> {
    // Verify the environment exists somewhere
    config::find_env_scope(name)?;

    let cfg_path = config::config_path(write_scope);
    let base_cfg = config::read_config::<Config>(&cfg_path)
        .or_else(|_| config::read_config::<Config>(&config::config_path(Scope::System)))
        .unwrap_or_default();
    let new_cfg = Config {
        default_env: Some(name.to_string()),
        ..base_cfg
    };
    config::write_config(&cfg_path, &new_cfg)
}

/// Clear the default environment recorded in `write_scope`'s config, leaving
/// every other setting in that config alone.
///
/// Clearing the LOCAL default does not necessarily leave the machine with no
/// default: `resolve_default_env_name` falls through to the system config, so a
/// machine-wide default (if one is set) becomes the effective answer again.
/// Callers report which of the two happened via [`effective_default_env_name`].
pub fn clear_default_environment(write_scope: Scope) -> Result<()> {
    let cfg_path = config::config_path(write_scope);
    let base_cfg = config::read_config::<Config>(&cfg_path).unwrap_or_default();
    let new_cfg = Config {
        default_env: None,
        ..base_cfg
    };
    config::write_config(&cfg_path, &new_cfg)
}

/// The default environment name that commands would resolve right now, or
/// `None` when there is none. Unlike `resolve_default_environment` this reads
/// only the name and never errors, so it can report the state after a change.
pub fn effective_default_env_name() -> Option<String> {
    resolve_default_env_name().ok()
}

/// Resolve the default environment. Checks local config first, then system.
/// Returns (name, scope where env config lives, EnvironmentConfig).
pub fn resolve_default_environment() -> Result<(String, Scope, EnvironmentConfig)> {
    // Find default_env name from config (local first, then system)
    let name = resolve_default_env_name()?;

    // Find which scope has the environment config
    let scope = config::find_env_scope(&name)?;
    let ec = config::read_env_config(scope, &name)?;
    Ok((name, scope, ec))
}

/// Resolve just the default environment name from config.
/// Skips names that don't resolve to an actual environment (e.g., stale
/// entries from old config formats).
fn resolve_default_env_name() -> Result<String> {
    if let Ok(cfg) = config::read_config::<Config>(&config::config_path(Scope::Local)) {
        if let Some(ref name) = cfg.default_env {
            if config::find_env_scope(name).is_ok() {
                return Ok(name.clone());
            }
        }
    }
    if let Ok(cfg) = config::read_config::<Config>(&config::config_path(Scope::System)) {
        if let Some(ref name) = cfg.default_env {
            if config::find_env_scope(name).is_ok() {
                return Ok(name.clone());
            }
        }
    }
    // Check if any environments exist to give a better suggestion
    let local_envs = config::list_env_names(Scope::Local);
    let system_envs = config::list_env_names(Scope::System);
    if local_envs.is_empty() && system_envs.is_empty() {
        Err(ManagerError::NoDefaultEnvironment)
    } else {
        // Label each entry with its scope so same-named envs are distinguishable.
        let mut available: Vec<String> = local_envs
            .iter()
            .map(|n| format!("{n} (local)"))
            .collect();
        available.extend(system_envs.iter().map(|n| format!("{n} (system)")));
        Err(ManagerError::EnvError(format!(
            "No default environment set. Pass --env <name>, or set a default with: \
             mim modify --env <name> --set-default\n\
             Available: {}",
            available.join(", ")
        )))
    }
}

// ======================================================================
// Internal
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn env_image_tag_is_name_derived() {
        assert_eq!(env_image_tag("myenv"), "localhost/morloc-env:myenv");
    }

    #[test]
    fn relocate_path_moves_only_paths_under_the_old_root() {
        let old = Path::new("/data/environments/old");
        let new = Path::new("/data/environments/new");
        assert_eq!(
            relocate_path("/data/environments/old/sif/base.sif", old, new).as_deref(),
            Some("/data/environments/new/sif/base.sif")
        );
        // A sibling whose name merely starts with the old name is not under it.
        assert_eq!(relocate_path("/data/environments/older/base.sif", old, new), None);
        assert_eq!(relocate_path("/elsewhere/base.sif", old, new), None);
    }

    #[test]
    fn local_default_follows_a_local_rename() {
        assert!(default_pointer_follows(Scope::Local, Scope::Local, false));
        // Even when a system env of the old name remains: the local pointer
        // resolved local-first, to the env that moved.
        assert!(default_pointer_follows(Scope::Local, Scope::Local, true));
    }

    #[test]
    fn local_default_follows_a_system_rename_only_when_unshadowed() {
        assert!(default_pointer_follows(Scope::Local, Scope::System, false));
        // A local env of the old name is what the pointer named; leave it.
        assert!(!default_pointer_follows(Scope::Local, Scope::System, true));
    }

    #[test]
    fn system_default_follows_only_a_system_rename() {
        assert!(default_pointer_follows(Scope::System, Scope::System, false));
        assert!(default_pointer_follows(Scope::System, Scope::System, true));
        assert!(!default_pointer_follows(Scope::System, Scope::Local, false));
        assert!(!default_pointer_follows(Scope::System, Scope::Local, true));
    }
}
