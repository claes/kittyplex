"""Kitty adapter for termlab. Bulk transport and process management live in Rust."""
import json
import os
import shutil
import socket
import tempfile

from kittens.tui.handler import result_handler
from kitty.constants import config_dir
from kitty.fast_data_types import get_options

# Kitty compiles custom kittens without defining __file__.
DIRECTORY = os.path.dirname(globals().get("__file__", os.path.join(config_dir, "termlab.py")))

PACKAGED_BINARY = None


def binary_path():
    binary = os.environ.get("TERMLAB_BINARY") or PACKAGED_BINARY or shutil.which("termlab")
    if not binary:
        raise ValueError("install termlab locally or set TERMLAB_BINARY before starting Kitty")
    return os.path.abspath(binary)


def probe_nonce(value):
    return isinstance(value, str) and len(value) == 32 and all(c in "0123456789abcdef" for c in value)


KINDS = {"bootstrap", "create", "select", "output", "input_credit", "drop",
         "heartbeat", "shutdown", "abort", "finish"}
FIELDS = {"v", "g", "seq", "kind", "id", "data", "n", "rows", "cols", "pixel_width", "pixel_height"}


def validate(raw):
    if not isinstance(raw, str) or len(raw.encode()) > 32768:
        raise ValueError("carrier envelope exceeds limit")
    m = json.loads(raw)
    if not isinstance(m, dict) or set(m) != FIELDS:
        raise ValueError("invalid envelope fields")
    if type(m["v"]) is not int or m["v"] != 2 or not isinstance(m["g"], str) or len(m["g"]) != 32:
        raise ValueError("incompatible carrier protocol; install matching carrier v2 binaries and Kitty integration")
    if any(c not in "0123456789abcdef" for c in m["g"]):
        raise ValueError("invalid group identifier")
    for key in ("seq", "id", "n", "rows", "cols"):
        if type(m[key]) is not int or not 0 <= m[key] <= 2**53:
            raise ValueError("invalid numeric field")
    for key in ("rows", "cols", "pixel_width", "pixel_height"):
        if type(m[key]) is not int or not 0 <= m[key] <= 65535:
            raise ValueError("invalid terminal dimensions")
    if not isinstance(m["data"], str) or not isinstance(m["kind"], str) or m["kind"] not in KINDS:
        raise ValueError("unsupported carrier operation")
    return m


def emit(endpoint, message):
    """One nonblocking atomic datagram; busy is an explicit retry condition."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
        client.setblocking(False)
        client.sendto(json.dumps(message, separators=(",", ":")).encode(), endpoint)


def event(group, kind, session=0, data=""):
    return dict(v=2, g=group, seq=0, kind=kind, id=session, data=data,
                n=0, rows=0, cols=0, pixel_width=0, pixel_height=0)


def groups(boss):
    if not hasattr(boss, "_termlab_groups"):
        boss._termlab_groups = {}
    return boss._termlab_groups


def start(boss, window, message):
    from kitty.launch import launch, parse_launch_args
    from kitty.window import Watchers
    binary = binary_path()
    directory = tempfile.mkdtemp(prefix="termlab-kitty-")
    state = dict(carrier=window.id, directory=directory, views={},
                 seq=0, binary=os.path.abspath(binary),
                 context="".join(c for c in message["data"] if c.isprintable())[:128])
    groups(boss)[message["g"]] = state
    window.set_user_var("termlab_group", message["g"])
    watcher = Watchers()
    watcher.on_close.append(carrier_closed)
    window.watchers.add(watcher)
    state["on_close"] = carrier_closed
    helper = os.path.join(DIRECTORY, "termlab.py")
    opts, cmd = parse_launch_args([
        "--type=background", "--allow-remote-control", state["binary"],
        "__broker", directory, message["g"], str(window.id), helper,
        str(max(10, min(message["n"], 86400)))])
    def broker_exited(status, error):
        if message["g"] in groups(boss):
            local(boss, event(message["g"], "cleanup"))
            shutil.rmtree(directory, ignore_errors=True)
            boss.show_error("termlab", "Local broker exited unexpectedly; see kitty stderr for details")
    try:
        launch(boss, opts, cmd, rc_from_window=window, child_death_callback=broker_exited)
    except Exception:
        groups(boss).pop(message["g"], None)
        window.set_user_var("termlab_group", None)
        window.watchers.on_close.remove(carrier_closed)
        shutil.rmtree(directory, ignore_errors=True)
        raise
    return state


def carrier_closed(boss, window, data):
    group = window.user_vars.get("termlab_group")
    state = groups(boss).pop(group, None)
    if state:
        try:
            emit(os.path.join(state["directory"], "events.sock"), event(group, "lost"))
        except OSError:
            pass
        views = [boss.window_id_map[w] for w in state["views"].values()
                 if w in boss.window_id_map]
        if views:
            boss.close_windows_no_confirm(views)


def local(boss, m):
    from kitty.launch import launch, parse_launch_args
    state = groups(boss).get(m["g"])
    if not state:
        return
    carrier = boss.window_id_map.get(state["carrier"])
    if m["kind"] == "cleanup":
        groups(boss).pop(m["g"], None)
        views = [boss.window_id_map[w] for w in state["views"].values()
                 if w in boss.window_id_map]
        for w in views:
            w.set_user_var("termlab_socket", None)
        if views:
            boss.close_windows_no_confirm(views)
        if carrier:
            if state.get("on_close") in carrier.watchers.on_close:
                carrier.watchers.on_close.remove(state["on_close"])
            carrier.set_user_var("termlab_group", None)
            boss.set_active_window(carrier)
        return
    if not carrier:
        emit(os.path.join(state["directory"], "events.sock"), event(m["g"], "lost"))
        return
    if m["kind"] == "create":
        if m["id"] in state["views"]:
            return
        state["views"] = {sid: wid for sid, wid in state["views"].items() if wid in boss.window_id_map}
        if len(state["views"]) >= 8:
            raise ValueError("maximum eight live views per group")
        endpoint = os.path.join(state["directory"], "events.sock")
        opts, cmd = parse_launch_args([
            "--type=tab", "--location=last",
            "--watcher=" + os.path.join(DIRECTORY, "termlab_watcher.py"),
            "--var=termlab_socket=" + endpoint,
            "--var=termlab_group=" + m["g"],
            "--var=termlab_session=" + str(m["id"]),
            "--var=termlab_context=" + state["context"],
            state["binary"], "__view", state["directory"], m["g"], str(m["id"])])
        view = launch(boss, opts, cmd, target_tab=carrier.tabref(), rc_from_window=carrier)
        if view is None:
            raise ValueError("kitty did not create a session view")
        # Set an initial child title without pinning an override that blocks OSC 2.
        view.title_changed(memoryview(("termlab " + str(m["id"])).encode()))
        state["views"][m["id"]] = view.id
    elif m["kind"] == "select":
        window = boss.window_id_map.get(state["views"].get(m["id"]))
        if window:
            boss.set_active_window(window)


def main(args):
    pass


@result_handler(no_ui=True)
def handle_result(args, answer, target_window_id, boss):
    window = boss.window_id_map.get(target_window_id)
    if window is None:
        return "busy"
    try:
        if len(args) == 3 and args[1] == "probe" and probe_nonce(args[2]):
            if get_options().allow_remote_control != "password":
                raise ValueError("Kitty must use scoped password-mode remote control")
            binary = binary_path()
            if not os.access(binary, os.X_OK):
                raise ValueError("configured local termlab binary is not executable")
            return json.dumps(dict(nonce=args[2], desktop_api=1, carrier=2, version="0.4.0", binary=binary))
        if len(args) == 3 and args[1] == "_local":
            # Only the broker's inherited private kitty FD can reach this branch:
            # the terminal authorization callback explicitly rejects _local.
            local(boss, json.loads(args[2]))
            return True
        if len(args) == 3 and args[1] == "carrier":
            m = validate(args[2])
            state = groups(boss).get(m["g"])
            if state is None:
                if m["kind"] in {"abort", "shutdown"}:
                    return f'accepted:{m["seq"]}'
                if m["kind"] != "bootstrap" or m["seq"] != 1:
                    raise ValueError("carrier group has not bootstrapped")
                if window.user_vars.get("termlab_group"):
                    raise ValueError("carrier already belongs to a group")
                state = start(boss, window, m)
            if state["carrier"] != window.id:
                raise ValueError("carrier origin does not match group")
            if m["seq"] <= state["seq"]:
                return f'accepted:{state["seq"]}'
            if m["seq"] != state["seq"] + 1:
                return "busy"
            if m["kind"] != "bootstrap":
                try:
                    emit(os.path.join(state["directory"], "events.sock"), m)
                except (BlockingIOError, FileNotFoundError, ConnectionRefusedError):
                    return "busy"
            state["seq"] = m["seq"]
            return f'accepted:{m["seq"]}'
        command = " ".join(args[1:])
        if command not in {"new", "next", "previous", "quit"} and command not in {
            f"select {n}" for n in range(1, 10)
        }:
            raise ValueError("unknown management command")
        endpoint = window.user_vars.get("termlab_socket")
        group = window.user_vars.get("termlab_group")
        if endpoint and group:
            emit(endpoint, event(group, "command", int(window.user_vars.get("termlab_session", "0")), data=command))
    except (ValueError, OSError, TypeError, KeyError) as error:
        if len(args) > 1 and args[1] == "carrier":
            return "error:" + str(error)
        boss.show_error("termlab", str(error))
        return "busy"
