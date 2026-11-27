//! Files that make an interactive environment shell usable: the default bash
//! startup file and the editor syntax files baked into container images.

use std::fs;
use std::path::{Path, PathBuf};

/// The built-in default bash startup file.
pub const DEFAULT_BASHRC: &str = include_str!("../assets/bashrc");

/// Name of the user's editable copy of the default, in mim's config dir.
pub const BASHRC_FILE: &str = "bashrc";

/// The user's editable copy of the default bash startup file.
pub fn bashrc_path(config_dir: &Path) -> PathBuf {
    config_dir.join(BASHRC_FILE)
}

/// The default bash startup file: the user's copy in `config_dir` when it is
/// readable, else the built-in, which is first written there (best-effort) so
/// the user has a file to edit.
pub fn default_bashrc(config_dir: &Path) -> String {
    let path = bashrc_path(config_dir);
    if let Ok(text) = fs::read_to_string(&path) {
        return text;
    }
    if !path.exists() && fs::create_dir_all(config_dir).is_ok() {
        let _ = fs::write(&path, DEFAULT_BASHRC);
    }
    DEFAULT_BASHRC.to_string()
}

/// Build-context directory holding the editor files, mirrored at `/` in the
/// image.
pub const EDITOR_CONTEXT_DIR: &str = "editor";

/// Editor files baked into an environment image: (path in the image, contents).
/// Ubuntu and Debian nano include `/usr/share/nano/*.nanorc`; vim's default
/// runtimepath includes `/usr/share/vim/vimfiles`.
pub const EDITOR_FILES: &[(&str, &str)] = &[
    ("usr/share/nano/morloc.nanorc", include_str!("../assets/editor/morloc.nanorc")),
    ("usr/share/vim/vimfiles/syntax/loc.vim", include_str!("../assets/editor/loc.vim")),
    (
        "usr/share/vim/vimfiles/ftdetect/loc.vim",
        "au BufRead,BufNewFile *.loc set filetype=loc\n",
    ),
];

/// System packages an environment image adds so its shell can edit files.
/// `vim` and not `vim-tiny`, which has no syntax highlighting.
pub const EDITOR_PACKAGES: &[&str] = &["vim", "nano"];

/// Write the editor files into `context/editor/`, mirroring their image paths.
pub fn stage_editor_files(context: &Path) -> std::io::Result<()> {
    for (rel, text) in EDITOR_FILES {
        let path = context.join(EDITOR_CONTEXT_DIR).join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, text)?;
    }
    Ok(())
}

/// Image cache-key fragment for the editor files. The Dockerfile names only
/// their directory, so their contents must ride the key for an edit to
/// rebuild the image.
pub fn editor_key_fragment() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (rel, text) in EDITOR_FILES {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(text.as_bytes());
        h.update([0]);
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("# editor-files: {hex}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_bashrc_is_seeded_for_editing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("morloc");
        assert_eq!(default_bashrc(&dir), DEFAULT_BASHRC);
        assert_eq!(fs::read_to_string(bashrc_path(&dir)).unwrap(), DEFAULT_BASHRC);
    }

    #[test]
    fn an_edited_default_bashrc_is_used() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(bashrc_path(tmp.path()), "PS1='custom> '\n").unwrap();
        assert_eq!(default_bashrc(tmp.path()), "PS1='custom> '\n");
    }

    #[test]
    fn editor_files_are_staged_at_their_image_paths() {
        let tmp = tempfile::tempdir().unwrap();
        stage_editor_files(tmp.path()).unwrap();
        for (rel, text) in EDITOR_FILES {
            let got = fs::read_to_string(tmp.path().join(EDITOR_CONTEXT_DIR).join(rel)).unwrap();
            assert_eq!(&got, text);
        }
    }

    #[test]
    fn editor_files_carry_both_morloc_syntaxes() {
        let has = |suffix: &str, needle: &str| {
            EDITOR_FILES.iter().any(|(rel, text)| rel.ends_with(suffix) && text.contains(needle))
        };
        assert!(has("nano/morloc.nanorc", "syntax morloc"));
        assert!(has("syntax/loc.vim", "b:current_syntax"));
        assert!(has("ftdetect/loc.vim", "filetype=loc"));
    }
}
