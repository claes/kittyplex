"""Forward native view events and preserve target context in tab titles."""
import json
import socket


def emit(window, kind):
    endpoint = window.user_vars.get("kittyplex_socket")
    group = window.user_vars.get("kittyplex_group")
    session = window.user_vars.get("kittyplex_session")
    if not endpoint or not group or not session:
        return
    message = dict(v=2, g=group, seq=0, kind=kind, id=int(session),
                   data="", n=0, rows=0, cols=0, pixel_width=0, pixel_height=0)
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


def on_title_change(boss, window, data):
    context = window.user_vars.get("kittyplex_context")
    if isinstance(context, bytes):
        context = context.decode("utf-8", "replace")
    if context and (tab := window.tabref()) is not None:
        tab.set_title(context + " · " + window.title)
