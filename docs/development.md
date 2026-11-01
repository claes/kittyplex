# Development and verification

Run these commands from the project root. The locked development shell provides
Rust, Kitty, the manual-page validator and the SSH/Xvfb test dependencies.
For installation and daily use, see the [installation guide](integration-instructions.md)
and [usage guide](usage.md).

## Automated verification

```sh
nix develop --command ./scripts/test.sh
```

This runs formatting, Clippy, Rust codec/PTY tests, real kitty configuration and
authorization tests, the real SSH/kitty integration suite, and the
[Kitty feature audit](kitty-compatibility.md). The GUI suites use an
isolated virtual display when Xvfb is available. It reports explicit skips when
socket binding or a graphical display is unavailable; a skip is not an E2E pass.
`nix flake check path:.` builds the package and installed manual, checks documentation/help consistency and
Python syntax, and validates the
NixOS/Home Manager modules and generated Kitty config. It does not run the
graphical acceptance suite. The separate `nix build path:.#desktop-test -L` runs
actual cross-user run0 cases in a NixOS VM: Bash/Zsh/Fish, both PTY modes,
identity/socket ownership, directory/group overrides, job control and rejection.
Its passwordless authorization rule exists only inside the test VM. A skipped
VM run does not establish authentication behavior on your desktop.

For optional measurements against direct SSH, using eight sessions and hidden
output:

```sh
nix develop --command python3 tests/benchmark.py --rtt-ms 100 --output /tmp/kittyplex-benchmark.json
```

The benchmark records samples, p95 latency, and throughput. Add `--samples 100`
for the longer measurement used in the implementation reference. `--rtt-ms 100` adds
50 ms in each direction through a test-only SSH stdio proxy; the default adds no
delay. Screen polling has 20 ms resolution. These are local automated
measurements, not a guarantee for every server or network.

For dated measurements and their limitations, see the
[performance record](original-terminal-carrier.md#03-performance-follow-up).

To run acceptance tests over the same delayed transport:

```sh
nix develop --command env KITTYPLEX_TEST_RTT_MS=100 ./scripts/test.sh
```

## Working on this project with Codex

```sh
./codex-dev.sh
./codex-dev.sh "Run the complete integration suite and fix any failures"
./codex-dev.sh resume --last
```

The launcher finds this project even when called from another directory and
enters its locked Nix development shell before starting your installed Codex CLI.
It uses `--no-daemon` so the session starts with the development tools on PATH,
inherits the shell environment with your existing environment filters, and
disables login shells so command execution keeps that PATH. Your configured
model and reasoning settings are unchanged; additional arguments go to Codex.

The launcher selects a `kittyplex-dev` permission profile extending `:workspace`,
keeps `on-request` approvals, and enables direct command networking with local
binding. It disables Codex's network proxy for this invocation so tests can
create TCP listeners and Unix sockets for SSH, kitty IPC, and Xvfb. Network access
is broad; file writes retain the workspace profile's restrictions. It does not
change your global Codex configuration or the permissions of an already running
session. Restart through the launcher to apply the changes.

Do not pass `--sandbox` or configure legacy `sandbox_mode` settings alongside this
profile: Codex gives those legacy settings precedence. Administrator-managed
network and permission requirements can still restrict the requested access.

Requires Codex CLI with `--no-daemon` support (checked with 0.164.0). Configuration
behavior is described in the official [OpenAI configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
and [approvals documentation](https://learn.chatgpt.com/docs/agent-approvals-security).
