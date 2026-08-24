---
name: project-container-dotfiles-seeding
description: Design verdict for seeding interactive-shell dotfiles (PS1/aliases, later .vimrc/.gitconfig) into docker/podman per-env home; why dotfiles-dir (X) beats inline shell_init lines (Y)
metadata:
  type: project
---

Design review (2026-08-21) of getting interactive-shell settings (custom PS1, colored
`grep`/`ls` aliases; later full dotfiles .vimrc/.gitconfig) into docker/podman dev
containers. Direction already settled: seed the per-env home (not image-bake, not setup-shell
hook); accept engine asymmetry (docker/podman = curated `<env>/home`; Apptainer = host $HOME).

**Verdict: Design X (`dotfiles: Option<String>` host dir, seed-if-target-absent), NOT
Design Y (`shell_init: Vec<String>` inline lines -> managed .bashrc block).**

Load-bearing facts (verified, don't rederive):
- docker/podman `$HOME` = `/opt/morloc-state/home` = `<env_data_dir>/home` on host: bind-mounted,
  WRITABLE, PERSISTENT. `morloc-env clean` does NOT touch it. `--shell` = interactive non-login
  bash -> reads `$HOME/.bashrc` only (NOT /etc/profile, NOT ~/.bash_profile).
- The home dir is created at RUN time via `fs::create_dir_all` at THREE sites: serve.rs:283,
  main.rs:3005, main.rs:3957 -- all idempotent, error-ignored, NON-DESTRUCTIVE (never wipe).
  => files seeded into `<env>/home` survive every subsequent serve/run. Verified 2026-08-21.
- /etc/skel does NOT apply: skel copies only at user-creation; `<env>/home` is a pre-existing
  bind mount, so it starts with NO .bashrc unless seeded.

Why X wins (decisive axis = the user's OWN stated end-state):
- .vimrc/.gitconfig are NOT shell-init lines. Y cannot express them; to reach the stated
  goal Y must GROW a file-seeder = become X. So Y dead-ends exactly at the named requirement.
  X extends to full dotfiles for free (they ARE a dir of files).
- PS1 escaping: Y pushes `export PS1='\[\e[..\]\u..\$ '` through YAML+CLI+shell. Via CLI
  double-quotes bash eats `\$` -> `$`, silently WRONG prompt. X = plain file, zero escaping.
- Y's `shell_init` NAME is misleading: implies it configures "the shell", but .bashrc is
  interactive-only; when user later wants `export EDITOR` to reach non-interactive `morloc make`
  (reads only `$BASH_ENV`), the name over-promises. `dotfiles:` is honest (interactive-scoped
  by nature). NOTE: neither design delivers env vars to `morloc make` -- that needs $BASH_ENV
  or entrypoint env, not dotfiles.

Clobbering resolution (dissolves the tension in BOTH designs):
- Seed **if-target-absent, lazily at RUN time** (right after create_dir_all at serve.rs:283),
  per-file. Never clobbers live edits; safe on every serve; makes "just edit `<env>/home`
  directly" an HONEST instruction. Cost: pushing a CHANGED source dotfile needs delete-target
  or a deferred `--reseed`. Legible opt-in clobber, not silent.
- Reject seed-at-`new`: home dir isn't made until run time; would add a 4th creation site to
  drift. Reject Y managed-block markers: fragile (user deletes marker -> dup/lost idempotency;
  in-block edits clobbered silently), and yields a thin .bashrc missing distro guards
  (`case $- in *i*)`).

Watch-for before shipping: grep for any `read_dir(<env>/home)` that treats non-empty home as a
"first run" signal (the 3 create_dir_all sites don't, but a seed would populate it early).

Minimal v1: add `dotfiles: Option<String>` to EnvironmentConfig (`#[serde(default)]`, same
back-compat pattern as the just-added `system_packages`), `--dotfiles` on new/update, copy-if-
absent at run time. Defer: `--reseed`, any `shell_init` inline, managed-block writer.
Even-simpler alt if only one env: skip the field, just mkdir `<env>/home` at `new` + document
"drop dotfiles here"; add `dotfiles:` when author-once/seed-many across N envs is wanted.

Related: [[project-pliable-container-env]] (same `<env>/home` bind-mount substrate).
