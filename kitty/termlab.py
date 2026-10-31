"""A no-UI kitty kitten: route commands to this window's termlab instance."""

import socket

from kittens.tui.handler import result_handler


def main(args):
    pass


@result_handler(no_ui=True)
def handle_result(args, answer, target_window_id, boss):
    window = boss.window_id_map.get(target_window_id)
    if window is None:
        return
    endpoint = window.user_vars.get("termlab_socket")
    if not endpoint:
        return
    command = " ".join(args[1:])
    if command not in {"new", "next", "previous", "quit"} and command not in {
        f"select {i}" for i in range(1, 10)
    }:
        boss.show_error("termlab", "Unknown management command")
        return
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            # This handler runs in kitty. Keep failures bounded so a stale
            # socket cannot freeze its UI. No remote-control permission needed.
            client.settimeout(0.25)
            client.connect(endpoint)
            client.sendall((command + "\n").encode("ascii"))
            response = client.recv(4096)
            if not response.startswith(b"OK "):
                raise OSError(response.decode("utf-8", "replace").strip())
    except OSError as error:
        boss.show_error("termlab", f"Management command failed: {error}")
