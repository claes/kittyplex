#!/usr/bin/env python3
"""Linux PTY integration tests. Run after cargo build: python3 tests/integration.py."""

import base64
import errno
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/termlab"
PROMPT = b"TL_PROMPT> "


def can_bind_socket():
    with tempfile.TemporaryDirectory(prefix="termlab-bind-check-") as directory:
        try:
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(directory + "/socket")
            return True
        except PermissionError:
            return False


CAN_BIND = can_bind_socket()


def terminal_settings(fd):
    settings = termios.tcgetattr(fd)
    # glibc and Python can encode the same input baud rate differently in
    # c_cflag. Compare speeds through the separate ispeed/ospeed fields,
    # retaining every other flag and control character in this comparison.
    settings[2] &= ~getattr(termios, "CIBAUD", 0)
    return settings


class TerminalApp:
    def __init__(self, directory, **overrides):
        self.directory = Path(directory)
        wrapper = self.directory / "shell"
        bash = shutil.which("bash")
        wrapper.write_text(
            f"#!{sys.executable}\nimport os\nos.execv({bash!r}, "
            f"[{bash!r}, '--noprofile', '--norc', '-i'])\n"
        )
        wrapper.chmod(0o700)
        env = dict(os.environ, SHELL=str(wrapper), PS1=PROMPT.decode(),
                   INPUTRC="/dev/null", HISTFILE="/dev/null", TERM="xterm-kitty",
                   KITTY_WINDOW_ID="test-window")
        env.pop("TERMLAB_SOCKET", None)
        env.update(overrides)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            # Let the parent snapshot cooked settings before termlab enters raw.
            time.sleep(0.05)
            os.execve(BINARY, [str(BINARY)], env)
        self.original = terminal_settings(self.fd)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        self.buffer = b""
        self.waited = False
        self.endpoint = None

    def start(self):
        registration = self.until(b"\x07")
        match = re.search(rb"SetUserVar=termlab_socket=([^\x07]+)", registration)
        if not match:
            raise AssertionError(f"Missing socket registration: {registration!r}")
        self.endpoint = os.fsdecode(base64.b64decode(match[1]))
        self.until(PROMPT)
        self.command("stty -echo")
        return self

    def until(self, marker, timeout=5):
        deadline = time.monotonic() + timeout
        while marker not in self.buffer:
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timed out waiting for {marker!r}: {self.buffer[-2000:]!r}")
            if select.select([self.fd], [], [], 0.05)[0]:
                try:
                    data = os.read(self.fd, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        raise AssertionError(f"Application exited: {self.buffer!r}") from error
                    raise
                if not data:
                    raise AssertionError("Unexpected PTY EOF")
                self.buffer += data
        end = self.buffer.index(marker) + len(marker)
        data, self.buffer = self.buffer[:end], self.buffer[end:]
        return data

    def command(self, command):
        os.write(self.fd, command.encode() + b"\n")
        return self.until(PROMPT)

    def ctl(self, command):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(3)
            client.connect(self.endpoint)
            client.sendall(command.encode() + b"\n")
            return client.recv(4096).decode().strip()

    def status(self):
        return dict(field.split("=", 1) for field in self.ctl("status").split()[1:])

    def wait(self):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.waited = True
                return os.waitstatus_to_exitcode(status)
            # Drain output so normal shutdown can finish flushing.
            if select.select([self.fd], [], [], 0.02)[0]:
                try:
                    self.buffer += os.read(self.fd, 65536)
                except OSError:
                    pass
        raise AssertionError("Application did not exit")

    def close(self):
        if not self.waited:
            os.kill(self.pid, signal.SIGTERM)
            try:
                self.wait()
            except AssertionError:
                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
        os.close(self.fd)


@unittest.skipUnless(CAN_BIND, "environment denies Unix socket bind")
class Integration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="termlab-test-")
        self.app = TerminalApp(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(self.app.close)
        self.app.start()

    def test_sessions_and_cli(self):
        self.assertIn(b"session=1", self.app.command("printf 'session=%s\\n' \"$TERMLAB_SESSION\""))
        self.assertIn("active=2 sessions=2", self.app.ctl("new"))
        self.app.until(PROMPT)
        self.app.command("stty -echo")
        self.assertIn(b"session=2", self.app.command("printf 'session=%s\\n' \"$TERMLAB_SESSION\""))
        self.assertIn("active=1", self.app.ctl("next"))
        self.app.until(PROMPT)
        self.assertIn("active=2", self.app.ctl("previous"))
        self.app.until(PROMPT)
        self.assertIn("active=1", self.app.ctl("select 1"))
        self.app.until(PROMPT)
        result = subprocess.run([BINARY, "ctl", "--socket", self.app.endpoint, "status"], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(b"sessions=2", result.stdout)

    def test_control_errors_and_partial_clients(self):
        with socket.socket(socket.AF_UNIX) as partial:
            partial.connect(self.app.endpoint)
            partial.sendall(b"sel")
            self.assertIn("sessions=1", self.app.ctl("status"))
            for command in ["select 0", "select 999", "select -1", "nonsense", "new extra"]:
                self.assertTrue(self.app.ctl(command).startswith("ERR "), command)
            self.assertIn(b"alive", self.app.command("printf 'alive\\n'"))

    def test_background_output_does_not_stall(self):
        marker = self.app.directory / "finished"
        program = self.app.directory / "output.py"
        program.write_text("import os, pathlib, time\ntime.sleep(.2)\n"
                           "for _ in range(1024): os.write(1, b'x' * 8192)\n"
                           f"pathlib.Path({str(marker)!r}).touch()\n")
        os.write(self.app.fd, f"{sys.executable} {program}\n".encode())
        self.assertIn("active=2", self.app.ctl("new"))
        self.app.until(PROMPT)
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue(marker.exists(), "Inactive output was not drained")
        self.assertIn("sessions=2", self.app.ctl("status"))

    def test_input_bytes_are_unchanged(self):
        result = self.app.directory / "bytes"
        probe = self.app.directory / "probe.py"
        # Contains legacy Ctrl-B, Ctrl-M, kitty-encoded Ctrl-Shift-M, a
        # management-looking bracketed paste, and a release event.
        payload = b"\x02\r\x1b[109;6u\x1b[200~next\nquit\x1b[201~\x1b[109;6:3u"
        probe.write_text(
            "import os, pathlib, termios, tty\nold=termios.tcgetattr(0)\n"
            "tty.setraw(0)\nprint('RAW_READY', flush=True)\ndata=b''\n"
            f"while len(data)<{len(payload)}: data+=os.read(0,{len(payload)}-len(data))\n"
            f"pathlib.Path({str(result)!r}).write_bytes(data)\n"
            "termios.tcsetattr(0,termios.TCSANOW,old)\n"
        )
        os.write(self.app.fd, f"{sys.executable} {probe}\n".encode())
        self.app.until(b"RAW_READY")
        os.write(self.app.fd, payload)
        self.app.until(PROMPT)
        self.assertEqual(result.read_bytes(), payload)
        self.assertEqual(self.app.status()["sessions"], "1")

    def test_job_control(self):
        os.write(self.app.fd, b"sleep 30\n")
        time.sleep(0.1)
        os.write(self.app.fd, b"\x03")
        self.app.until(PROMPT)
        os.write(self.app.fd, b"sleep 30\n")
        time.sleep(0.1)
        os.write(self.app.fd, b"\x1a")
        self.app.until(PROMPT)
        self.assertIn(b"Stopped", self.app.command("jobs"))
        os.write(self.app.fd, b"fg\n")
        time.sleep(0.1)
        os.write(self.app.fd, b"\x03")
        self.app.until(PROMPT)

    def test_resize_all_sessions(self):
        self.app.ctl("new")
        self.app.until(PROMPT)
        self.app.command("stty -echo")
        fcntl.ioctl(self.app.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 37, 101, 0, 0))
        os.kill(self.app.pid, signal.SIGWINCH)
        time.sleep(0.1)
        self.app.buffer = b""
        self.assertIn(b"37 101", self.app.command("stty size"))
        self.app.ctl("select 1")
        self.app.until(PROMPT)
        self.assertIn(b"37 101", self.app.command("stty size"))

    def test_active_and_last_shell_exit(self):
        self.app.ctl("new")
        self.app.until(PROMPT)
        os.write(self.app.fd, b"exit\n")
        self.app.until(PROMPT)
        self.assertEqual(self.app.status()["sessions"], "1")
        os.write(self.app.fd, b"exit\n")
        self.assertEqual(self.app.wait(), 0)
        self.assertEqual(terminal_settings(self.app.fd), self.app.original)
        self.assertFalse(Path(self.app.endpoint).parent.exists())

    def test_termination_restores_terminal_and_hangs_up_job(self):
        marker = self.app.directory / "job-pid"
        self.app.command(f"sleep 30 & echo $! > {marker}")
        job_pid = int(marker.read_text())
        self.assertEqual(self.app.ctl("quit"), "OK quitting")
        self.assertEqual(self.app.wait(), 0)
        self.assertEqual(terminal_settings(self.app.fd), self.app.original)
        self.assertFalse(Path(self.app.endpoint).exists())
        deadline = time.monotonic() + 2
        while Path(f"/proc/{job_pid}").exists() and time.monotonic() < deadline:
            # An orphan zombie is already dead, awaiting the host's init reaper.
            if Path(f"/proc/{job_pid}/stat").read_text().split()[2] == "Z":
                break
            time.sleep(0.02)
        if Path(f"/proc/{job_pid}/stat").exists():
            self.assertEqual(Path(f"/proc/{job_pid}/stat").read_text().split()[2], "Z")

    def test_sigterm_restores_terminal(self):
        os.kill(self.app.pid, signal.SIGTERM)
        self.assertEqual(self.app.wait(), 0)
        self.assertEqual(terminal_settings(self.app.fd), self.app.original)
        self.assertFalse(Path(self.app.endpoint).parent.exists())

    def test_independent_instances(self):
        with tempfile.TemporaryDirectory(prefix="termlab-other-") as directory:
            other = TerminalApp(directory).start()
            try:
                self.assertNotEqual(other.endpoint, self.app.endpoint)
                self.app.ctl("new")
                self.assertEqual(other.status()["sessions"], "1")
                self.assertEqual(self.app.status()["sessions"], "2")
            finally:
                other.close()


class Startup(unittest.TestCase):
    def test_non_terminal_rejected(self):
        result = subprocess.run([BINARY], capture_output=True, env={k: v for k, v in os.environ.items() if k != "TERMLAB_SOCKET"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"must be a terminal", result.stderr)

    @unittest.skipUnless(CAN_BIND, "environment denies Unix socket bind")
    def test_exec_failure_restores_terminal(self):
        with tempfile.TemporaryDirectory(prefix="termlab-fail-") as directory:
            app = TerminalApp(directory, SHELL="/nonexistent/termlab-shell")
            try:
                self.assertEqual(app.wait(), 1)
                self.assertEqual(terminal_settings(app.fd), app.original)
            finally:
                app.close()

    @unittest.skipIf(CAN_BIND, "requires a socket-restricted environment")
    def test_socket_bind_failure_restores_terminal(self):
        with tempfile.TemporaryDirectory(prefix="termlab-fail-") as directory:
            app = TerminalApp(directory)
            try:
                self.assertEqual(app.wait(), 1)
                self.assertIn(b"bind control socket", app.buffer)
                self.assertEqual(terminal_settings(app.fd), app.original)
            finally:
                app.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
