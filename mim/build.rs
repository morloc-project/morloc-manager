//! Bake the real release tag into the binary when building in CI.
//!
//! `mim` fetches the platform-matched `mim` (its in-environment dependency agent)
//! for a cross-arch container build from `releases/download/<tag>/mim-<triple>`. To
//! target the ACTUAL release rather than a tag reconstructed from the crate
//! version, CI (a tag push) exposes the ref as `GITHUB_REF_NAME`; we bake it as
//! `MIM_RELEASE_TAG`. Local/non-CI builds leave it unset, and the runtime falls
//! back to `v<CARGO_PKG_VERSION>` -- reached only by a cross-arch build, which a
//! local checkout rarely does and can always override with `MORLOC_MIM_ENV`.

use std::env;

fn main() {
    if let Ok(tag) = env::var("GITHUB_REF_NAME") {
        if tag.starts_with('v') {
            println!("cargo:rustc-env=MIM_RELEASE_TAG={tag}");
        }
    }
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
}
