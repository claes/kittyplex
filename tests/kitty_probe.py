"""Read native Kitty state through the isolated test controller, never production authorization."""
import array
import fcntl
import hashlib
import json
import termios
from kittens.tui.handler import result_handler


def main(args):
    pass


@result_handler(no_ui=True)
def handle_result(args, answer, target_window_id, boss):
    w = boss.window_id_map[target_window_id]
    operation = args[1]
    if operation == 'geometry':
        size = array.array('H', [0, 0, 0, 0])
        fcntl.ioctl(w.child.child_fd, termios.TIOCGWINSZ, size)
        result = list(size)
    elif operation == 'pixel-resize':
        size = array.array('H', [0, 0, 0, 0])
        fcntl.ioctl(w.child.child_fd, termios.TIOCGWINSZ, size)
        size[2], size[3] = int(args[2]), int(args[3])
        fcntl.ioctl(w.child.child_fd, termios.TIOCSWINSZ, size)
        result = list(size)
    elif operation == 'graphics':
        g = w.screen.grman
        result = dict(count=g.image_count)
        if len(args) > 2:
            image = g.image_for_client_id(int(args[2]))
            if image is not None:
                result['image'] = {k: v for k, v in image.items() if isinstance(v, (int, str, bool, float, type(None)))}
                result['image']['extra_frame_count'] = len(image.get('extra_frames', ()))
                data = image.get('data')
                if isinstance(data, bytes):
                    result['sha256'] = hashlib.sha256(data).hexdigest()
    elif operation == 'clipboard':
        result = boss.clipboard.get_text()
    elif operation == 'metadata':
        cwd = w.screen.last_reported_cwd
        if isinstance(cwd, bytes):
            cwd = cwd.decode('utf-8', 'replace')
        result = dict(cwd=cwd, at_prompt=w.at_prompt)
    else:
        raise ValueError(operation)
    return json.dumps(result)
