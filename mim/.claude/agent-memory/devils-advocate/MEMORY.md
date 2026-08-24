# Devil's Advocate Memory Index

- [Container UID/HOME mismatch](project_container_uid_home_mismatch.md) — manager runs as host UID in-container but hands it a root-owned/unmounted HOME + tool dirs; recurring EACCES/tool-not-found class.
- [Native/container two-axis refactor](project_native_container_refactor.md) — design risks in the pixi-lowered native+container backend refactor: apt-package gap, no-solve/merge_constraint concat, flag-reuse silent breakage, platform-bound lock vs freeze.
- [Dev-env schema redesign](project_dev_env_schema.md) — risks in `--dev` explicit opt-in + per-env dev block: overloaded morloc_version reincarnates orig bug, drop is_dev for tagged block, host-compiler-in-container go/no-go, external!=build behavior fork, Model 1 doesn't serve onboarding.
- [macOS/NixOS native backends](project_macos_nixos_native_backends.md) — verified blockers: macOS SHM name >31 (PSHMNAMLEN) SEV1; PDEATHSIG no-Darwin gap; linker $ORIGIN/-lrt/-z isms inventory; NIX_LD propagation not free; DYLD/SIP is a red herring for SHM.
