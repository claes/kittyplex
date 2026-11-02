#!/usr/bin/env python3
"""Three Enter prompts around the public Restore Display exerciser commands."""
import argparse
import json
import os
from pathlib import Path
import shutil
import select
import signal
import subprocess
import sys
import tempfile
import time


class DemoError(RuntimeError):
    pass


class DemoClosed(RuntimeError):
    """The user quit the image application normally."""


def process_identity(pid):
    """A start time prevents mistaking a reused PID for one of our processes."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
        return None if fields[0] == 'Z' else fields[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


class Walkthrough:
    def __init__(self, bin_dir):
        self.exerciser = bin_dir / 'restore-display-exerciser'
        self.application = bin_dir / 'restore-display-demo-rust-direct'
        self.root = Path(tempfile.mkdtemp(prefix='rd-walk-'))
        self.session = self.root / 'session'
        self.log = (self.root / 'walkthrough.log').open('w')
        self.processes = set()
        self.started = False
        self.advance = 0
        self.console_input = b''
        self.last_inspection = None

    def command(self, *args, timeout=25):
        argv = [str(self.exerciser), args[0], '--session-dir', str(self.session), *map(str, args[1:])]
        try:
            result = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise DemoError(f'The {args[0]} operation timed out.') from error
        if args[0] != 'inspect' or result.returncode or result.stdout != self.last_inspection:
            self.log.write(json.dumps({'command': argv}) + '\n')
            self.log.write(json.dumps({'exit': result.returncode, 'stdout': result.stdout,
                                       'stderr': result.stderr}) + '\n')
            self.log.flush()
        if args[0] == 'inspect':
            self.last_inspection = result.stdout
        if result.returncode:
            detail = result.stderr.strip() or f'exit status {result.returncode}'
            raise DemoError(f'Could not {args[0]}: {detail}')
        try:
            state = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise DemoError(f'The {args[0]} operation returned an invalid response.') from error
        self.remember(state)
        return state

    def remember(self, state):
        display = state.get('display') or {}
        for pid in (state.get('pid'), state.get('controller_pid'), display.get('emulator_pid')):
            if isinstance(pid, int) and pid > 1:
                identity = process_identity(pid)
                if identity:
                    self.processes.add((pid, identity))

    def state(self):
        try:
            return self.command('inspect', '--json', timeout=5)
        except (DemoError, OSError) as error:
            try:
                final = json.loads((self.session / 'final.json').read_text())
            except (OSError, ValueError):
                raise error
            if final.get('status') == 'disabled':
                raise DemoClosed() from None
            raise DemoError('The controller session ended unexpectedly.') from None

    def wait(self, predicate, description, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.state()
            if predicate(state):
                return state
            time.sleep(.05)
        raise DemoError(f'Timed out waiting for {description}.')

    def pause(self, message, allow_detach=False):
        print(message, flush=True)
        while True:
            state = self.state()
            advance = ((state.get('snapshot') or {}).get('walkthrough') or {}).get('advance', 0)
            if advance > self.advance:
                self.advance = advance
                return
            if allow_detach and state.get('display_status') == 'detached':
                return
            if b'\n' in self.console_input:
                _, self.console_input = self.console_input.split(b'\n', 1)
                return
            if select.select([sys.stdin], [], [], .1)[0]:
                data = os.read(sys.stdin.fileno(), 4096)
                if not data:
                    raise EOFError()
                self.console_input += data

    def run(self):
        print('Restore Display walkthrough\n'
              'You can press Enter in this terminal OR in the image window.\n'
              'In the image window, i generates a new numbered image and q finishes the demo.\n'
              'Your selected image is preserved across detach and reattach.\n', flush=True)
        self.started = True
        self.command('start', '--demo-state', '--', self.application, '--guided')
        initial = self.wait(lambda state: state.get('enabled'), 'application enablement')
        identity = (initial['pid'], initial['process_start'])
        self.command('attach', '--display', 'kitty', '--fresh')
        first = self.command('ctl', 'restore')
        first_image = (first.get('model') or {}).get('image')
        print(f'1. The new window shows image {first_image}. Application PID: {identity[0]}.', flush=True)
        self.pause('In either window, press Enter to detach the image window.', allow_detach=True)

        # Detach preserves the user's current selection; never inject an image change.
        if self.state().get('display_status') != 'detached':
            self.command('ctl', 'detach')
        detached = self.wait(lambda state: state.get('display_status') == 'detached'
                             and (state.get('snapshot') or {}).get('protocol', {}).get('display') == 'detached'
                             and not (state.get('model') or {}).get('armed'), 'application detachment')
        if (detached['pid'], detached['process_start']) != identity:
            raise DemoError('The application process changed during detachment.')
        current_image = detached['model']['image']
        self.advance = ((detached.get('snapshot') or {}).get('walkthrough') or {}).get('advance', 0)
        print(f'\n2. The image window has closed. Application PID {identity[0]} is still running.\n'
              f'   Its current image is {current_image}, unchanged by detachment; the new window will restore it.', flush=True)
        self.pause('Press Enter to open a fresh window and restore the display.')

        self.command('attach', '--display', 'kitty', '--fresh')
        restored = self.command('ctl', 'restore')
        if ((restored['pid'], restored['process_start']) != identity
                or (restored.get('model') or {}).get('image') != current_image
                or restored['display']['identity'] == first['display']['identity']
                or restored['display']['emulator_pid'] == first['display']['emulator_pid']):
            raise DemoError('The new display did not restore the current image in the same application process.')
        print(f'\n3. The fresh window shows image {current_image}. Application PID is still {identity[0]}.\n'
              '   This is the running application drawing its current image again.\n'
              '   You can press i to generate another image, or q to quit.', flush=True)
        self.pause('In either window, press Enter to finish and close this demo.')

    def cleanup(self):
        if not self.started:
            return
        # If start was interrupted, allow its private socket to appear before stopping it.
        deadline = time.monotonic() + 2
        while self.session.exists() and not (self.session / 'control.sock').exists():
            if (self.session / 'final.json').exists() or time.monotonic() > deadline:
                break
            time.sleep(.05)
        if (self.session / 'control.sock').exists():
            try:
                self.state()  # Capture a window that attached just before cancellation.
            except DemoClosed:
                pass
            else:
                self.command('stop', timeout=8)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if all(process_identity(pid) != identity for pid, identity in self.processes):
                return
            time.sleep(.05)
        raise DemoError('Some walkthrough processes have not exited; diagnostics were retained.')


def main(argv=None):
    parser = argparse.ArgumentParser(description='Guided image detach/reattach demonstration. '
                                     'Enter works in the image window or the original terminal.')
    parser.add_argument('--bin-dir', type=Path,
                        default=Path(__file__).resolve().parent / 'target/debug',
                        help='directory containing the demo and exerciser (normally supplied by Nix)')
    args = parser.parse_args(argv)
    for name in ('restore-display-exerciser', 'restore-display-demo-rust-direct'):
        if not os.access(args.bin_dir / name, os.X_OK):
            parser.error(f'Missing {name}. Run ./scripts/restore-display-demo.sh to build automatically.')
    if not shutil.which('kitty') or not shutil.which('kitten'):
        parser.error('Kitty is required. Run ./scripts/restore-display-demo.sh to supply the tools automatically.')
    if not os.environ.get('DISPLAY') and not os.environ.get('WAYLAND_DISPLAY'):
        parser.error('Run the walkthrough from your graphical desktop so Kitty can open its image windows.')

    demo = Walkthrough(args.bin_dir.resolve())
    outcome = 0
    try:
        demo.run()
    except DemoClosed:
        print('\nThe image application was closed. Finishing the walkthrough...', flush=True)
    except (KeyboardInterrupt, EOFError):
        print('\nWalkthrough cancelled. Closing its session...', flush=True)
        outcome = 130
    except (DemoError, OSError) as error:
        print(f'\nWalkthrough failed: {error}', file=sys.stderr, flush=True)
        outcome = 1
    finally:
        # A second Ctrl-C must not abandon the private application or its windows.
        previous = signal.signal(signal.SIGINT, signal.SIG_IGN)
        try:
            demo.cleanup()
        except (DemoError, OSError) as error:
            print(f'Cleanup failed: {error}', file=sys.stderr, flush=True)
            outcome = 1
        finally:
            signal.signal(signal.SIGINT, previous)
            demo.log.close()
        if outcome:
            print(f'Diagnostics retained at: {demo.root}', file=sys.stderr, flush=True)
        else:
            try:
                shutil.rmtree(demo.root)
            except OSError as error:
                print(f'Could not remove temporary files: {error}\nDiagnostics: {demo.root}', file=sys.stderr)
                outcome = 1
    if not outcome:
        print('\nFinished. The demo application and its windows are closed.', flush=True)
    return outcome


if __name__ == '__main__':
    sys.exit(main())
