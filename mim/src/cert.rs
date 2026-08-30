//! Manager-side certificate orchestration: resolve a configured CA bundle,
//! preflight it (with strong, actionable output), materialize the normalized
//! files an environment uses, and detect drift.
//!
//! The trust-free parsing/normalization core lives in [`morloc_deps::cert`];
//! this module is the filesystem + user-facing shell around it. It exists so
//! `main.rs` stays free of certificate logic.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use morloc_deps::cert::{self, CertError, CertReport};
use sha2::{Digest, Sha256};

use crate::config as cfg;
use crate::error::{ManagerError, Result};
use crate::types::{EnvironmentConfig, Scope};

pub use morloc_deps::cert::{cert_env_pairs, fingerprints, CERT_ENV_VARS};

/// The in-image path of the trust store after `update-ca-certificates`. All the
/// cert env vars point here inside a container.
pub const CONTAINER_CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

/// Subdirectory (under the env data dir and the build context) that holds the
/// materialized cert files, and the two filenames within it.
const CERTS_SUBDIR: &str = "certs";
const HOST_BUNDLE_FILE: &str = "host-bundle.pem";
const CORP_FILE: &str = "corp.pem";
/// The corp bundle's path inside a container build context (matches [`corp_path`]).
const CONTEXT_CORP_REL: &str = "certs/corp.pem";

/// (name, value) pairs to set on a host subprocess (pixi solve/lock/activation,
/// curl) so it trusts the host bundle.
pub fn host_env_pairs(host_bundle: &Path) -> Vec<(String, String)> {
    cert_env_pairs(&host_bundle.to_string_lossy())
}

/// Load and validate a bundle, printing a "checking..." line, the neutral facts
/// for each certificate, and any warnings. Returns the report, or an error (the
/// verdict is printed before returning) for an unambiguous problem.
pub fn preflight(path: &Path) -> Result<CertReport> {
    eprintln!("Checking certificate bundle {}...", path.display());
    match load_report(path) {
        Ok(report) => {
            print_report(&report, Utc::now());
            Ok(report)
        }
        Err((manager_err, verdict)) => {
            anstream::eprintln!("  \x1b[1;31m[EE]\x1b[0m {verdict}");
            Err(manager_err)
        }
    }
}

/// Validate a bundle without printing the full per-certificate report -- for
/// inline, at-prompt feedback where the detailed report would otherwise be
/// printed again by the build-time `preflight`. Returns the certificate count on
/// success; on failure returns the error (its message is the verdict).
pub fn quick_check(path: &Path) -> Result<usize> {
    load_report(path).map(|r| r.certs.len()).map_err(|(e, _)| e)
}

/// Materialize the normalized files under `<env_data_dir>/certs/`: the host
/// bundle (corp + public roots, for `SSL_CERT_FILE`) and the corp-only file (for
/// the container build context). Callers derive the paths by convention via
/// [`host_bundle_path`] / [`corp_path`].
pub fn materialize_bundles(report: &CertReport, env_data_dir: &Path) -> Result<()> {
    let dir = env_data_dir.join(CERTS_SUBDIR);
    fs::create_dir_all(&dir).map_err(|e| {
        ManagerError::EnvError(format!("could not create {}: {e}", dir.display()))
    })?;
    let host_bundle = dir.join(HOST_BUNDLE_FILE);
    let corp = dir.join(CORP_FILE);
    fs::write(&host_bundle, cert::host_bundle_with_roots(report)).map_err(|e| {
        ManagerError::EnvError(format!("could not write {}: {e}", host_bundle.display()))
    })?;
    fs::write(&corp, cert::normalize_to_pem(report)).map_err(|e| {
        ManagerError::EnvError(format!("could not write {}: {e}", corp.display()))
    })?;
    Ok(())
}

/// Drift verdict for `doctor`: does the configured source bundle still match
/// what the environment was built with?
pub enum DriftStatus {
    /// No cert bundle configured for this environment.
    NotConfigured,
    /// Source bundle is present and matches the recorded fingerprints.
    InSync,
    /// The source path is gone or unreadable.
    SourceMissing(String),
    /// The source parses but its certificates differ from what was built.
    Drifted,
}

/// Compare the environment's recorded fingerprints against the current source
/// bundle. Pure of any printing so `doctor` can render it in its own style.
pub fn drift_status(ec: &EnvironmentConfig) -> DriftStatus {
    let Some(path) = ec.cert_bundle.as_deref() else {
        return DriftStatus::NotConfigured;
    };
    let report = match load_report(Path::new(path)) {
        Ok(r) => r,
        Err((_, _)) => return DriftStatus::SourceMissing(path.to_string()),
    };
    let mut current = cert::fingerprints(&report);
    let mut recorded = ec.cert_fingerprints.clone();
    current.sort();
    recorded.sort();
    if current == recorded {
        DriftStatus::InSync
    } else {
        DriftStatus::Drifted
    }
}

// --- internals ---------------------------------------------------------------

/// Load + parse, enforcing the size cap and resolving symlinks. On failure
/// returns both the manager error (to propagate) and the verdict (to print).
fn load_report(path: &Path) -> std::result::Result<CertReport, (ManagerError, String)> {
    let canonical = fs::canonicalize(path).map_err(|e| {
        let msg = format!("cannot read certificate bundle {}: {e}", path.display());
        (ManagerError::EnvError(msg.clone()), msg)
    })?;
    let meta = fs::metadata(&canonical).map_err(|e| {
        let msg = format!("cannot stat certificate bundle {}: {e}", canonical.display());
        (ManagerError::EnvError(msg.clone()), msg)
    })?;
    if meta.len() > cert::DEFAULT_SIZE_CAP {
        let verdict = CertError::TooLarge { size: meta.len(), cap: cert::DEFAULT_SIZE_CAP };
        return Err((ManagerError::EnvError(verdict.to_string()), verdict.to_string()));
    }
    let bytes = fs::read(&canonical).map_err(|e| {
        let msg = format!("cannot read certificate bundle {}: {e}", canonical.display());
        (ManagerError::EnvError(msg.clone()), msg)
    })?;
    cert::parse_bundle(&bytes)
        .map_err(|verdict| (ManagerError::EnvError(verdict.to_string()), verdict.to_string()))
}

fn print_report(report: &CertReport, now: DateTime<Utc>) {
    for c in &report.certs {
        let f = &c.facts;
        let subject = describe_name(&f.subject_cn, &f.subject_o, &f.subject);
        anstream::eprintln!("  \x1b[1;32m[ok]\x1b[0m {subject}");
        let ca = match f.is_ca {
            Some(true) => "CA:yes",
            Some(false) => "CA:no",
            None => "CA:?",
        };
        let signed = if f.is_self_signed { "self-signed" } else { "issued by another CA" };
        anstream::eprintln!(
            "         {ca}  {signed}  valid {} -> {}",
            f.not_before, f.not_after
        );
        anstream::eprintln!("         SHA256 {}", f.sha256_fingerprint);
        // Expiry is treated as a warning, not a verdict: a skewed host clock can
        // make a valid certificate look expired, so we surface the clock rather
        // than blocking.
        if let Some(ts) = f.not_after_ts {
            if let Some(expiry) = Utc.timestamp_opt(ts, 0).single() {
                if expiry < now {
                    anstream::eprintln!(
                        "  \x1b[1;33m[!!]\x1b[0m certificate expired {} (host clock now {}); \
                         if this is wrong, check the system clock",
                        f.not_after,
                        now.format("%Y-%m-%d %H:%M:%SZ")
                    );
                }
            }
        }
        if let Some(ts) = f.not_before_ts {
            if let Some(start) = Utc.timestamp_opt(ts, 0).single() {
                if start > now {
                    anstream::eprintln!(
                        "  \x1b[1;33m[!!]\x1b[0m certificate not valid until {} (host clock now {})",
                        f.not_before,
                        now.format("%Y-%m-%d %H:%M:%SZ")
                    );
                }
            }
        }
    }
    for e in &report.excluded {
        anstream::eprintln!(
            "  \x1b[2m[--] skipped block {} ({}): {}\x1b[0m",
            e.index, e.label, e.reason
        );
    }
}

fn describe_name(cn: &Option<String>, o: &Option<String>, full: &str) -> String {
    match (cn, o) {
        (Some(cn), Some(o)) => format!("{cn}  ({o})"),
        (Some(cn), None) => cn.clone(),
        (None, Some(o)) => o.clone(),
        (None, None) => full.to_string(),
    }
}

/// The environment's data directory (where the `certs/` subtree lives).
pub fn env_certs_dir(scope: Scope, name: &str) -> PathBuf {
    cfg::env_data_dir(scope, name).join(CERTS_SUBDIR)
}

/// The host bundle (corp + public roots) for `SSL_CERT_FILE`, by convention.
pub fn host_bundle_path(scope: Scope, name: &str) -> PathBuf {
    env_certs_dir(scope, name).join(HOST_BUNDLE_FILE)
}

/// The corp-only PEM staged into container image builds, by convention.
pub fn corp_path(scope: Scope, name: &str) -> PathBuf {
    env_certs_dir(scope, name).join(CORP_FILE)
}

/// The host bundle path if it has been materialized, else `None`.
pub fn host_bundle_if_present(scope: Scope, name: &str) -> Option<PathBuf> {
    let p = host_bundle_path(scope, name);
    p.is_file().then_some(p)
}

/// The recorded certificates as a rebuild cache-key fragment, derived from the
/// materialized corp bundle. Empty when no cert bundle is configured, so it does
/// not perturb the key for environments without one.
pub fn cache_fragment_for_env(scope: Scope, name: &str) -> String {
    match fs::read(corp_path(scope, name)) {
        Ok(bytes) => format!("cert:{:x}", Sha256::digest(&bytes)),
        Err(_) => String::new(),
    }
}

/// A snapshot of an environment's materialized cert files, so a modify that
/// re-materializes a new bundle and then fails to rebuild can restore the exact
/// prior on-disk state (contents, or absence). `None` means the file was not
/// present at snapshot time.
pub struct CertSnapshot {
    host: Option<Vec<u8>>,
    corp: Option<Vec<u8>>,
}

/// Capture the current materialized cert files before overwriting them.
pub fn snapshot_certs(scope: Scope, name: &str) -> CertSnapshot {
    CertSnapshot {
        host: fs::read(host_bundle_path(scope, name)).ok(),
        corp: fs::read(corp_path(scope, name)).ok(),
    }
}

/// Restore the cert files to a prior snapshot: rewrite the recorded bytes, or
/// remove a file that was absent when the snapshot was taken. Best-effort — used
/// on a rollback path where the caller is already returning an error.
pub fn restore_certs(scope: Scope, name: &str, snap: &CertSnapshot) -> Result<()> {
    restore_one(&host_bundle_path(scope, name), &snap.host)?;
    restore_one(&corp_path(scope, name), &snap.corp)?;
    Ok(())
}

fn restore_one(path: &Path, bytes: &Option<Vec<u8>>) -> Result<()> {
    match bytes {
        Some(b) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    ManagerError::EnvError(format!("could not create {}: {e}", parent.display()))
                })?;
            }
            fs::write(path, b).map_err(|e| {
                ManagerError::EnvError(format!("could not restore {}: {e}", path.display()))
            })
        }
        None if path.exists() => fs::remove_file(path).map_err(|e| {
            ManagerError::EnvError(format!("could not remove {}: {e}", path.display()))
        }),
        None => Ok(()),
    }
}

/// The Dockerfile-relative cert path if a corp bundle is materialized, without
/// performing the copy. Pairs with [`stage_into_context`] (which copies) for
/// build paths that render the Dockerfile before staging.
pub fn context_cert_rel(scope: Scope, name: &str) -> Option<String> {
    corp_path(scope, name)
        .is_file()
        .then(|| CONTEXT_CORP_REL.to_string())
}

/// Stage the materialized corp bundle into a container build context and return
/// its Dockerfile-relative path (for `DockerfileInput::cert_file`). `None` when
/// no cert bundle is configured for this environment.
pub fn stage_into_context(scope: Scope, name: &str, context: &Path) -> Result<Option<String>> {
    let corp = corp_path(scope, name);
    if !corp.is_file() {
        return Ok(None);
    }
    let dest_dir = context.join(CERTS_SUBDIR);
    fs::create_dir_all(&dest_dir).map_err(|e| {
        ManagerError::EnvError(format!("could not create {}: {e}", dest_dir.display()))
    })?;
    fs::copy(&corp, dest_dir.join(CORP_FILE)).map_err(|e| {
        ManagerError::EnvError(format!("could not stage certificate bundle: {e}"))
    })?;
    Ok(Some(CONTEXT_CORP_REL.to_string()))
}

/// The persisted cert fields recorded on an `EnvironmentConfig` after preparing
/// a bundle: the canonical source path and the fingerprints materialized from it.
pub struct PreparedCert {
    pub bundle_path: String,
    pub fingerprints: Vec<String>,
}

impl PreparedCert {
    /// Record this prepared bundle onto an environment config.
    pub fn apply_to(self, ec: &mut EnvironmentConfig) {
        ec.cert_bundle = Some(self.bundle_path);
        ec.cert_fingerprints = self.fingerprints;
    }
}

/// Preflight a configured bundle and materialize the normalized files this
/// environment will use. Returns `None` when `cert_bundle` is `None`; otherwise
/// prints the facts, aborts on a verdict, writes `certs/{host-bundle,corp}.pem`,
/// and returns the values to persist on the `EnvironmentConfig`.
pub fn prepare_for_env(
    scope: Scope,
    name: &str,
    cert_bundle: Option<&str>,
) -> Result<Option<PreparedCert>> {
    let Some(src) = cert_bundle else {
        return Ok(None);
    };
    let report = preflight(Path::new(src))?;
    let data_dir = cfg::env_data_dir(scope, name);
    materialize_bundles(&report, &data_dir)?;
    // Store the canonicalized path so later `make`/`doctor` runs resolve it
    // regardless of the working directory at creation time.
    let canonical = fs::canonicalize(src)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| src.to_string());
    Ok(Some(PreparedCert { bundle_path: canonical, fingerprints: fingerprints(&report) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real, valid multi-certificate PEM bundle (the vendored public roots).
    fn sample_bundle() -> &'static [u8] {
        cert::public_roots_pem()
    }

    /// The same bundle with its first certificate removed (a different, still
    /// valid, PEM) -- used to simulate a rotated source bundle.
    fn shorter_bundle() -> Vec<u8> {
        let marker = b"-----BEGIN CERTIFICATE-----";
        let bytes = sample_bundle();
        let first = find_sub(bytes, marker).unwrap();
        let second = find_sub(&bytes[first + 1..], marker).unwrap() + first + 1;
        bytes[second..].to_vec()
    }

    fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bundle.pem");
        fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn native_ec(name: &str) -> EnvironmentConfig {
        EnvironmentConfig::new_backend(
            name.to_string(),
            crate::types::Backend::Native,
            String::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
        )
    }

    #[test]
    fn preflight_accepts_a_valid_bundle() {
        let (_d, path) = write_temp(sample_bundle());
        let report = preflight(&path).expect("valid bundle");
        assert!(report.certs.len() > 1);
    }

    #[test]
    fn preflight_rejects_an_html_page() {
        let (_d, path) = write_temp(b"<!DOCTYPE html><html>proxy error</html>");
        assert!(preflight(&path).is_err());
    }

    #[test]
    fn preflight_rejects_a_missing_file() {
        assert!(preflight(Path::new("/no/such/cert.pem")).is_err());
    }

    #[test]
    fn materialize_writes_host_and_corp_bundles() {
        let env = tempfile::tempdir().unwrap();
        let report = cert::parse_bundle(sample_bundle()).unwrap();
        materialize_bundles(&report, env.path()).unwrap();
        let host = fs::read(env.path().join("certs/host-bundle.pem")).unwrap();
        let corp = fs::read(env.path().join("certs/corp.pem")).unwrap();
        // Host bundle = corp certs followed by the vendored public roots.
        assert!(host.starts_with(&corp));
        assert!(host.ends_with(cert::public_roots_pem()));
    }

    #[test]
    fn drift_status_tracks_source_changes() {
        let (_d, path) = write_temp(sample_bundle());
        let report = preflight(&path).unwrap();
        let mut ec = native_ec("t");
        ec.cert_bundle = Some(path.to_string_lossy().to_string());
        ec.cert_fingerprints = fingerprints(&report);
        assert!(matches!(drift_status(&ec), DriftStatus::InSync));

        // Overwrite the source with a different bundle -> drift.
        fs::write(&path, shorter_bundle()).unwrap();
        assert!(matches!(drift_status(&ec), DriftStatus::Drifted));
    }

    #[test]
    fn drift_status_none_when_unconfigured() {
        assert!(matches!(drift_status(&native_ec("t")), DriftStatus::NotConfigured));
    }

    #[test]
    fn snapshot_restores_prior_contents_and_absence() {
        let env = tempfile::tempdir().unwrap();
        let certs = env.path().join("certs");
        fs::create_dir_all(&certs).unwrap();
        let host = certs.join("host-bundle.pem");
        let corp = certs.join("corp.pem");

        // Snapshot with host present, corp absent.
        fs::write(&host, b"OLD-HOST").unwrap();
        let snap = CertSnapshot {
            host: fs::read(&host).ok(),
            corp: fs::read(&corp).ok(),
        };
        // Simulate a re-materialize that overwrote host and created corp.
        fs::write(&host, b"NEW-HOST").unwrap();
        fs::write(&corp, b"NEW-CORP").unwrap();

        restore_one(&host, &snap.host).unwrap();
        restore_one(&corp, &snap.corp).unwrap();
        assert_eq!(fs::read(&host).unwrap(), b"OLD-HOST", "host content restored");
        assert!(!corp.exists(), "corp removed (was absent at snapshot)");
    }
}
