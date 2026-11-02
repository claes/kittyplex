"""Observer loaded only into the private test-owned Kitty process."""
import hashlib
import json
from kittens.tui.handler import result_handler


def main(args):
    pass


@result_handler(no_ui=True)
def handle_result(args, answer, target_window_id, boss):
    window = boss.window_id_map[target_window_id]
    manager = window.screen.grman
    image = manager.image_for_client_id(int(args[2]))
    result = {'count': manager.image_count}
    if image:
        result['image'] = {k: v for k, v in image.items()
                           if isinstance(v, (int, str, bool, float, type(None)))}
        if isinstance(image.get('data'), bytes):
            result['sha256'] = hashlib.sha256(image['data']).hexdigest()
    result['geometry'] = {k: getattr(window.geometry, k)
                          for k in ('left', 'top', 'right', 'bottom')}
    result['cell'] = [window.screen.columns, window.screen.lines]
    return json.dumps(result)
