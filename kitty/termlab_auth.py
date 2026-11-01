"""Authorize scoped termlab carrier and read-only probe calls from their source TTY."""
import os
import runpy

_adapter = runpy.run_path(os.path.join(os.path.dirname(__file__), "termlab.py"))


def is_cmd_allowed(pcmd, window, from_socket, extra_data):
    if from_socket or window is None:
        return False
    if set(pcmd) - {"cmd", "version", "no_response", "payload", "password"}:
        return False
    if pcmd.get("cmd") != "kitten" or type(pcmd.get("no_response")) is not bool:
        return False
    payload = pcmd.get("payload", {})
    if set(payload) != {"kitten", "args"} or payload.get("kitten") != "termlab.py":
        return False
    args = payload.get("args")
    if not isinstance(args, list) or len(args) != 2:
        return False
    if args[0] == "probe":
        return pcmd["no_response"] is False and _adapter["probe_nonce"](args[1])
    if args[0] != "carrier":
        return False
    try:
        m = _adapter["validate"](args[1])
        if pcmd["no_response"] != (m["kind"] == "abort"):
            return False
        group = window.user_vars.get("termlab_group")
        return group == m["g"] or (not group and m["kind"] == "bootstrap" and m["seq"] == 1)
    except (ValueError, TypeError, KeyError):
        return False
