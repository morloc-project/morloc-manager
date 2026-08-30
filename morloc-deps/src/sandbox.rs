//! Building commands that run inside a `buildFHSEnv` sandbox (bubblewrap) on
//! hosts without a standard FHS (NixOS). The sandbox supplies the glibc dynamic
//! loader conda binaries bake at `/lib64/ld-linux-*`, which NixOS lacks.
//!
//! Both the manager (native run/serve/init/doctor) and this crate (the pixi solve)
//! wrap conda/glibc execs the same way, so the convention lives here once: the
//! launcher runs `bash -c "<shell>"` inside the namespace, and the inner shell is
//! `[export PATH=..;] [cd ..;] exec <program> <args...>` with every field
//! shell-quoted. Keeping it in one place means the quoting and the PATH/cwd
//! discipline cannot drift between the two call sites.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Single-quote a string for POSIX `sh`: wrap in single quotes and replace each
/// embedded `'` with `'\''`. Prevents a path with a quote/space/metachar from
/// breaking or injecting into the generated command line.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build a `Command` running `program args...`, wrapped in the FHS launcher when
/// `fhs` is `Some`.
///
/// Inside the sandbox the inner shell, in order:
/// - forces `PATH` when `force_path` is `Some` -- the launcher sets up an FHS
///   `/usr/bin` and may order it ahead of the inherited (conda-first) PATH, which
///   would shadow the pinned toolchain; re-exporting last makes our PATH win;
/// - `cd`s to `cwd` when `Some`, so cwd-relative paths resolve to the same place
///   inside (the host filesystem is bound at its real paths);
/// - `exec`s the program so no shell lingers in the process group.
///
/// `None` for `fhs` yields the bare command (with `cwd` applied via
/// `current_dir`, matching the sandboxed cwd behavior) -- identical to the
/// pre-FHS path on glibc-FHS Linux and macOS.
pub fn command(
    fhs: Option<&Path>,
    program: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    force_path: Option<&str>,
) -> Command {
    match fhs {
        Some(w) => {
            let mut inner = String::new();
            if let Some(p) = force_path {
                inner.push_str(&format!("export PATH={} && ", sh_quote(p)));
            }
            if let Some(d) = cwd {
                inner.push_str(&format!("cd {} && ", sh_quote(&d.to_string_lossy())));
            }
            inner.push_str(&format!("exec {}", sh_quote(&program.to_string_lossy())));
            for a in args {
                inner.push(' ');
                inner.push_str(&sh_quote(&a.to_string_lossy()));
            }
            let mut c = Command::new(w);
            c.arg("-c").arg(inner);
            c
        }
        None => {
            let mut c = Command::new(program);
            c.args(args);
            if let Some(d) = cwd {
                c.current_dir(d);
            }
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_quote("/plain/path"), "'/plain/path'");
    }

    #[test]
    fn bare_command_when_no_wrapper() {
        let c = command(None, Path::new("/bin/echo"), &[OsString::from("hi")], None, None);
        assert_eq!(c.get_program(), std::ffi::OsStr::new("/bin/echo"));
        let args: Vec<_> = c.get_args().collect();
        assert_eq!(args, vec![std::ffi::OsStr::new("hi")]);
    }

    #[test]
    fn wrapped_command_forces_path_then_cd_then_exec() {
        let c = command(
            Some(Path::new("/fhs/bin/morloc-fhs")),
            Path::new("/env/bin/morloc-nexus"),
            &[OsString::from("router")],
            Some(Path::new("/work dir")),
            Some("/conda/bin:/usr/bin"),
        );
        assert_eq!(c.get_program(), std::ffi::OsStr::new("/fhs/bin/morloc-fhs"));
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[0], "-c");
        assert_eq!(
            args[1],
            "export PATH='/conda/bin:/usr/bin' && cd '/work dir' && \
             exec '/env/bin/morloc-nexus' 'router'"
        );
    }
}
