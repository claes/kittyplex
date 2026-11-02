#!/usr/bin/env python3
"""Private-PTY and real Kitty acceptance. No SSH or Kittyplex runtime is used."""
import base64
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('RESTORE_DISPLAY_BIN', ROOT / 'target/debug'))
EXERCISER = BIN / 'restore-display-exerciser'
DEMO = BIN / 'restore-display-demo-rust-direct'


def rpc(directory, **value):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(str(directory / 'control.sock'))
        connection.sendall(json.dumps(value).encode() + b'\n')
        with connection.makefile('rb') as stream:
            answer = json.loads(stream.readline())
    if 'error' in answer:
        raise RuntimeError(answer['error'])
    return answer


def wait_for(callback, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = callback()
        if result:
            return result
        time.sleep(.02)
    raise AssertionError('condition timed out')


def box_color(image, seed=1, box=0):
    """Independent oracle for the documented 64-bit seeded box-color mixer."""
    mask = (1 << 64) - 1
    value = (seed + image * 0x9e3779b97f4a7c15 + (box + 1) * 0xbf58476d1ce4e5b9) & mask
    value = ((value ^ (value >> 30)) * 0xbf58476d1ce4e5b9) & mask
    value = ((value ^ (value >> 27)) * 0x94d049bb133111eb) & mask
    value ^= value >> 31
    return tuple(48 + ((value >> shift) & 255) % 192 for shift in (0, 8, 16))


class Fixture(unittest.TestCase):
    features = 'text'

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='rd-')
        self.directory = Path(self.temp.name) / 'session'
        self.addCleanup(self.cleanup)
        self.start([str(DEMO), '--features', self.features, '--log-file', str(self.directory / 'demo.log'), *getattr(self, 'extra_args', [])])
        wait_for(lambda: self.state()['enabled'])

    def start(self, program, support='on'):
        subprocess.run([EXERCISER, 'start', '--session-dir', self.directory,
                        '--demo-state', '--support', support, '--', *program],
                       check=True, capture_output=True)

    def cleanup(self):
        try:
            metadata = self.state().get('display') or {}
            kitty_pid = metadata.get('emulator_pid') if metadata.get('kind') == 'kitty' else None
        except (OSError, RuntimeError):
            kitty_pid = getattr(self, 'last_kitty_pid', None)
        if any(test is self for test, _ in self._outcome.result.failures + self._outcome.result.errors):
            artifact = os.environ.get('RESTORE_DISPLAY_TEST_ARTIFACTS')
            if artifact:
                destination = Path(artifact) / self.id().rsplit('.', 1)[-1]
                shutil.copytree(self.directory, destination, dirs_exist_ok=True,
                                ignore=lambda directory, names: [name for name in names if (Path(directory) / name).is_socket()])
                if hasattr(self, 'framebuffers'):
                    framebuffer = Path(self.framebuffers.name) / 'Xvfb_screen0'
                    if framebuffer.exists():
                        shutil.copy(framebuffer, destination / 'screen.xwd')
                    try:
                        (destination / 'graphics.json').write_text(json.dumps(self.image()))
                    except (OSError, subprocess.CalledProcessError):
                        pass
        try:
            rpc(self.directory, op='stop')
            wait_for(lambda: not (self.directory / 'control.sock').exists(), 4)
        except (OSError, RuntimeError):
            pass
        if kitty_pid:
            def kitty_exited():
                try:
                    return Path(f'/proc/{kitty_pid}/stat').read_text().rsplit(') ', 1)[1].startswith('Z ')
                except FileNotFoundError:
                    return True
            wait_for(kitty_exited, 5)
        self.temp.cleanup()

    def state(self):
        return rpc(self.directory, op='inspect')

    def send(self, text):
        rpc(self.directory, op='input', data=base64.b64encode(text.encode()).decode())

    def restore(self):
        request = rpc(self.directory, op='restore')
        result = wait_for(lambda: (s if (s := self.state())['status'] == 'complete' else None))
        self.assertEqual(result['transaction'], request['id'])
        self.assertTrue(result['valid_restore_order'])
        return result


class PTYTests(Fixture):
    def test_static_restore_model_change_and_resize(self):
        before = self.state()
        self.assertEqual(before['display_status'], 'detached')
        rpc(self.directory, op='headless', identity='one')
        self.restore()
        self.send('d')
        wait_for(lambda: self.state()['model'].get('armed'))
        rpc(self.directory, op='detach')
        wait_for(lambda: self.state()['model'].get('image') == '#2')
        rpc(self.directory, op='headless', identity='two')
        after = self.restore()
        self.assertEqual(before['pid'], after['pid'])
        self.assertEqual(before['process_start'], after['process_start'])
        self.assertEqual(after['model']['image'], '#2')
        rpc(self.directory, op='resize', cols=100, rows=30)
        wait_for(lambda: self.state()['status'] == 'complete')
        self.send('i')
        wait_for(lambda: self.state()['model'].get('image') == '#3')
        self.send('q')
        wait_for(lambda: not (self.directory / 'control.sock').exists())
        trace = [json.loads(line) for line in (self.directory / 'events.jsonl').read_text().splitlines()]
        self.assertTrue(any(e['event'] == 'protocol_output' and e['data'].get('action') == 'disable' for e in trace))

    def test_fragmented_input_and_stale_request(self):
        rpc(self.directory, op='headless', identity='one')
        rpc(self.directory, op='fault', kind='fragment', value=1)
        self.restore()
        self.restore()
        rpc(self.directory, op='fault', kind='stale_transaction')
        time.sleep(.15)
        self.assertEqual(self.state()['last_transaction'], 2)
        self.assertEqual(self.state()['status'], 'complete')

    def test_input_is_rejected_until_restored(self):
        rpc(self.directory, op='headless', identity='one')
        with self.assertRaisesRegex(RuntimeError, 'gated'):
            self.send('i')
        self.restore()
        rpc(self.directory, op='cancel')
        with self.assertRaisesRegex(RuntimeError, 'gated'):
            self.send('i')

    def test_scenario_and_report(self):
        path = Path(self.temp.name) / 'case.json'
        report = Path(self.temp.name) / 'report.json'
        path.write_text(json.dumps({'schema': 0, 'case': 'static', 'requires': ['text'], 'steps': [
            {'op': 'attach', 'display': 'headless'}, {'op': 'restore'},
            {'op': 'checkpoint', 'name': 'before'}, {'op': 'detach'},
            {'op': 'attach', 'display': 'headless'}, {'op': 'restore'},
            {'op': 'assert', 'check': 'same_process'},
            {'op': 'assert', 'check': 'model_equal', 'checkpoint': 'before'},
            {'op': 'assert', 'check': 'valid_restore_order'}]}))
        subprocess.run([EXERCISER, 'scenario', '--session-dir', self.directory,
                        '--file', path, '--report', report], check=True, capture_output=True)
        self.assertTrue(all(c['result']['status'] == 'pass' for c in json.loads(report.read_text())['checks']))
    def test_missing_screen_observer_does_not_pass(self):
        path = Path(self.temp.name) / 'missing.json'
        report = Path(self.temp.name) / 'report.json'
        path.write_text(json.dumps({'schema': 0, 'case': 'observer', 'steps': [
            {'op': 'attach', 'display': 'headless'}, {'op': 'restore'},
            {'op': 'checkpoint', 'name': 'before'},
            {'op': 'assert', 'check': 'text_equal', 'checkpoint': 'before'}]}))
        result = subprocess.run([EXERCISER, 'scenario', '--session-dir', self.directory,
                                 '--file', path, '--report', report], capture_output=True)
        self.assertEqual(result.returncode, 3)
        self.assertEqual(json.loads(report.read_text())['checks'][-1]['status'], 'not_run')



class RefusalTests(Fixture):
    extra_args = ['--restore-behavior', 'refuse']

    def test_refusal_keeps_input_gated_and_process_alive(self):
        rpc(self.directory, op='headless', identity='refusal')
        rpc(self.directory, op='restore')
        state = wait_for(lambda: (s if (s := self.state())['status'] == 'unavailable' else None))
        self.assertTrue(state['gated'])
        with self.assertRaisesRegex(RuntimeError, 'gated'):
            self.send('i')


class IndependentControllerTests(unittest.TestCase):
    def test_completion_before_begin_is_rejected(self):
        with tempfile.TemporaryDirectory(prefix='rd-negative-') as temp:
            directory = Path(temp) / 'session'
            # An independent implementation: no Rust codec/adapter is reused.
            script = r'''
import os, tty

tty.setraw(0)
os.write(1, b'\x1bP@restore-display;action=enable,version=0\x1b\\')
buffer = b''
while True:
    buffer += os.read(0, 4096)
    while b'\x1b\\' in buffer:
        frame, buffer = buffer.split(b'\x1b\\', 1)
        if b'action=restore,id=' in frame:
            number = frame.split(b'id=')[1]
            os.write(1, b'\x1bP@restore-display;action=complete,id=' + number + b'\x1b\\')
'''
            subprocess.run([EXERCISER, 'start', '--session-dir', directory, '--',
                            sys.executable, '-u', '-c', script], check=True, capture_output=True)
            try:
                wait_for(lambda: rpc(directory, op='inspect')['enabled'])
                rpc(directory, op='headless', identity='negative')
                rpc(directory, op='restore')
                wait_for(lambda: rpc(directory, op='inspect')['status'] == 'invalid_order')
                state = rpc(directory, op='inspect')
                self.assertTrue(state['gated'])
                self.assertFalse(state['valid_restore_order'])
            finally:
                rpc(directory, op='stop')
                wait_for(lambda: not (directory / 'control.sock').exists())


@unittest.skipUnless(shutil.which('kitty') and shutil.which('Xvfb'), 'real Kitty/Xvfb unavailable')
class KittyTests(Fixture):
    features = 'text,graphics'

    def setUp(self):
        read_fd, write_fd = os.pipe()
        self.framebuffers = tempfile.TemporaryDirectory(prefix='rd-fb-')
        xvfb_log = open(Path(self.framebuffers.name) / "xvfb.log", "wb")
        self.xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                     '1024x768x24', '-nolisten', 'tcp', '-noreset',
                                     '-fbdir', self.framebuffers.name], pass_fds=(write_fd,),
                                    stdout=subprocess.DEVNULL, stderr=xvfb_log)
        xvfb_log.close()
        os.close(write_fd)
        if not select.select([read_fd], [], [], 10)[0]:
            raise RuntimeError('Xvfb startup timed out')
        number = os.read(read_fd, 64).decode().strip()
        display = ':' + number
        os.close(read_fd)
        if not number or self.xvfb.poll() is not None:
            raise RuntimeError('Xvfb failed: ' + (Path(self.framebuffers.name) / 'xvfb.log').read_text())
        wait_for(lambda: (Path(self.framebuffers.name) / 'Xvfb_screen0').exists())
        self.old_display = os.environ.get('DISPLAY')
        os.environ['DISPLAY'] = display
        os.environ['LIBGL_ALWAYS_SOFTWARE'] = '1'
        self.addCleanup(self.stop_xvfb)
        super().setUp()

    def stop_xvfb(self):
        self.xvfb.terminate()
        self.xvfb.wait(timeout=5)
        self.framebuffers.cleanup()
        if self.old_display is None:
            os.environ.pop('DISPLAY', None)
        else:
            os.environ['DISPLAY'] = self.old_display

    def attach(self):
        result = json.loads(subprocess.check_output([EXERCISER, 'attach', '--session-dir',
                                                    self.directory, '--display', 'kitty', '--fresh']))
        self.last_kitty_pid = result['display']['emulator_pid']
        return result

    def rc(self, *args):
        return subprocess.check_output(['kitten', '@', '--to', 'unix:' + str(self.directory / 'kitty.sock'), *args])

    def image(self):
        return json.loads(self.rc('kitten', '--match', 'all', str(ROOT / 'assets/kitty_probe.py'), 'graphics', '1'))

    def visible_corner(self, expected, center=False):
        # XWD is the actual Xvfb framebuffer, rather than terminal resource state.
        content = (Path(self.framebuffers.name) / 'Xvfb_screen0').read_bytes()
        header = struct.unpack('>25I', content[:100])
        header_size, _, _, _, width, height, _, byte_order, _, _, _, bits, stride, _, red, green, blue, _, _, colors, *_ = header
        self.assertEqual(bits, 32)
        pixels = memoryview(content)[header_size + colors * 12:]
        probe = self.image()
        g = probe['geometry']
        cell_w = (g['right'] - g['left']) / probe['cell'][0]
        cell_h = (g['bottom'] - g['top']) / probe['cell'][1]
        # Sample inside the first colored box, or at the center for the solid PNG.
        box_w, box_h = min(32, probe['cell'][0] - 4) * cell_w, min(16, probe['cell'][1] - 7) * cell_h
        side = min(box_w, box_h)
        x = int(g['left'] + 2 * cell_w + (box_w - side) / 2 + side / (2 if center else 16))
        y = int(g['top'] + 4 * cell_h + (box_h - side) / 2 + side / (2 if center else 16))
        if not (0 <= x < width and 0 <= y < height):
            return False
        number = int.from_bytes(pixels[y * stride + x * 4:y * stride + x * 4 + 4], 'little' if byte_order == 0 else 'big')
        rgb = tuple((number & mask) >> ((mask & -mask).bit_length() - 1) for mask in (red, green, blue))
        return all(abs(a - b) <= 2 for a, b in zip(rgb, expected))

    def test_shipped_scenarios(self):
        for index, name in enumerate(('static.json', 'change-image.json')):
            if index:
                rpc(self.directory, op='stop')
                wait_for(lambda: not (self.directory / 'control.sock').exists())
                self.directory = Path(self.temp.name) / 'session2'
                self.start([str(DEMO)])
                wait_for(lambda: self.state()['enabled'])
            report = Path(self.temp.name) / (name + '.report')
            result = subprocess.run([EXERCISER, 'scenario', '--session-dir', self.directory,
                                     '--file', ROOT / 'scenarios' / name, '--report', report],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr + report.read_text())

    def test_png_bytes_survive_removal_of_the_file(self):
        import zlib
        rpc(self.directory, op='stop')
        wait_for(lambda: not (self.directory / 'control.sock').exists())
        self.directory = Path(self.temp.name) / 'png-session'
        path = Path(self.temp.name) / 'cyan.png'
        def chunk(kind, payload):
            return struct.pack('>I', len(payload)) + kind + payload + struct.pack('>I', zlib.crc32(kind + payload))
        path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>2I5B', 1, 1, 8, 6, 0, 0, 0))
                         + chunk(b'IDAT', zlib.compress(b'\0\0\xff\xff\xff')) + chunk(b'IEND', b''))
        self.start([str(DEMO), '--image', str(path)])
        wait_for(lambda: self.state()['enabled'])
        first = self.attach()
        self.restore()
        wait_for(lambda: self.image().get('image', {}).get('width') == 1)
        self.assertEqual(self.image()['sha256'], hashlib.sha256(bytes([0, 255, 255, 255])).hexdigest())
        wait_for(lambda: self.visible_corner((0, 255, 255), center=True))
        path.unlink()
        rpc(self.directory, op='detach')
        second = self.attach()
        self.restore()
        self.assertEqual(first['pid'], second['pid'])
        wait_for(lambda: self.visible_corner((0, 255, 255), center=True))

    def test_fresh_display_has_current_image_and_accepts_next_key(self):
        first = self.attach()
        self.restore()
        wait_for(lambda: self.image().get('image', {}).get('width') == 64)
        first_hash = self.image()['sha256']
        wait_for(lambda: self.visible_corner(box_color(0)))
        self.send('d')
        wait_for(lambda: self.state()['model'].get('armed'))
        rpc(self.directory, op='detach')
        wait_for(lambda: self.state()['model'].get('image') == '#2')
        second = self.attach()
        self.assertNotEqual(first['display']['emulator_pid'], second['display']['emulator_pid'])
        self.assertEqual(first['pid'], second['pid'])
        self.restore()
        wait_for(lambda: self.image().get('sha256') not in (None, first_hash))
        wait_for(lambda: self.visible_corner(box_color(1)))
        self.assertIn(b'image #2', self.rc('get-text', '--match', 'all'))
        self.send('i')
        wait_for(lambda: self.state()['model'].get('image') == '#3')
        wait_for(lambda: self.visible_corner(box_color(2)))
        self.assertNotEqual(self.image()['sha256'], first_hash)


if __name__ == '__main__':
    unittest.main()
