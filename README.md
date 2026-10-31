# termlab

A small Linux terminal session switcher, written in Rust. Each shell gets a PTY;
the selected session's input and output pass directly through your terminal.
Termlab does not parse terminal output or implement a screen model.

## Build and run

Requires Rust 1.85 or newer and Linux:

```sh
cargo build --release --locked
./target/release/termlab
```

For a Nix development environment, run `nix develop` first. To install the binary
on your PATH, run `cargo install --path . --locked`.

Termlab starts `$SHELL -i`, falling back to `/bin/sh -i`. Every new shell inherits
the directory and environment in which termlab started, rather than the active
shell's later directory changes. Shell startup files run normally.

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

For this session, it selects `workspace-write`, enables command networking for
dependency downloads and local socket tests, and uses `on-request` approvals for
operations needing additional permissions. Networking is enabled broadly, rather
than restricted to package hosts. File writes remain restricted by the workspace
sandbox, and managed requirements can still restrict the effective permissions.
It does not modify your global Codex configuration or this running session.

Requires Codex CLI with `--no-daemon` support (checked with 0.164.0). Configuration
behavior is described in the official [OpenAI configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
and [approvals documentation](https://learn.chatgpt.com/docs/agent-approvals-security).

## Kitty shortcuts

Copy `kitty/termlab.py` and `kitty/termlab.conf` into your kitty configuration
directory, normally `~/.config/kitty/`, then add this line to `kitty.conf`:

```conf
include termlab.conf
```

Reload kitty's configuration or open a new kitty process. These are optional,
manual setup steps; building or running termlab does not edit your kitty config.
The integration uses a no-UI kitten and requires no `allow_remote_control` setting.
It is validated against kitty 0.49.1.

Press **Ctrl-Shift-M**, release it, then press:

| Key | Action |
| --- | --- |
| `c` | Create and select a shell |
| `n` / `p` | Next / previous session, wrapping around |
| `1`–`9` | Select the session at that position |
| `q` | Quit termlab and hang up its shells and foreground jobs |
| `Escape` | Cancel without a bell |
| `Ctrl-Shift-M` again | Send one literal Ctrl-Shift-M to the application |

Management mode ends after one action. An unknown key rings kitty's bell, if
enabled, and cancels without passing the key to the application. Change both
`ctrl+shift+m` bindings in `termlab.conf` to use a different prefix. Management
bindings are active only in windows with a registered termlab instance.

Kitty recognizes the physical shortcut and the helper sends a management command
to that window's private Unix socket. Termlab reserves no input bytes: ordinary
Ctrl-M/Enter, pasted text, mouse reports, and application keyboard encodings pass
through unchanged. A second termlab window controls a separate instance. Nested
termlab instances are not supported.

## Command-line control

Inside a session:

```sh
termlab ctl new
termlab ctl next
termlab ctl previous
termlab ctl select 1
termlab ctl status
termlab ctl quit
```

The shell receives `TERMLAB_SOCKET` and a stable `TERMLAB_SESSION` identifier.
Selection uses the current one-based position; positions shift when shells exit.
`status` reports the selected position, count, stable IDs, and shell PIDs.
If termlab is not installed on PATH, use the binary's absolute path.

From another terminal, specify the socket printed by `echo "$TERMLAB_SOCKET"`
inside a session:

```sh
termlab ctl --socket /path/to/control.sock status
```

The socket lives in a randomly named directory with mode 0700 under
`$XDG_RUNTIME_DIR` when suitable, otherwise `/tmp`. The foreground process owns
it and removes it on normal exit or handled termination. There is no daemon or
detach/reattach support. No socket path is shared across windows.

## Display limitations

Inactive output is drained and discarded so background programs can continue.
Switching clears the visible display and changes then restores the selected PTY's
dimensions to encourage a redraw. Programs may coalesce resize signals or redraw
incompletely. For Bash, Ctrl-L usually redraws the prompt; Vim's Ctrl-L requests
a redraw. Old shell output is not reconstructed, and kitty's own scrollback may
contain output from multiple sessions.

Terminal modes are shared by all sessions: keyboard reporting, mouse tracking,
alternate-screen state, cursor modes, and terminal queries are not independently
virtualized. Applications using different modes may need a manual redraw or
reset after switching. Inactive terminal queries cannot be answered because
their output is discarded. These limitations are inherent in this first version's
pass-through design. It provides no panes or stored per-session scrollback.

When the active shell exits, termlab selects the next remaining position; the
last shell exiting closes termlab. Shutdown sends SIGHUP and SIGCONT to shell and
foreground process groups, reaps shells, and restores the outer terminal's saved
termios settings. Jobs deliberately ignoring hangup may survive. SIGKILL cannot
run cleanup; use `reset` if a forced kill leaves the terminal in a bad state.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
python3 tests/integration.py
kitty +runpy 'import runpy; runpy.run_path("tests/kitty_config.py", run_name="__main__")'
```

Rust tests exercise PTYs, raw bytes, foreground interruption, resizing, output
draining, and child reaping. The Python suite exercises the complete application,
session switching, control requests, job control, cleanup, and instance isolation.
It explicitly skips socket-dependent cases if the environment denies Unix socket
binding. Kitty tests use its real configuration parser and shortcut dispatcher
without opening a GUI; helper routing uses a simulated socket client.

For a visual check in kitty, run Bash, `less`, and Vim in separate sessions;
switch between them, resize the window, verify literal-prefix forwarding with
`kitten show-key -m kitty`, and check the redraw limitations above.
