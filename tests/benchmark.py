#!/usr/bin/env python3
"""Optional real-kitty/SSH benchmark. Writes measurements, never invents network RTT."""
import argparse
from datetime import datetime, timezone
import subprocess
import json
import os
from pathlib import Path
import secrets
import shutil
import shlex
import statistics
import sys
import time
import unittest

# integration.py re-enters through kitty's Python when crypto support is needed.
# Start there ourselves so runpy executes this benchmark rather than its test suite.
try:
    import kitty.remote_control
except ImportError:
    os.execvp("kitty", ["kitty", "+runpy", "import runpy; runpy.run_path(" + repr(str(Path(__file__).resolve())) + ", run_name='__main__')", *sys.argv[1:]])
sys.path.insert(0, str(Path(__file__).resolve().parent))
from integration import CAN_BIND, CarrierTests


class Benchmark(CarrierTests):
    def latency(self, window):
        timings = []
        for _ in range(self.samples):
            token = secrets.token_hex(8)
            start = time.monotonic()
            self.send(window, f"printf 'BENCH_%s\\n' {token}\n")
            self.wait_text(window, "BENCH_" + token, timeout=30)
            timings.append((time.monotonic() - start) * 1000)
            self.ready_after(window,"BENCH_" + token)
        return dict(samples_ms=timings, p95_ms=statistics.quantiles(timings, n=20)[18])

    def ready_after(self, window, marker):
        def ready():
            tail=self.text(window).rsplit(marker,1)[-1]
            return "remote session " in tail or "DIRECT_PROMPT>" in tail
        self.wait_for(ready)

    def throughput(self, window):
        token = secrets.token_hex(8)
        start = time.monotonic()
        self.send(window, "head -c 1048576 /dev/zero; " + f"printf 'DONE_%s\\n' {token}\n")
        self.wait_text(window, "DONE_" + token, timeout=120)
        rate = 1048576 / (time.monotonic() - start)
        self.ready_after(window,"DONE_" + token)
        return rate

    def runTest(self):
        baseline = int(self.rpc("launch", dict(type="tab", args=self.fixture.ssh[:-1] + ["-tt", self.fixture.ssh[-1],
            "exec " + shlex.join(["env", "PS1=DIRECT_PROMPT> ", shutil.which("bash"), "--noprofile", "--norc", "-i"])])))
        self.wait_text(baseline, "DIRECT_PROMPT>")
        self.send(baseline, "printf 'DIRECT_%s\\n' READY\n")
        self.wait_text(baseline, "DIRECT_READY")
        self.ready_after(baseline,"DIRECT_READY")
        direct = self.latency(baseline)
        direct_throughput = self.throughput(baseline)
        active = self.first
        for _ in range(7):
            active = self.create(active)
        self.send(self.first, "yes FLOOD\n")
        loaded = self.latency(active)
        self.send(self.first, "\x03")
        self.wait_for(lambda: self.text(self.first).rstrip().splitlines()[-1].startswith("remote session 1"))
        carrier_throughput = self.throughput(active)
        result = dict(recorded_at=datetime.now(timezone.utc).isoformat(),
                      kitty_version=subprocess.check_output(["kitty", "--version"],text=True).strip(),
                      kittyplex_version=subprocess.check_output([self.fixture.binary,"--version"],text=True).strip(),
                      network=("real localhost SSH with test-only delayed stdio transport" if self.fixture.rtt_ms else "real localhost SSH, no artificial delay"),
                      injected_rtt_ms=float(os.environ.get("KITTYPLEX_TEST_RTT_MS", "0")),
                      polling_resolution_ms=20, sessions=8, direct=direct, loaded=loaded,
                      additional_p95_ms=loaded["p95_ms"] - direct["p95_ms"],
                      direct_bytes_per_second=direct_throughput,
                      carrier_bytes_per_second=carrier_throughput)
        print(json.dumps(result, indent=2))
        if self.output:
            Path(self.output).write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output")
    parser.add_argument("--rtt-ms", type=float, default=0, help="added transport RTT in milliseconds (0–2000)")
    parser.add_argument("--samples", type=int, default=20, help="latency samples per case (at least 20)")
    args = parser.parse_args()
    if args.samples < 20:
        parser.error("at least 20 samples are required")
    if not CAN_BIND:
        parser.exit(2, "Benchmark NOT RUN: environment denies socket binding.\n")
    os.environ["KITTYPLEX_TEST_RTT_MS"] = str(args.rtt_ms)
    Benchmark.output = args.output
    Benchmark.samples = args.samples
    result = unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite([Benchmark("runTest")]))
    if not result.wasSuccessful():
        raise RuntimeError("benchmark failed")
