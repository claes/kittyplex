# Minimal Restore Display image demo

The first implementation is a static image and a status line, plus an independent
PTY exerciser. It uses the experimental [Restore Display protocol](terminal-recovery-protocol-spec.md).
The same application process survives destruction of its display. This example
uses no Kittyplex runtime, SSH connection, terminal emulator or old-screen replay
inside the controller.

## Guided walkthrough

Run one command from the repository root:

```sh
./scripts/restore-display-demo.sh
```

The launcher builds the demo and supplies Kitty and Python through Nix. It works
with new, unstaged source files and excludes build artifacts from its source copy.
A graphical desktop and Nix with flakes enabled are required.

Follow the three Enter prompts. Enter works in the guide terminal or in the image
window; while detached, use the guide terminal to reopen the window:

1. See generated image #1 and the application PID in a new window. Press `i` to
   generate more images of differently colored boxes, then Enter to detach.
2. The guide confirms that the same application is still running and reports the
   selected image number. Enter opens a fresh window and restores that exact image.
3. Check that the image and application PID are unchanged. Enter closes the demo.

The walkthrough never generates an image during detach or reattach. If you select
image #8, the replacement window shows image #8 with the same pixels. `d` is disabled
in guided mode. You do not need to manage session paths or type exerciser commands.
Pressing `q` in the image window finishes the walkthrough normally. Completion,
Ctrl-C and EOF stop only this walkthrough's session. Successful runs remove temporary
files; cancelled or failed runs print the location of retained diagnostics.

The packaged equivalent is `nix run .#restore-display-walkthrough`. The shell
launcher is convenient for a working checkout containing new source files.

## Build and run manually

From the repository root:

```sh
nix develop --command cargo build --locked --manifest-path demos/restore-display/Cargo.toml
```

The executables are in `demos/restore-display/target/debug`. A separate Nix package
is available with `nix build path:.#restore-display`; `nix run path:.#restore-display-demo`
and `nix run path:.#restore-display-exerciser -- --help` expose its two executables.
The exerciser's Kitty adapter also needs `kitty` and `kitten` on PATH, provided by
the development shell.

To see the image directly in Kitty, without protocol negotiation:

```sh
demos/restore-display/target/debug/restore-display-demo-rust-direct --protocol=off
```

Pass `--image picture.png` to display a PNG instead. Its bytes stay in application
memory and travel in-band, so the replacement terminal need not access that file.
Without a file, the demo generates a numbered 64×64 RGBA image with a 4×4 grid of colored boxes.
Each `i` press advances the image number and generates a new set of colors. The
seed and image number reproduce its pixels exactly on restoration.
Graphics capability is probed before transmission. An unavailable terminal shows
an explicit placeholder; a graphics-required restoration reports unavailable.
The probe has a three-second local deadline to accommodate first-window startup.

| Key | Action |
| --- | --- |
| `i` | Generate the next numbered image of colored boxes; a supplied PNG switches to the first generated image |
| `d` | Outside guided mode only: arm/cancel generation of a new image on the next detach |
| `q` | Disable participation when established, clean up and quit |

The demo redraws only on an event. It has no panels, editor, background counter,
mouse interaction or child launcher. The `text` and `graphics` profiles are
implemented; advanced keyboard, colors, pointer, text sizing, shell metadata,
effects and saved mode-stack profiles remain later work. Basic presentation uses
an alternate screen and hidden cursor, with a known default terminal baseline.

## Run inside Kittyplex

Kittyplex implements the controller side for explicit session-display detach/attach
while its carrier remains connected. Follow the [Kittyplex walkthrough](usage.md#try-the-image-demo-through-kittyplex):
run `./scripts/image-demo.sh` from the repository root in a managed shell,
press Ctrl-Shift-M then `d`, list
sessions with `kittyplex ctl list`, and run `kittyplex ctl attach ID` from a control
shell. Omit `--protocol=off`. This uses Kittyplex's actual transport and view lifecycle;
the independent guided walkthrough above exercises its own controller instead.

## Detach and restore an image

Run these commands inside `nix develop`, from the repository root. The parent
directory must exist; the session directory must not already exist.

```sh
rd_bin="$PWD/demos/restore-display/target/debug"
rd_session="$(mktemp -d)/session"
"$rd_bin/restore-display-exerciser" start --session-dir "$rd_session" --demo-state -- \
  "$rd_bin/restore-display-demo-rust-direct"
"$rd_bin/restore-display-exerciser" attach --session-dir "$rd_session" --display kitty --fresh
"$rd_bin/restore-display-exerciser" ctl --session-dir "$rd_session" restore
```

Press `d` in the image window. Its status line shows `armed true`. From the control
terminal:

```sh
"$rd_bin/restore-display-exerciser" ctl --session-dir "$rd_session" detach
"$rd_bin/restore-display-exerciser" attach --session-dir "$rd_session" --display kitty --fresh
"$rd_bin/restore-display-exerciser" ctl --session-dir "$rd_session" restore
"$rd_bin/restore-display-exerciser" inspect --session-dir "$rd_session" --json
```

The first window closes. The next is a new isolated Kitty process with the original
application PID. If you explicitly armed generation with `d`, the detach hook creates
the next image before restoration. Otherwise it restores the selected image unchanged.
The guided walkthrough always uses the latter behavior.
Press `i` to confirm subsequent input works. Press `q` to quit, or explicitly
terminate the private session with:

```sh
"$rd_bin/restore-display-exerciser" stop --session-dir "$rd_session"
```

Detach never terminates the application. `stop` sends SIGTERM to the private
application group, then SIGKILL after two seconds if needed. Captures and reports
remain in the private session directory. Kitty configuration/cache writes also
stay there; the adapter uses neither production configuration nor authorization.

`attach --display terminal` relays through the invoking terminal and stays in the
foreground; use a second control terminal. It rejects `--fresh`. `headless` supplies
simulated 80×24 geometry and no terminal replies: use `--features text` for parser
and PTY checks. It cannot prove real display or graphics behavior. Fresh Kitty
attachment is implemented; retained-view transport is explicitly unsupported.

## Copying the integration

The independent package is [demos/restore-display](../demos/restore-display/README.md).
Its main call sites are:

| Component | Role |
| --- | --- |
| `codec.rs`: `Decoder::feed`, `frame` | Bounded ASCII DCS codec; leaves unrelated terminal strings intact |
| `demo.rs`: `Model::detached` | Change/preserve application data without touching the display |
| `demo.rs`: `draw`, `graphics` | Reconstruct text, modes and current image from the model |
| `demo.rs`: `run` | Decoder before key dispatch, restore scheduling, markers and acknowledged disable |
| `exerciser/server.rs`: `restore`, `protocol`, `output` | Gate obsolete bytes before resize, forward after matching begin, accept matching completion |
| `exerciser/client.rs`: `viewer` | Relay ordinary terminal traffic, preserve terminal replies while keys are gated, acknowledge ordered writes |

The controller reads the PTY even while detached. Before restore it gates old
output and user interaction, applies TIOCSWINSZ, then sends the request. The
application reads current dimensions, writes begin, probes graphics, initializes
and redraws, then writes complete. Only after the viewer acknowledges writing
those bytes does the controller release user input. Terminal replies keep flowing
through the gate. SIGWINCH schedules UI work; it does not draw in a signal handler.

For example, the actual wire frames around ordinary terminal/graphics output are:

```text
Application -> Controller: ESC P @restore-display;action=enable,version=0 ESC \
Controller -> Application: ESC P @restore-display;action=enable,status=accepted,display=detached ESC \
Controller -> Application: ESC P @restore-display;action=restore,id=1 ESC \
Application -> Controller: ESC P @restore-display;action=begin,id=1 ESC \
Application -> Controller: [initialization, terminal probes, image bytes and text]
Application -> Controller: ESC P @restore-display;action=complete,id=1 ESC \
```

Spaces around framing are explanatory. The adapter handles split frames, request
IDs and serial callbacks; the application supplies only model and redraw hooks.
Graphics use direct transfer in chunks of at most 4096 base64 bytes, with explicit
image/placement ID 1 in the isolated demo view. A fresh display receives current
image data again. Cleanup deletes only that owned image.

## Scenarios and third-party programs

Launch any cooperating program after `start ... --`; `--demo-state` is optional.
Third-party programs need no private application socket or instrumentation.
The control client's input is separate from application keys.

```sh
"$rd_bin/restore-display-exerciser" scenario --session-dir "$rd_session" \
  --file demos/restore-display/scenarios/static.json --report /tmp/restore-display-report.json
```

Run a scenario on a newly started, detached session. `static.json` checks unchanged
model/image restoration; `change-image.json` explicitly arms generation of image #2 while detached. JSON scenario and
report files are management artifacts; wire messages use the ASCII protocol.

Captures are `input.raw`, `output.raw`, `events.jsonl`, `relay.jsonl` and optional
model snapshots in `inspect`. Each raw direction has a 64 MiB capture limit;
truncation is reported and invalidates checks depending on complete capture.
The controller also bounds protocol, relay and delayed-output queues. Command-line
restore waits for completion or failure; deadlines do not kill the application.
Fault scenarios can fragment, delay, disconnect, duplicate, insert stale/malformed
frames, inject an unavailable response, or drop the next controller frame. The
`unsupported_version` fixture injects enable refusal; it is not a new wire field.

Model checks require snapshots; graphics hashes inspect image ID 1 in the isolated
Kitty view. Text equality masks the demo's diagnostic PID/status line. Missing
observers and advanced profiles produce explicit not-run/unsupported results and
exit 3 rather than successful evidence. Pixel/placement resource inspection alone
is not proof that an image appeared: the graphical acceptance test separately
checks actual Xvfb framebuffer pixels and subsequent interaction.

## Verification and limits

```sh
cargo fmt --manifest-path demos/restore-display/Cargo.toml --check
cargo clippy --locked --manifest-path demos/restore-display/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path demos/restore-display/Cargo.toml
cargo build --locked --manifest-path demos/restore-display/Cargo.toml
python3 demos/restore-display/tests/integration.py -v
python3 demos/restore-display/tests/walkthrough.py -v
```

The GUI tests launch their own Xvfb and Kitty processes. The walkthrough tests
exercise Enter in either window, image changes before detach, normal `q` exit,
Ctrl-C, detached EOF and startup/restore failures,
including cleanup without affecting an unrelated process. The image acceptance test
checks the generated box colors in the framebuffer, destroys the old display,
restores the current generation into a fresh process, then generates another image
through normal input. The guided tests separately verify that selecting image #32
restores those same pixels without any automatic change. Protocol
rejection is also tested with a separate Python implementation rather than only
the shared Rust codec. Optional `RESTORE_DISPLAY_TEST_ARTIFACTS=DIR` retains failed
case captures and the Xvfb framebuffer. Missing GUI tools are an explicit skip.

This is an experimental raw-mode reference, not Bash/Readline or general
job-control support. PTY packet events expose flow stops and queue flushes, which
invalidate active attempts. External SIGSTOP, arbitrary exec/reader handoff and
saved parent mode stacks are not covered. The decoder stays installed after an
auto-mode enable timeout, but quitting against an arbitrarily late accepting
controller cannot prove a safe reader handoff. Use `--protocol=off` for direct
Kitty use and the private exerciser for the protocol demonstration. Established
participation uses disable acknowledgement before cleanup; a missing reply has a
bounded five-second exit wait. These limits must not be interpreted as successful
full lifecycle conformance.
