//! The CPU architecture a container environment targets.
//!
//! A container image runs Linux, so the OS is fixed; the architecture is the
//! one free dimension, and every tool spells it differently (Rust: `aarch64`,
//! conda: `linux-aarch64`, OCI: `arm64`, the release assets: `linux-arm64`).
//! `Arch` is the user-facing enumeration of the architectures an image can be
//! built for; each spelling comes from the (os, arch) table that already owns
//! it, so a target chosen once at `mim new` reaches the pixi lock, the prebuilt
//! compiler + mim downloads, and every `--platform` flag coherently.
//!
//! An architecture is buildable only when a prebuilt morloc compiler and mim
//! exist for its release triple, conda-forge has its platform, pixi ships a
//! binary for it, and the base image manifest lists it. Adding one is a new
//! variant here, its rows in those tables, and the release matrix that
//! publishes its binaries.

use serde::{Deserialize, Serialize};

use crate::error::{ManagerError, Result};
use crate::types::ContainerEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Arch {
    /// 64-bit x86 (`amd64`).
    #[value(name = "x86_64", alias = "amd64")]
    #[serde(rename = "x86_64")]
    X86_64,
    /// 64-bit ARM (`aarch64`).
    #[value(name = "arm64", alias = "aarch64")]
    #[serde(rename = "arm64")]
    Arm64,
}

impl Arch {
    /// The architecture of the machine running mim. Errors only where mim runs
    /// on a CPU no image is published for, which the release matrix (mim is
    /// built for exactly these architectures) makes unreachable in practice.
    pub fn host() -> Result<Arch> {
        Arch::from_rust_arch(std::env::consts::ARCH).ok_or_else(|| {
            ManagerError::BackendUnsupported(format!(
                "no container image is published for this host's CPU ({})",
                std::env::consts::ARCH
            ))
        })
    }

    /// From Rust's `std::env::consts::ARCH` vocabulary.
    pub fn from_rust_arch(arch: &str) -> Option<Arch> {
        match arch {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Arm64),
            _ => None,
        }
    }

    /// Rust's spelling, the key into the (os, arch) tables.
    fn rust_arch(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "aarch64",
        }
    }

    /// The canonical user-facing name (what `--arch` prints and env.yaml stores).
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    /// The conda platform of a Linux container on this architecture.
    pub fn conda_platform(self) -> String {
        morloc_deps::platform::conda_platform_for("linux", self.rust_arch())
    }

    /// The release triple (asset naming shared by the morloc and mim releases)
    /// of a Linux container on this architecture.
    pub fn release_triple(self) -> &'static str {
        crate::provision::release_triple("linux", self.rust_arch())
            .expect("every Arch has a published Linux release triple")
    }

    /// The OCI `os/arch` platform string docker and podman take in `--platform`.
    pub fn oci_platform(self) -> &'static str {
        match self {
            Arch::X86_64 => "linux/amd64",
            Arch::Arm64 => "linux/arm64",
        }
    }

    /// Whether an image for this architecture runs under emulation on this host.
    pub fn is_foreign_to_host(self) -> bool {
        Arch::host().ok() != Some(self)
    }

    /// The architecture that is not this host's, for tests of the emulated path.
    #[cfg(test)]
    pub fn foreign_to_host() -> Arch {
        match Arch::host().unwrap() {
            Arch::X86_64 => Arch::Arm64,
            Arch::Arm64 => Arch::X86_64,
        }
    }
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The rejection for `--arch` where no cross-architecture build exists: the
/// native backend runs on the host, apptainer builds for it.
pub fn arch_not_supported() -> ManagerError {
    ManagerError::EnvError(
        "--arch applies only to docker/podman environments: the native backend runs \
         on this machine's CPU, and apptainer builds for it"
            .to_string(),
    )
}

/// What a container environment is built for: an engine and an architecture.
/// The one validating constructor holds the rule that apptainer has no
/// cross-architecture build, so a target in hand is always buildable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerTarget {
    pub engine: ContainerEngine,
    pub arch: Arch,
}

impl ContainerTarget {
    /// The target for a new environment: the requested architecture, else the
    /// host's. Apptainer accepts only the host's.
    pub fn new(engine: ContainerEngine, requested: Option<Arch>) -> Result<ContainerTarget> {
        let host = Arch::host()?;
        let arch = requested.unwrap_or(host);
        if arch != host && !engine.is_oci() {
            return Err(arch_not_supported());
        }
        Ok(ContainerTarget { engine, arch })
    }

    /// The architecture the engine is told to build and run at: the target's
    /// for docker/podman, which take `--platform`; none for apptainer, which
    /// has no such flag and builds for the host.
    pub fn platform(self) -> Option<Arch> {
        self.engine.is_oci().then_some(self.arch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_arch_has_every_spelling() {
        assert_eq!(Arch::X86_64.conda_platform(), "linux-64");
        assert_eq!(Arch::X86_64.release_triple(), "linux-x86_64");
        assert_eq!(Arch::X86_64.oci_platform(), "linux/amd64");
        assert_eq!(Arch::X86_64.as_str(), "x86_64");
        assert_eq!(Arch::Arm64.conda_platform(), "linux-aarch64");
        assert_eq!(Arch::Arm64.release_triple(), "linux-arm64");
        assert_eq!(Arch::Arm64.oci_platform(), "linux/arm64");
        assert_eq!(Arch::Arm64.as_str(), "arm64");
    }

    #[test]
    fn rust_arch_vocabulary_maps() {
        assert_eq!(Arch::from_rust_arch("x86_64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_rust_arch("aarch64"), Some(Arch::Arm64));
        assert_eq!(Arch::from_rust_arch("riscv64"), None);
        // mim itself is only published for these two, so the host is always one.
        assert!(Arch::host().is_ok());
    }

    #[test]
    fn the_host_arch_is_never_foreign() {
        assert!(!Arch::host().unwrap().is_foreign_to_host());
        assert!(Arch::foreign_to_host().is_foreign_to_host());
    }

    #[test]
    fn a_target_defaults_to_the_host_and_gates_apptainer() {
        let host = Arch::host().unwrap();
        let foreign = Arch::foreign_to_host();
        for engine in ContainerEngine::ALL {
            assert_eq!(ContainerTarget::new(engine, None).unwrap().arch, host);
            assert_eq!(ContainerTarget::new(engine, Some(host)).unwrap().arch, host);
        }
        for engine in [ContainerEngine::Docker, ContainerEngine::Podman] {
            let t = ContainerTarget::new(engine, Some(foreign)).unwrap();
            assert_eq!(t.arch, foreign);
            assert_eq!(t.platform(), Some(foreign));
        }
        let err = ContainerTarget::new(ContainerEngine::Apptainer, Some(foreign))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--arch"), "{err}");
        // Apptainer builds for the host and is never handed a platform.
        assert_eq!(ContainerTarget::new(ContainerEngine::Apptainer, None).unwrap().platform(), None);
    }
}
