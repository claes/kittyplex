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
        self.options = load_config(str(ROOT / "kitty/kittyplex.conf"), accumulate_bad_lines=errors)
        assert not errors, errors
        self.window = SimpleNamespace(user_vars={"kittyplex_socket": "/tmp/test.sock"} if registered else {})
        self.actions = []
        self.bells = 0
        super().__init__(global_shortcuts={})

    def get_options(self):
        return self.options

    def get_active_window(self):
        return self.window

    def match_windows(self, expression):
        assert expression == "var:kittyplex_socket", expression
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
        expected = {"c": "new", "t": "new", "v": "split right", "h": "split below",
                    "n": "next", "p": "previous", "q": "quit"}
        expected.update({str(i): f"select {i}" for i in range(1, 10)})
        for key, command in expected.items():
            with self.subTest(key=key):
                mappings = MappingHarness()
                self.assertTrue(mappings.press("ctrl+shift+m"))
                self.assertEqual(mappings.current_keyboard_mode_name, "kittyplex")
                self.assertTrue(mappings.press(key))
                self.assertEqual(mappings.current_keyboard_mode_name, "")
                self.assertEqual(mappings.actions, [f"kitten kittyplex.py {command}"])

    def test_close_and_help_are_local_and_exit_mode(self):
        for key, action in {"w": "close_window", "?": "kitten kittyplex.py help ctrl+shift+m"}.items():
            mappings = MappingHarness()
            mappings.press("ctrl+shift+m")
            self.assertTrue(mappings.press(key))
            self.assertEqual(mappings.current_keyboard_mode_name, "")
            self.assertEqual(mappings.actions, [action])

    def test_help_matches_shifted_question_mark(self):
        mappings = MappingHarness()
        mappings.press("ctrl+shift+m")
        self.assertTrue(mappings.dispatch_possible_special_key(KeyEvent(key=47, mods=1, shifted_key=63)))
        self.assertEqual(mappings.actions, ["kitten kittyplex.py help ctrl+shift+m"])

    def test_prefix_passes_through_outside_kittyplex(self):
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
        return runpy.run_path(str(ROOT / "kitty/kittyplex.py"))

    def test_bootstrap_without_file_global_uses_kitty_config_directory(self):
        from kitty.constants import config_dir
        from kitty.window import Watchers
        module = {"__name__": "kittyplex_loader_regression"}
        exec(compile((ROOT / "kitty/kittyplex.py").read_text(), "kittyplex.py", "exec"), module)
        self.assertEqual(module["DIRECTORY"], config_dir)
        variables = {}
        window = SimpleNamespace(id=1, user_vars=variables, watchers=Watchers(),
            set_user_var=lambda key, value: variables.__setitem__(key, value))
        boss = SimpleNamespace(window_id_map={1: window})
        m = module["event"]("a" * 32, "bootstrap")
        m["n"] = 30
        with patch.dict(os.environ, KITTYPLEX_BINARY="/tmp/kittyplex-test-binary"), patch("kitty.launch.launch") as launch:
            state = module["start"](boss, window, m)
        self.addCleanup(shutil.rmtree, state["directory"])
        self.assertIn(os.path.join(config_dir, "kittyplex.py"), launch.call_args.args[2])
        self.assertIn("child_death_callback", launch.call_args.kwargs)

    def test_probe_is_read_only_and_requires_scoped_control(self):
        module = self.adapter()
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(user_vars={})})
        nonce = "a" * 32
        with patch.dict(module["handle_result"].impl.__globals__, get_options=lambda: SimpleNamespace(allow_remote_control="password")), patch.dict(os.environ, KITTYPLEX_BINARY="/bin/sh"):
            response = json.loads(module["handle_result"](["kittyplex.py", "probe", nonce], None, 1, boss))
        errors = []
        boss.show_error = lambda *args: errors.append(args)
        with patch.dict(module["handle_result"].impl.__globals__, get_options=lambda: SimpleNamespace(allow_remote_control="yes")):
            self.assertEqual(module["handle_result"](["kittyplex.py", "probe", nonce], None, 1, boss), "busy")
        self.assertTrue(errors)
        self.assertEqual((response["nonce"], response["desktop_api"]), (nonce, 1))
        self.assertFalse(hasattr(boss, "_kittyplex_groups"))
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
            n: SimpleNamespace(user_vars={"kittyplex_socket": f"/tmp/{n}.sock",
                "kittyplex_group": "a" * 32, "kittyplex_session": str(n)}) for n in (1, 2)
        }, show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", return_value=Client()):
            module["handle_result"](["kittyplex.py", "new"], None, 2, boss)
        self.assertIn(("blocking", False), received)
        message, endpoint = received[-1]
        self.assertEqual(endpoint, "/tmp/2.sock")
        self.assertEqual((message["kind"], message["id"], message["data"]), ("command", 2, "new"))
        self.assertFalse(errors)

    def test_split_placement_is_bound_to_origin_not_active_view(self):
        module = self.adapter()
        calls = []
        tab = SimpleNamespace(enabled_layouts=["stack"], goto_layout=lambda *a, **kw: calls.append((a, kw)))
        tab.set_enabled_layouts = lambda layouts: setattr(tab, "enabled_layouts", layouts)
        origin = SimpleNamespace(id=22, tabref=lambda: tab)
        carrier = SimpleNamespace(id=1, tabref=lambda: None)
        state = dict(carrier=1, directory="/tmp/test", views={2: 22}, binary="/bin/kittyplex", context="")
        boss = SimpleNamespace(window_id_map={1: carrier, 22: origin}, _kittyplex_groups={"a" * 32: state})
        view = SimpleNamespace(id=33, title_changed=lambda value: None)
        message = module["event"]("a" * 32, "create", 3, "below")
        message["n"] = 2
        with patch("kitty.launch.launch", return_value=view) as launch:
            module["local"](boss, message)
        self.assertEqual(calls, [(("splits",), {"raise_exception": True})])
        self.assertEqual(tab.enabled_layouts, ["stack", "splits"])
        self.assertIs(launch.call_args.kwargs["target_tab"], tab)
        self.assertIs(launch.call_args.kwargs["rc_from_window"], origin)
        opts = launch.call_args.args[1]
        self.assertEqual((opts.type, opts.location, opts.next_to), ("window", "hsplit", "id:22"))
        self.assertEqual(state["views"][3], 33)

    def test_missing_split_origin_cancels_creation(self):
        module = self.adapter()
        state = dict(carrier=1, directory="/tmp/test", views={}, binary="/bin/kittyplex", context="")
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(id=1)}, _kittyplex_groups={"a" * 32: state})
        message = module["event"]("a" * 32, "create", 3, "right")
        message["n"] = 2
        with patch("kitty.launch.launch") as launch, patch.dict(module["local"].__globals__, emit=lambda ep, m: state.update(cancel=m)):
            module["local"](boss, message)
        launch.assert_not_called()
        self.assertEqual((state["cancel"]["kind"], state["cancel"]["id"]), ("close", 3))

    def test_help_closes_with_parent_and_removes_temporary_callback(self):
        module = self.adapter()
        parent = SimpleNamespace(id=1, actions_on_close=[])
        overlay = SimpleNamespace(id=2, actions_on_removal=[])
        closed = []
        boss = SimpleNamespace(window_id_map={1: parent, 2: overlay},
            run_kitten_with_metadata=lambda *a, **kw: overlay,
            close_windows_no_confirm=closed.extend)
        module["show_help"](boss, parent, "ctrl+alt+m")
        parent.actions_on_close[0](parent)
        self.assertEqual(closed, [overlay])
        overlay.actions_on_removal[0](overlay)
        self.assertEqual(parent.actions_on_close, [])

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
            "kittyplex_socket": "/tmp/missing.sock", "kittyplex_group": "a" * 32})},
            show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", side_effect=OSError("missing")):
            module["handle_result"](["kittyplex.py", "next"], None, 1, boss)
        self.assertEqual(len(errors), 1)

    def test_carrier_duplicate_ack_and_busy_retry(self):
        module = self.adapter()
        message = module["event"]("a" * 32, "heartbeat")
        message["seq"] = 2
        state = dict(carrier=1, directory="/tmp/test", seq=1)
        boss = SimpleNamespace(window_id_map={1: SimpleNamespace(id=1, user_vars={})},
            _kittyplex_groups={"a" * 32: state}, show_error=lambda *args: self.fail(str(args)))
        client = SimpleNamespace(setblocking=lambda v: None, sendto=lambda *args: None)
        class Context:
            def __enter__(self): return client
            def __exit__(self, *args): pass
        with patch.object(module["socket"], "socket", return_value=Context()):
            args = ["kittyplex.py", "carrier", json.dumps(message)]
            self.assertEqual(module["handle_result"](args, None, 1, boss), "accepted:2")
            self.assertEqual(module["handle_result"](args, None, 1, boss), "accepted:2")
        message["seq"] = 3
        with patch.object(module["socket"], "socket", side_effect=BlockingIOError()):
            self.assertEqual(module["handle_result"](["kittyplex.py", "carrier", json.dumps(message)], None, 1, boss), "busy")
        self.assertEqual(state["seq"], 2)


class HelpTests(unittest.TestCase):
    def test_small_pane_scroll_resize_and_escape(self):
        Help = runpy.run_path(str(ROOT / "kitty/kittyplex_help.py"))["Help"]
        help = Help("ctrl+alt+m")
        writes, exits = [], []
        help._schedule_write = writes.append
        help._tui_loop = SimpleNamespace(quit=exits.append)
        help.screen_size = SimpleNamespace(rows=6, cols=24)
        help.initialize()
        self.assertIn(b"Escape to close", b"".join(writes))
        help.on_key(SimpleNamespace(matches=lambda key: key == "page_down"))
        self.assertGreater(help.offset, 0)
        help.on_resize(SimpleNamespace(rows=40, cols=100))
        self.assertEqual(help.offset, 0)
        self.assertIn("ctrl+alt+m", "\n".join(help.lines()))
        help.on_key(SimpleNamespace(matches=lambda key: key == "escape"))
        self.assertEqual(exits, [0])

    def test_background_title_does_not_replace_focused_pane_title(self):
        watcher = runpy.run_path(str(ROOT / "kitty/kittyplex_watcher.py"))
        titles = []
        tab = SimpleNamespace(active_window=None, set_title=titles.append)
        foreground = SimpleNamespace(user_vars={"kittyplex_context": "host"}, title="editor", tabref=lambda: tab)
        background = SimpleNamespace(user_vars={"kittyplex_context": "host"}, title="build", tabref=lambda: tab)
        tab.active_window = foreground
        watcher["on_title_change"](None, background, {})
        watcher["on_title_change"](None, foreground, {})
        self.assertEqual(titles, ["host · editor"])


class AuthorizationTests(unittest.TestCase):
    def setUp(self):
        adapter = runpy.run_path(str(ROOT / "kitty/kittyplex.py"))
        message = adapter["event"]("a" * 32, "bootstrap")
        message["seq"] = 1
        self.command = dict(cmd="kitten", version=[0, 47, 4], no_response=False,
            payload=dict(kitten="kittyplex.py", args=["carrier", json.dumps(message)]))
        self.window = SimpleNamespace(user_vars={})
        self.authorize = runpy.run_path(str(ROOT / "kitty/kittyplex_auth.py"))["is_cmd_allowed"]

    def test_probe_authorization_is_bounded_and_cannot_launch(self):
        def command(args, **changes):
            return dict(self.command, payload=dict(kitten="kittyplex.py", args=args), **changes)
        good = command(["probe", "a" * 32])
        self.assertTrue(self.authorize(good, self.window, False, {}))
        self.assertFalse(self.authorize(good, self.window, True, {}))
        self.assertFalse(self.authorize(command(["probe", "a" * 32], no_response=True), self.window, False, {}))
        for args in [["probe", "a" * 33], ["probe", "A" * 32], ["probe", {}], ["probe", "a" * 32, "extra"], ["run0", "-uroot"], ["start", ""]]:
            self.assertFalse(self.authorize(command(args), self.window, False, {}), args)
        launch = dict(self.command, payload=dict(kitten="kittyplex_launch.py", args=["run0", "-uroot"]))
        self.assertFalse(self.authorize(launch, self.window, False, {}))

    def test_rejects_unknown_helper_names(self):
        for helper in ["unknown.py", "unknown_launch.py", "unknown_auth.py"]:
            command = dict(self.command, payload=dict(self.command["payload"], kitten=helper))
            self.assertFalse(self.authorize(command, self.window, False, {}), helper)
        self.assertTrue(self.authorize(self.command, self.window, False, {}))

    def test_bootstrap_and_group_binding(self):
        self.assertTrue(self.authorize(self.command, self.window, False, {}))
        self.window.user_vars["kittyplex_group"] = "b" * 32
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
        self.window.user_vars["kittyplex_group"]="a"*32
        self.assertTrue(self.authorize(self.command,self.window,False,{}))
        message["kind"]="bootstrap"
        self.command["payload"]["args"][1]=json.dumps(message)
        self.assertFalse(self.authorize(self.command,self.window,False,{}))

    def test_rejects_large_envelopes_and_unknown_protocol(self):
        for raw in ["x" * 32769, "{}", "[]", '{"v":2}']:
            command = dict(self.command, payload=dict(kitten="kittyplex.py", args=["carrier", raw]))
            self.assertFalse(self.authorize(command, self.window, False, {}))


if __name__ == "__main__":
    result = unittest.main(argv=["kittyplex-kitty-tests"], verbosity=2, exit=False).result
    if not result.wasSuccessful():
        raise RuntimeError("kitty integration tests failed")
