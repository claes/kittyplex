"""Run inside kitty's Python: see README.md. Does not open a GUI."""

from pathlib import Path
import runpy
import json
import os
import shutil
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from kitty.config import load_config
from kitty.fast_data_types import KeyEvent
from kitty.keys import Mappings
from kitty.options.utils import parse_shortcut

ROOT = Path(__file__).resolve().parents[1]


class MappingHarness(Mappings):
    def __init__(self, registered=True):
        errors = []
        self.options = load_config(str(ROOT / "kitty/termlab.conf"), accumulate_bad_lines=errors)
        assert not errors, errors
        self.window = SimpleNamespace(user_vars={"termlab_socket": "/tmp/test.sock"} if registered else {})
        self.actions = []
        self.bells = 0
        super().__init__(global_shortcuts={})

    def get_options(self):
        return self.options

    def get_active_window(self):
        return self.window

    def match_windows(self, expression):
        assert expression == "var:termlab_socket", expression
        return iter([self.window] if self.window.user_vars else [])

    def set_ignore_os_keyboard_processing(self, value):
        pass

    def ring_bell(self):
        self.bells += 1

    def combine(self, definition):
        if definition.startswith("push_keyboard_mode "):
            self.push_keyboard_mode(definition.split()[1])
        elif definition == "pop_keyboard_mode":
            self.pop_keyboard_mode()
        else:
            self.actions.append(definition)
        return True

    def press(self, shortcut):
        key = parse_shortcut(shortcut)
        return self.dispatch_possible_special_key(KeyEvent(key=key.key, mods=key.mods))


class MappingTests(unittest.TestCase):
    def test_all_management_actions_exit_mode(self):
        expected = {"c": "new", "n": "next", "p": "previous", "q": "quit"}
        expected.update({str(i): f"select {i}" for i in range(1, 10)})
        for key, command in expected.items():
            with self.subTest(key=key):
                mappings = MappingHarness()
                self.assertTrue(mappings.press("ctrl+shift+m"))
                self.assertEqual(mappings.current_keyboard_mode_name, "termlab")
                self.assertTrue(mappings.press(key))
                self.assertEqual(mappings.current_keyboard_mode_name, "")
                self.assertEqual(mappings.actions, [f"kitten termlab.py {command}"])

    def test_prefix_passes_through_outside_termlab(self):
        mappings = MappingHarness(registered=False)
        self.assertFalse(mappings.press("ctrl+shift+m"))
        self.assertEqual(mappings.current_keyboard_mode_name, "")

    def test_enter_and_normal_typing_are_unreserved(self):
        mappings = MappingHarness()
        for key in ["ctrl+m", "enter", "m", "c", "n", "1", "q"]:
            self.assertFalse(mappings.press(key), key)

    def test_literal_chord(self):
        mappings = MappingHarness()
        mappings.press("ctrl+shift+m")
        self.assertTrue(mappings.press("ctrl+shift+m"))
        self.assertEqual(mappings.actions, ["send_key ctrl+shift+m"])
        self.assertEqual(mappings.current_keyboard_mode_name, "")

    def test_escape_and_unknown_key(self):
        mappings = MappingHarness()
        mappings.press("ctrl+shift+m")
        self.assertTrue(mappings.press("escape"))
        self.assertEqual(mappings.bells, 0)
        self.assertEqual(mappings.current_keyboard_mode_name, "")
        mappings.press("ctrl+shift+m")
        self.assertTrue(mappings.press("x"))
        self.assertEqual(mappings.bells, 1)
        self.assertEqual(mappings.current_keyboard_mode_name, "")
        self.assertEqual(mappings.actions, [])


class HelperTests(unittest.TestCase):
    def adapter(self):
        return runpy.run_path(str(ROOT / "kitty/termlab.py"))

    def test_bootstrap_without_file_global_uses_kitty_config_directory(self):
        from kitty.constants import config_dir
        from kitty.window import Watchers
        module = {"__name__": "termlab_loader_regression"}
        exec(compile((ROOT / "kitty/termlab.py").read_text(), "termlab.py", "exec"), module)
        self.assertEqual(module["DIRECTORY"], config_dir)
        variables = {}
        window = SimpleNamespace(id=1, user_vars=variables, watchers=Watchers(),
            set_user_var=lambda key, value: variables.__setitem__(key, value))
        boss = SimpleNamespace(window_id_map={1: window})
        m = module["event"]("a" * 32, "bootstrap")
        m["n"] = 30
        with patch.dict(os.environ, TERMLAB_BINARY="/tmp/termlab-test-binary"), patch("kitty.launch.launch") as launch:
            state = module["start"](boss, window, m)
        self.addCleanup(shutil.rmtree, state["directory"])
        self.assertIn(os.path.join(config_dir, "termlab.py"), launch.call_args.args[2])
        self.assertIn("child_death_callback", launch.call_args.kwargs)

    def test_probe_is_read_only_and_requires_scoped_control(self):
        module = self.adapter()
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(user_vars={})})
        nonce = "a" * 32
        with patch.dict(module["handle_result"].impl.__globals__, get_options=lambda: SimpleNamespace(allow_remote_control="password")), patch.dict(os.environ, TERMLAB_BINARY="/bin/sh"):
            response = json.loads(module["handle_result"](["termlab.py", "probe", nonce], None, 1, boss))
        errors = []
        boss.show_error = lambda *args: errors.append(args)
        with patch.dict(module["handle_result"].impl.__globals__, get_options=lambda: SimpleNamespace(allow_remote_control="yes")):
            self.assertEqual(module["handle_result"](["termlab.py", "probe", nonce], None, 1, boss), "busy")
        self.assertTrue(errors)
        self.assertEqual((response["nonce"], response["desktop_api"]), (nonce, 1))
        self.assertFalse(hasattr(boss, "_termlab_groups"))
        self.assertEqual(boss.window_id_map[1].user_vars, {})

    def test_routes_nonblocking_datagram_to_originating_window(self):
        module = self.adapter()
        received = []
        class Client:
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def setblocking(self, value): received.append(("blocking", value))
            def sendto(self, data, endpoint): received.append((json.loads(data), endpoint))
        errors = []
        boss = SimpleNamespace(window_id_map={
            n: SimpleNamespace(user_vars={"termlab_socket": f"/tmp/{n}.sock",
                "termlab_group": "a" * 32, "termlab_session": str(n)}) for n in (1, 2)
        }, show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", return_value=Client()):
            module["handle_result"](["termlab.py", "new"], None, 2, boss)
        self.assertIn(("blocking", False), received)
        message, endpoint = received[-1]
        self.assertEqual(endpoint, "/tmp/2.sock")
        self.assertEqual((message["kind"], message["id"], message["data"]), ("command", 2, "new"))
        self.assertFalse(errors)

    def test_v2_geometry_fields_and_legacy_rejection(self):
        module=self.adapter()
        message=module["event"]("a"*32,"heartbeat")
        self.assertEqual(message["v"],2)
        self.assertEqual((message["pixel_width"],message["pixel_height"]),(0,0))
        for key in ("pixel_width","pixel_height"):
            message[key]=65536
            with self.assertRaises(ValueError):module["validate"](json.dumps(message))
            message[key]=0
        message["v"]=1
        with self.assertRaises(ValueError):module["validate"](json.dumps(message))

    def test_stale_endpoint_shows_bounded_error(self):
        module = self.adapter()
        errors = []
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(user_vars={
            "termlab_socket": "/tmp/missing.sock", "termlab_group": "a" * 32})},
            show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", side_effect=OSError("missing")):
            module["handle_result"](["termlab.py", "next"], None, 1, boss)
        self.assertEqual(len(errors), 1)

    def test_carrier_duplicate_ack_and_busy_retry(self):
        module = self.adapter()
        message = module["event"]("a" * 32, "heartbeat")
        message["seq"] = 2
        state = dict(carrier=1, directory="/tmp/test", seq=1)
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(id=1, user_vars={})},
            _termlab_groups={"a" * 32: state}, show_error=lambda *args: self.fail(str(args)))
        client = SimpleNamespace(setblocking=lambda v: None, sendto=lambda *args: None)
        class Context:
            def __enter__(self): return client
            def __exit__(self, *args): pass
        with patch.object(module["socket"], "socket", return_value=Context()):
            args = ["termlab.py", "carrier", json.dumps(message)]
            self.assertEqual(module["handle_result"](args, None, 1, boss), "accepted:2")
            self.assertEqual(module["handle_result"](args, None, 1, boss), "accepted:2")
        message["seq"] = 3
        with patch.object(module["socket"], "socket", side_effect=BlockingIOError()):
            self.assertEqual(module["handle_result"](["termlab.py", "carrier", json.dumps(message)], None, 1, boss), "busy")
        self.assertEqual(state["seq"], 2)


class AuthorizationTests(unittest.TestCase):
    def setUp(self):
        adapter = runpy.run_path(str(ROOT / "kitty/termlab.py"))
        message = adapter["event"]("a" * 32, "bootstrap")
        message["seq"] = 1
        self.command = dict(cmd="kitten", version=[0, 47, 4], no_response=False,
            payload=dict(kitten="termlab.py", args=["carrier", json.dumps(message)]))
        self.window = SimpleNamespace(user_vars={})
        self.authorize = runpy.run_path(str(ROOT / "kitty/termlab_auth.py"))["is_cmd_allowed"]

    def test_probe_authorization_is_bounded_and_cannot_launch(self):
        def command(args, **changes):
            return dict(self.command, payload=dict(kitten="termlab.py", args=args), **changes)
        good = command(["probe", "a" * 32])
        self.assertTrue(self.authorize(good, self.window, False, {}))
        self.assertFalse(self.authorize(good, self.window, True, {}))
        self.assertFalse(self.authorize(command(["probe", "a" * 32], no_response=True), self.window, False, {}))
        for args in [["probe", "a" * 33], ["probe", "A" * 32], ["probe", {}], ["probe", "a" * 32, "extra"], ["run0", "-uroot"], ["start", ""]]:
            self.assertFalse(self.authorize(command(args), self.window, False, {}), args)
        launch = dict(self.command, payload=dict(kitten="termlab_launch.py", args=["run0", "-uroot"]))
        self.assertFalse(self.authorize(launch, self.window, False, {}))

    def test_bootstrap_and_group_binding(self):
        self.assertTrue(self.authorize(self.command, self.window, False, {}))
        self.window.user_vars["termlab_group"] = "b" * 32
        self.assertFalse(self.authorize(self.command, self.window, False, {}))

    def test_denies_generic_rpc_target_overrides_and_socket_origin(self):
        self.assertFalse(self.authorize(self.command, self.window, True, {}))
        self.assertFalse(self.authorize(self.command, None, False, {}))
        for key, value in [("cmd", "launch"), ("stream_id", "spoof"), ("kitty_window_id", 1), ("async_id", "spoof")]:
            command = dict(self.command, **{key: value})
            self.assertFalse(self.authorize(command, self.window, False, {}), key)
        for key, value in [("match", "all"), ("all", True), ("kitten", "/tmp/evil.py"), ("args", ["_local", "{}"])]:
            command = dict(self.command, payload=dict(self.command["payload"], **{key: value}))
            self.assertFalse(self.authorize(command, self.window, False, {}), key)

    def test_abort_cleanup_never_replies_into_the_resumed_shell(self):
        message=json.loads(self.command["payload"]["args"][1])
        message["kind"]="abort"
        self.command["payload"]["args"][1]=json.dumps(message)
        self.command["no_response"]=True
        self.window.user_vars["termlab_group"]="a"*32
        self.assertTrue(self.authorize(self.command,self.window,False,{}))
        message["kind"]="bootstrap"
        self.command["payload"]["args"][1]=json.dumps(message)
        self.assertFalse(self.authorize(self.command,self.window,False,{}))

    def test_rejects_large_envelopes_and_unknown_protocol(self):
        for raw in ["x" * 32769, "{}", "[]", '{"v":2}']:
            command = dict(self.command, payload=dict(kitten="termlab.py", args=["carrier", raw]))
            self.assertFalse(self.authorize(command, self.window, False, {}))


if __name__ == "__main__":
    result = unittest.main(argv=["termlab-kitty-tests"], verbosity=2, exit=False).result
    if not result.wasSuccessful():
        raise RuntimeError("kitty integration tests failed")
