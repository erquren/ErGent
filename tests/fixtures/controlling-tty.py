#!/usr/bin/env python3
"""Run an E2E child with an isolated controlling terminal and piped logs.

The JSON stream keeps /dev/tty output distinct from stdout/stderr so tests can
assert that synthetic first-account setup codes never enter ordinary logs.
No production behavior is changed and no credentials are written to disk.
"""

import argparse
import errno
import fcntl
import json
import os
import pty
import selectors
import signal
import subprocess
import sys
import termios


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-terminal", action="store_true")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("a child command is required")

    terminal = None
    slave = None
    if not args.no_terminal:
        terminal, slave = pty.openpty()

    def claim_terminal():
        # Popen's start_new_session has already called setsid. Merely wiring a
        # PTY to stdin is insufficient: /dev/tty requires a controlling tty.
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    child = subprocess.Popen(
        command,
        stdin=slave if slave is not None else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        preexec_fn=claim_terminal if slave is not None else None,
    )
    print(json.dumps({"pid": child.pid}), flush=True)
    if slave is not None:
        os.close(slave)

    def forward_signal(signum, _frame):
        try:
            os.killpg(child.pid, signum)
        except ProcessLookupError:
            pass

    signal.signal(signal.SIGINT, forward_signal)
    signal.signal(signal.SIGTERM, forward_signal)
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ, "stdout")
    selector.register(child.stderr, selectors.EVENT_READ, "stderr")
    if terminal is not None:
        selector.register(terminal, selectors.EVENT_READ, "terminal")
    try:
        while selector.get_map():
            for key, _events in selector.select(timeout=0.5):
                try:
                    data = os.read(key.fd, 65536)
                except OSError as error:
                    # Linux signals the closed slave side of a PTY as EIO.
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                if data:
                    print(
                        json.dumps(
                            {"stream": key.data, "data": data.decode("utf-8", "replace")}
                        ),
                        flush=True,
                    )
                else:
                    selector.unregister(key.fileobj)
                    if isinstance(key.fileobj, int):
                        os.close(key.fileobj)
                    else:
                        key.fileobj.close()
        status = child.wait()
        return status if status >= 0 else 128 - status
    finally:
        selector.close()
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()


if __name__ == "__main__":
    sys.exit(main())
