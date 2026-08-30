#!/bin/sh
# mim installer -- fetch the right prebuilt binary for this system into the
# current directory.
#
#   curl -fsSL https://raw.githubusercontent.com/morloc-project/morloc-manager/main/scripts/install.sh | sh
#
# Optional environment overrides:
#   MIM_VERSION   git tag to install (default: latest release)
#   MIM_DEST      directory to install into (default: current directory)
set -eu

REPO="morloc-project/morloc-manager"
BIN="mim"
VERSION="${MIM_VERSION:-latest}"
DEST="${MIM_DEST:-.}"

err() { printf 'mim-install: %s\n' "$1" >&2; exit 1; }

# --- detect platform ------------------------------------------------------
os_raw="$(uname -s)"
arch_raw="$(uname -m)"

case "$os_raw" in
  Linux)  os="linux" ;;
  Darwin) os="macos" ;;
  *) err "unsupported OS '$os_raw' (need Linux or macOS)" ;;
esac

case "$arch_raw" in
  x86_64|amd64)  arch="x86_64" ;;
  aarch64|arm64) arch="arm64" ;;
  *) err "unsupported architecture '$arch_raw'" ;;
esac

platform="${os}-${arch}"

# Only these platforms are currently published as release assets.
case "$platform" in
  linux-x86_64|linux-arm64|macos-arm64) ;;
  macos-x86_64)
    err "no Intel-macOS binary is published yet (Apple Silicon only).
Build from source, or run under an arm64 toolchain." ;;
  *) err "no prebuilt binary for '$platform'" ;;
esac

asset="${BIN}-${platform}"

# --- resolve download URLs ------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

# --- pick a downloader ----------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  err "need curl or wget on PATH"
fi

mkdir -p "$DEST"
tmp="$(mktemp)"
trap 'rm -f "$tmp" "$tmp.sha256"' EXIT

printf 'mim-install: downloading %s (%s)...\n' "$asset" "$VERSION" >&2
fetch "${base}/${asset}" "$tmp" || err "download failed: ${base}/${asset}"

# --- verify checksum (best-effort; skip only if sidecar is unavailable) ---
if fetch "${base}/${asset}.sha256" "$tmp.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "$tmp.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp" | awk '{print $1}')"
  else
    actual=""
  fi
  if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
    err "checksum mismatch (expected $expected, got $actual)"
  fi
fi

# --- install --------------------------------------------------------------
out="${DEST%/}/${BIN}"
mv "$tmp" "$out"
trap 'rm -f "$tmp.sha256"' EXIT
chmod 755 "$out"

printf 'mim-install: installed %s -> %s\n' "$platform" "$out" >&2
printf 'Run it with: ./%s --help\n' "$BIN" >&2
