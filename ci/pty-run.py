#!/usr/bin/env python3
"""Drive an interactive command through a pseudo-terminal.

    pty-run.py --ready TEXT --timeout SECS --input FILE -- CMD [ARGS...]

Starts CMD on a pty, waits until its output contains TEXT (the shell's
prompt) and then falls quiet, types the lines of FILE, and copies everything
CMD prints to stdout. FILE must end by printing `__E2E_RC=<status>` and
exiting the shell; that status is this script's exit status. A missing
marker or a timeout exits 1.
"""

import argparse
import os
import pty
import re
import select
import sys
import time

QUIET_SECS = 2.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ready", required=True)
    ap.add_argument("--timeout", type=float, default=2400)
    ap.add_argument("--input", required=True)
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    with open(args.input, "rb") as f:
        script = f.read()

    pid, fd = pty.fork()
    if pid == 0:
        os.environ.setdefault("TERM", "xterm-256color")
        os.execvp(cmd[0], cmd)

    out = bytearray()
    sent = False
    last_output = time.monotonic()
    deadline = last_output + args.timeout
    status = None
    while status is None:
        now = time.monotonic()
        if now > deadline:
            sys.stdout.write("\npty-run: timed out\n")
            os.kill(pid, 9)
            os.waitpid(pid, 0)
            sys.exit(1)
        r, _, _ = select.select([fd], [], [], 0.5)
        if fd in r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                chunk = b""
            if not chunk:
                # The pty closed: nothing more to read, so wait for the shell.
                status = os.waitpid(pid, 0)[1]
                break
            out += chunk
            last_output = time.monotonic()
            sys.stdout.buffer.write(chunk)
            sys.stdout.flush()
        if (not sent and args.ready.encode() in out
                and time.monotonic() - last_output > QUIET_SECS):
            os.write(fd, script)
            sent = True
        done, st = os.waitpid(pid, os.WNOHANG)
        if done:
            status = st
    while select.select([fd], [], [], 0.2)[0]:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
        sys.stdout.buffer.write(chunk)
    sys.stdout.flush()
    found =re.findall(rb"__E2E_RC=(\d+)", bytes(out))
    if not sent or not found:
        sys.stdout.write("\npty-run: the shell never reported a status\n")
        sys.exit(1)
    sys.exit(int(found[-1]))


if __name__ == "__main__":
    main()
