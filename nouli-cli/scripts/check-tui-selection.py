#!/usr/bin/env python3
"""Exercise native-copy mode in a real POSIX PTY, without a provider or extra packages."""
import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import tempfile
import termios
import time


class Probe:
    def __init__(self, binary, columns=160, rows=32):
        self.home = tempfile.TemporaryDirectory(prefix="nouli-selection-")
        directory = Path(self.home.name)
        config = directory / "config.toml"
        config.write_text(
            'default_provider = "local"\ndefault_model = "offline"\n'
            '[providers.local]\nprotocol = "system-one"\n'
            'base_url = "http://127.0.0.1:9"\n'
        )
        self.output = bytearray()
        self.exit_code = None
        self.pid, self.fd = pty.fork()
        if not self.pid:
            os.chdir(directory)
            env = dict(os.environ, HOME=str(directory), TERM="xterm-256color", COLORTERM="truecolor")
            env.pop("NO_COLOR", None)
            os.execve(str(Path(binary).resolve()), [
                str(Path(binary).resolve()), "--config", str(config),
                "--store", str(directory / "ledger"), "open", "copy-check",
            ], env)
        self.resize(columns, rows)
        self.wait_for(b"Ready")
        self.wait_for(b"\x1b[?1006h")

    def resize(self, columns, rows):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def pump(self, seconds=0.1):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            readable, _, _ = select.select([self.fd], [], [], max(0, deadline - time.monotonic()))
            if not readable:
                return
            try:
                data = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    return
                raise
            if not data:
                return
            self.output.extend(data)

    def send(self, data):
        os.write(self.fd, data)

    def wait_for(self, text, start=0, timeout=10):
        deadline = time.monotonic() + timeout
        while text not in self.output[start:]:
            self.pump(0.05)
            if time.monotonic() >= deadline:
                raise AssertionError("Expected terminal state was not reached")

    def close(self):
        if self.exit_code is None:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            _, status = os.waitpid(self.pid, 0)
            self.exit_code = os.waitstatus_to_exitcode(status)
        os.close(self.fd)
        self.home.cleanup()


def check(binary):
    terminal = Probe(binary)
    try:
        draft = b"selection-draft-proof"
        terminal.send(b"\x1b[200~" + draft + b"\x1b[201~")
        terminal.wait_for(draft)
        for enter in [b"\x1b[17~", b"\x1b[<0;153;1M"]:
            start = len(terminal.output)
            terminal.send(enter)
            terminal.wait_for(b"\x1b[?1006l", start)
            terminal.wait_for(b"Esc/F6 resume", start)
            terminal.pump(0.1)
            held = len(terminal.output)
            terminal.send(b"z\t\r\x1b[200~do-not-paste\x1b[201~")
            terminal.pump(0.25)
            assert len(terminal.output) == held, "Selection must hold the view without editing or sending"
            terminal.send(b"\x1b")
            terminal.wait_for(b"\x1b[?1006h", held)
        start = len(terminal.output)
        terminal.send(b"\x1b[17~")
        terminal.wait_for(b"\x1b[?1006l", start)
        start = len(terminal.output)
        terminal.resize(100, 24)
        terminal.wait_for(b"\x1b[?1006h", start)
        terminal.wait_for(draft, start)
        assert b"do-not-paste" not in terminal.output[start:], "Copying must preserve the original draft"
        terminal.send(b"\x03\x03")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            terminal.pump(0.05)
            pid, status = os.waitpid(terminal.pid, os.WNOHANG)
            if pid:
                terminal.exit_code = os.waitstatus_to_exitcode(status)
                break
        assert terminal.exit_code == 0, "The TUI must exit cleanly after clearing the test draft"
        assert terminal.output.rfind(b"\x1b[?1006l") > terminal.output.rfind(b"\x1b[?1006h")
        assert b"\x1b[?1049l" in terminal.output, "Alternate screen must be restored"
        assert b"\x1b[?2004l" in terminal.output, "Bracketed paste must be disabled on exit"
        assert b"\x1b]52;" not in terminal.output, "Copying must remain owned by the terminal"
    finally:
        terminal.close()
    print("PASS: native selection, stable view, preserved draft, resize, and terminal cleanup")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nouli", required=True, type=lambda value: Path(value).resolve())
    check(parser.parse_args().nouli)
