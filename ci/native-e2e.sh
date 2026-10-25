#!/usr/bin/env bash
# End-to-end test of the native backend on a clean host. Builds a fresh
# environment, fetches the demos, and runs the pacman demo's tests (Rust pools
# whose dependencies compile C through cc-rs):
#
#   run     under `mim run`, which applies no startup files; the recorded
#           toolchain must be pinned and carry no conda bookkeeping
#   conda   under `mim shell`, after `conda init` set a conda base to
#           auto-activate from the user's startup file
#   path    `mim shell` with a startup file that resets PATH must warn
#
# The cargo cache is cleared before each build so the C dependencies really
# compile. Usage: native-e2e.sh WORKDIR. Needs `mim` on PATH, $SHELL set to
# bash or zsh, network access, and a HOME whose startup file may be replaced.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=${1:?usage: native-e2e.sh WORKDIR}
env_name=e2e
mkdir -p "$work"
cd "$work"

case "$(basename "${SHELL:-}")" in
  zsh) rc="$HOME/.zshrc" ;;
  bash) rc="$HOME/.bashrc" ;;
  *) echo "native-e2e: \$SHELL must be bash or zsh, not '${SHELL:-}'" >&2; exit 2 ;;
esac

log() { printf '\n=== %s\n' "$*"; }
fail() { echo "native-e2e: $*" >&2; exit 1; }

log "$(mim --version)"
mim new --engine none "$env_name"
version=$(mim run --env "$env_name" -- morloc --version | awk '{print $NF}')
mim demos --morloc-version "$version" --force
set -- "$work"/examples-*/pacman
demo=$1
[ -f "$demo/Makefile" ] || fail "no pacman demo in the demos for morloc $version"
cache=$(mim run --env "$env_name" -- sh -c 'printf %s "${MORLOC_STATE:-$MORLOC_HOME}"')/cache/rust-build
[ -d "$cache" ] || fail "no cargo cache at '$cache' after mim new"

fresh_build() {
  rm -rf "$cache"
  (cd "$demo" && make clean >/dev/null)
}

# Type `input` into `mim shell` under a pty; its output goes to `$work/$1.log`.
shell_session() {
  local name=$1 input=$2 status=0
  python3 "$here/pty-run.py" --ready "($env_name)" --timeout 2400 --input "$input" \
    -- mim shell --env "$env_name" | tee "$work/$name.log" || status=$?
  return "$status"
}

log "recorded toolchain"
mim run --env "$env_name" -- env > "$work/run-env.log"
cc=$(sed -n 's/^CC=//p' "$work/run-env.log")
prefix=$(sed -n 's/^CONDA_PREFIX=//p' "$work/run-env.log")
case "$cc" in "$prefix"/bin/*) ;; *) fail "\$CC='$cc' is not in '$prefix/bin'" ;; esac
if grep -E '^(CONDA_SHLVL|CONDA_BACKUP_[A-Z_]*)=' "$work/run-env.log"; then
  fail "the recorded activation carries conda bookkeeping"
fi

log "build under mim run"
fresh_build
(cd "$demo" && mim run --env "$env_name" -- make test)

# Startup state this script changes (`conda init bash` writes .bash_profile on
# macOS), restored on exit for a local run.
startup_files=("$rc" "$HOME/.bash_profile" "$HOME/.condarc")
rm -f "$work"/*.orig
for f in "${startup_files[@]}"; do
  if [ -e "$f" ]; then cp "$f" "$work/$(basename "$f").orig"; fi
done
restore() {
  for f in "${startup_files[@]}"; do
    if [ -e "$work/$(basename "$f").orig" ]; then cp "$work/$(basename "$f").orig" "$f"; else rm -f "$f"; fi
  done
}
trap restore EXIT

log "build under mim shell with conda base auto-activated"
if [ ! -x "$HOME/miniforge3/bin/conda" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) installer=Miniforge3-MacOSX-arm64.sh ;;
    Linux-x86_64) installer=Miniforge3-Linux-x86_64.sh ;;
    Linux-aarch64) installer=Miniforge3-Linux-aarch64.sh ;;
    *) fail "no Miniforge installer for this host" ;;
  esac
  curl -fsSL -o "$work/miniforge.sh" \
    "https://github.com/conda-forge/miniforge/releases/latest/download/$installer"
  bash "$work/miniforge.sh" -b -p "$HOME/miniforge3"
fi
"$HOME/miniforge3/bin/conda" init "$(basename "$SHELL")"
"$HOME/miniforge3/bin/conda" config --set auto_activate_base true \
  || "$HOME/miniforge3/bin/conda" config --set auto_activate true
fresh_build
cat > "$work/conda-input" <<EOF
env | grep -E '^(CC|CXX|AR|TARGET_CC|CONDA_PREFIX|CONDA_SHLVL|CONDA_BACKUP_CC)=' | sort
cd '$demo' && make test < /dev/null; echo __E2E_RC=\$?
exit
EOF
shell_session conda "$work/conda-input"
grep -q 'mim: warning: your shell startup files changed CONDA_PREFIX' "$work/conda.log" \
  || fail "mim shell did not warn that conda replaced CONDA_PREFIX"
grep -qF "CC=$prefix/bin/" "$work/conda.log" \
  || fail "under conda base, \$CC no longer names the environment's compiler"
if grep -q '^CONDA_BACKUP_CC=' "$work/conda.log"; then
  fail "under conda base, a conda deactivation restored a backed-up \$CC"
fi

log "mim shell with PATH reset by the startup file"
restore
echo 'export PATH=/usr/bin:/bin:/usr/sbin:/sbin' >> "$rc"
printf 'echo __E2E_RC=$(( 0 ))\nexit\n' > "$work/path-input"
shell_session path "$work/path-input"
grep -qF "mim: warning: your shell startup files removed '$prefix/bin' from PATH" "$work/path.log" \
  || fail "mim shell did not warn that PATH lost the environment's bin"

log "all native checks passed"
