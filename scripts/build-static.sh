#!/usr/bin/env bash
# Build a fully static (musl) `mim` binary locally -- the SAME artifact the release
# CI (.github/workflows/release.yml) ships, so it runs on any Linux including NixOS
# and minimal containers.
#
# A plain `cargo build --release` targets the host's glibc and will NOT run on
# NixOS. Use this script whenever you need a portable/distributable binary.
#
# Requires rustup and a musl C toolchain for the linker:
#   Debian/Ubuntu:  sudo apt-get install musl-tools
#   Fedora:         sudo dnf install musl-gcc
#   Alpine:         apk add musl-dev
#
# Usage:
#   ./scripts/build-static.sh [target]    # default: this host's arch, musl
set -euo pipefail

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

echo "=== Building static mim for ${target} ==="
# musl targets default to static linking of the C runtime (crt-static), so no
# RUSTFLAGS are needed -- the result is a fully static binary.
rustup target add "$target"
cargo build --release --target "$target" -p mim

mkdir -p out
for bin in mim; do
    src="target/${target}/release/${bin}"
    dst="out/${bin}"
    cp "$src" "$dst"
    strip "$dst" || true
    # Confirm it is fully static (no dynamic loader), matching what CI verifies.
    if ldd "$dst" 2>&1 | grep -q "=>"; then
        echo "WARNING: ${dst} is NOT statically linked" >&2
    else
        echo "OK: ${dst} is fully static"
    fi
done

echo "=== Output in out/ ==="
ls -lh out/
