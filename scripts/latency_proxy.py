#!/usr/bin/env python3
"""Test-only SSH ProxyCommand: delay each direction without root or network setup."""
from collections import deque
import os
import select
import socket
import sys
import time


def main():
    host, port, rtt = sys.argv[1:]
    delay = float(rtt) / 2000
    with socket.create_connection((host, int(port)), timeout=5) as network:
        network.setblocking(False)
        network.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        os.set_blocking(0, False)
        os.set_blocking(1, False)
        # reader, writer, pending chunks, pending bytes, EOF
        streams = [[0, network.fileno(), deque(), 0, False],
                   [network.fileno(), 1, deque(), 0, False]]
        half_closed = False
        while True:
            now = time.monotonic()
            readers = [s[0] for s in streams if not s[4] and s[3] < 1024 * 1024]
            writers = [s[1] for s in streams if s[2] and s[2][0][0] <= now]
            deadlines = [s[2][0][0] - now for s in streams if s[2] and s[2][0][0] > now]
            timeout = min([.05, *deadlines])
            ready_read, ready_write, _ = select.select(readers, writers, [], timeout)
            for stream in streams:
                reader, writer, pending, count, eof = stream
                if reader in ready_read:
                    try:
                        data = os.read(reader, 65536)
                    except BlockingIOError:
                        continue
                    if data:
                        pending.append([time.monotonic() + delay, memoryview(data)])
                        stream[3] += len(data)
                    else:
                        stream[4] = True
                if writer in ready_write and pending:
                    try:
                        used = os.write(writer, pending[0][1])
                    except BlockingIOError:
                        continue
                    stream[3] -= used
                    pending[0][1] = pending[0][1][used:]
                    if not pending[0][1]:
                        pending.popleft()
            if streams[0][4] and not streams[0][2] and not half_closed:
                network.shutdown(socket.SHUT_WR)
                half_closed = True
            if streams[1][4] and not streams[1][2]:
                return


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, ConnectionResetError):
        pass
