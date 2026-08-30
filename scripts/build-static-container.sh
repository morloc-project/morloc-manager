#!/usr/bin/env bash
# Build the fully static (musl) mim + mim-env release binaries in a dedicated
# throwaway container, so a dev environment (conda Rust, host-user identity) is
# never burdened with rustup/musl. Output lands in ./out, owned by the caller.
#
# This wraps scripts/build-static.sh (the shared build+verify core) in an image
# that provides rustup + musl; use scripts/build-static.sh directly when the host
# already has that toolchain.
#
# Usage:
#   ./scripts/build-static-container.sh [target-triple]
# Default target is the build host's arch. A non-host arch needs a cross linker
# and is left to release CI.
set -euo pipefail

# Run from the repo root (this script lives in scripts/).
cd "$(dirname "$0")/.."

if [ "$#" -ge 1 ]; then
    target="$1"
else
    case "$(uname -m)" in
        x86_64)        target="x86_64-unknown-linux-musl" ;;
        aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
        *)
            echo "Unsupported host arch '$(uname -m)'; pass a target triple explicitly." >&2
            exit 1
            ;;
    esac
fi

# Prefer docker, fall back to podman.
if command -v docker >/dev/null 2>&1; then
    engine=docker
elif command -v podman >/dev/null 2>&1; then
    engine=podman
else
    echo "Neither docker nor podman found on PATH." >&2
    exit 1
fi

echo "=== Building static mim + mim-env for ${target} (via ${engine}) ==="
DOCKER_BUILDKIT=1 "$engine" build \
    --build-arg "TARGET=${target}" \
    -t mim-static-build \
    -f scripts/static-build.Dockerfile \
    .

# Extract with `<engine> cp` from a created (never run) container. The CLI writes
# to the host as the invoking user, sidestepping bind-mount ownership, SELinux
# relabeling, and rootless-userns mapping -- all of which broke a `-v out:/out`
# approach.
mkdir -p out
cid=$("$engine" create mim-static-build)
trap '"$engine" rm -f "$cid" >/dev/null 2>&1 || true' EXIT
"$engine" cp "$cid:/artifacts/mim" out/mim
"$engine" cp "$cid:/artifacts/mim-env" out/mim-env

echo "=== Output in out/ ==="
ls -lh out/mim out/mim-env
