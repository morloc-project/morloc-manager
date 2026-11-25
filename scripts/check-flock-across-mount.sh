#!/bin/sh
# Report whether an flock taken on the host excludes one taken in a container
# on the same file through a bind mount, in both directions (F71, DEP-4).
# Run it on the host, once per engine:
#   ./scripts/check-flock-across-mount.sh docker
#   ./scripts/check-flock-across-mount.sh podman
# A failure to run the probe itself is reported as an error, never as a result.
set -eu

engine=${1:?usage: $0 docker|podman [image]}
image=${2:-docker.io/library/debian:bookworm-slim}
command -v perl >/dev/null || { echo "perl is required" >&2; exit 2; }
busy=75
name="mim-flock-check-$$"

dir=$(mktemp -d "${HOME}/.mim-flock-check.XXXXXX")
cleanup() {
  "$engine" rm -f "$name" >/dev/null 2>&1 || true
  rm -rf "$dir"
}
trap cleanup EXIT INT TERM
lock="$dir/lock"
: > "$lock"

# A volume flag that SELinux lets the container read.
vol="$dir:/d"
if [ -x /usr/sbin/getenforce ] && [ "$(/usr/sbin/getenforce)" = Enforcing ]; then
  vol="$dir:/d:z"
fi

"$engine" pull -q "$image" >/dev/null

# Prints 1 when the host takes the lock without waiting, 0 when it cannot.
host_try() {
  perl -MFcntl=:flock -e 'open(my $f, "<", $ARGV[0]) or die "open: $!\n"; print flock($f, LOCK_EX|LOCK_NB) ? 1 : 0' "$lock"
}

echo "== host holds, container tries"
perl -MFcntl=:flock -e 'open(my $f, "<", $ARGV[0]) or die $!; flock($f, LOCK_EX) or die $!; sleep 30' "$lock" &
holder=$!
sleep 2
set +e
"$engine" run --rm -v "$vol" "$image" flock -n -E "$busy" /d/lock true
status=$?
set -e
kill "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true
case $status in
  0) echo "NOT EXCLUDED: the container took a lock the host holds"; first=no ;;
  "$busy") echo "excluded"; first=yes ;;
  *) echo "error: the container probe failed (status $status)" >&2; exit 1 ;;
esac

echo "== container holds, host tries"
"$engine" run --rm -d --name "$name" -v "$vol" "$image" flock /d/lock sleep 60 >/dev/null
# Wait until the container's own lock is taken: a second flock inside it fails.
held=no
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  set +e
  "$engine" exec "$name" flock -n -E "$busy" /d/lock true >/dev/null 2>&1
  status=$?
  set -e
  if [ "$status" = "$busy" ]; then
    held=yes
    break
  fi
  sleep 1
done
[ "$held" = yes ] || { echo "error: the container never took its lock" >&2; exit 1; }
got=$(host_try)
case $got in
  1) echo "NOT EXCLUDED: the host took a lock the container holds"; second=no ;;
  0) echo "excluded"; second=yes ;;
  *) echo "error: the host probe failed" >&2; exit 1 ;;
esac

echo "== $engine: host->container excluded=$first, container->host excluded=$second"
