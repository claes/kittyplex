#!/usr/bin/env python3
"""Exercise the human-facing guide and cancellation against private Kitty/Xvfb."""
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

from integration import BIN, ROOT, rpc, wait_for

GUIDE = ROOT / 'walkthrough.py'


def running(pid):
    try:
        return not Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].startswith('Z ')
    except FileNotFoundError:
        return False


class Arguments(unittest.TestCase):
    def test_help_requires_no_terminal(self):
        result = subprocess.run([sys.executable, GUIDE, '--help'], capture_output=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn(b'Enter works', result.stdout)

    def test_missing_binary_is_actionable(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, GUIDE, '--bin-dir', directory], capture_output=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b'./scripts/restore-display-demo.sh', result.stderr)

    def test_launcher_includes_new_sources_and_excludes_build_outputs(self):
        with tempfile.TemporaryDirectory(prefix='rd-launcher-') as directory:
            root = Path(directory)
            bin_dir = root / 'bin'
            bin_dir.mkdir()
            capture = root / 'capture.json'
            fake_nix = bin_dir / 'nix'
            fake_nix.write_text("#!/usr/bin/env python3\nimport json, os\nfrom pathlib import Path\nimport sys\nsource = next(a for a in sys.argv[1:] if a.startswith('path:'))\nsource = Path(source[5:].rsplit('#', 1)[0])\nassert (source / 'demos/restore-display/walkthrough.py').is_file()\nassert (source / 'demos/restore-display/Cargo.lock').is_file()\nassert (source / 'demos/restore-display/src/codec.rs').is_file()\nassert (source / 'src/restore.rs').is_file()\nassert not (source / 'target').exists()\nassert not (source / 'demos/restore-display/target').exists()\nPath(os.environ['CAPTURE']).write_text(json.dumps({'source': str(source), 'args': sys.argv[1:]}))\n")
            fake_nix.chmod(0o700)
            for script, args in [('restore-display-demo.sh', ['--bin-dir', 'a path with spaces']),
                                 ('demo.sh', ['--features']),
                                 ('image-demo.sh', ['--seed', '7'])]:
                with self.subTest(script=script):
                    launcher = ROOT.parents[1] / 'scripts' / script
                    result = subprocess.run([launcher, *args], cwd=root,
                                            env=dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ['PATH'],
                                                     CAPTURE=str(capture)), capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    recorded = json.loads(capture.read_text())
                    self.assertEqual(recorded['args'][-(len(args) + 1):], ['--', *args])
                    self.assertFalse(Path(recorded['source']).exists(), 'launcher left its source snapshot behind')



@unittest.skipUnless(shutil.which('kitty') and shutil.which('Xvfb'), 'real Kitty/Xvfb unavailable')
class GuidedTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.display_files = tempfile.TemporaryDirectory(prefix='rd-guide-x-')
        read_fd, write_fd = os.pipe()
        with open(Path(cls.display_files.name) / 'xvfb.log', 'wb') as log:
            cls.xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                         '1024x768x24', '-nolisten', 'tcp', '-noreset'],
                                        pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        if not select.select([read_fd], [], [], 10)[0]:
            cls.xvfb.terminate()
            cls.xvfb.wait(timeout=5)
            raise RuntimeError('Xvfb startup timed out')
        number = os.read(read_fd, 64).decode().strip()
        os.close(read_fd)
        if not number or cls.xvfb.poll() is not None:
            raise RuntimeError('Xvfb failed to start')
        cls.display = ':' + number
        cls.addClassCleanup(cls.display_files.cleanup)
        cls.addClassCleanup(cls.stop_display)

    @classmethod
    def stop_display(cls):
        cls.xvfb.terminate()
        cls.xvfb.wait(timeout=5)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='rd-guide-')
        self.addCleanup(self.temp.cleanup)
        self.env = dict(os.environ, DISPLAY=self.display, LIBGL_ALWAYS_SOFTWARE='1', TMPDIR=self.temp.name)
        self.output = b''
        self.guide = None
        self.addCleanup(self.stop_guide)

    def stop_guide(self):
        if self.guide and self.guide.poll() is None:
            self.guide.send_signal(signal.SIGINT)
            self.guide.communicate(timeout=20)
        if self.guide:
            if self.guide.stdin:
                self.guide.stdin.close()
            self.guide.stdout.close()

    def launch(self, fail=None):
        bin_dir = BIN
        if fail:
            bin_dir = Path(self.temp.name) / 'bin'
            bin_dir.mkdir()
            (bin_dir / 'restore-display-demo-rust-direct').symlink_to(BIN / 'restore-display-demo-rust-direct')
            wrapper = bin_dir / 'restore-display-exerciser'
            wrapper.write_text('''#!/usr/bin/env python3
import os, sys
args = sys.argv[1:]
if args[0] == os.environ['FAIL_OPERATION'] or (os.environ['FAIL_OPERATION'] == 'restore' and args[0] == 'ctl' and 'restore' in args):
    print('controlled walkthrough failure', file=sys.stderr)
    sys.exit(2)
os.execv(os.environ['REAL_EXERCISER'], [os.environ['REAL_EXERCISER'], *args])
''')
            wrapper.chmod(0o700)
            self.env.update(REAL_EXERCISER=str(BIN / 'restore-display-exerciser'), FAIL_OPERATION=fail)
        packaged = os.environ.get('RESTORE_DISPLAY_GUIDE_EXECUTABLE')
        argv = [packaged] if packaged else [sys.executable, GUIDE, '--bin-dir', bin_dir]
        if packaged and fail:
            argv += ['--bin-dir', bin_dir]
        self.guide = subprocess.Popen(argv, env=self.env,
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.STDOUT, start_new_session=True)

    def until(self, phrase):
        import time
        deadline = time.monotonic() + float(os.environ.get("RESTORE_DISPLAY_GUIDE_PROMPT_TIMEOUT", "20"))
        while phrase.encode() not in self.output:
            if time.monotonic() > deadline:
                self.fail('Guide prompt timed out: ' + self.output.decode(errors='replace'))
            if select.select([self.guide.stdout], [], [], .1)[0]:
                chunk = os.read(self.guide.stdout.fileno(), 65536)
                if not chunk:
                    self.fail('Guide exited early: ' + self.output.decode(errors='replace'))
                self.output += chunk

    def enter(self):
        self.guide.stdin.write(b'\n')
        self.guide.stdin.flush()

    def root(self):
        return next(Path(self.temp.name).glob('rd-walk-*'))

    def state(self):
        return rpc(self.root() / 'session', op='inspect')

    def image(self):
        session = self.root() / 'session'
        def ready():
            result = subprocess.run(['kitten', '@', '--to', 'unix:' + str(session / 'kitty.sock'),
                                     'kitten', '--match', 'all', str(ROOT / 'assets/kitty_probe.py'),
                                     'graphics', '1'], env=self.env, timeout=8, capture_output=True)
            if result.returncode:
                # Kitty temporarily unloads the old root frame during replacement.
                if b'Failed to get data for root frame' in result.stderr:
                    return None
                raise subprocess.CalledProcessError(result.returncode, result.args,
                                                    result.stdout, result.stderr)
            result = json.loads(result.stdout)
            return result if result.get('sha256') else None
        return wait_for(ready)

    def window_input(self, text):
        session = self.root() / 'session'
        subprocess.run(['kitten', '@', '--to', 'unix:' + str(session / 'kitty.sock'),
                        'send-text', '--match', 'all', '--stdin'], input=text.encode(),
                       check=True, capture_output=True, env=self.env, timeout=8)

    def finish(self):
        remaining, _ = self.guide.communicate(timeout=20)
        self.output += remaining
        return self.guide.returncode

    def test_three_enter_prompts_restore_the_same_process(self):
        unrelated = subprocess.Popen(['sleep', '30'])
        self.addCleanup(lambda: (unrelated.terminate(), unrelated.wait(timeout=5)))
        self.launch()
        self.until('press Enter to detach')
        first = self.state()
        self.assertEqual(first['model']['image'], '#1')
        first_hash = self.image()['sha256']
        root = self.root()
        self.enter()
        self.until('Press Enter to open a fresh window')
        detached = self.state()
        self.assertEqual(detached['display_status'], 'detached')
        self.assertEqual(detached['model']['image'], '#1')
        self.assertEqual(first['pid'], detached['pid'])
        self.enter()
        self.until('press Enter to finish')
        restored = self.state()
        self.assertEqual(restored['pid'], first['pid'])
        self.assertEqual(restored['process_start'], first['process_start'])
        self.assertNotEqual(restored['display']['emulator_pid'], first['display']['emulator_pid'])
        self.assertEqual(self.image()['sha256'], first_hash)
        self.enter()
        self.assertEqual(self.finish(), 0, self.output.decode())
        self.assertFalse(root.exists())
        for pid in (first['pid'], first['controller_pid'], first['display']['emulator_pid'], restored['display']['emulator_pid']):
            self.assertFalse(running(pid))
        self.assertIsNone(unrelated.poll(), 'cleanup touched an unrelated process')

    def test_generated_image_is_preserved_across_detach_and_restore(self):
        self.launch()
        self.until('press Enter to detach')
        initial = self.state()
        initial_hash = self.image()['sha256']
        self.window_input('d' + 'i' * 31)
        wait_for(lambda: self.state()['model']['image'] == '#32')
        self.assertFalse(self.state()['model']['armed'], 'guided mode must not arm a hidden image change')
        generated_hash = wait_for(lambda: (h if (h := self.image()['sha256']) != initial_hash else None))
        self.enter()
        self.until('Press Enter to open a fresh window')
        detached = self.state()
        self.assertEqual(detached['model']['image'], '#32')
        self.assertIn(b'current image is #32, unchanged by detachment', self.output)
        self.enter()
        self.until('press Enter to finish')
        restored = self.state()
        self.assertEqual(restored['model']['image'], '#32')
        self.assertEqual(restored['pid'], initial['pid'])
        self.assertEqual(self.image()['sha256'], generated_hash)
        self.window_input('i')
        wait_for(lambda: self.state()['model']['image'] == '#33')
        wait_for(lambda: self.image()['sha256'] != generated_hash)
        self.enter()
        self.assertEqual(self.finish(), 0, self.output.decode())

    def test_enter_in_image_window_advances_once_per_phase(self):
        self.launch()
        self.until('press Enter to detach')
        first = self.state()
        self.window_input('\r' * 6)
        self.until('Press Enter to open a fresh window')
        self.assertEqual(self.state()['display_status'], 'detached')
        self.enter()
        self.until('press Enter to finish')
        self.assertIsNone(self.guide.poll(), 'old Enter events skipped a later phase')
        self.assertEqual(self.state()['pid'], first['pid'])
        self.window_input('\r')
        self.until('Finished.')
        self.assertEqual(self.finish(), 0, self.output.decode())

    def test_q_in_image_window_finishes_without_an_inspect_error(self):
        self.launch()
        self.until('press Enter to detach')
        state = self.state()
        root = self.root()
        self.window_input('q')
        self.until('image application was closed')
        self.assertEqual(self.finish(), 0, self.output.decode())
        self.assertNotIn(b'Walkthrough failed', self.output)
        self.assertNotIn(b'Could not inspect', self.output)
        self.assertFalse(root.exists())
        self.assertFalse(running(state['pid']))

    def test_ctrl_c_closes_the_attached_session(self):
        self.launch()
        self.until('press Enter to detach')
        state = self.state()
        root = self.root()
        self.guide.send_signal(signal.SIGINT)
        self.assertEqual(self.finish(), 130, self.output.decode())
        self.assertIn(b'Diagnostics retained at:', self.output)
        self.assertTrue((root / 'walkthrough.log').exists())
        for pid in (state['pid'], state['controller_pid'], state['display']['emulator_pid']):
            self.assertFalse(running(pid))

    def test_eof_closes_the_detached_session(self):
        self.launch()
        self.until('press Enter to detach')
        self.enter()
        self.until('Press Enter to open a fresh window')
        state = self.state()
        self.guide.stdin.close()
        self.guide.stdin = None
        self.assertEqual(self.finish(), 130, self.output.decode())
        self.assertFalse(running(state['pid']))
        self.assertFalse(running(state['controller_pid']))

    def test_startup_failure_retains_diagnostics(self):
        self.launch(fail='start')
        self.assertEqual(self.finish(), 1, self.output.decode())
        self.assertIn(b'Could not start', self.output)
        self.assertIn('controlled walkthrough failure', (self.root() / 'walkthrough.log').read_text())

    def test_restore_failure_stops_the_private_application(self):
        self.launch(fail='restore')
        self.assertEqual(self.finish(), 1, self.output.decode())
        self.assertIn(b'Could not ctl', self.output)
        root = self.root()
        initial = json.loads((root / 'session/ready.json').read_text())
        self.assertFalse(running(initial['pid']))
        self.assertFalse(running(initial['controller_pid']))
        self.assertFalse((root / 'session/control.sock').exists())
        self.assertIn('controlled walkthrough failure', (root / 'walkthrough.log').read_text())


if __name__ == '__main__':
    unittest.main()
