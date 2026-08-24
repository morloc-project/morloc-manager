---
name: macos-nixos-native-backends
description: Verified code-grounded blockers for adding NixOS-native (nix-ld) and macOS-native (conda-clang) backends that keep pixi/conda as toolchain. Extends the native/container refactor.
metadata:
  type: project
---

Design to add two native backends behind the existing pixi/conda provisioning:
NixOS (nix-ld only, no nixpkgs backend) then macOS (conda clang_osx-arm64 + Xcode
CLT SDK). Extends [[native-container-refactor]]. Sequencing NixOS-first, macOS
gated behind a mac CI smoke. Below are DURABLE, code-grounded facts (verified
2026-08-21) so future sessions skip the grep.

**macOS SHM name-length is a REAL SEV1 blocker (not covered by the design).**
`morloc-nexus/src/process.rs:635` builds the SHM basename
`format!("morloc-{}-{:016x}", pid, job_hash)` (~29 chars), then
`morloc-runtime/src/shm.rs:715/589` appends `_<volume_index>`, and recovery adds
`-gen<N>` (process.rs:34). macOS `shm_open` enforces PSHMNAMLEN=31; this name is
at/over 31 for volume 0 and always over for vol>=10, 6-digit pids, or any
recovery generation -> ENAMETOOLONG. The SHM handoff is THE core IPC path, so
this breaks every macOS run regardless of the linker port. The existing macOS
`#[cfg]` branches (shm.rs errno/`__error`, `ftruncate` preallocate; process.rs
`__error`) are cosmetic and do NOT address naming. "Runtime is ported to macOS"
is overstated: it compiles, the IPC does not fit the name budget.

**Pool-lifecycle PDEATHSIG has no macOS equivalent (SEV2/3 parity gap).**
py `pool.py:337` does `ctypes.CDLL("libc.so.6").prctl(PDEATHSIG)` inside
try/except that silently `pass`es on macOS with a comment "macOS uses kqueue for
this" -- but NO kqueue impl exists. cpp `pool_host.cpp:35`/`pool.cpp` guard prctl
with `#ifdef __linux__`, no Darwin path. nexus `PR_SET_CHILD_SUBREAPER`
(process.rs:1342) also Linux-only. Net: SIGKILL to the nexus orphans pools + leaks
SHM on macOS. nexus's own SIGTERM/SIGINT handler (`kill(-pgid, SIGKILL)`,
process.rs:466) still works, so only the uncatchable-kill path degrades.

**Linker Linux-isms in the pool/shim build layer (M2 scope is real).** Inventory:
- `SystemConfig.hs:203` libmorloc.so link: `-shared -Wl,--whole-archive ... -lrt -ldl` (all GNU/ELF).
- `morloc-nexus/build.rs:19` + `rustmorloc/build.rs:16`: literal `-Wl,-rpath,$ORIGIN` (ld64 rejects `$ORIGIN`, needs `@loader_path`; also nexus/rust-cdylibs need `-install_name`).
- `data/lang/r/init.sh:28-31`: `-Wl,-Bsymbolic-functions -Wl,-z,relro -Wl,-rpath,$ORIGIN` (ld64 rejects all three).
- `data/lang/julia/init.sh:22`: `-shared -fPIC -Wl,-rpath,$ORIGIN`.
- `data/lang/py/setup.py:29`: `-Wl,-rpath,$ORIGIN/../lib` (also py ext on macOS wants `-bundle -undefined dynamic_lookup`).
- `data/lang/r/lang.yaml:23`: hardcoded `librmorloc.so` in `dyn.load` (R's SHLIB_EXT is `.so` on macOS too, so filename OK; the LINK step is what fails).
Only `morloc-nexus/build.rs` exists as a build.rs that bakes rpath; there is NO
PlatformDescriptor abstraction yet ($ORIGIN hardcoded everywhere).

**libmorloc is base-libc/libSystem only (NOT conda-world).** Strict-conda-prefix
rule (`SystemConfig.hs checkCondaCoherence`, abiBearingTools) applies to POOLS +
shims, not libmorloc. So a prebuilt libmorloc.dylib is ABI-legal -- BUT reusing
the withheld release dylib does NOT sidestep the Mach-O port because pools still
need the full rpath/install_name/force_load work, AND the SHM-name bug bites
prebuilt binaries too. Cheapest "prove value" step = fix SHM naming + run one
golden from hand-built artifacts BEFORE building the PlatformDescriptor/init
machinery.

**NixOS: loader-presence IS a sound nix-ld proxy, but NIX_LD propagation is not
free.** nix-ld.enable installs the stub loader at `/lib64/ld-linux-*.so.2`, so
`glibc_loader_present` is true iff nix-ld is on (or real FHS). The stub needs
`NIX_LD` set at RUNTIME; nix-ld only exports it in login-shell env, so a
service/GUI-launched context may lack it. Native run seam
(`main.rs:2272 native_run_env`, `:2494 run_native_morloc_init`) inherits environ
without env_clear and never sets NIX_LD -> if it's absent from the manager's
environ, pools inherit the gap. Design should INJECT/verify NIX_LD, not just warn.
`NIX_LD_LIBRARY_PATH` genuinely NOT needed for self-contained conda binaries
(RPATH into prefix). pixi (glibc binary) also needs the loader, so the gate is
consistent. hostprobe test `nixos_is_not_native_capable_even_with_loader`
(hostprobe.rs:125) is the lock to flip.

**DYLD_LIBRARY_PATH/SIP is a red herring for the SHM handoff.** Pools find
libmorloc via baked LC_RPATH (@loader_path once ported) and get the SHM name via
argv, NOT via DYLD_*. SIP strips DYLD_* only when exec'ing SIP-protected
binaries; conda/user-built pools are not protected, so conda lib resolution
(which itself uses @rpath/install_name, not DYLD_*) is unaffected. Watch only for
a `#!/bin/sh` launcher stripping DYLD_* before reaching nexus -- irrelevant since
nexus uses rpath.
