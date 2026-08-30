//! Host capability probe: decide whether the native (no-container) backend is
//! viable on this machine, and report the conda platform string.
//!
//! The native backend runs pool processes directly on the host against a
//! pixi/conda-provided toolchain. On Linux, conda binaries bake the glibc
//! dynamic loader at its conventional FHS path (`/lib64/ld-linux-x86-64.so.2`
//! on x86_64), so a host can run them natively only if it is a glibc + FHS
//! Linux -- or, on NixOS, if we can supply the missing FHS at runtime with a
//! `nix`-built `buildFHSEnv` sandbox (bubblewrap). NixOS is therefore
//! native-capable when `nix` is present AND unprivileged user namespaces are
//! usable (bubblewrap needs them); otherwise it routes to a container. musl
//! systems (Alpine) route to a container. macOS on Apple Silicon runs pool
//! processes against the system dyld and is native-capable; Intel macOS routes
//! to a container because no prebuilt compiler artifact is published for it.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    /// Whether the native backend can run on this host.
    pub native_capable: bool,
    /// Human-readable justification (for messaging / `--backend` errors).
    pub reason: String,
    /// conda platform string, e.g. "linux-64" | "osx-arm64" | "unknown".
    pub platform: String,
}

/// The conventional FHS path of the glibc dynamic loader for an architecture,
/// or None for architectures we don't map. Its presence is the direct test for
/// "can a stock conda binary launch here".
fn glibc_loader_path(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("/lib64/ld-linux-x86-64.so.2"),
        "aarch64" => Some("/lib/ld-linux-aarch64.so.1"),
        _ => None,
    }
}

/// Pure classifier: given the observable facts, decide native capability. Kept
/// separate from the filesystem/env reads in `probe_host` so it is unit-testable.
/// `fhs_capable` is meaningful only on NixOS: it means a `buildFHSEnv` sandbox
/// can be built and run here (nix present + unprivileged user namespaces usable).
fn classify(
    os: &str,
    arch: &str,
    glibc_loader_present: bool,
    is_nixos: bool,
    fhs_capable: bool,
) -> HostProfile {
    let platform = morloc_deps::platform::conda_platform_for(os, arch);
    match os {
        // macOS runs pool processes against a conda toolchain linked by the
        // system dyld (always present), so there is no glibc-loader test as on
        // Linux. Native support tracks published compiler artifacts: only
        // Apple Silicon (arm64) has them, so Intel Macs route to a container.
        "macos" if arch == "aarch64" => HostProfile {
            native_capable: true,
            reason: "macOS on Apple Silicon (native backend supported)".to_string(),
            platform,
        },
        "macos" => HostProfile {
            native_capable: false,
            reason: "no prebuilt morloc runtime is published for Intel macOS; \
                     use a container backend"
                .to_string(),
            platform,
        },
        "linux" => {
            if is_nixos {
                // NixOS lacks a standard FHS, so conda binaries cannot launch
                // directly. We supply one at runtime with a nix-built buildFHSEnv
                // sandbox -- but that needs `nix` (to build it) and unprivileged
                // user namespaces (for bubblewrap to enter it).
                if fhs_capable {
                    HostProfile {
                        native_capable: true,
                        reason: "NixOS via FHS sandbox (native backend supported)".to_string(),
                        platform,
                    }
                } else {
                    HostProfile {
                        native_capable: false,
                        reason: "NixOS needs `nix` and unprivileged user namespaces for the \
                                 native FHS sandbox (enable `security.unprivileged_userns` / \
                                 `boot.kernel.sysctl.\"kernel.unprivileged_userns_clone\"`); \
                                 otherwise use a container backend"
                            .to_string(),
                        platform,
                    }
                }
            } else if glibc_loader_present {
                HostProfile {
                    native_capable: true,
                    reason: "glibc + FHS Linux (native backend supported)".to_string(),
                    platform,
                }
            } else {
                HostProfile {
                    native_capable: false,
                    reason: "no standard glibc dynamic loader (musl or non-FHS host); \
                             use a container backend"
                        .to_string(),
                    platform,
                }
            }
        }
        other => HostProfile {
            native_capable: false,
            reason: format!("{other} is not supported for the native backend"),
            platform,
        },
    }
}

/// Whether this host needs the FHS sandbox to run conda binaries natively. True
/// only on NixOS; glibc-FHS Linux and macOS run them directly.
pub fn fhs_required() -> bool {
    Path::new("/etc/NIXOS").exists()
}

/// Is the `nix-build` executable reachable? This is the SAME tool the FHS-sandbox
/// builder (`fhs::ensure_fhs_wrapper`) invokes, so the capability gate and the
/// build agree -- probing for a different binary (e.g. the `nix` CLI) could pass
/// the gate and then fail at build time. Checked at the standard NixOS locations
/// and on PATH (a NixOS host always has it; a non-NixOS host with the Nix package
/// manager may too).
fn nix_build_available() -> bool {
    for p in [
        "/run/current-system/sw/bin/nix-build",
        "/nix/var/nix/profiles/default/bin/nix-build",
    ] {
        if Path::new(p).exists() {
            return true;
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("nix-build").exists() {
                return true;
            }
        }
    }
    false
}

/// Whether unprivileged user namespaces (which bubblewrap needs to enter the FHS
/// sandbox) are usable. `nix-build` runs through the root daemon and proves
/// nothing about this, so it is a distinct gate. Reads the two kernel knobs:
/// Debian/Ubuntu's `kernel.unprivileged_userns_clone` (absent on stock NixOS =
/// allowed) must not be 0, and `user.max_user_namespaces` must be > 0.
fn unprivileged_userns_available() -> bool {
    let read_int = |p: &str| -> Option<i64> {
        std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<i64>().ok())
    };
    if let Some(0) = read_int("/proc/sys/kernel/unprivileged_userns_clone") {
        return false;
    }
    match read_int("/proc/sys/user/max_user_namespaces") {
        Some(n) => n > 0,
        // Knob absent: user namespaces are compiled out or not exposed; assume
        // unavailable rather than optimistically claiming native capability.
        None => false,
    }
}

/// Probe the current host.
pub fn probe_host() -> HostProfile {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let is_nixos = fhs_required();
    let glibc_loader_present = glibc_loader_path(arch)
        .map(|p| Path::new(p).exists())
        .unwrap_or(false);
    // Only compute the (slightly costlier) FHS facts when they matter.
    let fhs_capable = is_nixos && nix_build_available() && unprivileged_userns_available();
    classify(os, arch, glibc_loader_present, is_nixos, fhs_capable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_silicon_is_native_capable() {
        // Apple Silicon has a published compiler artifact and runs pools against
        // the system dyld, so no glibc-loader test applies (pass false here).
        let arm = classify("macos", "aarch64", false, false, false);
        assert!(arm.native_capable);
        assert_eq!(arm.platform, "osx-arm64");
    }

    #[test]
    fn intel_macos_is_not_native_capable() {
        // No prebuilt compiler artifact is published for Intel macOS, so it
        // routes to a container; the platform string is still reported so the
        // container build targets the right conda platform.
        let intel = classify("macos", "x86_64", false, false, false);
        assert!(!intel.native_capable);
        assert_eq!(intel.platform, "osx-64");
    }

    #[test]
    fn glibc_fhs_linux_is_native_capable() {
        let p = classify("linux", "x86_64", true, false, false);
        assert!(p.native_capable);
        assert_eq!(p.platform, "linux-64");
    }

    #[test]
    fn nixos_with_fhs_sandbox_is_native_capable() {
        // NixOS is native-capable when a buildFHSEnv sandbox can be built and
        // run (nix + unprivileged userns), independent of the glibc-loader test.
        let p = classify("linux", "x86_64", false, true, true);
        assert!(p.native_capable);
        assert!(p.reason.contains("FHS sandbox"));
        assert_eq!(p.platform, "linux-64");
    }

    #[test]
    fn nixos_without_fhs_capability_is_not_native_capable() {
        // No nix or no unprivileged userns -> cannot build/enter the sandbox ->
        // route to a container, with an actionable reason.
        let p = classify("linux", "x86_64", true, true, false);
        assert!(!p.native_capable);
        assert!(p.reason.contains("NixOS") || p.reason.contains("user namespace"));
    }

    #[test]
    fn musl_or_nonfhs_linux_is_not_native_capable() {
        let p = classify("linux", "x86_64", false, false, false);
        assert!(!p.native_capable);
    }

    #[test]
    fn windows_is_not_native_capable() {
        let p = classify("windows", "x86_64", false, false, false);
        assert!(!p.native_capable);
        assert_eq!(p.platform, "unknown");
    }
}
