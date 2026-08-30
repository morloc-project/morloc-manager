# syntax=docker/dockerfile:1.7
# Portable static (musl) build of the mim release binary -- the SAME artifact
# release CI (.github/workflows/release.yml) ships, so it runs on any Linux
# including NixOS and minimal containers.
#
# This is RELEASE tooling, deliberately separate from a dev environment: a dev
# env builds the morloc compiler + runtime with the conda toolchain and runs as
# the host user, which cannot host rustup or a musl target. Building the static
# manager binaries is an unrelated packaging task, so it gets its own throwaway
# image with a real root user, rustup, and musl -- none of the dev env's managed
# identity or conda-primary Rust applies here.
#
# Build + extract (from the repo root):
#   DOCKER_BUILDKIT=1 docker build -t mim-static-build -f scripts/static-build.Dockerfile .
#   cid=$(docker create mim-static-build)
#   docker cp "$cid:/artifacts/." out/ && docker rm "$cid"
# Or just: ./scripts/build-static-container.sh
#
# Artifacts are extracted with `docker cp` (not a bind mount): the CLI writes
# them to the host as the invoking user, so there is no volume-ownership, SELinux
# relabel, or rootless-userns mapping to fight. Output:
#   ./out/mim  (a fully static musl binary)
#
# TARGET must match the BUILD HOST arch (musl-tools provides only the native musl
# linker); cross-arch builds need a cross linker and are left to CI.

# ===========================================================================
# Stage 1: Build the static musl binaries. The official rust image ships rustup
# + cargo, so the musl std installs with `rustup target add` and the toolchain
# is independent of the host.
# ===========================================================================
FROM docker.io/library/rust:1-bookworm AS builder

ARG TARGET=x86_64-unknown-linux-musl

# musl-tools supplies musl-gcc, the linker cargo invokes for the musl target
# (the rust base already provides rustup + cargo). The musl std itself is added
# by scripts/build-static.sh below.
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# Reuse the same script the host fast-path and this container share: it adds the
# musl target, builds -p mim, strips, and verifies the binary is fully static.
# Output lands in ./out.
RUN bash scripts/build-static.sh "${TARGET}"

# ===========================================================================
# Stage 2: Hold the artifacts in a tiny image. The build.sh wrapper extracts
# them with `docker cp` from a created (never run) container, so this stage only
# needs to carry the files at a known path.
# ===========================================================================
FROM docker.io/library/debian:bookworm-slim

COPY --from=builder /src/out /artifacts

CMD ["ls", "-lh", "/artifacts"]
