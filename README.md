# kittyplex

Remote PTY sessions with independent kitty-owned screens. Log in with ordinary
SSH, run `kittyplex`, and create or switch sessions with **Ctrl-Shift-M**. The remote
Rust process owns the applications and PTYs; local kitty retains each session's
screen, scrollback, cursor, parser, and terminal modes. Kittyplex transports bytes
without implementing a terminal emulator.

This is an experimental implementation of **approach 1, original terminal
carrier**. The real SSH/kitty acceptance suite now runs on an isolated virtual
display, including a test transport with 100 ms of added round-trip latency.
Use the demo below to check the interactive outcome on your desktop.
Read [How kittyplex works](docs/how-it-works.md) for a diagram-led explanation of
the processes, PTYs, and communication paths, starting with the basics. The
[carrier reference](docs/original-terminal-carrier.md) specifies the protocol and
failure contracts; the [compatibility reference](docs/kitty-compatibility.md)
records feature coverage and validation. The
[architecture analysis](docs/remote-session-architecture.md) preserves the
requirements and alternatives considered before implementation.

## Try it: one command

From a Linux graphical desktop, with Nix and flakes enabled:

```sh
./scripts/demo.sh
# Equivalent from the project directory (includes new working-tree files):
nix run path:.#demo
```

The first run builds/downloads dependencies. A separate kitty process opens,
connects to a temporary localhost SSH server, and starts kittyplex. No existing
kitty/SSH configuration, authorized keys, known hosts, or system service is changed.
No root access is needed. The fixture uses real SSH on one machine; it does not
simulate network delay or establish performance on a distant server.

The original terminal prints the shortcuts and fixture log location. Failed runs
retain their private temporary directory for diagnosis. Successful runs remove it
when the demo kitty process exits. If SSH authentication fails, inspect the printed
`sshd.log`; host account restrictions can still apply to an unprivileged server.

## Interactive walkthrough

Press **Ctrl-Shift-M**, release both keys, then press the action key.

1. In the initial session, run:

   ```sh
   echo "session=$KITTYPLEX_SESSION"; tty
   printf 'This is the first screen\n'
   ```

2. Press **Ctrl-Shift-M, c**. Run the same `echo`/`tty` command. The session ID
   and remote PTY must differ, and a new Kitty tab should appear. Switch back
   using the tab bar or Kitty’s native tab navigation, then use
   **Ctrl-Shift-M, 1** and verify that the first screen is still present.

3. Run `vim -Nu NONE -i NONE /tmp/kittyplex-demo-note` in the first session. Type
   `i`, enter a few lines, then Escape. Switch to session 2 and back using
   **Ctrl-Shift-M, n/p** or **1/2**. Vim's contents and cursor should persist.
   Exit Vim with `:q!` followed by Enter.

4. In session 2, run:

   ```sh
   for i in $(seq 1 100); do echo "counter-$i"; sleep .1; done
   ```

   Switch away immediately, wait a few seconds, and switch back. Output should
   have continued updating that session, without appearing in session 1.

5. Resize the kitty window and run `stty size` in each session. Try `sleep 30`,
   Ctrl-Z, `jobs`, `fg`, then Ctrl-C. These must affect the remote foreground
   job while leaving its shell usable. Close one session with `exit`; the
   remaining session should stay usable. Alternatively, close its native Kitty
   tab and verify that only that session ends.

6. Press **Ctrl-Shift-M, q**. You should return to the original SSH shell with
   working echo, Enter, and line editing. Run `echo restored`, then `exit` to
   close the demo and clean up its fixture.

## Kitty features and graphics

```sh
./scripts/demo.sh --features
# In a managed session:
kittyplex-features
```

This generates temporary PNG/GIF assets and prints commands for checking images,
large streamed transfers, animation, keyboard input, and sizing. Create two
sessions and display different images; switch using both the Kitty tab bar and
Ctrl-Shift-M shortcuts. Clear one image and check that the other remains, then
resize or change the font size and repeat.
The [Kitty compatibility reference](docs/kitty-compatibility.md) records automated
coverage, manual checks, and remote filesystem and desktop integration limits.

## Desktop installation

Choose how to configure Kitty:

- [Kittyplex NixOS/Home Manager modules](docs/integration-instructions.md#install-with-nixos-and-home-manager).
- [Explicit Nix settings without kittyplex modules](docs/integration-instructions.md#explicit-nix-configuration-without-kittyplex-modules), with a complete standard Home Manager flake example.
- [Your existing `kitty.conf`](docs/integration-instructions.md#manual-kitty-configuration), with package-only NixOS and plain Kitty examples.

Try `kittyplex run0 -u bob --pty` or `kittyplex ssh user@server` after installation.

## Install for a real remote server

Linux and kitty 0.47.4 (the pinned Nix version) are the implementation baseline;
the acceptance suite also passes on kitty 0.49.1. Build the Rust
binary with Rust 1.85 or newer:

```sh
cargo build --release --locked
# Or build the package and integration files, including new working-tree files:
nix build path:.
```

Install `kittyplex` on PATH **locally and remotely**, using the matching source
version. The local binary runs only the broker and view relays. Each machine can
build for its own architecture; no Nix installation is required on the server if
you install a compatible binary there.

Configure local Kitty using one of the [desktop installation paths](#desktop-installation).
The [manual setup guide](docs/integration-instructions.md#manual-kitty-configuration)
shows all five integration files, the config include and executable discovery
for source builds and Nix packages.

Then use your normal login:

```sh
ssh server
kittyplex
```

No forwarding configuration, ControlMaster, public-key environment forwarding,
additional SSH connections, or kitty patches are required. The original SSH
window remains the carrier in its original tab. Session views appear in a
separate native Kitty tab for each session; keep the carrier window open until
the group ends. New session tabs are appended in the carrier’s Kitty OS window.
They start with a `kittyplex N` title and follow application title changes. Your
Kitty tab-bar settings control their appearance.
The same integration also supports running `kittyplex` locally inside kitty.

### Upgrading an existing installation

Use the installation steps above to update both local and remote binaries and
all five Kitty integration files together. End existing groups and start a
**fresh Kitty process**: reloading configuration alone can retain cached Python
authorization code. Carrier v2 is incompatible with v1; its
[protocol contract](docs/original-terminal-carrier.md#protocol-v2-kittyplex-03)
records the version requirements. Version 0.3 fixes missing PTY pixel dimensions
that prevented ordinary `kitten icat` use.

Version 0.4 keeps carrier v2 and adds desktop launch commands. Automatic target
startup requires matching 0.4 binaries and the updated helpers. Existing bare
`kittyplex` startup remains supported.

## Controls and lifecycle

| After Ctrl-Shift-M | Action |
| --- | --- |
| `c` | Create and select a shell |
| `n` / `p` | Next / previous session |
| `1`–`9` | Select the session at that position, if present |
| `q` | Close the entire group |
| Escape | Cancel |
| Ctrl-Shift-M again | Send one literal Ctrl-Shift-M to the application |

Management mode ends after one action. Shortcuts activate only in session views.
Ordinary Enter/Ctrl-M, typing, paste, mouse reports, and terminal replies remain
application input. Change both `ctrl+shift+m` mappings to customize the prefix.

Each new session starts `$SHELL -i`, falling back to `/bin/sh -i`, inheriting the
controller's initial directory and environment. Native kitty focus changes are
synchronized, including tab clicks and native Kitty next/previous-tab navigation.
Kittyplex’s next/previous and numbered shortcuts stay within their own group and
use session creation order; manually reordering tabs does not renumber sessions.
Management commands identify their originating session.
Closing a session tab closes its remote session; exiting the last shell ends
the group. Quitting closes only kittyplex’s session views, preserving unrelated
tabs and ordinary windows you manually added.
Eight simultaneous sessions are supported initially.

**There is no detach/reattach or persistence after connection loss.** Closing the
carrier, losing SSH, or losing the controller closes the group. Quit hangs up
shells and foreground process groups and restores the original carrier termios.
Jobs deliberately ignoring hangup can survive; SIGKILL cannot run cleanup.

Heartbeats run every five seconds with a default thirty-second loss timeout:

```sh
kittyplex --heartbeat-timeout 60
```

Application output is never discarded to implement switching. Bounded queues
apply backpressure to producers. Saturated sessions can therefore block until
their downstream display catches up. Binary framing/base64 and kitty RPC calls
add overhead; see the measured results in the carrier implementation reference.

## Command-line control

From a managed remote shell:

```sh
kittyplex ctl new
kittyplex ctl next
kittyplex ctl previous
kittyplex ctl select 1
kittyplex ctl status
kittyplex ctl quit
```

`KITTYPLEX_SOCKET` identifies the private **remote** control socket;
`KITTYPLEX_SESSION` is the stable session ID. `status` reports the selected position,
ready-session count, IDs, and shell PIDs. Creating a view is asynchronous: `new`
acknowledges the request, and the session appears in `status` once its view is
ready. Positions shift when sessions exit. From another shell on the same server,
use `kittyplex ctl --socket /path/to/control.sock status`.

## Automated verification

```sh
nix develop --command ./scripts/test.sh
```

This runs formatting, Clippy, Rust codec/PTY tests, real kitty configuration and
authorization tests, the real SSH/kitty integration suite, and the
[Kitty feature audit](docs/kitty-compatibility.md). The GUI suites use an
isolated virtual display when Xvfb is available. It reports explicit skips when
socket binding or a graphical display is unavailable; a skip is not an E2E pass.
`nix flake check path:.` builds the package, checks Python syntax and validates the
NixOS/Home Manager modules and generated Kitty config. It does not run the
graphical acceptance suite. The separate `nix build path:.#desktop-test -L` runs
actual cross-user run0 cases in a NixOS VM; see the integration instructions.

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

The latest 0.3 measurements use eight sessions and one hidden flood. The
localhost case still exceeds the 100 ms additional-p95 latency target; the
100 ms injected-RTT case meets it in this run. See the
[raw results and limitations](docs/original-terminal-carrier.md#03-performance-follow-up).


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
