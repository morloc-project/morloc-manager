//! Generate a Dockerfile that builds a morloc environment image from the SAME
//! requirement->pixi lowering the native backend uses.
//!
//! The image is requirement-derived, not hand-authored: a slim base, a pinned
//! pixi, the environment's `pixi.toml`/`pixi.lock`, the pinned morloc compiler +
//! Rust source, and `morloc init` run against the pixi toolchain (which builds
//! libmorloc.so + morloc-nexus from that source). Container and native thus
//! share one source of truth (the pixi manifest); they differ only in
//! isolation. Build-extras (OS packages conda cannot provide) are the one
//! container-only input -- native has no build layer.

/// Build-time extras that conda cannot provide. `system_packages` are installed
/// with the base image's package manager (assumed Debian-family `apt`). Extra
/// conda channels are NOT here -- they belong in the `pixi.toml` the manifest
/// renderer produces.
#[derive(Debug, Clone, Default)]
pub struct BuildExtras {
    pub system_packages: Vec<String>,
}

/// Inputs to the Dockerfile generator. The `pixi.toml`, `pixi.lock`, and the
/// `runtime/` directory (morloc compiler + Rust source) are supplied through the
/// build context (COPY-ed in), not here; this struct carries only the
/// image-shape parameters.
pub struct DockerfileInput<'a> {
    /// Slim base image (glibc + a Debian-family package manager), fully
    /// qualified so podman needs no short-name registry config,
    /// e.g. "docker.io/library/debian:bookworm-slim".
    pub base_image: &'a str,
    /// Pinned pixi version without a leading 'v', e.g. "0.76.2".
    pub pixi_version: &'a str,
    /// In-image MORLOC_HOME (where `morloc init` installs the shims).
    pub morloc_home: &'a str,
    /// Build-extras (container-only OS packages).
    pub extras: &'a BuildExtras,
    /// Script-provisioned languages (e.g. futhark) whose `install.sh` the builder
    /// has written into the build context as `install-<lang>.sh`. Each is COPY-ed
    /// in and run at image build. OCI-only (only the Dockerfile path supports it).
    pub lang_installs: &'a [String],
    /// Dev environment: the compiler + Rust source are NOT baked in (they are
    /// built from a mounted source tree at materialize time), so the
    /// `COPY runtime/` step and the baked `MORLOC_RUST_DIR` are omitted;
    /// `CONTAINER_RUNTIME_BIN` becomes a mount target instead.
    pub dev: bool,
    /// Build-context-relative path to a corporate CA PEM (e.g. "certs/corp.pem"),
    /// or `None`. When set it is trusted before any network `RUN`. See [`crate::cert`].
    pub cert_file: Option<&'a str>,
}

/// Render the Dockerfile text. Deterministic for a given input.
pub fn generate_dockerfile(input: &DockerfileInput) -> String {
    let mut out = String::new();
    out.push_str(&format!("FROM {}\n", input.base_image));
    out.push_str("ENV DEBIAN_FRONTEND=noninteractive\n");
    out.push('\n');

    // Copy the corporate CA in before any network RUN; it is registered into the
    // trust store at the apt step below. (The bookworm-slim apt mirror is plain
    // http, so apt-get update itself needs no CA.)
    if let Some(cert_file) = input.cert_file {
        out.push_str("# Corporate CA bundle (trusted before any network fetch)\n");
        out.push_str(&format!(
            "COPY {cert_file} /usr/local/share/ca-certificates/morloc-corp.crt\n\n"
        ));
    }

    // Base tools needed to bootstrap the image before the conda env exists: curl
    // for the pixi installer, CA certs for TLS. Everything else (compilers, git,
    // language runtimes) comes from the pixi-solved conda env, so the base stays
    // minimal. Plus any container-only system packages conda cannot provide.
    // libnss-wrapper is installed so the entrypoint can synthesize a passwd/group
    // entry for the host UID under `--userns=keep-id` (which has no /etc/passwd
    // entry of its own). Its real .so path is arch-dependent, so symlink it to a
    // fixed location the entrypoint can reference.
    out.push_str("# Base tools + build-extras (system packages conda cannot provide)\n");
    out.push_str("RUN apt-get update \\\n");
    out.push_str(
        " && apt-get install -y --no-install-recommends ca-certificates curl libnss-wrapper",
    );
    for pkg in &input.extras.system_packages {
        out.push_str(&format!(" {pkg}"));
    }
    if input.dev {
        // A dev env builds the Haskell compiler from source with stack/GHC, which
        // needs a NATIVE GNU toolchain (gcc/ld/libc-dev) plus GHC's build deps
        // (libgmp, libtinfo/ncurses, libffi, libz, xz to unpack the bindist). This
        // must come from apt, NOT the conda env: conda ships a sysroot-wrapped
        // toolchain (x86_64-conda-linux-gnu-cc/-ld) whose CentOS-style /lib64
        // sysroot does not exist on the Ubuntu root, so GHC's Template Haskell
        // object load fails resolving /lib64/libc.so.6. The conda toolchain is
        // still the right one for building language pools (`morloc make`); only the
        // Haskell build is carved onto the native toolchain (see the stack shim
        // below). The morloc compiler is a standalone glibc binary, so building it
        // against the base OS toolchain is correct and needs no conda coherence.
        out.push_str(
            " build-essential libgmp-dev libncurses-dev libffi-dev zlib1g-dev xz-utils pkg-config",
        );
        // Passwordless sudo for the (non-root, host-UID) dev shell: `sudo` for the
        // capability, `libnss-extrausers` so setuid sudo can resolve the host UID
        // (nss_wrapper is LD_PRELOAD and thus invisible to setuid binaries).
        out.push_str(" sudo libnss-extrausers");
    }
    out.push_str(" \\\n");
    if input.cert_file.is_some() {
        // Fold the corporate CA into the system trust store now that
        // ca-certificates is installed.
        out.push_str(" && update-ca-certificates \\\n");
    }
    out.push_str(&format!(
        " && ln -sf \"$(dpkg -L libnss-wrapper | grep -m1 '/libnss_wrapper\\.so$')\" {} \\\n",
        crate::serve::CONTAINER_NSS_WRAPPER_LIB
    ));
    out.push_str(" && rm -rf /var/lib/apt/lists/*\n");
    out.push('\n');

    // Point the CA env vars at the merged system bundle so later RUNs and runtime
    // processes trust the corporate CA. The bundle includes the public roots, so
    // this cannot break public HTTPS.
    if input.cert_file.is_some() {
        out.push_str("# Trust the corporate CA across the toolchain\n");
        out.push_str("ENV");
        for var in crate::cert::CERT_ENV_VARS {
            out.push_str(&format!(" {var}={}", crate::cert::CONTAINER_CA_BUNDLE));
        }
        out.push('\n');
        out.push('\n');
    }

    // Env-owned container identity. Under `--userns=keep-id` the container runs as
    // the host UID, so the base image's preinstalled user is neither the process
    // owner nor a useful home. Give the env a stable HOME (`CONTAINER_HOME`) that
    // symlinks into the mounted, writable state root, so dotfiles/caches persist.
    out.push_str("# Env-owned home (symlink into the mounted state root)\n");
    out.push_str(&format!(
        "RUN mkdir -p /home && ln -sfn {}/home {}\n\n",
        crate::serve::CONTAINER_MORLOC_STATE,
        crate::serve::CONTAINER_HOME
    ));

    // The heavy (ubuntu) base ships a preinstalled UID/GID-1000 `ubuntu` user that
    // collides with a host UID of 1000 under keep-id. Reclaim it so the env owns
    // that id. Harmless if already absent. (The slim debian base has no such user.)
    if input.base_image.contains("ubuntu") {
        out.push_str("# Reclaim UID/GID 1000 from the preinstalled base user\n");
        out.push_str(
            "RUN userdel -r ubuntu 2>/dev/null || true; groupdel ubuntu 2>/dev/null || true\n\n",
        );
    }

    // Script-provisioned languages (e.g. futhark): their upstream binary is not on
    // conda-forge, so run the committed install.sh (staged into the build context)
    // against the base OS. Runs as root at build; the script does its own apt.
    for lang in input.lang_installs {
        out.push_str(&format!("# {lang}: provisioned by data/lang/{lang}/install.sh\n"));
        out.push_str(&format!("COPY install-{lang}.sh /tmp/morloc-install-{lang}.sh\n"));
        out.push_str(&format!(
            "RUN bash /tmp/morloc-install-{lang}.sh && rm /tmp/morloc-install-{lang}.sh\n\n"
        ));
    }

    // Pinned pixi (the conda package manager).
    out.push_str("# Pinned pixi (conda package manager)\n");
    out.push_str("ENV PIXI_HOME=/opt/pixi\n");
    out.push_str("ENV PATH=\"/opt/pixi/bin:${PATH}\"\n");
    // The `test -x` is load-bearing, not belt-and-braces: a failed download pipes
    // an EMPTY script into bash, which exits 0 and commits an image with no pixi.
    // Without the check the build "succeeds" and the missing binary only surfaces
    // when the environment is materialized.
    out.push_str(&format!(
        "RUN curl -fsSL https://pixi.sh/install.sh | PIXI_VERSION=v{} bash \\\n  && test -x {}\n",
        input.pixi_version,
        crate::serve::CONTAINER_PIXI_BIN
    ));
    out.push('\n');

    // The prebuilt morloc COMPILER + the Rust SOURCE, supplied via the build
    // context. `morloc init` (below) builds libmorloc.so + morloc-nexus from that
    // source with the pixi toolchain, so the runtime is ABI-coherent with the
    // pools. The COPY destination is shared with the run-side PATH
    // (serve::container_path), so it comes from one constant, not a literal.
    //
    // Dev envs skip the COPY + baked MORLOC_RUST_DIR: the developer builds the
    // compiler from their own source checkout and installs it into
    // CONTAINER_RUNTIME_BIN (a mount, not a baked layer), setting MORLOC_RUST_DIR
    // to point at that checkout themselves. Only the PATH entry is kept here, so
    // the built compiler is found.
    let runtime_bin = crate::serve::CONTAINER_RUNTIME_BIN;
    if input.dev {
        out.push_str("# morloc compiler + rust source are built from a mounted source tree\n");
    } else {
        out.push_str("# morloc compiler + rust source (from the build context)\n");
        out.push_str(&format!("COPY runtime/ {runtime_bin}/\n"));
        // COPY bakes the tree root-owned, but the container runs as the host UID
        // (keep-id) and `morloc init` builds libmorloc from this source with cargo,
        // which writes Cargo.lock next to the workspace manifest. Make the Rust
        // source world-writable so that in-place write succeeds (target-dir and
        // CARGO_HOME already point at the writable state mount).
        out.push_str(&format!("RUN chmod -R a+rwX {runtime_bin}/rust\n"));
        out.push_str(&format!("ENV MORLOC_RUST_DIR={runtime_bin}/rust\n"));
    }
    // Both variants put the compiler dir on PATH (baked COPY dest, or a mount).
    out.push_str(&format!("ENV PATH=\"{runtime_bin}:${{PATH}}\"\n"));
    out.push('\n');

    if input.dev {
        // Bake the Haskell toolchain (ghcup + stack) into the dev image, so
        // `stack`/`ghc` are on PATH in an interactive dev shell -- a dev env is a
        // place to build/edit/rebuild the compiler, not just run a prebuilt one
        // (mirrors the project's reference dev container). GHC itself is NOT baked:
        // `stack setup` fetches the exact version stack.yaml pins into
        // `$HOME/.stack` (host-mounted), so it persists and stays authoritative.
        // MINIMAL installs ghcup only; the second step adds a current stack.
        let ghcup_bin = crate::serve::CONTAINER_GHCUP_BIN;
        out.push_str("# Haskell toolchain (ghcup + stack) to build the compiler from source\n");
        out.push_str("ENV GHCUP_INSTALL_BASE_PREFIX=/opt BOOTSTRAP_HASKELL_NONINTERACTIVE=1 BOOTSTRAP_HASKELL_MINIMAL=1\n");
        out.push_str("RUN curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh \\\n");
        out.push_str(&format!("  && {ghcup_bin}/ghcup install stack --set \\\n"));
        // Carve the Haskell build onto the native toolchain. Every container
        // process runs conda-active (the ENTRYPOINT sources conda's activate.d so
        // pool builds get $CC/$AR), which exports CC=x86_64-conda-linux-gnu-cc plus
        // conda CFLAGS/LDFLAGS/CONDA_BUILD_SYSROOT. If `stack setup` runs in that
        // env, GHC bakes conda's sysroot-wrapped cc/ld into its settings and later
        // fails to load Template Haskell objects (libc resolves to a nonexistent
        // /lib64/libc.so.6). Two independent channels leak the conda toolchain into
        // a build -- GHC's baked settings AND the ambient $CC that Configure and
        // custom-Setup.hs packages read directly -- so overriding only GHC's -pgm*
        // is insufficient. Instead wrap `stack` so the WHOLE stack process tree
        // (stack -> ghc -> gcc/ld, and any configure child) sees the native
        // toolchain and no conda compiler env. The `-u` list clears every conda
        // channel that could still route a native-gcc compile through conda's tree
        // -- the compiler flags/sysroot AND the header/lib discovery paths
        // (CPATH/C_INCLUDE_PATH/CPLUS_INCLUDE_PATH/LIBRARY_PATH/LD_LIBRARY_PATH/
        // PKG_CONFIG_PATH/CMAKE_PREFIX_PATH); `env -u` on an unset var is a no-op,
        // so listing ones conda may not export is harmless. `-u` flags MUST precede
        // the NAME=VALUE assignments (GNU env stops option parsing at the first
        // assignment). Replacing ghcup's `stack` in place (rather than shadowing via
        // PATH) is robust: the run-time PATH is set by serve::container_path, which
        // orders /usr/local/bin AFTER the ghcup dir.
        //
        // Prepend /usr/bin so binutils tools Cabal discovers by PATH search rather
        // than env var (strip, ranlib) resolve to the native /usr/bin copies, not
        // the pixi env's. CC/CXX/AR/LD as env vars do NOT cover these: Cabal's
        // stripProgram/ranlibProgram are `simpleProgram` lookups with no env-var
        // channel, and container PATH puts the pixi bin first (serve::container_path
        // + the ENTRYPOINT's pixi shell-hook). Worse, Cabal freezes the resolved
        // path into setup-config at configure time, so a conda `strip` cached there
        // detonates as `posix_spawnp: does not exist` at the later `copy` step once
        // the mutable pixi env re-solves and that exact binary moves/vanishes.
        // Prepending /usr/bin makes configure cache the stable native path instead.
        out.push_str(&format!("  && mv {ghcup_bin}/stack {ghcup_bin}/stack.real \\\n"));
        out.push_str(&format!(
            "  && printf '#!/bin/sh\\nexec env -u LD_LIBRARY_PATH -u LIBRARY_PATH -u CPATH -u C_INCLUDE_PATH -u CPLUS_INCLUDE_PATH -u PKG_CONFIG_PATH -u CMAKE_PREFIX_PATH -u CFLAGS -u CXXFLAGS -u CPPFLAGS -u LDFLAGS -u CONDA_BUILD_SYSROOT PATH=/usr/bin:/bin:$PATH CC=/usr/bin/gcc CXX=/usr/bin/g++ AR=/usr/bin/ar LD=/usr/bin/ld {ghcup_bin}/stack.real \"$@\"\\n' > {ghcup_bin}/stack \\\n"
        ));
        out.push_str(&format!("  && chmod +x {ghcup_bin}/stack\n"));
        out.push_str(&format!("ENV PATH=\"{ghcup_bin}:${{PATH}}\"\n"));
        out.push('\n');

        // Passwordless sudo for the dev shell. The container runs as the host UID,
        // and setuid `sudo` must resolve it via getpwuid() -- but the loader strips
        // the entrypoint's LD_PRELOAD=nss_wrapper from setuid binaries, so sudo
        // cannot see the synthesized user. podman injects a real passwd entry for
        // the keep-id UID (named after the host user), but docker's `--user` leaves
        // it nameless; `libnss-extrausers` (a real NSS module, honored by setuid
        // tools) guarantees an entry on both: point nsswitch at it and have the
        // entrypoint write the UID into its world-writable file. The NOPASSWD rule
        // then targets ALL users, since that name varies by engine.
        let ex = crate::serve::CONTAINER_EXTRAUSERS_DIR;
        out.push_str("# Passwordless sudo for the dev shell (identity via libnss-extrausers)\n");
        out.push_str(
            "RUN sed -i -E '/^(passwd|group):/ s/$/ extrausers/' /etc/nsswitch.conf \\\n",
        );
        out.push_str(&format!(
            "  && install -d {ex} && : > {ex}/passwd && : > {ex}/group && chmod 0666 {ex}/passwd {ex}/group \\\n"
        ));
        // The NOPASSWD rule targets ALL users, not a fixed name: the runtime user
        // is named by whatever provides the passwd entry -- podman injects the host
        // username (e.g. `z`) under keep-id, docker's --user leaves it nameless
        // (resolved via extrausers as `morloc`) -- so a name-specific rule would
        // miss. A single-user throwaway dev container makes ALL the right scope.
        out.push_str(
            "  && printf 'Defaults env_keep += \"http_proxy https_proxy no_proxy HTTP_PROXY HTTPS_PROXY NO_PROXY\"\\nALL ALL=(ALL) NOPASSWD: ALL\\n' > /etc/sudoers.d/nopasswd \\\n",
        );
        out.push_str("  && chmod 0440 /etc/sudoers.d/nopasswd\n");
        out.push('\n');
    }

    // The conda env is NOT baked into the image: it is materialized into a
    // host-mounted /env at env setup and bind-mounted at run, so an in-container
    // `morloc make` can mutate it. This only puts that (mounted-at-run) toolchain
    // bin on PATH -- PATH is the load-bearing part of pixi activation, so language
    // runtimes resolve without re-activating.
    out.push_str("# Toolchain PATH (the conda env is mounted at /env at run time)\n");
    out.push_str(&format!(
        "ENV PATH=\"{}:${{PATH}}\"\n",
        crate::serve::CONTAINER_PIXI_ENV_BIN
    ));
    out.push('\n');

    // The morloc runtime shims (libmorloc.so, morloc-nexus, language bindings)
    // are built by `morloc init` into MORLOC_HOME at env-setup (a container step
    // that bind-mounts MORLOC_HOME), NOT baked here -- so they live in a
    // host-mounted, mutable dir, rebuildable if a dependency bumps the core
    // toolchain. This ENV points at that (mounted-at-run) prefix.
    out.push_str("# MORLOC_HOME (shims materialized into it at env setup, mounted at run)\n");
    out.push_str(&format!("ENV MORLOC_HOME={}\n", input.morloc_home));
    out.push('\n');
    out.push_str(&format!("WORKDIR {}\n", crate::serve::CONTAINER_WORK));
    out.push('\n');

    // Establish container identity for the host UID, then self-activate the conda
    // toolchain -- for EVERY container process (the interactive shell, `morloc
    // make`, and the cargo/cc-rs it spawns; see serve::conda_activate_lines for why
    // activate.d must be sourced). The pixi path is absolute because the run-time
    // PATH does not include /opt/pixi/bin. Both identity paths deliberately do NOT
    // set HOME -- HOME is supplied by the container run env (serve::oci_base_env) so
    // the interactive and served paths agree. Lines avoid single quotes so the
    // single-quote-wrapped printf emitter below reproduces them verbatim.
    //
    // Dev images run interactively as the host UID and need passwordless sudo:
    // setuid `sudo` resolves the UID through getpwuid, which honors real NSS
    // modules but never the LD_PRELOAD nss_wrapper, so dev writes the entry to
    // libnss-extrausers (its nsswitch source). Release images keep nss_wrapper.
    let identity_block: Vec<String> = if input.dev {
        let ex = crate::serve::CONTAINER_EXTRAUSERS_DIR;
        let home = crate::serve::CONTAINER_HOME;
        vec![
            // Fill any missing passwd/group entry (podman's keep-id injection may
            // already provide one; docker --user leaves the UID nameless). The two
            // are independent, so a host that has a passwd entry but no matching
            // group still gets its group synthesized.
            format!(
                "getent passwd \"$(id -u)\" >/dev/null 2>&1 || printf \"morloc:x:%s:%s:morloc:{home}:/bin/bash\\n\" \"$(id -u)\" \"$(id -g)\" >> {ex}/passwd 2>/dev/null || true"
            ),
            format!(
                "getent group \"$(id -g)\" >/dev/null 2>&1 || printf \"morloc:x:%s:\\n\" \"$(id -g)\" >> {ex}/group 2>/dev/null || true"
            ),
            // Match $USER to the resolved identity (podman's injected host username,
            // or extrausers' `morloc`); skip when the UID does not resolve, so USER
            // is never exported as an empty string.
            "_u=\"$(id -un 2>/dev/null)\"; [ -n \"$_u\" ] && export USER=\"$_u\" LOGNAME=\"$_u\"".to_string(),
        ]
    } else {
        // nss_wrapper: synthesize the entry only when the UID has none (keep-id),
        // via an LD_PRELOAD interposer over temp files naming the `morloc` user.
        let lib = crate::serve::CONTAINER_NSS_WRAPPER_LIB;
        let home = crate::serve::CONTAINER_HOME;
        vec![
            format!("if [ -f {lib} ] && ! getent passwd \"$(id -u)\" >/dev/null 2>&1; then"),
            "  _p=\"$(mktemp)\"; _g=\"$(mktemp)\"".to_string(),
            "  if [ -n \"$_p\" ] && [ -n \"$_g\" ]; then".to_string(),
            format!(
                "    printf \"morloc:x:%s:%s:morloc:{home}:/bin/bash\\n\" \"$(id -u)\" \"$(id -g)\" > \"$_p\""
            ),
            "    printf \"morloc:x:%s:\\n\" \"$(id -g)\" > \"$_g\"".to_string(),
            format!(
                "    export LD_PRELOAD={lib} NSS_WRAPPER_PASSWD=\"$_p\" NSS_WRAPPER_GROUP=\"$_g\" USER=morloc LOGNAME=morloc"
            ),
            "  fi".to_string(),
            "fi".to_string(),
        ]
    };
    let script_lines: String = identity_block
        .iter()
        .chain(crate::serve::conda_activate_lines().iter())
        .map(|l| format!(" '{l}'"))
        .collect();
    out.push_str("# Env identity + conda activation for every container process\n");
    out.push_str(&format!(
        "RUN printf '%s\\n' '#!/bin/bash'{script_lines} 'exec \"$@\"' > /usr/local/bin/morloc-activate && chmod +x /usr/local/bin/morloc-activate\n"
    ));
    out.push_str("ENTRYPOINT [\"/usr/local/bin/morloc-activate\"]\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_expected_dockerfile() {
        let extras = BuildExtras {
            system_packages: vec!["libgl1".to_string(), "graphviz".to_string()],
        };
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        let expected = "\
FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive

# Base tools + build-extras (system packages conda cannot provide)
RUN apt-get update \\
 && apt-get install -y --no-install-recommends ca-certificates curl libnss-wrapper libgl1 graphviz \\
 && ln -sf \"$(dpkg -L libnss-wrapper | grep -m1 '/libnss_wrapper\\.so$')\" /usr/local/lib/morloc-nss-wrapper.so \\
 && rm -rf /var/lib/apt/lists/*

# Env-owned home (symlink into the mounted state root)
RUN mkdir -p /home && ln -sfn /opt/morloc-state/home /home/morloc

# Pinned pixi (conda package manager)
ENV PIXI_HOME=/opt/pixi
ENV PATH=\"/opt/pixi/bin:${PATH}\"
RUN curl -fsSL https://pixi.sh/install.sh | PIXI_VERSION=v0.76.2 bash \\
  && test -x /opt/pixi/bin/pixi

# morloc compiler + rust source (from the build context)
COPY runtime/ /opt/morloc-runtime/
RUN chmod -R a+rwX /opt/morloc-runtime/rust
ENV MORLOC_RUST_DIR=/opt/morloc-runtime/rust
ENV PATH=\"/opt/morloc-runtime:${PATH}\"

# Toolchain PATH (the conda env is mounted at /env at run time)
ENV PATH=\"/env/.pixi/envs/default/bin:${PATH}\"

# MORLOC_HOME (shims materialized into it at env setup, mounted at run)
ENV MORLOC_HOME=/opt/morloc

WORKDIR /work

# Env identity + conda activation for every container process
RUN printf '%s\\n' '#!/bin/bash' 'if [ -f /usr/local/lib/morloc-nss-wrapper.so ] && ! getent passwd \"$(id -u)\" >/dev/null 2>&1; then' '  _p=\"$(mktemp)\"; _g=\"$(mktemp)\"' '  if [ -n \"$_p\" ] && [ -n \"$_g\" ]; then' '    printf \"morloc:x:%s:%s:morloc:/home/morloc:/bin/bash\\n\" \"$(id -u)\" \"$(id -g)\" > \"$_p\"' '    printf \"morloc:x:%s:\\n\" \"$(id -g)\" > \"$_g\"' '    export LD_PRELOAD=/usr/local/lib/morloc-nss-wrapper.so NSS_WRAPPER_PASSWD=\"$_p\" NSS_WRAPPER_GROUP=\"$_g\" USER=morloc LOGNAME=morloc' '  fi' 'fi' 'export CONDA_PREFIX=/env/.pixi/envs/default' 'eval \"$(/opt/pixi/bin/pixi shell-hook --manifest-path /env/pixi.toml --shell bash 2>/dev/null)\" || true' 'for f in \"$CONDA_PREFIX/etc/conda/activate.d/\"*.sh; do [ -r \"$f\" ] && . \"$f\"; done' 'exec \"$@\"' > /usr/local/bin/morloc-activate && chmod +x /usr/local/bin/morloc-activate
ENTRYPOINT [\"/usr/local/bin/morloc-activate\"]
";
        assert_eq!(got, expected);
    }

    #[test]
    fn pixi_install_step_verifies_the_binary_landed() {
        // `curl ... | bash` cannot fail the build on its own: a failed download
        // feeds bash an empty script, which exits 0 and commits an image with no
        // pixi -- the failure then surfaces much later, at materialization.
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &BuildExtras::default(),
            lang_installs: &[],
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        // The step is a line-continued RUN, so take it up to the blank line.
        let at = got.find("pixi.sh/install.sh").expect("a pixi install step");
        let step = &got[at..got[at..].find("\n\n").map(|e| at + e).unwrap_or(got.len())];
        assert!(
            step.contains(&format!("test -x {}", crate::serve::CONTAINER_PIXI_BIN)),
            "pixi install must be verified in the same RUN: {step}"
        );
    }

    #[test]
    fn no_extra_packages_still_installs_base_tools() {
        let extras = BuildExtras::default();
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        assert!(got.contains("ca-certificates curl libnss-wrapper \\"));
        assert!(!got.contains("  \\")); // no dangling double space before continuation
    }

    #[test]
    fn lang_install_scripts_are_copied_and_run() {
        let extras = BuildExtras::default();
        let installs = vec!["futhark".to_string()];
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &installs,
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        assert!(got.contains("COPY install-futhark.sh /tmp/morloc-install-futhark.sh"));
        assert!(got.contains("RUN bash /tmp/morloc-install-futhark.sh"));
        // Installed against the base OS, before the pixi install.
        let install_at = got.find("morloc-install-futhark.sh").unwrap();
        let pixi_at = got.find("pixi.sh/install.sh").unwrap();
        assert!(install_at < pixi_at);
    }

    #[test]
    fn dev_dockerfile_omits_copy_runtime() {
        let extras = BuildExtras::default();
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: true,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        // Nothing is baked in: the compiler is built from a mounted source tree.
        assert!(!got.contains("COPY runtime/"));
        assert!(!got.contains("ENV MORLOC_RUST_DIR="));
        // But CONTAINER_RUNTIME_BIN (a mount target) is still on PATH.
        assert!(got.contains(&format!("ENV PATH=\"{}:", crate::serve::CONTAINER_RUNTIME_BIN)));
    }

    #[test]
    fn dev_dockerfile_installs_native_toolchain_and_stack_shim() {
        let extras = BuildExtras::default();
        let ghcup_bin = crate::serve::CONTAINER_GHCUP_BIN;
        let dev = DockerfileInput {
            base_image: "ubuntu:24.04",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: true,
            cert_file: None,
        };
        let got = generate_dockerfile(&dev);
        // The native GNU toolchain + GHC build deps are apt-installed (a dev env
        // builds the Haskell compiler; conda's sysroot-wrapped cc/ld would make GHC
        // seek a nonexistent /lib64/libc.so.6 at Template Haskell load time).
        assert!(got.contains("build-essential"));
        assert!(got.contains("libgmp-dev"));
        // `stack` is wrapped so the whole stack->ghc->gcc/ld tree runs on the native
        // toolchain, with conda's compiler env carved out.
        assert!(got.contains(&format!("mv {ghcup_bin}/stack {ghcup_bin}/stack.real")));
        assert!(got.contains("CC=/usr/bin/gcc"));
        assert!(got.contains("-u CONDA_BUILD_SYSROOT"));
        // /usr/bin is prepended so Cabal's PATH-discovered binutils tools (strip,
        // ranlib) resolve to the native copies rather than the mutable pixi env's.
        assert!(got.contains("PATH=/usr/bin:/bin:$PATH"));
        assert!(got.contains(&format!("{ghcup_bin}/stack.real \"$@\"")));

        // A release (non-dev) image ships a prebuilt compiler, so it gets neither
        // the Haskell build toolchain nor the stack shim.
        let release = DockerfileInput {
            dev: false,
            ..dev
        };
        let got = generate_dockerfile(&release);
        assert!(!got.contains("build-essential"));
        assert!(!got.contains("stack.real"));
    }

    #[test]
    fn dev_dockerfile_enables_passwordless_sudo() {
        let extras = BuildExtras::default();
        let ex = crate::serve::CONTAINER_EXTRAUSERS_DIR;
        let dev = DockerfileInput {
            base_image: "ubuntu:24.04",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: true,
            cert_file: None,
        };
        let got = generate_dockerfile(&dev);
        // sudo + a real NSS module (setuid sudo cannot see the LD_PRELOAD nss_wrapper).
        assert!(got.contains("sudo libnss-extrausers"));
        assert!(got.contains("/etc/nsswitch.conf"));
        // Rule targets ALL users, not a fixed name (the runtime user may be named
        // by podman's passwd injection rather than extrausers).
        assert!(got.contains("ALL ALL=(ALL) NOPASSWD: ALL"));
        // The dev entrypoint resolves identity via extrausers, not nss_wrapper.
        assert!(got.contains(&format!(">> {ex}/passwd")));
        assert!(!got.contains("LD_PRELOAD="));

        // Release images keep nss_wrapper and get no sudo/extrausers.
        let release = DockerfileInput {
            dev: false,
            ..dev
        };
        let got = generate_dockerfile(&release);
        assert!(!got.contains("libnss-extrausers"));
        assert!(!got.contains("NOPASSWD"));
        assert!(got.contains("LD_PRELOAD="));
    }

    #[test]
    fn env_owned_identity_is_baked() {
        let extras = BuildExtras::default();
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        // nss_wrapper installed and symlinked to the fixed entrypoint path.
        assert!(got.contains("libnss-wrapper"));
        assert!(got.contains(crate::serve::CONTAINER_NSS_WRAPPER_LIB));
        // Env-owned home symlinks into the mounted state root.
        assert!(got.contains(&format!(
            "ln -sfn {}/home {}",
            crate::serve::CONTAINER_MORLOC_STATE,
            crate::serve::CONTAINER_HOME
        )));
        // Entrypoint synthesizes the passwd entry but must NOT set HOME (that comes
        // from the container run env).
        assert!(got.contains("NSS_WRAPPER_PASSWD"));
        assert!(got.contains("USER=morloc LOGNAME=morloc"));
        assert!(!got.contains("export HOME="));
        // The slim debian base has no preinstalled `ubuntu` user to reclaim.
        assert!(!got.contains("userdel -r ubuntu"));
    }

    #[test]
    fn heavy_base_reclaims_ubuntu_user() {
        let extras = BuildExtras::default();
        let input = DockerfileInput {
            base_image: "ubuntu:24.04",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: false,
            cert_file: None,
        };
        let got = generate_dockerfile(&input);
        assert!(got.contains("userdel -r ubuntu 2>/dev/null || true"));
        assert!(got.contains("groupdel ubuntu 2>/dev/null || true"));
    }

    #[test]
    fn cert_bundle_is_copied_and_trusted_before_network() {
        let extras = BuildExtras::default();
        let input = DockerfileInput {
            base_image: "debian:bookworm-slim",
            pixi_version: "0.76.2",
            morloc_home: "/opt/morloc",
            extras: &extras,
            lang_installs: &[],
            dev: false,
            cert_file: Some("certs/corp.pem"),
        };
        let got = generate_dockerfile(&input);
        assert!(got.contains("COPY certs/corp.pem /usr/local/share/ca-certificates/morloc-corp.crt"));
        assert!(got.contains("&& update-ca-certificates"));
        assert!(got.contains(&format!(
            "SSL_CERT_FILE={}",
            crate::cert::CONTAINER_CA_BUNDLE
        )));
        assert!(got.contains(&format!(
            "GIT_SSL_CAINFO={}",
            crate::cert::CONTAINER_CA_BUNDLE
        )));
        // cargo reads none of the other CA vars; without CARGO_HTTP_CAINFO its
        // crates.io fetch during the Rust runtime build can't trust the corp CA.
        assert!(got.contains(&format!(
            "CARGO_HTTP_CAINFO={}",
            crate::cert::CONTAINER_CA_BUNDLE
        )));
        // The CA must be trusted before the first network fetch (the pixi
        // installer), so the COPY and the trust ENV precede it.
        let copy_at = got.find("morloc-corp.crt").unwrap();
        let env_at = got.find("SSL_CERT_FILE=").unwrap();
        let pixi_at = got.find("pixi.sh/install.sh").unwrap();
        assert!(copy_at < pixi_at);
        assert!(env_at < pixi_at);
    }
}
