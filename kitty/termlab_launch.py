"""Trusted Kitty action frontend. Never exposed through terminal authorization."""
import os
import runpy
from kitty.constants import config_dir
from kittens.tui.handler import result_handler

DIRECTORY = os.path.dirname(globals().get("__file__", os.path.join(config_dir, "termlab_launch.py")))


def main(args):
    pass


def carrier_closed(boss, window, data):
    source = window.user_vars.get("termlab_launch_origin")
    if source:
        try:
            origin = boss.window_id_map.get(int(source))
            if origin:
                boss.set_active_window(origin)
        except (ValueError, TypeError):
            pass


@result_handler(no_ui=True)
def handle_result(args, answer, target_window_id, boss):
    from kitty.launch import launch, parse_launch_args
    from kitty.window import Watchers
    try:
        if len(args) < 2 or args[1] not in {"start", "run0", "ssh"}:
            raise ValueError("expected start, run0 or ssh")
        source = boss.window_id_map.get(target_window_id)
        if source is None:
            return
        adapter = runpy.run_path(os.path.join(DIRECTORY, "termlab.py"))
        opts, cmd = parse_launch_args([
            "--type=tab", "--location=last", "--title=termlab carrier",
            "--env=TERMLAB_SOCKET", "--env=TERMLAB_SESSION", "--env=TERMLAB_CONTEXT",
            "--var=termlab_launch_origin=" + str(source.id),
            adapter["binary_path"](), "__launch-tab", *args[1:]])
        view = launch(boss, opts, cmd, target_tab=source.tabref(), rc_from_window=source)
        if view:
            view.set_title(None)
            view.title_changed(memoryview(("termlab " + args[1]).encode()))
            watcher = Watchers()
            watcher.on_close.append(carrier_closed)
            view.watchers.add(watcher)
    except (ValueError, OSError, TypeError) as error:
        boss.show_error("termlab launch", str(error))
