#!/usr/bin/env python3
"""Real kitty + ordinary SSH acceptance tests. Requires socket binding and a display/Xvfb."""
import base64
import json
import os
from pathlib import Path
import secrets
import shutil
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from demo import Fixture

INTEGRATION = Path(os.environ.get("KITTYPLEX_TEST_INTEGRATION", ROOT / "kitty"))
BINARY = Path(os.environ.get("KITTYPLEX_TEST_BINARY", ROOT / "target/debug/kittyplex"))


def can_bind():
    try:
        with tempfile.TemporaryDirectory() as directory:
            with socket.socket(socket.AF_UNIX) as sock:
                sock.bind(directory + "/probe")
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
        return True
    except PermissionError:
        return False


CAN_BIND = can_bind()
# Use kitty's own authenticated RC implementation, avoiding a second crypto stack.
if CAN_BIND:
    try:
        from kitty.remote_control import CommandEncrypter
    except ImportError:
        os.execvp("kitty", ["kitty", "+runpy",
            "import runpy; runpy.run_path(" + repr(str(Path(__file__).resolve())) + ", run_name='__main__')", *sys.argv[1:]])


class StartupTests(unittest.TestCase):
    def test_help_describes_carrier_and_does_not_require_a_tty(self):
        result = subprocess.run([BINARY, "--help"], capture_output=True, check=True)
        self.assertIn(b"Kitty-owned", result.stdout)
        self.assertNotIn(b"Inactive output is discarded", result.stdout)

    def test_executable_and_help_use_project_identity(self):
        self.assertEqual(BINARY.name, "kittyplex")
        result = subprocess.run([BINARY, "--version"], capture_output=True, check=True)
        self.assertEqual(result.stdout.strip(), b"kittyplex 0.4.0")
        help_text = subprocess.check_output([BINARY, "run0", "--help"])
        self.assertIn(b"kittyplex run0", help_text)

    def test_command_help_needs_no_terminal_or_controller(self):
        env = dict(os.environ, KITTYPLEX_SOCKET="/nonexistent/control.sock")
        for mode in ['start','run0','ssh','ctl','doctor']:
            for flag in ['--help','-h']:
                result=subprocess.run([BINARY,mode,flag],env=env,capture_output=True,check=True)
                self.assertIn(('Usage: kittyplex '+mode).encode(),result.stdout)
                self.assertNotIn(b'__broker',result.stdout)
        result=subprocess.run([BINARY,'ctl','--socket','/missing','--help'],capture_output=True,check=True)
        self.assertIn(b'Commands:',result.stdout)
        result=subprocess.run([BINARY,'ssh','host','--','--help'],capture_output=True)
        self.assertNotEqual(result.returncode,0)
        self.assertNotIn(b'Usage:',result.stdout)

    def test_invalid_heartbeat_and_nested_start_fail(self):
        result = subprocess.run([BINARY, "--heartbeat-timeout", "1"], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"at least 10", result.stderr)
        result = subprocess.run([BINARY], env=dict(os.environ, KITTYPLEX_SOCKET="/unused"), capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"nested", result.stderr)


@unittest.skipUnless(CAN_BIND, "environment denies Unix/localhost socket binding; real SSH/kitty E2E not run")
class KittySSHCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.xvfb = None
        cls.display = os.environ.get("DISPLAY")
        if shutil.which("Xvfb"):
            read_fd, write_fd = os.pipe()
            cls.xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1280x800x24", "-noreset", "-nolisten", "tcp"],
                                        pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            os.close(write_fd)
            import select
            try:
                if not select.select([read_fd], [], [], 10)[0]:
                    raise RuntimeError("Xvfb did not become ready")
                number = os.read(read_fd, 64).decode().strip()
                if not number:
                    raise RuntimeError("Xvfb failed to start")
                cls.display = ":" + number
            finally:
                os.close(read_fd)
            def stop_display():
                cls.xvfb.terminate()
                cls.xvfb.wait(timeout=5)
            cls.addClassCleanup(stop_display)
        if not cls.display and not os.environ.get("WAYLAND_DISPLAY"):
            raise unittest.SkipTest("no graphical display or Xvfb; real kitty E2E not run")

    def setUp(self):
        self.fixture = Fixture(BINARY, INTEGRATION, rtt_ms=os.environ.get("KITTYPLEX_TEST_RTT_MS", "0"), features=getattr(self, "features", False))
        self.fixture.__enter__()
        self.addCleanup(self.cleanup)
        self.control = self.fixture.directory / "test-control.sock"
        self.password = secrets.token_hex(24)
        config = self.fixture.directory / "kitty/kitty.conf"
        with config.open("a") as stream:
            # Only the isolated test process gets a test-controller capability.
            stream.write(f"remote_control_password {self.password} *\n")
        env = dict(self.fixture.env, LIBGL_ALWAYS_SOFTWARE="1")
        if self.display:
            env["DISPLAY"] = self.display
        log = open(self.fixture.directory / "kitty.log", "wb")
        self.fixture.logs.append(log)
        options = ["--listen-on", "unix:" + str(self.control)]
        if self.display:
            options += ["--override", "linux_display_server=x11"]
        public_key = self.fixture.directory / "kitty-public-key"
        entry = self.fixture.directory / "test-entry.py"
        entry.write_text("import os, pathlib, sys\np = pathlib.Path(" + repr(str(public_key)) +
                         ")\nt = p.with_suffix('.tmp')\nt.write_text(os.environ['KITTY_PUBLIC_KEY'])\n"
                         "t.replace(p)\nos.execvp(sys.argv[1], sys.argv[1:])\n")
        self.process = subprocess.Popen(self.fixture.kitty + options + [shutil.which("python3"), str(entry)] + self.fixture.command, env=env,
                                        stdout=log, stderr=log, start_new_session=True)
        def key_ready():
            if self.process.poll() is not None:
                self.fail("Kitty exited before bootstrap: " + (self.fixture.directory / "kitty.log").read_text(errors="replace"))
            return public_key.exists()
        self.wait_for(key_ready, timeout=15)
        version, key = public_key.read_text().split(":", 1)
        self.encrypt = CommandEncrypter(base64.b85decode(key), version, self.password)
        self.wait_for(lambda: len(self.views()) == 1, timeout=35)
        self.first = self.views()[0]["id"]
        self.wait_text(self.first, "remote session 1")
        self.setup_complete = True

    def cleanup(self):
        outcome = self._outcome.result
        failed = any(test is self for test, _ in outcome.failures + outcome.errors)
        if failed and os.environ.get("KITTYPLEX_TEST_RUN0_USER"):
            print(subprocess.run(["ps", "-eo", "user,pid,ppid,pgid,sid,stat,wchan:25,args"], capture_output=True, text=True).stdout, flush=True)
        process = getattr(self, "process", None)
        if failed and process and process.poll() is None and hasattr(self, "encrypt"):
            try:
                screens = [dict(id=w["id"], user_vars=w.get("user_vars"), text=self.text(w["id"])) for w in self.windows()]
                (self.fixture.directory / "screens.json").write_text(json.dumps(screens, indent=2))
            except (OSError, RuntimeError):
                pass
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        self.fixture.failed = failed or not getattr(self, "setup_complete", False)
        self.fixture.__exit__(None, None, None)

    def rpc(self, cmd, payload):
        response_expected = cmd not in {"send-text", "send-key"}
        command = dict(cmd=cmd, version=[0, 47, 4], no_response=not response_expected, payload=payload)
        wire = b"\x1bP@kitty-cmd" + json.dumps(self.encrypt(command)).encode() + b"\x1b\\"
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect(str(self.control))
            client.sendall(wire)
            if not response_expected:
                return None
            data = b""
            while b"\x1b\\" not in data:
                part = client.recv(65536)
                if not part:
                    raise RuntimeError("kitty control EOF")
                data += part
        response = json.loads(data.split(b"@kitty-cmd", 1)[1].split(b"\x1b\\", 1)[0])
        if not response.get("ok"):
            raise RuntimeError(response.get("error", str(response)))
        return response.get("data")

    def windows(self):
        return [dict(window, tab_id=tab["id"], tab_title=tab["title"],
                     tab_is_active=tab["is_active"], os_window_id=oswindow["id"])
                for oswindow in json.loads(self.rpc("ls", {}))
                for tab in oswindow["tabs"] for window in tab["windows"]]

    def active_window_id(self, os_window_id=None):
        return next((w["id"] for w in self.windows()
                     if w["tab_is_active"] and w["is_active"]
                     and (os_window_id is None or w["os_window_id"] == os_window_id)), None)

    def views(self):
        return [window for window in self.windows() if "kittyplex_session" in window.get("user_vars", {})]

    def wait_for(self, condition, timeout=15):
        deadline = time.monotonic() + timeout * float(os.environ.get("KITTYPLEX_TEST_TIMEOUT_FACTOR", "1"))
        last = None
        while time.monotonic() < deadline:
            try:
                value = condition()
                if value:
                    return value
            except (OSError, RuntimeError) as error:
                last = error
            time.sleep(.02)
        self.fail(f"Timed out; last error: {last}; logs: {self.fixture.directory}")

    def text(self, window):
        return self.rpc("get-text", dict(match=f"id:{window}", extent="all"))

    def wait_text(self, window, marker, timeout=15):
        return self.wait_for(lambda: marker in self.text(window), timeout)

    def send(self, window, text):
        self.rpc("send-text", dict(match=f"id:{window}", data="text:" + text))

    def manage(self, window, *args):
        self.rpc("kitten", dict(match=f"id:{window}", kitten=str(ROOT / "kitty/kittyplex.py"), args=list(args)))

    def create(self, window):
        old = {w["id"] for w in self.views()}
        self.manage(window, "new")
        self.wait_for(lambda: len(self.views()) == len(old) + 1)
        new = next(w["id"] for w in self.views() if w["id"] not in old)
        self.wait_text(new, "remote session")
        return new

    def probe(self, window, operation, *args):
        data=self.rpc("kitten",dict(match=f"id:{window}",kitten=str(ROOT / "tests/kitty_probe.py"),args=[operation,*map(str,args)]))
        return json.loads(data)



class CarrierTests(KittySSHCase):
    def test_startup_instructions_stay_in_carrier_and_appear_once(self):
        carrier = next(w["id"] for w in self.windows()
                       if "kittyplex_session" not in w.get("user_vars", {}))
        instructions = [
            "kittyplex keeps this connection open; shells run in other Kitty tabs.",
            "Keep this tab open. Select a session tab to work.",
            "In a session tab, with default bindings, press Ctrl-Shift-M,",
            "release it, then press:",
            "  c       Create and select a session",
            "  n / p   Next / previous session",
            "  1-9     Select an existing session by position",
            "  q       Close the group and its sessions",
            "Escape cancels; repeat the prefix to send it. Kitty tab selection works too.",
        ]
        self.wait_text(carrier, instructions[-1])
        text = self.text(carrier)
        start = text.index(instructions[0])
        end = text.index(instructions[-1]) + len(instructions[-1])
        lines = text[start:end].splitlines()
        self.assertEqual(lines, instructions)
        self.assertLessEqual(len(lines), 10)

        second = self.create(self.first)
        for line in instructions:
            self.assertEqual(self.text(carrier).count(line), 1)
            for view in (self.first, second):
                self.assertNotIn(line, self.text(view))

        self.manage(second, "quit")
        self.wait_for(lambda: not self.views())
        self.wait_text(carrier, "Back in the original SSH shell")
        self.wait_text(carrier, "remote session carrier")
        self.send(carrier, "printf 'STARTUP_%s\\n' RESTORED\n")
        self.wait_text(carrier, "STARTUP_RESTORED")

    def test_native_tabs_titles_navigation_and_creation_order(self):
        first = next(w for w in self.views() if w["id"] == self.first)
        carrier = next(w for w in self.windows() if "kittyplex_session" not in w.get("user_vars", {}))
        self.assertEqual(first["tab_title"], "kittyplex 1")
        second = self.create(self.first)
        windows = self.windows()
        second_view = next(w for w in windows if w["id"] == second)
        self.assertEqual(len({carrier["tab_id"], first["tab_id"], second_view["tab_id"]}), 3)
        self.assertEqual({w["os_window_id"] for w in windows}, {carrier["os_window_id"]})
        self.assertTrue(all(sum(v["tab_id"] == w["tab_id"] for v in windows) == 1 for w in self.views()))
        self.assertEqual(self.active_window_id(), second)

        # The fixture server is on this machine. Discover its actual remote
        # control socket once, then query it without injecting management input.
        path = self.fixture.directory / "remote-control-path"
        prompts = self.text(self.first).count("remote session 1")
        self.send(self.first, "printf '%s' \"$KITTYPLEX_SOCKET\" >" + shlex.quote(str(path)) + "\n")
        self.wait_for(lambda: path.exists() and path.read_text())
        self.wait_for(lambda: self.text(self.first).count("remote session 1") > prompts)
        def status():
            text = subprocess.check_output([BINARY, "ctl", "--socket", path.read_text(), "status"], timeout=5).decode()
            return dict(item.split("=", 1) for item in text.split())

        self.rpc("focus-tab", dict(match=f'id:{first["tab_id"]}'))
        self.wait_for(lambda: self.active_window_id() == self.first and status()["active"] == "1")
        self.rpc("action", dict(action="next_tab", match_window=f"id:{self.first}"))
        self.wait_for(lambda: self.active_window_id() == second and status()["active"] == "2")
        self.rpc("action", dict(action="previous_tab", match_window=f"id:{second}"))
        self.wait_for(lambda: self.active_window_id() == self.first and status()["active"] == "1")
        self.manage(self.first, "next")
        self.wait_for(lambda: self.active_window_id() == second)
        self.rpc("action", dict(action="move_tab_backward", match_window=f"id:{second}"))
        self.assertEqual([w["id"] for w in self.views()], [second, self.first])
        self.manage(second, "select", "1")
        self.wait_for(lambda: self.active_window_id() == self.first)
        self.manage(self.first, "select", "2")
        self.wait_for(lambda: self.active_window_id() == second)
        self.assertEqual(status()["ids"], "1,2")
        self.send(second, "printf '\\033]2;session-tab-title\\007'\n")
        self.wait_for(lambda: next(w for w in self.views() if w["id"] == second)["tab_title"] == "session-tab-title")

    def test_tab_creation_and_cleanup_preserve_unrelated_windows(self):
        carrier = next(w for w in self.windows() if "kittyplex_session" not in w.get("user_vars", {}))
        first = next(w for w in self.views() if w["id"] == self.first)
        # Even when another OS window is selected, create in the carrier's OS window.
        unrelated = int(self.rpc("launch", dict(type="os-window", args=[shutil.which("sleep"), "120"])))
        second = self.create(self.first)
        second_view = next(w for w in self.views() if w["id"] == second)
        self.assertEqual(second_view["os_window_id"], carrier["os_window_id"])
        self.assertEqual(self.active_window_id(carrier["os_window_id"]), second)
        self.assertEqual(next(w for w in self.windows() if w["id"] == second)["tab_id"],
                         [w for w in self.windows() if w["os_window_id"] == carrier["os_window_id"]][-1]["tab_id"])
        self.rpc("focus-window", dict(match=f"id:{unrelated}"))
        self.assertEqual(len(self.views()), 2)
        # An ordinary window manually added to a session tab is not owned by kittyplex.
        ordinary = int(self.rpc("launch", dict(type="window", match=f'id:{first["tab_id"]}',
                       args=[shutil.which("sleep"), "120"])))
        self.assertEqual(next(w for w in self.windows() if w["id"] == ordinary)["tab_id"], first["tab_id"])
        self.rpc("close-tab", dict(match=f'id:{second_view["tab_id"]}'))
        self.wait_for(lambda: len(self.views()) == 1)
        self.manage(self.first, "quit")
        self.wait_for(lambda: not self.views())
        self.wait_text(carrier["id"], "remote session carrier")
        self.assertEqual(self.active_window_id(carrier["os_window_id"]), carrier["id"])
        self.assertTrue({carrier["id"], unrelated, ordinary} <= {w["id"] for w in self.windows()})
        self.rpc("close-window", dict(match=f"id:{ordinary}"))
        self.rpc("close-window", dict(match=f"id:{unrelated}"))
        self.send(carrier["id"], "kittyplex\n")
        self.wait_for(lambda: len(self.views()) == 1)
        last = self.views()[0]
        self.wait_text(last["id"], "remote session 1")
        prompts = self.text(carrier["id"]).count("remote session carrier")
        self.rpc("close-tab", dict(match=f'id:{last["tab_id"]}'))
        self.wait_for(lambda: not self.views())
        self.wait_for(lambda: self.text(carrier["id"]).count("remote session carrier") > prompts)

    def test_independent_screens_hidden_output_and_native_focus(self):
        self.send(self.first, "printf 'FIRST_%s\\n' SCREEN\n")
        self.wait_text(self.first, "FIRST_SCREEN")
        second = self.create(self.first)
        self.send(second, "printf '\\033[?1049h\\033[2J\\033[HSECOND_%s\\033[?1000h' SCREEN\n")
        self.wait_text(second, "SECOND_SCREEN")
        self.wait_for(lambda: next(w for w in self.views() if w["id"] == second)["in_alternate_screen"])
        self.assertFalse(next(w for w in self.views() if w["id"] == self.first)["in_alternate_screen"])
        for _ in range(3):
            self.manage(second, "previous")
            self.assertIn("FIRST_SCREEN", self.text(self.first))
            self.manage(self.first, "next")
            self.assertIn("SECOND_SCREEN", self.text(second))
        self.send(self.first, "for i in $(seq 1 50); do echo HIDDEN_$i; done\n")
        self.wait_text(self.first, "HIDDEN_50")
        self.assertIn("SECOND_SCREEN", self.text(second))
        self.rpc("focus-window", dict(match=f"id:{self.first}"))
        self.manage(self.first, "next")
        self.wait_for(lambda: self.active_window_id() == second)

    def test_terminal_query_job_control_and_exit_drain(self):
        # Bash read receives the DSR reply produced by this session's kitty screen.
        prompt = "remote session 1"
        previous_prompt = self.text(self.first).count(prompt)
        self.send(self.first, "stty -echo -icanon min 1 time 0; printf '\\033[6n'; if IFS= read -r -d R -t 5 reply; then printf 'QUERY_%s_%q\\n' OK \"$reply\"; else echo QUERY_FAILED; fi; stty icanon\n")
        self.wait_text(self.first, "QUERY_OK_")
        self.wait_for(lambda: self.text(self.first).count(prompt) > previous_prompt)
        self.send(self.first, "printf 'JOB_%s\\n' START; sleep 30\n")
        self.wait_text(self.first, "JOB_START")
        previous_prompt = self.text(self.first).count(prompt)
        self.send(self.first, "\x1a")
        self.wait_text(self.first, "Stopped")
        self.wait_for(lambda: self.text(self.first).count(prompt) > previous_prompt)
        previous_job = self.text(self.first).count("sleep 30")
        self.send(self.first, "fg\n")
        self.wait_for(lambda: self.text(self.first).count("sleep 30") > previous_job)
        previous_prompt = self.text(self.first).count(prompt)
        self.send(self.first, "\x03")
        self.wait_for(lambda: self.text(self.first).count(prompt) > previous_prompt)
        self.send(self.first, "printf 'JOB_%s\\n' RESUMED\n")
        self.wait_text(self.first, "JOB_RESUMED")
        second = self.create(self.first)
        self.send(second, "exit\n")
        self.wait_for(lambda: len(self.views()) == 1)
        self.manage(self.first, "quit")
        self.wait_for(lambda: not self.views())
        carrier = self.windows()[0]["id"]
        self.wait_text(carrier, "Back in the original SSH shell")
        self.wait_text(carrier, "remote session carrier")
        self.send(carrier, "printf 'RESTORED_%s\\n' SHELL\n")
        self.wait_text(carrier, "RESTORED_SHELL")

    def test_vim_state_and_real_resize(self):
        self.send(self.first, "vim -Nu NONE -i NONE /tmp/kittyplex-vim-" + secrets.token_hex(6) + "\n")
        time.sleep(.3)
        self.send(self.first, "iVIM_RETAINED_CONTENT\x1b")
        self.wait_text(self.first, "VIM_RETAINED_CONTENT")
        second = self.create(self.first)
        self.manage(second, "previous")
        self.wait_text(self.first, "VIM_RETAINED_CONTENT")
        self.send(self.first, ":q!\n")
        self.wait_text(self.first, "remote session 1")
        self.rpc("resize-os-window", dict(match=f"id:{second}", action="resize", unit="cells", width=100, height=35))
        time.sleep(.2)
        self.send(second, "printf 'SIZE_%s ' NOW; stty size\n")
        geometry = next(w for w in self.views() if w["id"] == second)
        self.wait_for(lambda: f"SIZE_NOW {geometry['lines']} {geometry['columns']}" in self.text(second))

    def test_two_groups_do_not_control_each_other(self):
        self.rpc("launch", dict(type="tab", args=self.fixture.command))
        self.wait_for(lambda: len(self.views()) == 2, timeout=35)
        second = next(w for w in self.views() if w["id"] != self.first)
        first = next(w for w in self.views() if w["id"] == self.first)
        self.assertNotEqual(first["user_vars"]["kittyplex_group"], second["user_vars"]["kittyplex_group"])
        self.wait_text(second["id"], "remote session 1")
        self.manage(self.first, "quit")
        self.wait_for(lambda: len(self.views()) == 1)
        self.send(second["id"], "printf 'ISOLATED_%s\\n' ALIVE\n")
        self.wait_text(second["id"], "ISOLATED_ALIVE")

    def test_partial_escape_and_utf8_survive_hidden_switch(self):
        second = self.create(self.first)
        self.send(second, "printf 'OTHER_%s\\n' SCREEN\n")
        self.wait_text(second,"OTHER_SCREEN")
        self.manage(second,"previous")
        code = "import os,time; os.write(1,b'PARSER_'+b'READY\\n\\x1b[3'); time.sleep(.5); os.write(1,b'1m\\xe2'); time.sleep(.5); os.write(1,b'\\x82\\xacPARTIAL_'+b'RETAINED\\x1b[0m\\n')"
        self.send(self.first,"python3 -c " + shlex.quote(code) + "\n")
        self.wait_text(self.first,"PARSER_READY")
        self.manage(self.first,"next")
        self.wait_text(self.first,"€PARTIAL_RETAINED")
        self.assertIn("OTHER_SCREEN",self.text(second))
        self.manage(second,"previous")
        self.assertIn("€PARTIAL_RETAINED",self.text(self.first))

    def test_repeated_quit_and_restart_restores_carrier(self):
        carrier = next(w["id"] for w in self.windows() if "kittyplex_session" not in w.get("user_vars", {}))
        view = self.first
        for iteration in range(3):
            previous = self.text(carrier).count("remote session carrier")
            self.manage(view, "quit")
            self.wait_for(lambda: not self.views())
            self.wait_for(lambda: self.text(carrier).count("remote session carrier") > previous)
            self.wait_text(carrier, "remote session carrier")
            marker = f"RESTORED_{iteration}"
            self.send(carrier, f"printf 'RESTORED_%s\\n' {iteration}\n")
            self.wait_text(carrier, marker)
            if iteration != 2:
                self.send(carrier, "kittyplex\n")
                self.wait_for(lambda: len(self.views()) == 1)
                view = self.views()[0]["id"]
                self.wait_text(view, "remote session 1")

    def owned_processes(self, root):
        pending=[root]
        seen=set()
        result=[]
        while pending:
            pid=pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            try:
                args=[os.fsdecode(a) for a in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if a]
                result.append((pid,args))
                for task in Path(f"/proc/{pid}/task").glob("*/children"):
                    pending.extend(map(int,task.read_text().split()))
            except (FileNotFoundError,ProcessLookupError):
                pass
        return result

    def check_failure_returns_to_shell(self, root, internal, signum):
        carrier=next(w["id"] for w in self.windows() if "kittyplex_session" not in w.get("user_vars",{}))
        def target():
            for pid,args in self.owned_processes(root):
                if args and Path(args[0]).name=="kittyplex" and (args[1:2]==[internal] if internal else len(args)==1):
                    return pid
        pid=self.wait_for(target)
        os.kill(pid,signum)
        self.wait_for(lambda:not self.views())
        self.wait_text(carrier,"remote session carrier")
        self.send(carrier,"printf 'RECOVERED_%s\\n' SHELL\n")
        self.wait_text(carrier,"RECOVERED_SHELL")

    def test_broker_crash_cleans_up_and_preserves_shell_input(self):
        self.check_failure_returns_to_shell(self.process.pid,"__broker",signal.SIGKILL)

    def test_controller_termination_restores_the_shell(self):
        self.check_failure_returns_to_shell(self.fixture.server.pid,None,signal.SIGTERM)

    def test_view_close_and_carrier_loss_close_the_group(self):
        second = self.create(self.first)
        self.rpc("close-window", dict(match=f"id:{second}"))
        self.wait_for(lambda: len(self.views()) == 1)
        carrier = next(w["id"] for w in self.windows() if "kittyplex_session" not in w.get("user_vars", {}))
        self.rpc("close-window", dict(match=f"id:{carrier}"))
        self.wait_for(lambda: self.process.poll() is not None, timeout=35)


if __name__ == "__main__":
    result = unittest.main(verbosity=2, exit=False).result
    if not result.wasSuccessful():
        raise RuntimeError("integration tests failed")
