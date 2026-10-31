"""Forward native view focus/close events without blocking kitty's UI."""
import json
import socket


def emit(window, kind):
    endpoint = window.user_vars.get("termlab_socket")
    group = window.user_vars.get("termlab_group")
    session = window.user_vars.get("termlab_session")
    if not endpoint or not group or not session:
        return
    message = dict(v=1, g=group, seq=0, kind=kind, id=int(session),
                   data="", n=0, rows=0, cols=0)
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
            client.setblocking(False)
            client.sendto(json.dumps(message).encode(), endpoint)
    except OSError:
        # Relay EOF independently reports close; focus is advisory and retried
        # by the next explicit management action if the mailbox is congested.
        pass


def on_focus_change(boss, window, data):
    if data.get("focused"):
        emit(window, "focus")


def on_close(boss, window, data):
    emit(window, "close")
