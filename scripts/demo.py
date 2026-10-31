#!/usr/bin/env python3
"""An isolated, real SSH fixture shared by the interactive demo and E2E tests."""
import argparse
import contextlib
import getpass
import os
from pathlib import Path
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time


class Fixture:
    def __init__(self, binary, integration, rtt_ms=0, features=False):
        self.binary = str(Path(binary).resolve())
        self.integration = Path(integration).resolve()
        self.features = features
        self.rtt_ms = float(rtt_ms)
        if not 0 <= self.rtt_ms <= 2000:
            raise ValueError("test RTT must be between 0 and 2000 ms")
        self.directory = Path(tempfile.mkdtemp(prefix="termlab-demo-"))
        self.server = None
        self.failed = True
        self.logs = []

    def __enter__(self):
        try:
            self.start()
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def run(self, args):
        return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)

    def start(self):
        root = self.directory
        for executable in ("sshd", "ssh", "ssh-keygen", "bash", "kitty"):
            if not shutil.which(executable):
                raise RuntimeError(f"Missing {executable}; use nix run .#demo for the complete environment")
        self.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / "host")])
        self.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / "client")])
        shutil.copy(root / "client.pub", root / "authorized_keys")
        (root / "authorized_keys").chmod(0o600)
        # Reserve a candidate port, then retry the bounded bind race if necessary.
        for attempt in range(5):
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", 0))
                self.port = probe.getsockname()[1]
            config = f"""ListenAddress 127.0.0.1
Port {self.port}
HostKey {root}/host
PidFile {root}/sshd.pid
AuthorizedKeysFile {root}/authorized_keys
PubkeyAuthentication yes
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
PermitRootLogin prohibit-password
AllowUsers {getpass.getuser()}
DisableForwarding yes
PermitUserEnvironment no
PrintMotd no
PrintLastLog no
LogLevel VERBOSE
"""
            (root / "sshd_config").write_text(config)
            log = open(root / "sshd.log", "ab", buffering=0)
            self.logs.append(log)
            self.server = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(root / "sshd_config")],
                                           stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                           start_new_session=True)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and self.server.poll() is None:
                try:
                    with socket.create_connection(("127.0.0.1", self.port), timeout=.1):
                        break
                except OSError:
                    time.sleep(.05)
            else:
                if self.server.poll() is None:
                    self.server.terminate()
                    self.server.wait(timeout=5)
                continue
            break
        else:
            raise RuntimeError("The unprivileged localhost SSH server failed to start; inspect sshd.log")
        public = (root / "host.pub").read_text().split()
        (root / "known_hosts").write_text(f"[127.0.0.1]:{self.port} {public[0]} {public[1]}\n")
        self.ssh = ["ssh", "-F", "/dev/null", "-p", str(self.port), "-i", str(root / "client"),
                    "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
                    "-o", "UserKnownHostsFile=" + str(root / "known_hosts"),
                    "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ControlMaster=no",
                    "-o", "ConnectTimeout=5", "-o", "LogLevel=ERROR",
                    getpass.getuser() + "@127.0.0.1"]
        if self.rtt_ms:
            proxy = shlex.join([shutil.which("python3"), str(Path(__file__).with_name("latency_proxy.py")), "%h", "%p", str(self.rtt_ms)])
            self.ssh[-1:-1] = ["-o", "ProxyCommand=" + proxy]
        self.run(self.ssh + ["true"])
        conf = root / "kitty"
        conf.mkdir(mode=0o700)
        for name in ("termlab.py", "termlab_auth.py", "termlab_watcher.py", "termlab.conf"):
            shutil.copy(self.integration / name, conf / name)
        (conf / "kitty.conf").write_text("include termlab.conf\nconfirm_os_window_close 0\nshell_integration disabled\n")
        # Clean Bash startup files, but retain the real user's home and identity.
        (root / "bashrc").write_text("""PS1='remote session ${TERMLAB_SESSION:-carrier} \\w $ '
if [ -z "${TERMLAB_SESSION:-}" ]; then
    printf '\\nConnected through ordinary localhost SSH. Starting termlab.\\n'
    termlab
    printf '\\nBack in the original SSH shell. Type exit to finish the demo.\\n'
fi
""")
        if self.features:
            from feature_demo import assets
            media = assets(root / "feature-assets")
            command = shlex.join([shutil.which("python3"), str(Path(__file__).with_name("feature_demo.py")), "--show", str(media)])
            with (root / "bashrc").open("a") as rc:
                rc.write("\ntermlab-features() { " + command + "; }\n")
                rc.write("if [ -n \"${TERMLAB_SESSION:-}\" ]; then echo 'Run termlab-features for image, sizing and input checks.'; fi\n")
        shell = root / "shell"
        shell.write_text("#!/usr/bin/env bash\nexec " + shlex.quote(shutil.which("bash")) +
                         " --noprofile --rcfile " + shlex.quote(str(root / "bashrc")) + " -i\n")
        shell.chmod(0o700)
        # Login shells may reset PATH; send an explicit environment in the command.
        environment = dict(PATH=str(Path(self.binary).parent) + ":" + os.environ["PATH"],
                           SHELL=str(shell), TERM="xterm-kitty")
        remote = "exec env " + " ".join(shlex.quote(k + "=" + v) for k, v in environment.items()) + " " + shlex.quote(str(shell))
        self.command = self.ssh[:-1] + ["-tt", self.ssh[-1], remote]
        self.env = dict(os.environ, KITTY_CONFIG_DIRECTORY=str(conf), TERMLAB_BINARY=self.binary)
        self.kitty = ["kitty", "--config", str(conf / "kitty.conf"), "--title", "termlab SSH demo"]

    def __exit__(self, exc_type, exc, tb):
        if self.server is not None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(self.server.pid, signal.SIGTERM)
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.server.wait(timeout=5)
        for log in self.logs:
            log.close()
        if exc_type or self.failed:
            print(f"Demo logs and configuration retained at {self.directory}", file=sys.stderr)
        else:
            shutil.rmtree(self.directory)


def main():
    def interrupted(signum, frame):
        raise SystemExit(128 + signum)
    for signum in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--integration", required=True)
    parser.add_argument("--features", action="store_true", help="include guided Kitty feature checks and temporary images")
    args = parser.parse_args()
    if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        parser.exit(1, "A graphical session is required. Run the demo from your desktop terminal.\n")
    print("Starting a private kitty process and real localhost SSH connection.\n"
          "Ctrl-Shift-M, c: new session; n/p or 1–9: switch; q: quit group.\n"
          "Try: echo \"$TERMLAB_SESSION\"; tty\n"
          "Run vim in one session, switch away and back; its screen should persist.\n"
          "Hidden output: for i in $(seq 1 100); do echo counter-$i; sleep .1; done\n"
          "Quit with Ctrl-Shift-M, q, then type exit in the original SSH shell.", flush=True)
    with Fixture(args.binary, args.integration, features=args.features) as fixture:
        print(f"Fixture logs: {fixture.directory}", flush=True)
        log = open(fixture.directory / "kitty.log", "wb")
        fixture.logs.append(log)
        process = subprocess.Popen(fixture.kitty + fixture.command, env=fixture.env,
                                   stdout=log, stderr=log, start_new_session=True)
        try:
            code = process.wait()
            if code:
                raise RuntimeError(f"kitty exited with status {code}; inspect kitty.log")
            fixture.failed = False
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"termlab demo: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.decode(errors="replace"), file=sys.stderr)
        sys.exit(1)
