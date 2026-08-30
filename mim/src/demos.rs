//! `mim demos` -- fetch curated example programs (the morloc-dungeon) for a
//! morloc version.
//!
//! The dungeon-master admin tool publishes, per morloc version, one GitHub
//! release `demos-<version>` on morloc-dungeon/dungeon-master carrying two
//! assets: `demos.tar.gz` (the passing demos' sources, laid out as
//! `demos/<repo>/...`) and `manifest.tsv` (one `repo<TAB>hash<TAB>tags<TAB>
//! synopsis` row per demo). Because the bundle is produced by honest builds
//! with the compiler gate in force, every demo in it is known to build and pass
//! on that version -- so this path needs no knowledge of `morloc-version`
//! constraints and no gate overrides.
//!
//! Discovery avoids the GitHub REST API (its unauthenticated 60 req/hr cap is
//! unreliable behind shared IPs), using predictable asset URLs and the
//! `releases/latest` redirect, exactly as the compiler-release path does.

use std::process::Command;

use crate::environment;
use crate::error::{ManagerError, Result};
use crate::provision::{
    curl_capture, curl_capture_quiet, curl_download, curl_effective_url, parse_tag_from_release_url,
};

/// Release base for the dungeon. Override with `MORLOC_DEMOS_BASE` to test
/// against a fork/mirror.
const DEFAULT_DEMOS_BASE: &str =
    "https://github.com/morloc-dungeon/dungeon-master/releases";

fn demos_base() -> String {
    std::env::var("MORLOC_DEMOS_BASE").unwrap_or_else(|_| DEFAULT_DEMOS_BASE.to_string())
}

fn asset_url(base: &str, version: &str, asset: &str) -> String {
    format!("{base}/download/demos-{version}/{asset}")
}

/// One demo, parsed from a manifest row.
struct Demo {
    repo: String,
    hash: String,
    tags: Vec<String>,
    synopsis: String,
}

/// Parse a manifest.tsv body into demos. Comment/blank lines are skipped;
/// missing trailing columns default to empty.
fn parse_manifest(text: &str) -> Vec<Demo> {
    let mut demos = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let repo = it.next().unwrap_or("").to_string();
        if repo.is_empty() {
            continue;
        }
        let hash = it.next().unwrap_or("").to_string();
        let tags_raw = it.next().unwrap_or("");
        let synopsis = it.next().unwrap_or("").to_string();
        let tags = if tags_raw.is_empty() {
            Vec::new()
        } else {
            tags_raw.split(',').map(|t| t.trim().to_string()).collect()
        };
        demos.push(Demo { repo, hash, tags, synopsis });
    }
    demos
}

/// Strip a leading `v` so an env/CLI/`releases/latest` tag becomes a bare
/// semver matching the dungeon's `demos-<version>` scheme.
fn bare_version(s: &str) -> String {
    s.trim().strip_prefix('v').unwrap_or(s.trim()).to_string()
}

/// Determine the target morloc version and whether it was requested explicitly.
/// Explicit (`--morloc-version`) is a contract: a missing release is a hard
/// error. Otherwise (active env, or the latest release) a missing release falls
/// back to the most recent published dungeon release.
fn resolve_target(morloc_version: Option<&str>) -> Result<(String, bool)> {
    if let Some(v) = morloc_version {
        return Ok((bare_version(v), true));
    }
    // An active default environment pins the version.
    if let Ok((_, _, ec)) = environment::resolve_default_environment() {
        if let Some(v) = ec.morloc_version {
            return Ok((v.show(), false));
        }
    }
    // Not in an environment: default to the latest morloc release.
    let tag = crate::provision::fetch_latest_tag()?;
    Ok((bare_version(&tag), false))
}

/// The most recent published dungeon release version, via the `releases/latest`
/// redirect (no REST API).
fn latest_dungeon_version(base: &str) -> Result<String> {
    let final_url = curl_effective_url(&format!("{base}/latest"))?;
    let tag = parse_tag_from_release_url(&final_url).ok_or_else(|| {
        ManagerError::EnvError(format!(
            "could not determine the latest dungeon release from {final_url}"
        ))
    })?;
    // Tag is `demos-<version>`; recover the version.
    tag.strip_prefix("demos-")
        .map(|v| v.to_string())
        .ok_or_else(|| ManagerError::EnvError(format!("unexpected dungeon release tag '{tag}'")))
}

/// Fetch the manifest for the target version, applying the explicit/fallback
/// policy. Returns the resolved version (which may differ from the target on a
/// fallback) and the manifest body.
fn fetch_manifest(base: &str, target: &str, explicit: bool) -> Result<(String, String)> {
    // A miss here is expected (fallback) or reported cleanly (explicit), so
    // probe quietly -- curl's own 404 line would otherwise precede our message.
    match curl_capture_quiet(&asset_url(base, target, "manifest.tsv")) {
        Ok(body) => Ok((target.to_string(), body)),
        Err(_) if explicit => Err(ManagerError::EnvError(format!(
            "no demos published for morloc {target}"
        ))),
        Err(_) => {
            // Fall back to the most recent published dungeon release.
            let latest = latest_dungeon_version(base).map_err(|_| {
                ManagerError::EnvError(
                    "no demos are published yet".to_string(),
                )
            })?;
            let body = curl_capture(&asset_url(base, &latest, "manifest.tsv"))?;
            eprintln!(
                "warning: no demos published for morloc {target}; using the most recent ({latest})"
            );
            Ok((latest, body))
        }
    }
}

/// Filter demos by an optional tag, erroring if the tag matches nothing.
fn select<'a>(demos: &'a [Demo], tag: Option<&str>, version: &str) -> Result<Vec<&'a Demo>> {
    let selected: Vec<&Demo> = match tag {
        None => demos.iter().collect(),
        Some(t) => demos.iter().filter(|d| d.tags.iter().any(|x| x == t)).collect(),
    };
    if let Some(t) = tag {
        if selected.is_empty() {
            return Err(ManagerError::EnvError(format!(
                "no demos tagged '{t}' in the morloc {version} release"
            )));
        }
    }
    Ok(selected)
}

/// Print the demos: repo, first 8 hex of the source commit (enough to cite in an
/// issue report), tags, and the one-line synopsis.
fn print_list(version: &str, tag: Option<&str>, demos: &[&Demo]) {
    match tag {
        Some(t) => println!("demos for morloc {version} (tag: {t})"),
        None => println!("demos for morloc {version}"),
    }
    let repo_w = demos.iter().map(|d| d.repo.len()).max().unwrap_or(0);
    let tags_w = demos
        .iter()
        .map(|d| d.tags.join(",").len())
        .max()
        .unwrap_or(0);
    for d in demos {
        let short = d.hash.get(..8).unwrap_or(&d.hash);
        let tags = d.tags.join(",");
        println!(
            "  {:<repo_w$}  {}  {:<tags_w$}  {}",
            d.repo, short, tags, d.synopsis,
            repo_w = repo_w, tags_w = tags_w,
        );
    }
}

/// Download the bundle and extract the selected demos into
/// `examples-<tag>-<version>/` in the current directory.
fn pull(base: &str, version: &str, tag: Option<&str>, demos: &[&Demo], force: bool) -> Result<()> {
    let dir_tag = tag.unwrap_or("all");
    let dest = std::path::PathBuf::from(format!("examples-{dir_tag}-{version}"));
    if dest.exists() {
        if !force {
            return Err(ManagerError::EnvError(format!(
                "{}/ already exists (pass --force to overwrite)",
                dest.display()
            )));
        }
        std::fs::remove_dir_all(&dest)
            .map_err(|e| ManagerError::EnvError(format!("could not remove {}: {e}", dest.display())))?;
    }
    std::fs::create_dir_all(&dest)
        .map_err(|e| ManagerError::EnvError(format!("could not create {}: {e}", dest.display())))?;

    let tmp = std::env::temp_dir().join(format!("morloc-demos-{}.tar.gz", std::process::id()));
    curl_download(&asset_url(base, version, "demos.tar.gz"), &tmp)?;

    // Extract only the selected members. The tarball lays them out under
    // `demos/<repo>/...`; --strip-components=1 drops that prefix so each demo
    // lands at <dest>/<repo>/. Every selected repo is listed in the manifest,
    // hence present in the tarball.
    let mut cmd = Command::new("tar");
    cmd.arg("-xzf").arg(&tmp)
        .arg("-C").arg(&dest)
        .arg("--strip-components=1");
    for d in demos {
        cmd.arg(format!("demos/{}", d.repo));
    }
    let status = cmd.status().map_err(|e| {
        ManagerError::EnvError(format!("could not run tar (is it installed?): {e}"))
    });
    let _ = std::fs::remove_file(&tmp);
    if !status?.success() {
        return Err(ManagerError::EnvError(
            "could not extract the demos bundle".to_string(),
        ));
    }

    println!("Extracted {} demo(s) into {}/", demos.len(), dest.display());
    Ok(())
}

/// Entry point for `mim demos`.
pub fn run(list: bool, tag: Option<String>, morloc_version: Option<String>, force: bool) -> Result<()> {
    let base = demos_base();
    let tag = tag.as_deref();

    let (target, explicit) = resolve_target(morloc_version.as_deref())?;
    let (version, manifest) = fetch_manifest(&base, &target, explicit)?;
    let demos = parse_manifest(&manifest);
    let selected = select(&demos, tag, &version)?;

    if list {
        print_list(&version, tag, &selected);
        return Ok(());
    }
    ensure_tar()?;
    pull(&base, &version, tag, &selected, force)
}

fn ensure_tar() -> Result<()> {
    if crate::which("tar") {
        Ok(())
    } else {
        Err(ManagerError::EnvError(
            "`tar` is required to extract demos but was not found on PATH".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_version_strips_leading_v() {
        assert_eq!(bare_version("v0.98.2"), "0.98.2");
        assert_eq!(bare_version("0.98.2"), "0.98.2");
        assert_eq!(bare_version("  v1.2.3 "), "1.2.3");
    }

    #[test]
    fn asset_url_is_predictable() {
        assert_eq!(
            asset_url("https://x/releases", "0.98.2", "manifest.tsv"),
            "https://x/releases/download/demos-0.98.2/manifest.tsv"
        );
    }

    #[test]
    fn parse_manifest_columns_comments_and_empties() {
        let text = "# morloc-version: 0.98.2\n\
                    hello\tabc123\tbasic,py\tA hello demo\n\
                    bare\tdef456\tmisc\t\n\
                    notags\t999\t\t\n";
        let d = parse_manifest(text);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].repo, "hello");
        assert_eq!(d[0].tags, vec!["basic", "py"]);
        assert_eq!(d[0].synopsis, "A hello demo");
        assert_eq!(d[1].synopsis, "");
        assert!(d[2].tags.is_empty());
    }

    #[test]
    fn parse_manifest_tolerates_missing_synopsis_column() {
        let d = parse_manifest("hello\tabc\tbasic\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].synopsis, "");
        assert_eq!(d[0].tags, vec!["basic"]);
    }

    fn demo(repo: &str, tags: &[&str]) -> Demo {
        Demo {
            repo: repo.into(),
            hash: "0123456789abcdef".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            synopsis: String::new(),
        }
    }

    #[test]
    fn select_none_returns_all() {
        let ds = vec![demo("a", &["x"]), demo("b", &["y"])];
        assert_eq!(select(&ds, None, "0.98.2").unwrap().len(), 2);
    }

    #[test]
    fn select_filters_by_tag() {
        let ds = vec![demo("a", &["x"]), demo("b", &["y", "x"])];
        assert_eq!(select(&ds, Some("x"), "0.98.2").unwrap().len(), 2);
        let only_y = select(&ds, Some("y"), "0.98.2").unwrap();
        assert_eq!(only_y.len(), 1);
        assert_eq!(only_y[0].repo, "b");
    }

    #[test]
    fn select_unknown_tag_is_error() {
        let ds = vec![demo("a", &["x"])];
        assert!(select(&ds, Some("nope"), "0.98.2").is_err());
    }
}
