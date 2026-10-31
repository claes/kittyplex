"""Run inside kitty's Python: see README.md. Does not open a GUI."""

from pathlib import Path
import runpy
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
    def test_routes_to_target_window_without_remote_control(self):
        module = runpy.run_path(str(ROOT / "kitty/termlab.py"))
        received = []

        class Client:
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def settimeout(self, value): received.append(("timeout", value))
            def connect(self, path): received.append(("connect", path))
            def sendall(self, data): received.append(("send", data))
            def recv(self, count): return b"OK active=2 sessions=2\n"

        errors = []
        boss = SimpleNamespace(window_id_map={
            1: SimpleNamespace(user_vars={"termlab_socket": "/tmp/first.sock"}),
            2: SimpleNamespace(user_vars={"termlab_socket": "/tmp/second.sock"}),
        }, show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", return_value=Client()):
            module["handle_result"](["termlab.py", "new"], None, 2, boss)
        self.assertIn(("connect", "/tmp/second.sock"), received)
        self.assertIn(("send", b"new\n"), received)
        self.assertFalse(errors)

    def test_stale_endpoint_shows_bounded_error(self):
        module = runpy.run_path(str(ROOT / "kitty/termlab.py"))
        errors = []
        boss = SimpleNamespace(window_id_map={
            1: SimpleNamespace(user_vars={"termlab_socket": "/tmp/missing.sock"}),
        }, show_error=lambda *args: errors.append(args))
        with patch.object(module["socket"], "socket", side_effect=OSError("missing")):
            module["handle_result"](["termlab.py", "next"], None, 1, boss)
        self.assertEqual(len(errors), 1)


if __name__ == "__main__":
    unittest.main(argv=["termlab-kitty-tests"], verbosity=2)
