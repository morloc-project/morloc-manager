//! Native-environment provisioning: fetch the version-matched morloc compiler
//! and the rust sources from a GitHub release into `runtimes/<version>/<triple>/`, so
//! `morloc init` (pointed at `<dir>/runtime` via MORLOC_RUNTIME_DIR) can build the
//! runtime from source and set up a native environment with no container.
//!
//! Coherence: every piece is pulled from a SINGLE release tag, so they share a
//! version by construction; the tag's manifest declares that version. This
//! single-tag fetch is the interim coherence guarantee while the ABI-version
//! gate (`morloc_abi_version()`) stays deferred.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use morloc_deps::version::{ENVSPEC_MIN_VERSION, ENVSPEC_VERSION, LANG_SCHEMA_MAJOR, MANIFEST_SCHEMA};

use crate::config;
use crate::error::{ManagerError, Result};
use crate::types::Scope;

/// GitHub release download base for the morloc project.
const RELEASE_BASE: &str = "https://github.com/morloc-project/morloc/releases/download";
/// GitHub release *page* base (used to check that a tag exists, independent of
/// which assets it publishes).
const RELEASE_TAG_BASE: &str = "https://github.com/morloc-project/morloc/releases/tag";
/// The manifest asset attached to every release (see .github/workflows/release.yml).
const MANIFEST_ASSET: &str = "morloc-release-manifest.json";
/// Pinned pixi version mim provisions + generates container images
/// against (single source of truth for both). Bundling pixi keeps the native
/// solve self-contained; linking rattler is the eventual replacement.
pub const PIXI_VERSION: &str = "0.76.2";
/// The language-support table asset (compiler-owned static data). Downloaded so
/// the manager can render pixi manifests without executing the compiler on the
/// host. Filename in the runtime store once downloaded.
const LANG_SUPPORT_ASSET: &str = "morloc-lang-support.json";
/// Filename of the downloaded lang-support table inside a runtime store dir.
pub const LANG_SUPPORT_FILE: &str = "lang-support.json";
/// The per-platform prebuilt binaries in a release: just the morloc compiler.
/// libmorloc.so + morloc-nexus are NOT here -- they are built from the
/// (platform-independent) Rust source at `morloc init`. The manager (mim) is
/// released and installed from its own repository, never bundled here.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TripleAssets {
    pub morloc: String,
}

/// The contract/ABI versions a release emits, inlined into the manifest from the
/// compiler's `morloc versions` output. Lets the manager gate on compatibility
/// BEFORE downloading anything. Fields are the versions the COMPILER controls;
/// the manager compares them against its own supported versions
/// (`morloc-deps::version`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MorlocVersions {
    pub morloc_version: String,
    /// morloc C ABI version. Not checked: libmorloc is built from source at
    /// `morloc init`, so a runtime is ABI-coherent by construction.
    pub abi_version: u32,
    /// `envspec.json` schema version (integer).
    pub envspec_version: u32,
    /// `morloc lang-support` schema version (semver "MAJOR.MINOR").
    pub lang_support_schema: String,
}

impl MorlocVersions {
    /// PROV-3: reject a release whose contract versions this manager cannot
    /// read -- newer or older -- so the failure surfaces at install time with
    /// the direction of the fix, rather than as a parse error mid-build. The
    /// accepted set is exactly what the parsers accept: envspec
    /// `ENVSPEC_MIN_VERSION` to `ENVSPEC_VERSION`, and lang-support major
    /// `LANG_SCHEMA_MAJOR` with any minor.
    pub fn check_supported(&self) -> Result<()> {
        let refuse = |what: &str, declared: &str, readable: &str, newer: bool| {
            let fix = if newer {
                "upgrade mim"
            } else {
                "pick a newer morloc release"
            };
            Err(ManagerError::EnvError(format!(
                "this morloc release writes {what} {declared} but this mim reads only \
                 {readable}; {fix}."
            )))
        };
        if !(ENVSPEC_MIN_VERSION..=ENVSPEC_VERSION).contains(&self.envspec_version) {
            return refuse(
                "envspec_version",
                &self.envspec_version.to_string(),
                &format!("{ENVSPEC_MIN_VERSION} to {ENVSPEC_VERSION}"),
                self.envspec_version > ENVSPEC_VERSION,
            );
        }
        let lang_major = self
            .lang_support_schema
            .split('.')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| {
                ManagerError::EnvError(format!(
                    "release lang_support_schema '{}' is not a MAJOR.MINOR version",
                    self.lang_support_schema
                ))
            })?;
        if lang_major != LANG_SCHEMA_MAJOR {
            return refuse(
                "lang-support schema",
                &self.lang_support_schema,
                &format!("{LANG_SCHEMA_MAJOR}.x"),
                lang_major > LANG_SCHEMA_MAJOR,
            );
        }
        Ok(())
    }
}

/// The native-install manifest attached to a release, of the one schema
/// `from_json` accepts.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub runtime_src: String,
    /// The contract/ABI versions this release emits (see `MorlocVersions`).
    pub versions: MorlocVersions,
    /// Release-triple ("linux-x86_64", ...) -> its prebuilt assets. Only triples
    /// with a complete, tested asset set are present.
    pub triples: BTreeMap<String, TripleAssets>,
    /// Asset filename -> lowercase hex SHA-256, published for every asset. Each
    /// downloaded artifact is verified against its digest after fetch; an asset
    /// with no digest here is refused, since there is nothing to verify it
    /// against (a missing digest must never silently disable verification).
    pub sha256: BTreeMap<String, String>,
}

impl ReleaseManifest {
    pub fn from_json(text: &str) -> Result<Self> {
        let parse_err = |e: serde_json::Error| ManagerError::EnvError(format!("Failed to parse {MANIFEST_ASSET}: {e}"));
        #[derive(Deserialize)]
        struct Header {
            schema: u32,
        }
        // PROV-3: the schema is read before the fields it governs.
        let schema = serde_json::from_str::<Header>(text).map_err(parse_err)?.schema;
        if schema > MANIFEST_SCHEMA {
            return Err(ManagerError::EnvError(format!(
                "{MANIFEST_ASSET} schema {schema} is newer than this mim reads \
                 ({MANIFEST_SCHEMA}); upgrade mim."
            )));
        }
        if schema < MANIFEST_SCHEMA {
            return Err(ManagerError::EnvError(format!(
                "{MANIFEST_ASSET} schema {schema} is older than this mim reads \
                 ({MANIFEST_SCHEMA}); pick a newer morloc release, or install this one \
                 with an older mim."
            )));
        }
        let m: ReleaseManifest = serde_json::from_str(text).map_err(parse_err)?;
        m.versions.check_supported()?;
        Ok(m)
    }

    /// The prebuilt assets for a release triple, if this release provides one.
    pub fn assets_for(&self, triple: &str) -> Option<&TripleAssets> {
        self.triples.get(triple)
    }
}

/// The release triple string (matching release.yml asset naming) for an
/// (os, arch) pair, or None where no native artifacts are published.
pub fn release_triple(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        // Intel macOS, Windows, etc.: no native artifacts published.
        _ => None,
    }
}

/// The release triple for the current host.
pub fn host_release_triple() -> Option<&'static str> {
    release_triple(std::env::consts::OS, std::env::consts::ARCH)
}


/// GitHub download URL for `asset` of a release `tag` (e.g. "v0.98.3" or "dev").
pub fn asset_url(tag: &str, asset: &str) -> String {
    format!("{RELEASE_BASE}/{tag}/{asset}")
}

/// Fetch the stdlib module-pin snapshot published alongside a morloc release
/// (asset `stdlib-snapshot.txt` under the `v<version>` tag). This is the default
/// "latest stdlib for your morloc" that mim deposits into a new environment.
/// Best-effort: returns None on any failure (a release without the asset,
/// offline, network error) so provisioning proceeds with no default snapshot
/// rather than failing.
pub fn fetch_stdlib_snapshot(trust: &Trust, version: &str) -> Option<String> {
    let url = asset_url(&format!("v{version}"), "stdlib-snapshot.txt");
    curl_capture_quiet(trust, &url).ok()
}

/// The public release page for `tag` (HTTP 200 if the release exists, 404
/// otherwise). Used as an asset-independent existence check and in diagnostics.
pub fn release_page_url(tag: &str) -> String {
    format!("{RELEASE_TAG_BASE}/{tag}")
}

/// Verify a requested version resolves to a published release, returning the
/// resolved tag. Checks the release PAGE (which 404s for a nonexistent tag)
/// rather than a specific asset, so it does not depend on which assets a given
/// release publishes.
pub fn verify_release(trust: &Trust, requested: &str) -> Result<String> {
    let tag = resolve_tag(trust, requested)?;
    let url = release_page_url(&tag);
    // -I/-o /dev/null: a HEAD that discards the body; -f (from curl_base) makes a
    // 404 a non-success exit. Silence curl's own stderr so the caller controls
    // the message.
    let ok = curl_base(trust)
        .args(["-I", "-o", "/dev/null"])
        .arg(&url)
        .stderr(Stdio::null())
        .status()
        .map_err(curl_spawn_err)?
        .success();
    if ok {
        Ok(tag)
    } else {
        Err(ManagerError::EnvError(format!("no published release at {url}")))
    }
}

/// PROV-9: the certificates a download trusts. Every download names one, so
/// none can fall back to the system store by omission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// curl's own store.
    System,
    /// Exactly this bundle (`--cacert` replaces the system store).
    Bundle(PathBuf),
}

impl Trust {
    /// A download belonging to no environment: the user's `CURL_CA_BUNDLE` or
    /// `SSL_CERT_FILE`, else the system store.
    pub fn user() -> Trust {
        Self::from_vars(|k| std::env::var_os(k))
    }

    fn from_vars(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Trust {
        ["CURL_CA_BUNDLE", "SSL_CERT_FILE"]
            .iter()
            .find_map(|k| var(k).filter(|v| !v.is_empty()))
            .map_or(Trust::System, |v| Trust::Bundle(PathBuf::from(v)))
    }

    /// A download for environment `name`: its materialized host bundle (its
    /// corporate certificates plus the public roots), else the user's.
    pub fn for_env(scope: Scope, name: &str) -> Trust {
        crate::cert::host_bundle_if_present(scope, name).map_or_else(Trust::user, Trust::Bundle)
    }
}

/// A curl command hardened with the same TLS/proto flags and stdio for every
/// fetch. Callers add `-o <dest>` (download) or just the URL (capture).
fn curl_base(trust: &Trust) -> Command {
    let mut c = Command::new("curl");
    c.args(["--proto", "=https", "--tlsv1.2", "-fsSL"])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit());
    if let Trust::Bundle(path) = trust {
        c.arg("--cacert").arg(path);
    }
    c
}

fn curl_spawn_err(e: std::io::Error) -> ManagerError {
    ManagerError::EnvError(format!("could not run curl (is it installed?): {e}"))
}

/// Capture a URL's body to a string via curl (follows redirects; fails on HTTP
/// error). Used to fetch the release manifest.
pub(crate) fn curl_capture(trust: &Trust, url: &str) -> Result<String> {
    let out = curl_base(trust).arg(url).output().map_err(curl_spawn_err)?;
    if !out.status.success() {
        return Err(ManagerError::EnvError(format!("download failed: {url}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Like `curl_capture` but silences curl's own stderr. For probes where a 404
/// is an expected, handled outcome (the caller reports it), so curl's raw error
/// line does not precede the caller's message.
pub(crate) fn curl_capture_quiet(trust: &Trust, url: &str) -> Result<String> {
    let out = curl_base(trust)
        .arg(url)
        .stderr(Stdio::null())
        .output()
        .map_err(curl_spawn_err)?;
    if !out.status.success() {
        return Err(ManagerError::EnvError(format!("download failed: {url}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The final URL after following redirects for `url` (an HTTP HEAD; the body is
/// discarded). Used to resolve the `releases/latest` redirect to its tag page.
pub(crate) fn curl_effective_url(trust: &Trust, url: &str) -> Result<String> {
    let out = curl_base(trust)
        .args(["-I", "-o", "/dev/null", "-w", "%{url_effective}"])
        .arg(url)
        .output()
        .map_err(curl_spawn_err)?;
    if !out.status.success() {
        return Err(ManagerError::EnvError(format!(
            "could not resolve the latest release (is there a published release?): {url}"
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Extract the release tag from a `.../releases/tag/<tag>` URL (the target of the
/// `releases/latest` redirect).
pub(crate) fn parse_tag_from_release_url(url: &str) -> Option<String> {
    url.rsplit_once("/tag/")
        .map(|(_, tag)| tag.trim_end_matches('/').to_string())
        .filter(|t| !t.is_empty())
}

/// Resolve the GitHub tag of the latest published morloc release.
///
/// This follows the unauthenticated `.../releases/latest` redirect (which lands
/// on `.../releases/tag/<tag>`) rather than the REST API. The REST API's
/// unauthenticated limit is 60 requests/hour PER IP, which silently breaks
/// provisioning behind shared IPs / CI / NAT; the web redirect carries no such
/// per-hour cap and needs no token. Set `MORLOC_RELEASE_TAG` to pin an explicit
/// tag and skip this lookup entirely.
pub fn fetch_latest_tag(trust: &Trust) -> Result<String> {
    let latest = "https://github.com/morloc-project/morloc/releases/latest";
    let final_url = curl_effective_url(trust, latest)?;
    parse_tag_from_release_url(&final_url).ok_or_else(|| {
        ManagerError::EnvError(format!(
            "could not determine the latest release tag from {final_url}"
        ))
    })
}

/// Resolve a requested release specifier to a concrete GitHub tag. "latest"
/// follows GitHub's releases/latest redirect; a bare version like "0.98.3" becomes "v0.98.3"; anything
/// else (an explicit tag such as "dev" or "v0.98.3") is used verbatim.
pub fn resolve_tag(trust: &Trust, requested: &str) -> Result<String> {
    let requested = requested.trim();
    if requested.eq_ignore_ascii_case("latest") {
        fetch_latest_tag(trust)
    } else if requested.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Ok(format!("v{requested}"))
    } else {
        Ok(requested.to_string())
    }
}

/// The runtime store directory for one (version, release triple) pair. The
/// compiler binary inside is built for `triple`, so the store is keyed on both:
/// a native env and a container env at the same version, or two container envs
/// of different architectures, each get their own directory.
pub fn runtimes_dir(scope: Scope, version: &str, triple: &str) -> PathBuf {
    config::data_dir(scope).join("runtimes").join(version).join(triple)
}

/// Download a URL to a path via curl (follows redirects; fails on HTTP error).
pub(crate) fn curl_download(trust: &Trust, url: &str, dest: &Path) -> Result<()> {
    let status = curl_base(trust)
        .arg("-o")
        .arg(dest)
        .arg(url)
        .status()
        .map_err(curl_spawn_err)?;
    if !status.success() {
        return Err(ManagerError::EnvError(format!("download failed: {url}")));
    }
    Ok(())
}

/// Lowercase hex of a byte slice.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Streaming SHA-256 of a file, as lowercase hex.
fn file_sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| ManagerError::EnvError(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| ManagerError::EnvError(format!("cannot read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// A unique sibling `<name>.<pid>.<n>.part` staging path for a download
/// destination. The pid + monotonic counter keep two concurrent fetches to the
/// SAME cache path (e.g. two envs materializing at once) from writing into one
/// another's staging file; the post-verify atomic rename still elects a single
/// winner into `dest`.
fn part_path(dest: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dest.with_file_name(format!("{name}.{}.{n}.part", std::process::id()))
}

/// Download `asset` of release `tag` to `dest` atomically, verifying its SHA-256
/// against `digests` (an asset with no published digest is refused). The body
/// lands on a temporary `.part` path and is renamed into place only after the
/// download and verification succeed, so an interrupted, corrupt, or unverifiable
/// fetch never leaves a file that a presence check would bless as valid.
fn download_asset(
    trust: &Trust,
    tag: &str,
    asset: &str,
    dest: &Path,
    digests: &BTreeMap<String, String>,
) -> Result<()> {
    let part = part_path(dest);
    let result = (|| {
        curl_download(trust, &asset_url(tag, asset), &part)?;
        let expected = digests.get(asset).ok_or_else(|| {
            ManagerError::EnvError(format!(
                "no SHA-256 published for asset {asset}; refusing an unverified download"
            ))
        })?;
        let actual = file_sha256(&part)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(ManagerError::EnvError(format!(
                "checksum mismatch for {asset}: expected {expected}, got {actual} \
                 (corrupt or tampered download)"
            )));
        }
        std::fs::rename(&part, dest).map_err(|e| {
            ManagerError::EnvError(format!("cannot finalize {}: {e}", dest.display()))
        })
    })();
    // On any failure the staged body is never promoted; drop it so it can't be
    // mistaken for a finished download (a no-op when rename already consumed it).
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// chmod 0755 (downloaded binaries arrive without the executable bit).
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| ManagerError::EnvError(format!("cannot stat {}: {e}", path.display())))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|e| ManagerError::EnvError(format!("cannot chmod {}: {e}", path.display())))
}

// Layout of a provisioned runtime store (`runtimes/<version>/<triple>/`), shared by the
// code that writes it (provision_runtime), copies it (stage_runtime), and
// consumes it (main.rs). One place to change if the store layout ever moves.

/// The prebuilt morloc compiler within a provisioned runtime store.
pub fn runtime_morloc_bin(dir: &Path) -> PathBuf {
    dir.join("morloc")
}

/// GitHub release download base for the mim repository (still named
/// `morloc-manager` on GitHub). mim IS
/// the in-environment dependency agent (`mim sync ...`), so a container image or a
/// cross-arch native env fetches the matching-platform `mim` from here.
const MIM_RELEASE_BASE: &str = "https://github.com/morloc-project/morloc-manager/releases/download";

/// The release tag whose `mim` binaries are coherent with THIS mim. Baked from the
/// actual CI ref (`GITHUB_REF_NAME`, via build.rs) so the URL targets the real
/// release rather than a reconstruction; falls back to `v<crate version>` for
/// local builds (where the fetch is only reached by a cross-arch build anyway).
pub fn mim_release_tag() -> String {
    option_env!("MIM_RELEASE_TAG")
        .map(str::to_string)
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")))
}

/// The `mim` binary within a runtime-bin dir (an in-container staging dir). The
/// container build hook (MORLOC_BUILD_HOOK) points at this in-image path.
pub fn runtime_mim_bin(dir: &Path) -> PathBuf {
    dir.join("mim")
}

/// The tag+triple-keyed cache for a downloaded foreign-platform `mim`. Keyed
/// by the release tag `download_mim` fetches from, so the cache holds exactly
/// what that tag served; keyed by triple so a host can cache both its own and
/// a container platform's binary.
pub fn mim_cache_bin(scope: Scope, triple: &str) -> PathBuf {
    config::data_dir(scope)
        .join("mim")
        .join(mim_release_tag())
        .join(triple)
        .join("mim")
}

/// The running mim executable. Shared by the native hook and the same-triple
/// container path: natively the agent runs on the host, so the running binary is
/// always the right platform AND coherent with the manager by construction.
fn running_mim() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| {
        ManagerError::EnvError(format!("cannot locate the running mim executable: {e}"))
    })
}

/// The `MORLOC_MIM_ENV` path override for a binary we are about to STAGE (copy),
/// if set. Dev environments point this at a locally-built mim (whose version need
/// not match any release); it also serves as the air-gap / escape hatch and, being
/// consulted first, always wins. Set-but-missing is a hard error, since staging
/// copies the file immediately -- a typo must not silently fall through to a
/// download. (The native-run hook uses [`mim_agent_for_host`], which does NOT
/// validate: there the override is only a value handed to the compiler.)
fn mim_env_override_for_staging() -> Result<Option<PathBuf>> {
    match std::env::var("MORLOC_MIM_ENV") {
        Ok(p) if !p.is_empty() => {
            let path = PathBuf::from(&p);
            if path.is_file() {
                Ok(Some(path))
            } else {
                Err(ManagerError::EnvError(format!(
                    "MORLOC_MIM_ENV is set to {p}, but that is not a file"
                )))
            }
        }
        _ => Ok(None),
    }
}

/// The in-environment dependency agent for a NATIVE env: the executable the build
/// hook names (`MORLOC_BUILD_HOOK`, run as `mim sync ...`). Returns the
/// `MORLOC_MIM_ENV` override verbatim if set, else the running mim.
///
/// This does NOT validate the override's existence: the hook is only a value
/// handed to the compiler, which validates it (and errors clearly) if and only if
/// `morloc make` actually invokes it. Validating here would hard-fail unrelated
/// native commands (`run -- ls`) that never provision dependencies. A bad override
/// is still surfaced -- by `doctor` and by `morloc make` itself.
pub fn mim_agent_for_host(_scope: Scope) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("MORLOC_MIM_ENV").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    running_mim()
}

/// Resolve an executable `mim` for the given release `triple`, to STAGE (copy) as
/// the in-environment dependency agent into a container image (the build hook runs
/// `mim sync ...`).
///
/// Precedence:
///   1. `MORLOC_MIM_ENV` override (see [`mim_env_override_for_staging`]).
///   2. host triple == `triple` -- the running mim IS the matching agent; return
///      it. This is every same-arch container (the common case, e.g. a Linux host
///      building a Linux image), and needs no network: coherence is definitional
///      (same binary).
///   3. foreign triple -- download the matching-platform `mim` from this mim's
///      release, verify it, cache it, and return the cached path. Reached only
///      when building an image for a different platform than the host (e.g. a
///      macOS-arm64 host building a Linux image).
pub fn mim_for_triple(trust: &Trust, scope: Scope, triple: &str) -> Result<PathBuf> {
    let over = mim_env_override_for_staging()?;
    let running = running_mim()?;
    match agent_source(over.as_deref(), triple, host_release_triple(), &running) {
        AgentSource::Override(p) if elf_is_static(p) == Some(false) => {
            return Err(ManagerError::EnvError(format!(
                "MORLOC_MIM_ENV names {}, which is dynamically linked and cannot run in the \
                 environment's container; point it at a static build \
                 (./scripts/build-static-container.sh)",
                p.display()
            )));
        }
        AgentSource::Override(p) => return Ok(p.to_path_buf()),
        AgentSource::Running => return Ok(running),
        AgentSource::Release => {}
    }
    let cached = mim_cache_bin(scope, triple);
    if cached.is_file() {
        return Ok(cached);
    }
    download_mim(trust, triple, &cached)?;
    Ok(cached)
}

/// PROV-7: the identity of the agent `mim_for_triple` stages for `triple`, for
/// the image cache key. A file is identified by size and mtime, as the compiler
/// is; a release agent by its release and platform.
pub fn agent_identity(triple: &str) -> Result<String> {
    Ok(agent_identity_for(
        mim_env_override_for_staging()?.as_deref(),
        triple,
        host_release_triple(),
        &running_mim()?,
    ))
}

fn agent_identity_for(over: Option<&Path>, triple: &str, host: Option<&str>, running: &Path) -> String {
    let source = agent_source(over, triple, host, running);
    let file = |p: &Path| {
        let m = std::fs::metadata(p).ok();
        let mtime = m
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        format!("{}:{}:{mtime}", p.display(), m.map_or(0, |m| m.len()))
    };
    match source {
        AgentSource::Override(p) => format!("override {}", file(p)),
        AgentSource::Running => format!("running {}", file(running)),
        AgentSource::Release => format!("release {}/{triple}", mim_release_tag()),
    }
}

/// Which mim a container of platform `triple` gets (PROV-8): the override,
/// else the running mim when it is built for that platform and statically
/// linked, else this version's release for `triple`.
#[derive(Debug, PartialEq, Eq)]
enum AgentSource<'a> {
    Override(&'a Path),
    Running,
    Release,
}

fn agent_source<'a>(over: Option<&'a Path>, triple: &str, host: Option<&str>, running: &Path) -> AgentSource<'a> {
    match over {
        Some(p) => AgentSource::Override(p),
        None if host == Some(triple) && elf_is_static(running) != Some(false) => AgentSource::Running,
        None => AgentSource::Release,
    }
}

/// Whether the 64-bit ELF at `path` is statically linked (has no `PT_INTERP`
/// program header). `None` when it is not a 64-bit ELF or cannot be read.
fn elf_is_static(path: &Path) -> Option<bool> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(..4)? != b"\x7fELF" || *bytes.get(4)? != 2 {
        return None;
    }
    let little = *bytes.get(5)? == 1;
    let num = |at: usize, n: usize| -> Option<u64> {
        let b = bytes.get(at..at + n)?;
        Some(b.iter().enumerate().fold(0u64, |acc, (i, &x)| {
            let shift = if little { i } else { n - 1 - i } * 8;
            acc | (u64::from(x) << shift)
        }))
    };
    let (phoff, phentsize, phnum) = (num(0x20, 8)? as usize, num(0x36, 2)? as usize, num(0x38, 2)? as usize);
    const PT_INTERP: u64 = 3;
    for i in 0..phnum {
        if num(phoff + i * phentsize, 4)? == PT_INTERP {
            return Some(false);
        }
    }
    Some(true)
}

/// Download `mim-<triple>` from this mim's release into `dest` atomically,
/// verifying its SHA-256 against the published `mim-<triple>.sha256` checksum file.
/// An absent checksum file is a hard error -- an unverifiable binary is never
/// installed. The body lands on a temporary `.part` path and is renamed into place
/// only after the download and verification succeed.
fn download_mim(trust: &Trust, triple: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ManagerError::EnvError(format!("cannot create {}: {e}", parent.display()))
        })?;
    }
    let tag = mim_release_tag();
    let asset = format!("mim-{triple}");
    let url = format!("{MIM_RELEASE_BASE}/{tag}/{asset}");
    let sha_url = format!("{url}.sha256");
    eprintln!("Fetching the dependency agent ({asset}, {tag})...");
    let part = part_path(dest);
    let result = (|| {
        curl_download(trust, &url, &part)?;
        // The checksum file is REQUIRED: never install an unverified binary. The
        // fetch can fail because the file is genuinely absent (a partial release)
        // OR for a transport reason (proxy/TLS/DNS); surface the underlying error
        // rather than asserting one cause.
        let sha_body = curl_capture(trust, &sha_url).map_err(|e| {
            ManagerError::EnvError(format!(
                "could not fetch the checksum file {asset}.sha256 from {sha_url} ({e}); \
                 refusing to install an unverified {asset}"
            ))
        })?;
        let expected = sha_body.split_whitespace().next().unwrap_or("").trim();
        if expected.is_empty() {
            return Err(ManagerError::EnvError(format!(
                "checksum file {asset}.sha256 is empty"
            )));
        }
        let actual = file_sha256(&part)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(ManagerError::EnvError(format!(
                "checksum mismatch for {asset}: expected {expected}, got {actual} \
                 (corrupt or tampered download)"
            )));
        }
        std::fs::rename(&part, dest).map_err(|e| {
            ManagerError::EnvError(format!("cannot finalize {}: {e}", dest.display()))
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result?;
    make_executable(dest)
}

/// Copy a `mim` agent binary into a runtime-bin dir (an in-container staging dir)
/// and set its exec bit, so an in-container `morloc make` runs it via
/// MORLOC_BUILD_HOOK. (`copy_file` follows a symlink, so a dev-staged symlink
/// copies the real binary in.)
pub fn stage_mim_agent(src: &Path, runtime_bin_dir: &Path) -> Result<()> {
    let dest = runtime_mim_bin(runtime_bin_dir);
    copy_file(src, &dest)?;
    make_executable(&dest)
}

/// The Rust workspace source within a provisioned runtime store; this is
/// `morloc init`'s MORLOC_RUNTIME_DIR.
pub fn runtime_src_dir(dir: &Path) -> PathBuf {
    dir.join("runtime")
}

/// Extract a .tar.gz into `dest` (the runtime-src tarball unpacks to `runtime/`).
/// `--no-same-owner`: extracted files belong to the current user, not the uids
/// recorded in the archive (restoring those fails for a non-root user, and for
/// root inside a user namespace where the uid is unmapped).
fn extract_tar_gz(tar: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("--no-same-owner")
        .arg("-xzf")
        .arg(tar)
        .arg("-C")
        .arg(dest)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| ManagerError::EnvError(format!("could not run tar: {e}")))?;
    if !status.success() {
        return Err(ManagerError::EnvError(format!(
            "tar extract failed: {}",
            tar.display()
        )));
    }
    Ok(())
}

/// The completion stamp of a provisioned runtime: its version, then the digest
/// of each asset it was made from.
fn stamp_text(manifest: &ReleaseManifest, assets: &TripleAssets) -> String {
    let digest = |a: &str| manifest.sha256.get(a).map_or("", String::as_str).to_ascii_lowercase();
    format!(
        "{}\n{} {}\n{} {}\n",
        manifest.version,
        assets.morloc,
        digest(&assets.morloc),
        manifest.runtime_src,
        digest(&manifest.runtime_src)
    )
}

/// PROV-4: a complete entry is used only for the release it was made from. A
/// stamp holding only a version predates the digests and cannot be compared.
fn check_stamp(found: &str, expected: &str, tag: &str, entry: &Path) -> Result<()> {
    if found.trim().lines().count() <= 1 || found == expected {
        return Ok(());
    }
    Err(ManagerError::EnvError(format!(
        "release {tag} was re-published with different assets than the runtime \
         provisioned from it in {}; a provisioned release is never modified. Use \
         another release, or remove that entry if no environment uses it.",
        entry.display()
    )))
}

/// Fetch and parse the release manifest attached to a GitHub `tag`. The
/// manifest declares the concrete version and the per-triple asset names.
pub fn fetch_manifest(trust: &Trust, tag: &str) -> Result<ReleaseManifest> {
    let text = curl_capture(trust, &asset_url(tag, MANIFEST_ASSET))?;
    ReleaseManifest::from_json(&text)
}

/// Provision the runtime for `triple` into `runtimes/<version>/<triple>/` and
/// return that directory (its `runtime/` subdir is `morloc init`'s MORLOC_RUNTIME_DIR). Downloads
/// the version-matched morloc compiler plus the rust sources; `morloc init`
/// builds libmorloc.so, morloc-nexus, and rustmorloc locally from that source.
/// The already-installed `mim` bootstrap is not re-fetched.
///
/// `triple` is the release triple the artifacts must target: the host's for a
/// native backend, but the CONTAINER's Linux triple for a container backend
/// (the compiler runs, and libmorloc is built, inside the image -- so on a macOS
/// host a Linux/arm64 container needs the linux-arm64 runtime, not macos-arm64).
pub fn provision_runtime(
    trust: &Trust,
    scope: Scope,
    requested: &str,
    triple: &str,
) -> Result<(PathBuf, String)> {
    // Resolve "latest"/a bare version to a concrete download tag (asset URLs need
    // the real tag, not the "latest" alias).
    let tag = resolve_tag(trust, requested)?;
    eprintln!("Provisioning morloc runtime from release {tag}...");
    let manifest = fetch_manifest(trust, &tag)?;
    let version = manifest.version.clone();
    let assets = manifest.assets_for(triple).ok_or_else(|| {
        ManagerError::BackendUnsupported(format!(
            "release {tag} publishes no runtime for {triple}"
        ))
    })?;

    let dir = runtimes_dir(scope, &version, triple);
    // A completion stamp -- written only after every artifact is downloaded,
    // verified, and extracted -- is the idempotency gate. Gating on mere file
    // presence would bless a truncated or half-extracted runtime from an
    // interrupted prior run; the stamp is absent until provisioning fully
    // succeeds, so a crash always re-provisions.
    let stamp = dir.join(".provisioned");
    let expected = stamp_text(&manifest, assets);
    if let Ok(found) = std::fs::read_to_string(&stamp) {
        check_stamp(&found, &expected, &tag, &dir)?;
    } else {
        std::fs::create_dir_all(&dir)
            .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", dir.display())))?;
        // Ship only the prebuilt morloc COMPILER (it never links into a pool) and
        // the Rust SOURCE. libmorloc.so + morloc-nexus are NOT prebuilt: they are
        // built from this source by `morloc init` with the env's own toolchain,
        // so the runtime that pools link/load is ABI-coherent with the toolchain
        // that compiles those pools.
        let morloc_dest = runtime_morloc_bin(&dir);
        download_asset(trust, &tag, &assets.morloc, &morloc_dest, &manifest.sha256)?;
        make_executable(&morloc_dest)?;

        // Wipe any stale runtime/ from an interrupted prior extraction so a
        // re-provision never mixes old and new files.
        let src_tar = dir.join(&manifest.runtime_src);
        download_asset(trust, &tag, &manifest.runtime_src, &src_tar, &manifest.sha256)?;
        let _ = std::fs::remove_dir_all(runtime_src_dir(&dir));
        extract_tar_gz(&src_tar, &dir)?;
        let _ = std::fs::remove_file(&src_tar);

        std::fs::write(&stamp, &expected).map_err(|e| {
            ManagerError::EnvError(format!("cannot write {}: {e}", stamp.display()))
        })?;
    }

    // The lang-support table lets the manager render pixi manifests without
    // running the compiler on the host (essential where the glibc compiler can't
    // execute, e.g. NixOS/musl). Best-effort: older releases lack this asset.
    // Older releases lack the asset and publish no digest for it; a release
    // that publishes one must deliver it intact (PROV-2).
    let ls = dir.join(LANG_SUPPORT_FILE);
    if !ls.exists() && manifest.sha256.contains_key(LANG_SUPPORT_ASSET) {
        download_asset(trust, &tag, LANG_SUPPORT_ASSET, &ls, &manifest.sha256)?;
    }

    Ok((dir, version))
}

// ======================================================================
// pixi provisioning (self-contained conda solver; no host install)
// ======================================================================

/// pixi's release target-triple for an (os, arch) pair. pixi ships static musl
/// on Linux, so one binary runs on any glibc/musl/NixOS host.
fn pixi_triple(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// Ensure a pinned pixi binary is available and return its path. `$MORLOC_PIXI`
/// overrides (dev/testing); otherwise a per-user copy is downloaded once into
/// `<data_dir>/bin/pixi` and reused. This makes the conda solve self-contained --
/// the host does not need pixi installed.
/// The file beside the host pixi naming its version.
const PIXI_VERSION_STAMP: &str = ".pixi-version";

/// Whether `bin_dir` holds the pinned pixi: the host lock and an image's
/// `pixi install --locked` must run one version.
fn host_pixi_current(bin_dir: &Path) -> bool {
    bin_dir.join("pixi").is_file()
        && std::fs::read_to_string(bin_dir.join(PIXI_VERSION_STAMP)).is_ok_and(|v| v.trim() == PIXI_VERSION)
}

pub fn provision_pixi(trust: &Trust, scope: Scope) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("MORLOC_PIXI") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let bin_dir = config::data_dir(scope).join("bin");
    let dest = bin_dir.join("pixi");
    if host_pixi_current(&bin_dir) {
        return Ok(dest);
    }
    let triple = pixi_triple(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        ManagerError::BackendUnsupported(format!(
            "no pinned pixi is available for this platform ({}/{})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", bin_dir.display())))?;
    eprintln!("Provisioning pixi {PIXI_VERSION}...");
    // pixi ships as a .tar.gz that unpacks to a bare `pixi` binary. Download and
    // unpack in a staging dir, then atomically rename the finished binary into
    // place, so an interrupted fetch/extract never leaves a truncated `pixi` that
    // the `dest.is_file()` gate above would treat as ready.
    let url = format!(
        "https://github.com/prefix-dev/pixi/releases/download/v{PIXI_VERSION}/pixi-{triple}.tar.gz"
    );
    let stage = bin_dir.join(format!(".pixi-stage.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    let result = (|| {
        std::fs::create_dir_all(&stage).map_err(|e| {
            ManagerError::EnvError(format!("cannot create {}: {e}", stage.display()))
        })?;
        let tar = stage.join("pixi.tar.gz");
        curl_download(trust, &url, &tar)?;
        extract_tar_gz(&tar, &stage)?;
        let extracted = stage.join("pixi");
        if !extracted.is_file() {
            return Err(ManagerError::EnvError(format!(
                "pixi binary not found at {} after extraction",
                extracted.display()
            )));
        }
        make_executable(&extracted)?;
        std::fs::rename(&extracted, &dest).map_err(|e| {
            ManagerError::EnvError(format!("cannot finalize {}: {e}", dest.display()))
        })?;
        std::fs::write(bin_dir.join(PIXI_VERSION_STAMP), PIXI_VERSION).map_err(|e| {
            ManagerError::EnvError(format!("cannot record the pixi version in {}: {e}", bin_dir.display()))
        })
    })();
    // Drop the staging dir on every path (success or any early `?` failure).
    let _ = std::fs::remove_dir_all(&stage);
    result.map(|()| dest)
}

// ======================================================================
// Staging a provisioned runtime into a container build context
// ======================================================================

/// Copy a file, mapping IO errors to a legible provisioning error.
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst).map(|_| ()).map_err(|e| {
        ManagerError::EnvError(format!(
            "cannot copy {} -> {}: {e}",
            src.display(),
            dst.display()
        ))
    })
}

/// Recursively copy `src` into `dst`, skipping any directory whose name is in
/// `skip` (e.g. `target` build artifacts). A symlink to a file is always followed
/// and copied as content. A symlink to a DIRECTORY is skipped by default (a
/// cycle/escape hazard, e.g. a dotfiles dir that symlinks to `$HOME`); pass
/// `follow_symlinked_dirs = true` to instead recurse into its canonical target
/// (with a visited-set cycle guard) -- used when copying a vendored local
/// dependency, whose subpackages may legitimately be symlinks. Existing files are
/// overwritten (`fs::copy` semantics).
pub fn copy_dir_excluding(
    src: &Path,
    dst: &Path,
    skip: &[&str],
    follow_symlinked_dirs: bool,
) -> Result<()> {
    copy_dir_excluding_at_root(src, dst, &[], skip, follow_symlinked_dirs)
}

/// `copy_dir_excluding`, also leaving out the directories named in
/// `root_skip` directly under `src` (and only there).
pub fn copy_dir_excluding_at_root(
    src: &Path,
    dst: &Path,
    root_skip: &[&str],
    skip: &[&str],
    follow_symlinked_dirs: bool,
) -> Result<()> {
    let mut visited = std::collections::HashSet::new();
    copy_dir_inner(src, dst, root_skip, skip, follow_symlinked_dirs, &mut visited)
}

fn copy_dir_inner(
    src: &Path,
    dst: &Path,
    root_skip: &[&str],
    skip: &[&str],
    follow_symlinked_dirs: bool,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Result<()> {
    std::fs::create_dir_all(dst)
        .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", dst.display())))?;
    let entries = std::fs::read_dir(src)
        .map_err(|e| ManagerError::EnvError(format!("cannot read {}: {e}", src.display())))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let name = entry.file_name();
        let to = dst.join(&name);
        let skipped = |list: &[&str]| list.iter().any(|s| name.to_str() == Some(s));
        // file_type() does NOT follow the link, so we can tell a symlink apart
        // from a real directory before deciding whether to recurse.
        let is_symlink = entry
            .file_type()
            .map(|t| t.is_symlink())
            .unwrap_or(false);
        if is_symlink {
            if from.is_dir() {
                if !follow_symlinked_dirs {
                    continue;
                }
                // Recurse into the canonical target unless it is a directory
                // this copy is inside of (a cycle); `visited` holds the targets
                // of the links on the current path only.
                let canon = std::fs::canonicalize(&from).map_err(|e| {
                    ManagerError::EnvError(format!("cannot resolve symlink {}: {e}", from.display()))
                })?;
                if skipped(root_skip) {
                    continue;
                }
                let src_canon = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
                if !src_canon.starts_with(&canon) && visited.insert(canon.clone()) {
                    let copied = copy_dir_inner(&canon, &to, &[], skip, follow_symlinked_dirs, visited);
                    visited.remove(&canon);
                    copied?;
                }
            } else {
                // A symlink to a file: copy its target content (fs::copy follows).
                copy_file(&from, &to)?;
            }
        } else if from.is_dir() {
            if skipped(skip) || skipped(root_skip) {
                continue;
            }
            copy_dir_inner(&from, &to, &[], skip, follow_symlinked_dirs, visited)?;
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

/// Stage a provisioned runtime (`runtimes/<version>/<triple>/`, from `provision_runtime`)
/// into a container build context's `runtime/` directory: the morloc compiler
/// plus the Rust workspace source. libmorloc.so + morloc-nexus are built from
/// that source in-image by `morloc init`, so they are not staged. Everything
/// comes from the downloaded runtime -- no host morloc install is required.
pub fn stage_runtime(trust: &Trust, scope: Scope, triple: &str, src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .map_err(|e| ManagerError::EnvError(format!("cannot create {}: {e}", dest.display())))?;
    let from = runtime_morloc_bin(src);
    if !from.exists() {
        return Err(ManagerError::EnvError(format!(
            "provisioned runtime is missing morloc at {}",
            from.display()
        )));
    }
    let morloc_dest = runtime_morloc_bin(dest);
    copy_file(&from, &morloc_dest)?;
    make_executable(&morloc_dest)?;
    // Stage the mim binary into the image alongside the compiler as the
    // in-environment dependency agent: an in-container `morloc make` runs it via
    // MORLOC_BUILD_HOOK (`mim sync ...`). The staged binary must match the
    // CONTAINER's platform, not the host's, so a macOS host building a Linux image
    // stages the Linux mim -- `mim_for_triple` returns the running mim when the
    // triples match and downloads the matching release binary otherwise.
    let agent = mim_for_triple(trust, scope, triple)?;
    stage_mim_agent(&agent, dest)?;
    let rust = runtime_src_dir(src);
    if !rust.is_dir() {
        return Err(ManagerError::EnvError(format!(
            "provisioned runtime is missing the rust source at {}",
            rust.display()
        )));
    }
    copy_dir_excluding(&rust, &runtime_src_dir(dest), &["target"], false)?;
    Ok(())
}

#[cfg(test)]
mod tests {

    /// PROV-9: a bundle replaces curl's store with exactly that file; the
    /// system trust passes nothing.
    #[test]
    fn a_download_carries_the_bundle_it_trusts() {
        let args = |t: &Trust| -> Vec<String> {
            curl_base(t).get_args().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let bundle = args(&Trust::Bundle(PathBuf::from("/c/host.pem")));
        assert!(bundle.windows(2).any(|w| w[0] == "--cacert" && w[1] == "/c/host.pem"), "{bundle:?}");
        assert!(!args(&Trust::System).iter().any(|a| a == "--cacert"));
    }

    /// PROV-9: a download for no environment honours the user's bundle
    /// variables, `CURL_CA_BUNDLE` first.
    #[test]
    fn a_download_for_no_environment_honours_the_users_bundle() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| std::ffi::OsString::from(v))
        };
        assert_eq!(Trust::from_vars(vars(&[])), Trust::System);
        assert_eq!(Trust::from_vars(vars(&[("CURL_CA_BUNDLE", "")])), Trust::System);
        assert_eq!(Trust::from_vars(vars(&[("SSL_CERT_FILE", "/s")])), Trust::Bundle("/s".into()));
        assert_eq!(
            Trust::from_vars(vars(&[("SSL_CERT_FILE", "/s"), ("CURL_CA_BUNDLE", "/c")])),
            Trust::Bundle("/c".into())
        );
    }

    /// PROV-9: an environment's download trusts its materialized host bundle.
    #[test]
    fn a_download_for_an_environment_trusts_its_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        config::test_roots::set(tmp.path());
        let host = crate::cert::host_bundle_path(Scope::Local, "e");
        std::fs::create_dir_all(host.parent().unwrap()).unwrap();
        std::fs::write(&host, "x").unwrap();
        assert_eq!(Trust::for_env(Scope::Local, "e"), Trust::Bundle(host));
    }

    /// PROV-9: mim runs curl in one place, so no download can skip its trust.
    #[test]
    fn curl_is_built_only_by_curl_base() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let mut offenders = Vec::new();
        let mut dirs = vec![root.join("mim/src"), root.join("morloc-deps/src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let body = text.find("\nmod tests {").map_or(&text[..], |i| &text[..i]);
                let n = body.matches(&format!("Command::new({:?})", "curl")).count();
                let allowed = usize::from(path.ends_with("mim/src/provision.rs"));
                if n != allowed {
                    offenders.push(format!("{}: {n}", path.display()));
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:?}");
    }
    use super::*;

    /// Two links to one directory each get its content; only a link back into
    /// a directory being copied (a cycle) is skipped.
    #[test]
    fn every_link_to_a_shared_directory_is_copied() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("shared")).unwrap();
        std::fs::write(src.join("shared/x"), "x").unwrap();
        for l in ["a", "b"] {
            std::os::unix::fs::symlink(src.join("shared"), src.join(l)).unwrap();
        }
        std::os::unix::fs::symlink(&src, src.join("shared/up")).unwrap();
        let dst = d.path().join("dst");
        copy_dir_excluding(&src, &dst, &[], true).unwrap();
        for l in ["a/x", "b/x", "shared/x"] {
            assert!(dst.join(l).is_file(), "{l} missing");
        }
        assert!(!dst.join("shared/up").exists());
    }

    /// The host pixi is the pinned version: one from an older mim, or with no
    /// version recorded, is fetched again.
    #[test]
    fn a_host_pixi_of_another_version_is_not_reused() {
        let d = tempfile::tempdir().unwrap();
        assert!(!host_pixi_current(d.path()));
        std::fs::write(d.path().join("pixi"), "bin").unwrap();
        assert!(!host_pixi_current(d.path()));
        std::fs::write(d.path().join(PIXI_VERSION_STAMP), "0.0.1").unwrap();
        assert!(!host_pixi_current(d.path()));
        std::fs::write(d.path().join(PIXI_VERSION_STAMP), PIXI_VERSION).unwrap();
        assert!(host_pixi_current(d.path()));
    }

    /// PROV-4: a provisioned entry records the digests it was made from, so a
    /// release re-published under the same version is refused, not served
    /// from the stale entry.
    #[test]
    fn a_republished_release_is_detected() {
        let m = ReleaseManifest::from_json(SAMPLE).unwrap();
        let assets = m.assets_for("linux-x86_64").unwrap();
        let stamp = stamp_text(&m, assets);
        assert!(check_stamp(&stamp, &stamp, "v0.98.3", Path::new("/r")).is_ok());
        assert!(check_stamp("0.98.3", &stamp, "v0.98.3", Path::new("/r")).is_ok());
        let mut moved = m.clone();
        moved.sha256.insert(assets.morloc.clone(), "ff".to_string());
        let err = check_stamp(&stamp, &stamp_text(&moved, assets), "v0.98.3", Path::new("/r")).unwrap_err();
        assert!(err.to_string().contains("re-published"), "{err}");
    }

    /// PROV-8: a dynamically linked mim never reaches a container: the release
    /// of this version stands in for the running one.
    #[test]
    fn a_dynamic_mim_is_not_staged_into_a_container() {
        let elf = |interp: bool| {
            let mut b = vec![0u8; 0x40 + 56];
            b[..4].copy_from_slice(b"\x7fELF");
            b[4] = 2;
            b[5] = 1;
            b[0x20] = 0x40;
            b[0x36] = 56;
            b[0x38] = 1;
            b[0x40] = if interp { 3 } else { 1 };
            b
        };
        let d = tempfile::tempdir().unwrap();
        let (dynamic, fixed) = (d.path().join("dyn"), d.path().join("static"));
        std::fs::write(&dynamic, elf(true)).unwrap();
        std::fs::write(&fixed, elf(false)).unwrap();
        assert_eq!(elf_is_static(&dynamic), Some(false));
        assert_eq!(elf_is_static(&fixed), Some(true));
        assert_eq!(elf_is_static(&d.path().join("missing")), None);
        let t = "linux-x86_64";
        assert_eq!(agent_source(None, t, Some(t), &dynamic), AgentSource::Release);
        assert_eq!(agent_source(None, t, Some(t), &fixed), AgentSource::Running);
        assert_eq!(agent_source(Some(&dynamic), t, Some(t), &fixed), AgentSource::Override(&dynamic));
    }

    /// PROV-7, PROV-8: the image key names the agent it bakes, so a new mim, a
    /// rebuilt override or another release's agent rebuilds the image.
    #[test]
    fn the_agent_identity_follows_the_staged_binary() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("mim");
        std::fs::write(&bin, "v1").unwrap();
        let running = agent_identity_for(None, "linux-x86_64", Some("linux-x86_64"), &bin);
        let over = agent_identity_for(Some(&bin), "linux-x86_64", Some("linux-x86_64"), &bin);
        let foreign = agent_identity_for(None, "linux-aarch64", Some("linux-x86_64"), &bin);
        assert_ne!(running, over);
        assert!(foreign.contains(&mim_release_tag()) && foreign.contains("linux-aarch64"), "{foreign}");
        std::fs::write(&bin, "v2, longer").unwrap();
        assert_ne!(agent_identity_for(None, "linux-x86_64", Some("linux-x86_64"), &bin), running);
        assert_ne!(agent_identity_for(Some(&bin), "linux-x86_64", None, &bin), over);
    }

    const SAMPLE: &str = r#"{
      "schema": 2,
      "version": "0.98.3",
      "runtime_src": "morloc-runtime-src.tar.gz",
      "versions": {
        "morloc_version": "0.98.3",
        "abi_version": 1,
        "envspec_version": 2,
        "lang_support_schema": "1.0"
      },
      "triples": {
        "linux-x86_64": {
          "morloc": "morloc-linux-x86_64"
        }
      },
      "sha256": {
        "morloc-linux-x86_64": "aa",
        "morloc-runtime-src.tar.gz": "cc"
      }
    }"#;

    /// PROV-3: a contract version this mim cannot read, older or newer, is
    /// refused before anything is installed, naming the fix.
    #[test]
    fn the_gate_refuses_every_contract_version_the_parsers_reject() {
        let v = |envspec: u32, lang: &str| MorlocVersions {
            morloc_version: "0.99.0".to_string(),
            abi_version: 1,
            envspec_version: envspec,
            lang_support_schema: lang.to_string(),
        };
        let cur = ENVSPEC_VERSION;
        let major = LANG_SCHEMA_MAJOR;
        assert!(v(cur, &format!("{major}.0")).check_supported().is_ok());
        assert!(v(cur, &format!("{major}.99")).check_supported().is_ok());
        assert!(v(ENVSPEC_MIN_VERSION, &format!("{major}.0")).check_supported().is_ok());
        let older = v(ENVSPEC_MIN_VERSION - 1, &format!("{major}.0")).check_supported().unwrap_err().to_string();
        assert!(older.contains("newer morloc release"), "{older}");
        let newer = v(cur + 1, &format!("{major}.0")).check_supported().unwrap_err().to_string();
        assert!(newer.contains("upgrade mim"), "{newer}");
        let lang_old = v(cur, &format!("{}.5", major - 1)).check_supported().unwrap_err().to_string();
        assert!(lang_old.contains("newer morloc release"), "{lang_old}");
        let lang_new = v(cur, &format!("{}.0", major + 1)).check_supported().unwrap_err().to_string();
        assert!(lang_new.contains("upgrade mim"), "{lang_new}");
    }

    /// The foreign-platform mim cache is keyed by the tag it is downloaded from.
    #[test]
    fn the_mim_cache_is_keyed_by_the_download_tag() {
        let p = mim_cache_bin(Scope::Local, "linux-arm64");
        assert!(p.components().any(|c| c.as_os_str() == mim_release_tag().as_str()), "{}", p.display());
    }

    #[test]
    fn parses_manifest() {
        let m = ReleaseManifest::from_json(SAMPLE).unwrap();
        assert_eq!(m.version, "0.98.3");
        assert_eq!(m.runtime_src, "morloc-runtime-src.tar.gz");
        let a = m.assets_for("linux-x86_64").unwrap();
        assert_eq!(a.morloc, "morloc-linux-x86_64");
        assert_eq!(m.versions.envspec_version, 2);
        assert_eq!(m.versions.lang_support_schema, "1.0");
        // A triple with no published runtime is absent.
        assert!(m.assets_for("macos-arm64").is_none());
    }

    /// PROV-3: a manifest schema other than this mim's is refused by name,
    /// with the direction of the fix, before its fields are read.
    #[test]
    fn a_manifest_schema_other_than_this_mims_is_refused() {
        let older = r#"{"schema":1,"version":"0.112.0","rust_src":"morloc-rust-src.tar.gz",
          "versions":{"morloc_version":"0.112.0","abi_version":1,"envspec_version":2,"lang_support_schema":"1.0"},
          "triples":{},"sha256":{}}"#;
        let e = ReleaseManifest::from_json(older).unwrap_err().to_string();
        assert!(e.contains("schema 1 is older") && e.contains("newer morloc release"), "{e}");
        let newer = r#"{"schema":3}"#;
        let e = ReleaseManifest::from_json(newer).unwrap_err().to_string();
        assert!(e.contains("schema 3 is newer") && e.contains("upgrade mim"), "{e}");
    }

    #[test]
    fn rejects_future_schema() {
        let j = r#"{"schema":999,"version":"9.9.9","runtime_src":"x",
          "versions":{"morloc_version":"9","abi_version":1,"envspec_version":2,"lang_support_schema":"1.0"},
          "triples":{},"sha256":{}}"#;
        assert!(ReleaseManifest::from_json(j).is_err());
    }

    #[test]
    fn rejects_incompatible_contract_versions() {
        // A release whose envspec_version exceeds what this mim supports is refused
        // at manifest-parse (install) time with an actionable message.
        let j = r#"{"schema":2,"version":"9.9.9","runtime_src":"x",
          "versions":{"morloc_version":"9","abi_version":1,"envspec_version":4,"lang_support_schema":"1.0"},
          "triples":{},"sha256":{}}"#;
        assert!(ReleaseManifest::from_json(j).is_err());
        // A newer lang-support MAJOR is likewise refused.
        let k = r#"{"schema":2,"version":"9.9.9","runtime_src":"x",
          "versions":{"morloc_version":"9","abi_version":1,"envspec_version":2,"lang_support_schema":"2.0"},
          "triples":{},"sha256":{}}"#;
        assert!(ReleaseManifest::from_json(k).is_err());
    }

    #[test]
    fn triple_mapping() {
        assert_eq!(release_triple("linux", "x86_64"), Some("linux-x86_64"));
        assert_eq!(release_triple("linux", "aarch64"), Some("linux-arm64"));
        assert_eq!(release_triple("macos", "aarch64"), Some("macos-arm64"));
        // Intel macOS and Windows have no native artifacts.
        assert_eq!(release_triple("macos", "x86_64"), None);
        assert_eq!(release_triple("windows", "x86_64"), None);
    }

    #[test]
    fn asset_urls_use_the_tag() {
        assert_eq!(
            asset_url("v0.98.3", "morloc-linux-x86_64"),
            "https://github.com/morloc-project/morloc/releases/download/v0.98.3/morloc-linux-x86_64"
        );
        // A channel tag (e.g. "dev") is used verbatim.
        assert_eq!(
            asset_url("dev", "morloc-release-manifest.json"),
            "https://github.com/morloc-project/morloc/releases/download/dev/morloc-release-manifest.json"
        );
    }

    #[test]
    fn resolve_tag_normalizes_versions_and_passes_channels() {
        assert_eq!(resolve_tag(&Trust::System, "0.98.3").unwrap(), "v0.98.3");
        assert_eq!(resolve_tag(&Trust::System, "v0.98.3").unwrap(), "v0.98.3");
        assert_eq!(resolve_tag(&Trust::System, "dev").unwrap(), "dev");
    }

    #[test]
    fn runtimes_dir_is_keyed_on_version_and_triple() {
        let d = runtimes_dir(Scope::Local, "0.98.3", "linux-x86_64");
        assert!(d.ends_with("morloc/runtimes/0.98.3/linux-x86_64"));
        // Two triples of one version never share a compiler binary.
        assert_ne!(d, runtimes_dir(Scope::Local, "0.98.3", "linux-arm64"));
    }

    #[test]
    fn sha256_matches_known_vectors() {
        let dir = std::env::temp_dir().join(format!("morloc-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            file_sha256(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let abc = dir.join("abc");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            file_sha256(&abc).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_tag_from_latest_redirect() {
        assert_eq!(
            parse_tag_from_release_url(
                "https://github.com/morloc-project/morloc/releases/tag/v0.99.0"
            )
            .as_deref(),
            Some("v0.99.0")
        );
        // A trailing slash on the redirect target is tolerated.
        assert_eq!(
            parse_tag_from_release_url(
                "https://github.com/morloc-project/morloc/releases/tag/dev/"
            )
            .as_deref(),
            Some("dev")
        );
        // A URL that never reached a /tag/ page (e.g. no releases) yields None.
        assert_eq!(
            parse_tag_from_release_url("https://github.com/morloc-project/morloc/releases"),
            None
        );
    }

    #[test]
    fn manifest_requires_sha256() {
        let vers = r#""versions":{"morloc_version":"1","abi_version":1,"envspec_version":2,"lang_support_schema":"1.0"}"#;
        let j = format!(
            r#"{{"schema":2,"version":"1.0.0","runtime_src":"r",{vers},"triples":{{}},
          "sha256":{{"morloc-linux-x86_64":"abc123"}}}}"#
        );
        let m = ReleaseManifest::from_json(&j).unwrap();
        assert_eq!(
            m.sha256.get("morloc-linux-x86_64").map(|s| s.as_str()),
            Some("abc123")
        );
        // The digest map is mandatory: a manifest omitting it is refused rather
        // than silently installing unverified downloads.
        let no_digests =
            format!(r#"{{"schema":2,"version":"1.0.0","runtime_src":"r",{vers},"triples":{{}}}}"#);
        assert!(ReleaseManifest::from_json(&no_digests).is_err());
    }

    #[test]
    fn mim_env_override_precedence() {
        // All MORLOC_MIM_ENV assertions live in ONE test: it mutates the process
        // env, and no other test reads that var, so keeping them serial here avoids
        // the parallel-test env race. The original value is saved and restored.
        let key = "MORLOC_MIM_ENV";
        let saved = std::env::var_os(key);

        // Unset: no staging override; the native hook falls back to the running
        // exe; and the same-triple staging path returns the running exe with no
        // download.
        std::env::remove_var(key);
        assert!(mim_env_override_for_staging().unwrap().is_none());
        assert_eq!(mim_agent_for_host(Scope::Local).unwrap(), running_mim().unwrap());
        if let Some(host) = host_release_triple() {
            let running = running_mim().unwrap();
            let want = if elf_is_static(&running) == Some(false) { AgentSource::Release } else { AgentSource::Running };
            assert_eq!(agent_source(None, host, Some(host), &running), want);
        }

        // Empty string is treated as unset.
        std::env::set_var(key, "");
        assert!(mim_env_override_for_staging().unwrap().is_none());

        // Set to an existing file: staging returns it; the hook returns it verbatim.
        let present = std::env::temp_dir().join(format!("mim-override-present-{}", std::process::id()));
        std::fs::write(&present, b"x").unwrap();
        std::env::set_var(key, &present);
        assert_eq!(mim_env_override_for_staging().unwrap().as_deref(), Some(present.as_path()));
        assert_eq!(mim_agent_for_host(Scope::Local).unwrap(), present);
        let _ = std::fs::remove_file(&present);

        // Set to a MISSING path: staging is a hard error (it copies immediately),
        // but the native hook is lenient -- it returns the path verbatim, leaving
        // validation to the compiler when `morloc make` actually invokes the hook.
        let missing =
            std::env::temp_dir().join(format!("mim-override-missing-{}", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        std::env::set_var(key, &missing);
        assert!(mim_env_override_for_staging().is_err());
        assert_eq!(mim_agent_for_host(Scope::Local).unwrap(), missing);

        match saved {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
