---
name: container-uid-home-mismatch
description: Recurring bug class - manager runs in-container as host UID but hands it a root-owned/unmounted HOME and tool dirs, causing EACCES/tool-not-found
metadata:
  type: project
---

Recurring failure class in the containerized manager path (docker/podman engines).

The manager runs `morloc init`/`morloc make` INSIDE the container as the non-root
host UID (docker `--user uid:gid`, podman `--userns=keep-id`; container.rs:664-685)
so bind-mounted `/opt/morloc` artifacts are host-owned. But it also hands the
container an environment that assumes root or a mounted HOME:

- Sets `HOME` to the HOST home path (main.rs:2902) which is NOT mounted under
  docker/podman -> unwritable/nonexistent in-container.
- Derives `MORLOC_BIN_LINK_DIR = $HOME/.local/share/morloc/bin` from that
  host HOME (main.rs:2900,2904). SystemConfig.hs:310 `createDirectoryIfMissing`
  throws EACCES; the single outer `try` (SystemConfig.hs:42) aborts ALL of init.
- Replaces PATH (main.rs:2907) omitting cargo; cargo also lives in root-only
  `/root/.cargo` (full/Dockerfile:44) which the non-root UID cannot even traverse.
- Does not set CARGO_HOME -> defaults to unmounted `$HOME/.cargo`.

**Why:** the MORLOC_BIN_LINK_DIR/HOME logic was written for Apptainer, where the
host $HOME IS mounted and writable. Applying it to docker/podman is the bug.

**How to apply:** when reviewing container env/setup changes, check every path the
in-container process writes/execs against "non-root UID + unmounted host HOME +
root-only tool dirs". The graceful path already exists: SystemConfig bin-link
`Nothing` branch (SystemConfig.hs:312-315) SKIPS when the dir is absent; the
manager forcing a bad value is what breaks it. Related: [[project_pymorloc_rpath_fix]],
[[project_morloc_home_config_override]], [[project_deployment_architecture]].
