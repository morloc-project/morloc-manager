//! Interactive-prompt layer over `inquire`.
//!
//! Thin, typed wrappers the wizard calls instead of touching `inquire` directly,
//! so prompt conventions live in one place: rendering to stderr (inquire's
//! default, keeping stdout clean for machine consumers), and a uniform cancel
//! path. `Ctrl-C` aborts the session immediately; `Esc` is guarded -- it asks
//! for confirmation before leaving, so an accidental press (a reflex to "go
//! back") does not discard a half-filled session. Both a confirmed cancel and
//! `Ctrl-C` surface as `PromptError::Cancelled`; any other prompt failure and
//! any `ManagerError` raised mid-session become `PromptError::Other`.

use std::path::{Path, PathBuf};

use inquire::autocompletion::{Autocomplete, Replacement};
use inquire::error::InquireError;
use inquire::list_option::ListOption;
use inquire::{Confirm, CustomUserError, MultiSelect, Select, Text};

use crate::error::ManagerError;

/// A prompt outcome: either the user aborted (Esc / Ctrl-C), or a real error
/// occurred (a terminal failure, or a `ManagerError` from validation/IO run
/// between prompts).
pub enum PromptError {
    Cancelled,
    Other(ManagerError),
}

pub type Result<T> = std::result::Result<T, PromptError>;

impl From<ManagerError> for PromptError {
    fn from(e: ManagerError) -> Self {
        PromptError::Other(e)
    }
}

/// The Esc-guard confirmation. Domain-neutral so the layer is not coupled to any
/// one flow's phrasing; the caller decides what a resulting `Cancelled` aborts.
const CANCEL_PROMPT: &str = "Cancel and discard your entries?";

/// Map an inquire prompt failure to a `PromptError`, surfacing every non-cancel
/// error (IO, NotTTY, InvalidConfiguration, ...) rather than masking it.
fn prompt_failure<T>(e: InquireError) -> Result<T> {
    Err(PromptError::Other(ManagerError::EnvError(format!("prompt failed: {e}"))))
}

/// Run an inquire prompt, mapping its cancel signals to session policy:
/// `Ctrl-C` (OperationInterrupted) aborts immediately; `Esc` (OperationCanceled)
/// asks [`CANCEL_PROMPT`] and, unless confirmed, re-runs the same prompt -- so an
/// accidental Esc does not abandon the flow. `attempt` is a closure so it can be
/// re-issued after a declined cancel. Both a confirmed cancel and `Ctrl-C` return
/// `Cancelled`; a genuine failure of either prompt returns `Other`.
fn guarded<T>(mut attempt: impl FnMut() -> std::result::Result<T, InquireError>) -> Result<T> {
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(InquireError::OperationInterrupted) => return Err(PromptError::Cancelled),
            Err(InquireError::OperationCanceled) => {
                // Esc: confirm before leaving. Declining (or an Esc on the guard
                // itself) re-runs the prompt; a real failure of the guard surfaces.
                match Confirm::new(CANCEL_PROMPT).with_default(false).prompt() {
                    Ok(true) | Err(InquireError::OperationInterrupted) => {
                        return Err(PromptError::Cancelled)
                    }
                    Ok(false) | Err(InquireError::OperationCanceled) => continue,
                    Err(other) => return prompt_failure(other),
                }
            }
            Err(other) => return prompt_failure(other),
        }
    }
}

/// Ask a yes/no question. `default` is the answer used when the user just
/// presses enter.
pub fn confirm(message: &str, default: bool) -> Result<bool> {
    let message = message.to_owned();
    guarded(|| Confirm::new(&message).with_default(default).prompt())
}

/// A free-text prompt whose `default` is returned on an empty answer (shown as
/// `(default)` by inquire). Use for values the user rarely overrides.
pub fn text(message: &str, default: &str) -> Result<String> {
    let title = titled(message);
    guarded(|| Text::new(&title).with_default(default).prompt())
}

/// A free-text prompt pre-filled with `initial` on an editable line, so the user
/// tweaks a suggested value rather than retyping it. Use for names.
pub fn text_editable(message: &str, initial: &str) -> Result<String> {
    let title = titled(message);
    guarded(|| Text::new(&title).with_initial_value(initial).prompt())
}

/// A path prompt: free text with an optional help line and TAB filesystem
/// autocompletion. Blank is allowed and returned verbatim; the caller decides
/// what an empty answer means.
pub fn path(message: &str, help: &str) -> Result<String> {
    let title = titled(message);
    guarded(|| {
        Text::new(&title)
            .with_help_message(help)
            .with_autocomplete(FilePathCompleter::default())
            .prompt()
    })
}

/// Like [`path`], but pre-fills `initial` on the editable line so the user can
/// keep a current value with a bare Enter (or erase it) rather than having an
/// unrelated default silently replace it. Use when re-editing an existing path.
pub fn path_seeded(message: &str, help: &str, initial: &str) -> Result<String> {
    let title = titled(message);
    let initial = initial.to_owned();
    guarded(|| {
        Text::new(&title)
            .with_help_message(help)
            .with_initial_value(&initial)
            .with_autocomplete(FilePathCompleter::default())
            .prompt()
    })
}

/// Append a `:` to a field prompt so the label reads as a title and does not run
/// into the answer inquire renders beside it (e.g. `Environment name: v0.98.0`).
/// Not used for yes/no questions, which already end in `?`.
fn titled(message: &str) -> String {
    format!("{message}:")
}

/// The longest string that prefixes every item (used to complete an unambiguous
/// path fragment, like a shell does on TAB).
fn longest_common_prefix(items: &[String]) -> String {
    let mut iter = items.iter();
    let mut prefix = match iter.next() {
        Some(first) => first.clone(),
        None => return String::new(),
    };
    for item in iter {
        while !item.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return String::new();
            }
        }
    }
    prefix
}

/// Filesystem autocompleter for path prompts: on each keystroke it lists the
/// entries under the typed directory whose names match the typed fragment, and
/// on TAB it fills in either the highlighted suggestion or the longest common
/// prefix. Directory suggestions keep a trailing `/` so completion can descend.
/// The directory listing is cached, so typing successive characters of a fragment
/// re-filters in memory instead of re-reading the directory.
#[derive(Clone, Default)]
struct FilePathCompleter {
    /// The directory whose listing is cached in `entries` (`None` before any scan).
    scanned_dir: Option<PathBuf>,
    /// `(name, is_dir)` for every entry of `scanned_dir`.
    entries: Vec<(String, bool)>,
    suggestions: Vec<String>,
    lcp: String,
}

impl FilePathCompleter {
    /// Recompute suggestions for `input`, reading the target directory only when it
    /// changes from the last call and re-filtering the cached listing otherwise.
    fn refresh(&mut self, input: &str) -> std::result::Result<(), CustomUserError> {
        // The directory to scan and the fragment being completed. An empty input, a
        // bare "~", or a trailing "/" means "list this directory"; otherwise the text
        // after the last "/" is the fragment. `dir_display` comes from the TYPED
        // input (not the expanded path), so a suggestion reads back the way the user
        // is typing -- relative stays relative, "~/" stays "~/", and the root "/" is
        // never doubled.
        let expanded = crate::expand_tilde(input);
        let slash = input.rfind('/');
        let (scan_dir, fragment, dir_display): (PathBuf, String, String) =
            if input.is_empty() || input == "~" || input.ends_with('/') {
                let dir = if input.is_empty() { PathBuf::from(".") } else { expanded };
                let display = match input {
                    "" => String::new(),
                    "~" => "~/".to_string(),
                    other => other.to_string(),
                };
                (dir, String::new(), display)
            } else {
                let fragment = input[slash.map_or(0, |i| i + 1)..].to_string();
                let dir = expanded
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                let display = slash.map_or(String::new(), |i| input[..=i].to_string());
                (dir, fragment, display)
            };

        // Re-read the directory only when it changes; otherwise reuse the cache.
        if self.scanned_dir.as_deref() != Some(scan_dir.as_path()) {
            self.entries.clear();
            if let Ok(rd) = std::fs::read_dir(&scan_dir) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    self.entries.push((name, is_dir));
                }
            }
            self.scanned_dir = Some(scan_dir);
        }

        self.suggestions = self
            .entries
            .iter()
            .filter(|(name, _)| name.starts_with(&fragment))
            .map(|(name, is_dir)| {
                let mut suggestion = format!("{dir_display}{name}");
                if *is_dir {
                    suggestion.push('/');
                }
                suggestion
            })
            .collect();
        self.suggestions.sort();
        self.lcp = longest_common_prefix(&self.suggestions);
        Ok(())
    }
}

impl Autocomplete for FilePathCompleter {
    fn get_suggestions(&mut self, input: &str) -> std::result::Result<Vec<String>, CustomUserError> {
        self.refresh(input)?;
        Ok(self.suggestions.clone())
    }

    fn get_completion(
        &mut self,
        input: &str,
        highlighted: Option<String>,
    ) -> std::result::Result<Replacement, CustomUserError> {
        self.refresh(input)?;
        Ok(match highlighted {
            Some(s) => Replacement::Some(s),
            None if !self.lcp.is_empty() && self.lcp != input => Replacement::Some(self.lcp.clone()),
            None => Replacement::None,
        })
    }
}

/// Single-choice menu returning the chosen option's index, for menus whose rows
/// carry their own state (e.g. greyed "(unavailable)" entries the caller must
/// re-check against a parallel list).
pub fn select_indexed(message: &str, labels: Vec<String>, start: usize) -> Result<usize> {
    let title = titled(message);
    let chosen: ListOption<String> = guarded(|| {
        Select::new(&title, labels.clone())
            .with_starting_cursor(start)
            .raw_prompt()
    })?;
    Ok(chosen.index)
}

/// Checkbox multi-select returning the chosen indices (into `labels`). `default`
/// pre-checks options by index.
pub fn multiselect(
    message: &str,
    labels: Vec<String>,
    default: &[usize],
) -> Result<Vec<usize>> {
    let title = titled(message);
    let chosen: Vec<ListOption<String>> = guarded(|| {
        MultiSelect::new(&title, labels.clone())
            // The option lists are short (a handful of languages), so type-to-filter
            // adds nothing and gets in the way of selecting several; disable it and
            // drop "type to filter" from the help line.
            .without_filtering()
            .with_help_message("up/down to move, space to select, -> all, <- none")
            .with_default(default)
            .raw_prompt()
    })?;
    Ok(chosen.into_iter().map(|o| o.index).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completer_lists_matching_entries_with_typed_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        std::fs::write(base.join("alpha"), "").unwrap();
        std::fs::write(base.join("beta"), "").unwrap();
        std::fs::create_dir(base.join("adir")).unwrap();

        let mut c = FilePathCompleter::default();
        let prefix = format!("{}/", base.display());
        c.refresh(&format!("{prefix}a")).unwrap();

        // Only "a*" entries; a directory keeps a trailing slash, a file does not.
        assert!(c.suggestions.contains(&format!("{prefix}alpha")));
        assert!(c.suggestions.contains(&format!("{prefix}adir/")));
        assert!(!c.suggestions.iter().any(|s| s.ends_with("beta")));
        // Suggestions read back with the exact typed prefix and never double a slash.
        for s in &c.suggestions {
            assert!(s.starts_with(&prefix), "{s} lacks the typed prefix");
            assert!(!s.contains("//"), "{s} has a doubled slash");
        }
    }

    #[test]
    fn completer_caches_directory_across_fragment_changes() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        std::fs::write(base.join("alpha"), "").unwrap();

        let mut c = FilePathCompleter::default();
        c.refresh(&format!("{}/a", base.display())).unwrap();
        let scanned = c.scanned_dir.clone();
        // Narrowing the fragment within the same directory reuses the cached scan.
        c.refresh(&format!("{}/al", base.display())).unwrap();
        assert_eq!(c.scanned_dir, scanned);
        assert!(c.suggestions.contains(&format!("{}/alpha", base.display())));
    }
}
