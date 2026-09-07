//! Case-fold safety for a solved conda prefix.
//!
//! A filesystem that folds letter case (APFS and HFS+ in their default macOS
//! configuration, NTFS, and any share layered over them) cannot hold two files
//! whose names differ only in case. Conda packages do ship such pairs: ncurses
//! aliases terminal descriptions (`share/terminfo/32/2621A` and `.../2621a`),
//! and the Linux UAPI headers in `kernel-headers_linux-*` carry eight of them
//! (`netfilter_ipv6/ip6t_HL.h` and `.../ip6t_hl.h`, and the `xt_*`/`ipt_*`
//! equivalents). On such a filesystem the second file of a pair opens the first
//! and overwrites it.
//!
//! The consequence is not a build failure -- the install succeeds and the prefix
//! is quietly not what the lockfile says it is, so a compiler reads one header
//! believing it is the other. That is why this check exists: an environment is
//! either verified to survive the filesystem it was installed on, or refused.
//!
//! Whether a collapse loses anything is decided per pair, from the content
//! hashes conda records, not from a list of packages known to be tolerable:
//! ncurses' aliases are byte-identical (both are symlinks to the same terminal
//! description), so folding them away changes nothing, while the kernel headers
//! hold different declarations and folding them corrupts the toolchain. A pair
//! whose content is unknown counts as harmful.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::{DepsError, Result};

/// How many collisions an error message lists before summarizing the rest.
const MAX_REPORTED: usize = 12;

/// One path in a colliding group, and the package that installs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub package: String,
    pub path: String,
}

/// A set of prefix-relative paths that differ only in letter case, and so
/// occupy one name on a case-folding filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The single name the members collapse onto.
    pub folded: String,
    /// The colliding paths, ordered by path.
    pub members: Vec<Member>,
}

/// Whether `dir`'s filesystem folds letter case, decided by writing a probe file
/// under an upper-case name and looking for it under the lower-case one.
///
/// Probes the directory itself rather than any ancestor: a prefix can sit on its
/// own mount (a container volume, a disk image), and only the directory that
/// will hold the files answers for the filesystem that will hold them. The probe
/// file is removed on every path out.
pub fn folds_case(dir: &Path) -> Result<bool> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Lower-casing the whole name must yield the sibling name, so every
    // character except the final marker is already lower case.
    let upper = format!(".mim-case-probe-{}-{}-A", std::process::id(), nanos);
    let lower = upper.to_lowercase();
    let upper_path = dir.join(&upper);
    std::fs::write(&upper_path, b"").map_err(|e| {
        DepsError::Env(format!(
            "cannot test whether {} folds letter case: {e}",
            dir.display()
        ))
    })?;
    let folded = dir.join(&lower).symlink_metadata().is_ok();
    let _ = std::fs::remove_file(&upper_path);
    Ok(folded)
}

/// The path records of every installed package, as `(package, path, content
/// hash)`, read from a directory of `conda-meta` records. `paths_data` carries a
/// hash per file; a record old enough to have only a `files` list yields no
/// hash, which makes any collision it takes part in count as harmful.
fn declared_paths(meta_dir: &Path) -> Vec<(String, String, Option<String>)> {
    #[derive(Deserialize)]
    struct PathEntry {
        #[serde(rename = "_path")]
        path: String,
        #[serde(default)]
        sha256: Option<String>,
    }
    #[derive(Deserialize)]
    struct PathsData {
        #[serde(default)]
        paths: Vec<PathEntry>,
    }
    #[derive(Deserialize)]
    struct Meta {
        name: String,
        #[serde(default)]
        paths_data: Option<PathsData>,
        #[serde(default)]
        files: Vec<String>,
    }
    let mut out = Vec::new();
    crate::abi::for_each_conda_meta(meta_dir, |m: Meta| {
        match m.paths_data {
            Some(pd) => out.extend(
                pd.paths
                    .into_iter()
                    .map(|e| (m.name.clone(), e.path, e.sha256)),
            ),
            None => out.extend(m.files.into_iter().map(|f| (m.name.clone(), f, None))),
        }
    });
    out
}

/// The case collisions in a solved prefix that lose content when they collapse.
///
/// Two packages declaring the SAME path is a clobber rather than a fold, so a
/// group counts only when it holds at least two distinct spellings. Such a group
/// is harmless exactly when every spelling carries one and the same content
/// hash, since collapsing then yields the bytes each name promised.
pub fn harmful_collisions(meta_dir: &Path) -> Vec<Collision> {
    let mut groups: BTreeMap<String, Vec<(String, String, Option<String>)>> = BTreeMap::new();
    for (package, path, sha) in declared_paths(meta_dir) {
        groups
            .entry(path.to_lowercase())
            .or_default()
            .push((package, path, sha));
    }
    groups
        .into_iter()
        .filter_map(|(folded, entries)| {
            let spellings: BTreeSet<&str> = entries.iter().map(|(_, p, _)| p.as_str()).collect();
            if spellings.len() < 2 {
                return None;
            }
            let hashes: BTreeSet<Option<&str>> =
                entries.iter().map(|(_, _, s)| s.as_deref()).collect();
            if hashes.len() == 1 && !hashes.contains(&None) {
                return None;
            }
            let mut members: Vec<Member> = entries
                .into_iter()
                .map(|(package, path, _)| Member { package, path })
                .collect();
            members.sort_by(|a, b| a.path.cmp(&b.path));
            members.dedup_by(|a, b| a.path == b.path);
            Some(Collision { folded, members })
        })
        .collect()
}

/// Refuse a solved prefix whose contents the filesystem it sits on cannot
/// represent.
///
/// Verifies only a prefix this process can reach. When the records resolve to
/// the host-side mirror the prefix lives on an engine volume, whose filesystem
/// is not this one and is checked where it is materialized instead. A
/// case-sensitive filesystem folds nothing, so the scan is skipped there too and
/// the check costs one probe file.
pub fn check_pixi_dir(pixi_dir: &Path) -> Result<()> {
    let prefix = crate::abi::conda_prefix(pixi_dir);
    let meta = crate::abi::meta_dir(pixi_dir);
    if !meta.starts_with(&prefix) {
        return Ok(());
    }
    if !folds_case(&prefix)? {
        return Ok(());
    }
    let harmful = harmful_collisions(&meta);
    if harmful.is_empty() {
        return Ok(());
    }
    Err(DepsError::Env(describe(&prefix, &harmful)))
}

/// The refusal message: what collapsed, and how to get a filesystem that holds
/// it. Written for someone who has never heard of conda path records.
fn describe(conda_prefix: &Path, harmful: &[Collision]) -> String {
    let mut msg = format!(
        "the solved conda environment cannot be represented on this filesystem:\n  \
         {}\n\n\
         This filesystem folds letter case, so files whose names differ only in case \
         collapse onto one name. These {} collapse into a file holding the wrong \
         contents, leaving a toolchain that is not what the lockfile describes:\n",
        conda_prefix.display(),
        if harmful.len() == 1 { "does".to_string() } else { format!("{} do", harmful.len()) },
    );
    for c in harmful.iter().take(MAX_REPORTED) {
        let package = c
            .members
            .first()
            .map(|m| m.package.as_str())
            .unwrap_or("?");
        msg.push_str(&format!("\n  {package}\n"));
        for m in &c.members {
            msg.push_str(&format!("    {}\n", m.path));
        }
    }
    if harmful.len() > MAX_REPORTED {
        msg.push_str(&format!("\n  ... and {} more\n", harmful.len() - MAX_REPORTED));
    }
    msg.push_str(
        "\nBuild the environment on a case-sensitive filesystem. On macOS, add a \
         case-sensitive APFS volume -- it shares free space with the volume you \
         already have, so it costs no disk -- and point mim at it:\n\n  \
         diskutil list\n  \
         sudo diskutil apfs addVolume <container> \"Case-sensitive APFS\" morloc\n  \
         export XDG_DATA_HOME=/Volumes/morloc/share\n\n\
         then create the environment again.\n",
    );
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a conda-meta record listing `paths` as `(path, sha256)` pairs.
    fn record(dir: &Path, name: &str, paths: &[(&str, Option<&str>)]) {
        let entries: Vec<String> = paths
            .iter()
            .map(|(p, sha)| match sha {
                Some(s) => format!(r#"{{"_path":"{p}","path_type":"hardlink","sha256":"{s}"}}"#),
                None => format!(r#"{{"_path":"{p}","path_type":"hardlink"}}"#),
            })
            .collect();
        std::fs::write(
            dir.join(format!("{name}-1.0-h0.json")),
            format!(
                r#"{{"name":"{name}","version":"1.0","paths_data":{{"paths_version":1,"paths":[{}]}}}}"#,
                entries.join(",")
            ),
        )
        .unwrap();
    }

    /// A directory of conda-meta records, as `meta_dir` resolves it.
    fn meta_with(records: impl FnOnce(&Path)) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        records(tmp.path());
        tmp
    }

    #[test]
    fn identical_content_is_not_harmful() {
        // ncurses aliases a terminal description under two spellings; both records
        // carry the same hash, so collapsing them loses nothing.
        let tmp = meta_with(|d| {
            record(
                d,
                "ncurses",
                &[
                    ("share/terminfo/32/2621A", Some("aaaa")),
                    ("share/terminfo/32/2621a", Some("aaaa")),
                ],
            )
        });
        assert_eq!(harmful_collisions(tmp.path()), vec![]);
    }

    #[test]
    fn differing_content_is_harmful() {
        // The kernel headers: two distinct declarations under one folded name.
        let tmp = meta_with(|d| {
            record(
                d,
                "kernel-headers_linux-64",
                &[
                    ("include/linux/netfilter_ipv6/ip6t_HL.h", Some("aaaa")),
                    ("include/linux/netfilter_ipv6/ip6t_hl.h", Some("bbbb")),
                ],
            )
        });
        let got = harmful_collisions(tmp.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].folded, "include/linux/netfilter_ipv6/ip6t_hl.h");
        assert_eq!(
            got[0].members.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
            vec![
                "include/linux/netfilter_ipv6/ip6t_HL.h",
                "include/linux/netfilter_ipv6/ip6t_hl.h",
            ]
        );
        assert_eq!(got[0].members[0].package, "kernel-headers_linux-64");
    }

    #[test]
    fn unknown_content_is_harmful() {
        // A record without hashes cannot prove the collapse is lossless, so the
        // check fails closed rather than assuming the two are the same file.
        let tmp = meta_with(|d| {
            record(d, "old", &[("share/A", None), ("share/a", None)])
        });
        assert_eq!(harmful_collisions(tmp.path()).len(), 1);
    }

    #[test]
    fn a_hashed_and_an_unhashed_spelling_are_harmful() {
        let tmp = meta_with(|d| {
            record(d, "mixed", &[("share/A", Some("aaaa")), ("share/a", None)])
        });
        assert_eq!(harmful_collisions(tmp.path()).len(), 1);
    }

    #[test]
    fn distinct_names_do_not_collide() {
        let tmp = meta_with(|d| {
            record(
                d,
                "perl",
                &[("lib/Pod/Usage.pm", Some("aaaa")), ("lib/pod/perl.pod", Some("bbbb"))],
            )
        });
        // `Pod/` and `pod/` merge into one directory, but no file inside them
        // shares a folded name, so every file keeps its own content.
        assert_eq!(harmful_collisions(tmp.path()), vec![]);
    }

    #[test]
    fn a_directory_merged_onto_a_file_is_harmful() {
        let tmp = meta_with(|d| {
            record(d, "a", &[("share/Thing", Some("aaaa"))]);
            record(d, "b", &[("share/thing", Some("bbbb"))]);
        });
        let got = harmful_collisions(tmp.path());
        assert_eq!(got.len(), 1);
        // The report names both packages, since neither alone explains the clash.
        assert_eq!(
            got[0].members.iter().map(|m| m.package.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn the_same_path_from_two_packages_is_not_a_case_collision() {
        // A clobber (identical spellings) is a different failure and is not this
        // check's business -- a case-sensitive filesystem would not fix it.
        let tmp = meta_with(|d| {
            record(d, "a", &[("share/thing", Some("aaaa"))]);
            record(d, "b", &[("share/thing", Some("bbbb"))]);
        });
        assert_eq!(harmful_collisions(tmp.path()), vec![]);
    }

    #[test]
    fn an_unreadable_prefix_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(harmful_collisions(tmp.path()), vec![]);
    }

    #[test]
    fn the_probe_leaves_no_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        folds_case(tmp.path()).unwrap();
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_case_sensitive_prefix_passes_without_scanning() {
        // The collisions are real, so passing proves the scan was skipped on the
        // strength of the probe. Skipped where the test filesystem folds case.
        let tmp = tempfile::tempdir().unwrap();
        let meta = crate::abi::conda_prefix(tmp.path()).join("conda-meta");
        std::fs::create_dir_all(&meta).unwrap();
        record(&meta, "k", &[("include/A.h", Some("aaaa")), ("include/a.h", Some("bbbb"))]);
        if folds_case(&meta).unwrap() {
            return;
        }
        assert!(check_pixi_dir(tmp.path()).is_ok());
    }

    #[test]
    fn a_mirrored_prefix_is_left_to_the_environment_that_holds_it() {
        // Records beside the manifest mean the prefix is on an engine volume: this
        // process cannot probe that filesystem, so it must not judge it. The
        // collisions here are real and must still be passed over.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::abi::conda_prefix(tmp.path())).unwrap();
        let mirror = tmp.path().join(crate::abi::CONDA_META_MIRROR);
        std::fs::create_dir_all(&mirror).unwrap();
        record(&mirror, "k", &[("include/A.h", Some("aaaa")), ("include/a.h", Some("bbbb"))]);
        assert_eq!(harmful_collisions(&mirror).len(), 1);
        assert!(check_pixi_dir(tmp.path()).is_ok());
    }

    #[test]
    fn a_refusal_names_the_files_and_the_remedy() {
        let harmful = vec![Collision {
            folded: "include/a.h".to_string(),
            members: vec![
                Member { package: "k".into(), path: "include/A.h".into() },
                Member { package: "k".into(), path: "include/a.h".into() },
            ],
        }];
        let msg = describe(Path::new("/env"), &harmful);
        assert!(msg.contains("include/A.h"), "{msg}");
        assert!(msg.contains("include/a.h"), "{msg}");
        assert!(msg.contains("case-sensitive"), "{msg}");
        assert!(msg.contains("XDG_DATA_HOME"), "{msg}");
    }
}
