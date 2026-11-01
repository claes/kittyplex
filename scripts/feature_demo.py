#!/usr/bin/env python3
"""Deterministic, dependency-free assets and guided terminal compatibility checks."""
import argparse
import array
import fcntl
from pathlib import Path
import random
import shlex
import struct
import subprocess
import termios
import zlib


def png(path, width, height, noisy=False):
    rng = random.Random(73)
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            color = (rng.randrange(256), rng.randrange(256), rng.randrange(256)) if noisy else (
                (230, 50, 60) if (x // 32 + y // 32) % 2 else (30, 130, 240))
            rows.extend(color)
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    Path(path).write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', width, height, 8, 2, 0, 0, 0))
                           + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def assets(directory):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    png(directory / 'test.png', 256, 128)
    png(directory / 'large.png', 512, 512, noisy=True)
    # Two solid-color frames; clear codes keep the LZW code size at three bits.
    gif = bytearray(b'GIF89a' + struct.pack('<HHBBB', 2, 2, 128, 0, 0) + bytes([255, 0, 0, 0, 0, 255]))
    gif.extend(b'!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00')
    for color in (0, 1):
        codes = [4, color, 4, color, 4, color, 4, color, 5]
        packed = sum(value << (3 * index) for index, value in enumerate(codes)).to_bytes(4, 'little')
        gif.extend(b'!\xf9\x04\x00' + struct.pack('<H', 20) + b'\x00\x00')
        gif.extend(b',' + struct.pack('<4H', 0, 0, 2, 2) + b'\x00\x02\x04' + packed + b'\x00')
    gif.extend(b';')
    (directory / 'animation.gif').write_bytes(gif)
    return directory


def geometry():
    size = array.array('H', [0, 0, 0, 0])
    with open('/dev/tty', 'rb', buffering=0) as tty:
        fcntl.ioctl(tty, termios.TIOCGWINSZ, size)
    return list(size)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write-assets')
    parser.add_argument('--geometry', action='store_true')
    parser.add_argument('--show')
    args = parser.parse_args()
    if args.write_assets:
        assets(args.write_assets)
    elif args.geometry:
        print('rows, columns, pixel width, pixel height:', *geometry())
    elif args.show:
        root = Path(args.show)
        print('rows, columns, pixel width, pixel height:', *geometry(), flush=True)
        subprocess.run(['kitten', 'icat', str(root / 'test.png')], check=True)
        print('Ctrl-Shift-M, c creates a session tab. Run kittyplex-features there too.')
        print('Switch using the Kitty tab bar/native shortcuts and Ctrl-Shift-M, n/p.')
        print('Each tab should retain its own image; clearing one must preserve the other.')
        print('Resize or change the font size, then run kittyplex-features again.')
        print('Additional explicit checks:')
        print(shlex.join(['kitten', 'icat', '--transfer-mode=stream', str(root / 'large.png')]))
        print(shlex.join(['kitten', 'icat', '--loop=1', str(root / 'animation.gif')]))
        print('kitten icat --clear')
        print('kitten show-key -m kitty')
        print('Clipboard, hyperlink opening, mouse clicks and desktop notifications require manual checks; see docs/kitty-compatibility.md.')
    else:
        parser.print_help()


if __name__ == '__main__':
    main()
