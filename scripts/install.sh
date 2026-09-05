#!/bin/sh
# mim installer -- fetch the right prebuilt binary for this system into a
# user-owned bin directory.
#
#   curl -fsSL https://raw.githubusercontent.com/morloc-project/morloc-manager/main/scripts/install.sh | sh
#
# The installer never edits shell startup files. If the destination is not on
# PATH it prints the command to add it and leaves the decision to the user.
#
# Optional environment overrides:
#   MIM_VERSION   git tag to install (default: latest release)
#   MIM_DEST      directory to install into
#                 (default: $XDG_BIN_HOME, else ~/.local/bin)
set -eu

REPO="morloc-project/morloc-manager"
BIN="mim"
VERSION="${MIM_VERSION:-latest}"
DEST="${MIM_DEST:-${XDG_BIN_HOME:-${HOME:?is not set; set MIM_DEST to choose an install directory}/.local/bin}}"

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
# Resolve to an absolute path so the PATH check below compares like with like.
dest_abs="$(cd "$DEST" && pwd)"
out="${dest_abs}/${BIN}"
mv "$tmp" "$out"
trap 'rm -f "$tmp.sha256"' EXIT
chmod 755 "$out"

printf 'mim-install: installed %s -> %s\n' "$platform" "$out" >&2

# --- report PATH status ---------------------------------------------------
# The installer does not edit shell startup files. When the destination is
# missing from PATH it prints the command that would add it.
case ":${PATH}:" in
  *:"$dest_abs":*)
    printf 'Run it with: %s --help\n' "$BIN" >&2
    exit 0
    ;;
esac

# Debian and Ubuntu add ~/.local/bin in the stock ~/.profile, but only when the
# directory already exists at login. A first install is picked up next login.
if [ -n "${HOME:-}" ] && [ "$dest_abs" = "$HOME/.local/bin" ] &&
   grep -qs '\$HOME/\.local/bin' "$HOME/.profile"
then
  printf '\n%s is not on PATH in this shell, but your ~/.profile adds it\n' "$dest_abs" >&2
  printf 'when it exists. It will be on PATH after your next login.\n' >&2
  printf '\nFor this shell:\n    export PATH="%s:$PATH"\n' "$dest_abs" >&2
  printf '\nRun it meanwhile with: %s --help\n' "$out" >&2
  exit 0
fi

# rc is the startup file to suggest, or empty when the command is self-persisting
# (fish) or there is no home directory to put one in.
add="export PATH=\"$dest_abs:\$PATH\""
rc=""
if [ -n "${HOME:-}" ]; then
  case "$(basename "${SHELL:-sh}")" in
    # fish_add_path persists in a universal variable, so there is no file to edit.
    fish) add="fish_add_path $dest_abs" ;;
    zsh)  rc="$HOME/.zshrc" ;;
    # macOS Terminal opens login shells, which read .bash_profile, not .bashrc.
    bash) if [ "$os" = "macos" ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
    *)    rc="$HOME/.profile" ;;
  esac
fi

printf '\n%s is not on your PATH. To add it, run:\n\n' "$dest_abs" >&2
if [ -n "$rc" ]; then
  printf "    echo '%s' >> %s\n\n" "$add" "$rc" >&2
  printf 'Then open a new shell, or apply it to this one with:\n\n' >&2
fi
printf '    %s\n\n' "$add" >&2
printf 'Or skip PATH entirely and run it by full path: %s --help\n' "$out" >&2
