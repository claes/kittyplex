# Original terminal carrier: implementation reference

Status: experimental implementation with real SSH/kitty acceptance tests and
recorded performance measurements. Updated 2026-11-01. The pinned Nix shell uses
kitty 0.47.4; initial API inspection also used 0.49.1.

## Decisions and ownership

Approach 1, original terminal carrier, is selected. The
[explanatory guide](how-it-works.md) describes the running system and ownership;
the [architecture analysis](remote-session-architecture.md) preserves the
requirements and alternatives. This document specifies the implementation
contracts rather than repeating those explanations.

The implementation requires Linux and matching carrier-v2 components and permits
at most eight live sessions per group. It uses the original terminal transport
without a separate SSH login per view, forwarding service or terminal emulator.

## Processes and interfaces

For the process tree and role descriptions, see
[which processes run where](how-it-works.md#2-which-processes-run-where).
The exact invocation and endpoint contracts follow.

`termlab [--heartbeat-timeout SECONDS]` starts the remote controller. The timeout
is at least ten seconds and defaults to thirty. The controller samples cooked
carrier termios before entering raw mode and seeds new application PTYs with that
snapshot. Each spawned shell is a session leader with its own controlling PTY.

`termlab ctl [--socket PATH] new|next|previous|select N|status|quit` uses the
existing private remote Unix stream socket. `new` is asynchronous: it acknowledges
a creation request before a ready shell is necessarily present. Shells receive
`TERMLAB_SOCKET` and `TERMLAB_SESSION`.

`termlab __broker DIRECTORY GROUP CARRIER HELPER TIMEOUT` and
`termlab __view DIRECTORY GROUP SESSION` are internal local interfaces, not user
commands. The broker requires kitty's inherited `KITTY_LISTEN_ON=fd:N` private
remote-control descriptor. Its private directory is created locally with mode
0700; `events.sock` receives datagrams and `views.sock` accepts relay streams.
Remote paths and remote environment variables never select the local executable.
`TERMLAB_BINARY` in kitty's startup environment, or its own PATH, selects it.

## Bootstrap and data paths

The [startup sequence](how-it-works.md#7-startup-and-session-creation) and
separate [output](how-it-works.md#5-application-output-server-to-screen) and
[input](how-it-works.md#6-input-and-terminal-replies-screen-to-server) sequences
are explained in the guide. Framing and ordering are specified below.

Kitty callbacks must not wait for a socket response. A busy carrier mailbox
returns an explicit retry response. Management-send failures produce a user error;
watcher focus updates are advisory and may be coalesced/lost under congestion.
Management messages identify the originating view; relay EOF independently
reports closure.

## Native tab navigation and ownership

The [user-facing selection and ownership rules](../README.md#controls-and-lifecycle)
are maintained in the README. The guide explains
[how native focus and retained screens work](how-it-works.md#8-switching-resizing-and-graphics).
Protocol routing uses stable group/session identities, independently of mutable
tab titles or positions.

## Protocol v2 (termlab 0.3)

Every message is a JSON object with exactly these fields:

| Field | Meaning |
| --- | --- |
| `v` | Protocol version, currently `2` |
| `g` | 32-character hexadecimal group ID |
| `seq` | Direction-wide sequence; local callback messages use zero |
| `kind` | Operation |
| `id` | Stable session ID; zero for group operations |
| `data` | Base64 application bytes, or a management-command string |
| `n` | Credit amount; bootstrap uses heartbeat timeout seconds |
| `rows`, `cols` | Positive terminal cell dimensions for readiness/resize |
| `pixel_width`, `pixel_height` | Unsigned 16-bit pixel dimensions; zero means unknown |

All four dimensions come from the local view PTY’s `TIOCGWINSZ`, are sent on
readiness and every `SIGWINCH` (including pixel-only changes), and are applied
unchanged with `TIOCSWINSZ` to the corresponding remote application PTY. Unknown
pixel dimensions remain zero; the carrier does not invent a cell size. All v2
messages require the pixel fields, including messages where they are zero.
Carrier v1 is not negotiated; use the
[upgrade procedure](../README.md#upgrading-an-existing-installation).
The native Kitty remote-control version below is separate from termlab’s carrier
version. Feature evidence is recorded in [compatibility](kitty-compatibility.md).

Controller-to-kitty calls use `ESC P @kitty-cmd JSON ESC \\`, kitty protocol
version `[0,47,4]`, command `kitten`, and arguments `carrier` plus the serialized
envelope. Kitty responds with `accepted:SEQUENCE`, `busy`, or an explicit error.
Acceptance means the callback handed off a datagram, not that kitty consumed the
session's output. The helper acknowledges duplicates without forwarding them
again and holds later sequence numbers busy until preceding messages arrive.

Broker-to-controller messages use `ESC P @termlab: JSON ESC \\` inside kitty
`send-text` input directed only to the registered carrier window. This direction
uses strict monotonic sequences; duplicates are ignored and gaps fail the group.
Both kinds of DCS can be fragmented across reads or combined in one read. The
broker batches up to sixteen small carrier messages into one bounded send-text
call, retaining each message's sequence and destination.
Application data is base64, so embedded escapes and control bytes cannot escape
an envelope. Relays and the broker use newline-delimited envelopes over streams;
callback datagrams contain exactly one envelope.

| Operations | Direction/use |
| --- | --- |
| `bootstrap`, `create`, `select` | Controller bootstrap and view management |
| `ready`, `view_ready` | Broker ready; relay dimensions before remote spawn |
| `output`, `input` | Opaque session byte chunks |
| `credit`, `input_credit` | Downstream consumption acknowledgements |
| `resize`, `focus`, `command`, `close` | Session-specific management/events |
| `drop`, `shutdown`, `abort`, `closed`, `finish` | Ordered session/group termination |
| `heartbeat`, `lost` | Health and explicit carrier loss |

The broker's `_local` kitten entrypoint handles create, select, carrier check,
and cleanup operations. Terminal-origin authorization always rejects it; the
broker reaches it through its private FD.

### Flow control and scheduling

- Data chunks: at most 8 KiB; serialized envelope: at most 32 KiB.
- Controller: at most eight submitted, unacknowledged carrier calls. Up to four
  non-foreground messages can be in flight; the remaining capacity is reserved
  for foreground output. Background output is additionally limited to four
  submissions per 100 ms, preventing zero-latency floods from monopolizing kitty's
  UI thread. Producers receive backpressure; output is never dropped. Explicit
  busy responses retry without consuming
  application credits. A slow SSH response alone never triggers retransmission.
- Output: 256 KiB of outstanding application bytes per session. The relay returns
  credit only after writing those bytes into its local terminal endpoint. Credits
  are coalesced in 8 KiB increments or after 50 ms; small writes do not cause a
  separate kitty RPC each time.
- Input: 64 KiB per session. The remote PTY manager returns credit as PTY writes
  consume queued input.
- Local IPC queues: bounded to accommodate base64 framing; aggregate queue
  guard: 4 MiB per process/group. Eight live sessions keep the credit budgets
  within the intended aggregate envelope. Exceeding guards fails explicitly.
- Session reads and queued background output are scheduled round-robin.
  Foreground output has reserved capacity, while a foreground flood must leave
  capacity for other sessions. Management messages and input acknowledgements
  have priority within their transport budget. Per-session output order and the
  final-output-before-close barrier are preserved.

Credit acknowledges terminal-kernel acceptance rather than a rendered frame.
Kitty's normal PTY buffering and parser backpressure remain part of the path.
No bytes are silently discarded to make switching or flooding appear successful.

## Lifecycle and failures

| Trigger | Result |
| --- | --- |
| View ready | Spawn its remote application with that view's dimensions |
| Native view focus | Update remote selection; explicit commands also identify origin |
| Real resize | Resize that application's remote PTY |
| Shell exit and output EOF | Send final output, drop its relay, choose a remaining session |
| View closes | Relay EOF/watcher closes its remote session |
| Last session exits / quit | Stop applications; drain queued relay output; close views; acknowledge shutdown; restore carrier |
| Original SSH window closes | Local watcher closes owned views; SSH loss hangs up the remote controller |
| Broker crashes | Local death callback cleans up views and registration; remote traffic fails or times out |
| Missing/incompatible integration | Fail startup before creating application shells |
| Sequence corruption / limits exceeded | Fail the group explicitly |
| Heartbeat loss | Close the group after the configured deadline |

Heartbeats run every five seconds. The broker also checks whether its carrier
still exists. Relays independently time out a missing broker. Normal completion
focuses the original carrier and leaves its SSH shell usable. Raw-mode guards
restore termios without issuing global terminal-mode resets into another session.
Process cleanup sends SIGHUP/SIGCONT to shell and foreground job groups, then
bounds shell reaping. Deliberately hangup-resistant jobs may survive.

For orderly shutdown, the broker drains views and sends `closed` as its final
carrier-input frame, then stops emitting heartbeats and watcher messages. The
controller waits for outstanding carrier receipts and submits `finish`. Only
then does the broker unregister the carrier and clean up local views. The
controller restores termios after receiving the finish receipt. This barrier
keeps late protocol frames out of the resumed SSH shell.

There is no durable screen state in termlab and no detached process daemon.
SIGKILL, host crashes, and severed links cannot perform an orderly close handshake.

## Authorization and configuration

Installation and upgrades are covered by the [README](../README.md#install-for-a-real-remote-server).
The configuration uses password mode with an empty-password custom authorization
callback; no secret or public-key environment needs to be forwarded to the SSH
server. The callback permits only bounded protocol-v2 carrier calls from their
actual source terminal and checks its group binding.

It rejects generic remote-control operations, socket-origin requests, alternate
helper paths, local-adapter operations, target overrides, and streaming/async
control fields. The helper independently checks the real window ID against its
locally recorded carrier. Kitty's broker descriptor is a separate, locally
created capability and is not transported to the server. Configurations that
already grant unrestricted remote control bypass this restriction.

Primary API references: [kitty remote control](https://sw.kovidgoyal.net/kitty/remote-control/),
[protocol](https://sw.kovidgoyal.net/kitty/rc_protocol/),
[launch](https://sw.kovidgoyal.net/kitty/launch/), and
[custom kittens](https://sw.kovidgoyal.net/kitty/kittens/custom/).
The demo scopes kitty configuration with the documented
[`KITTY_CONFIG_DIRECTORY` mechanism](https://sw.kovidgoyal.net/kitty/invocation/).

## Demo, tests, and validation record

Demo and verification commands live in the [README](../README.md#automated-verification).
The fixture uses a separate Kitty process and a temporary localhost-only,
unprivileged sshd. Generated host keys are pinned in temporary known hosts; the
server disables password authentication and forwarding. `StrictModes no` is
scoped to the fixture because its authorized keys are in a private temporary
directory outside the user's home. The fixture does not modify user configuration
or system services.

The [compatibility reference](kitty-compatibility.md#automated-evidence-and-remaining-checks)
describes the isolated GUI controller and the limits of its evidence. Neither the
interactive demo nor product configuration enables that test-only capability.

`tests/benchmark.py` measures direct SSH and the carrier with eight sessions and
hidden output, recording raw samples and throughput in JSON. Screen polling is
20 ms. `--rtt-ms 100` adds 50 ms of buffering to each direction through a test-only
SSH ProxyCommand, requiring neither root nor a network service. Default demo
connections do not use this proxy. The latency target is at most 100 ms additional
p95 response time relative to direct SSH under the same test transport.

### Current 0.3 validation

Current test counts, tested versions, transport-delay coverage and untested
scenarios are maintained in the
[compatibility validation record](kitty-compatibility.md#validation-record-2026-11-01-europestockholm).
The dated initial records below are historical evidence, not a second current
validation summary.

### Initial 0.2 validation record

The initial restricted environment could not bind sockets. After restarting with
network access, real acceptance tests exposed and led to fixes for:

- A hardcoded kitty client version newer than the pinned Nix kitty.
- Custom kittens lacking `__file__`; helper discovery now uses kitty's config directory.
- The test controller assuming config-file Unix socket names lacked kitty's PID suffix.
- Premature test input before Bash finished preparing its prompt after startup/job control.
- Shutdown traffic ordering and a hidden flood monopolizing the output queue.
- Python bytecode compilation beside read-only Nix source files.

Validation on Linux x86_64:

- Rust tests: **14 passed**; formatting and Clippy passed.
- Kitty configuration, shortcut, authorization, and helper tests: **13 passed**.
- Real SSH/kitty acceptance and startup tests: **11 passed** on kitty 0.47.4
  with 100 ms added RTT, and **11 passed** on kitty 0.49.1 without added delay.
  These include broker-crash and controller-termination recovery, parser/UTF-8
  fragmentation, independent groups, and repeated quit/restart cycles.
- Nix package and Python checks passed; the packaged demo launcher built and its
  startup/help path executed. The packaged executable was checked separately.
- Desktop keystrokes and physical display refresh have not been instrumented;
  automated GUI checks use Xvfb. aarch64-linux has not been executed here.

### Initial 0.2 measured performance

Kitty 0.47.4, eight sessions, one continuously flooding hidden session, 100 latency
samples per case. Throughput is a separate 1 MiB transfer after stopping the flood.
It includes completion/polling overhead and is not a sustained raw-link benchmark.
Screen reads observe kitty's parsed screen state, not physical display refresh.

| Added transport RTT | Direct SSH p95 | Loaded carrier p95 | Added p95 | Carrier throughput |
| --- | ---: | ---: | ---: | ---: |
| None | 82.5 ms | 238.6 ms | 156.1 ms | 261.4 KiB/s |
| 100 ms | 194.8 ms | 249.6 ms | 54.8 ms | 219.2 KiB/s |

Raw samples: [localhost](benchmarks/original-carrier-localhost.json) and
[100 ms added RTT](benchmarks/original-carrier-100ms.json). Reproduce with
`python3 tests/benchmark.py --samples 100 --rtt-ms 100 --output /tmp/termlab.json`;
use `--rtt-ms 0` for the no-delay case. The delayed case meets the 100 ms
additional-p95 target in this run. The localhost flood case exceeds that target;
it remains a performance limitation of this implementation. Results vary with
host load and kitty version; 20 ms polling and a finite sample count limit precision.

Error-path aborts are fire-and-forget, so cleanup cannot leave a late RPC response
in the resumed shell's input stream. Normal shutdown uses the explicit finish
barrier described above.

The measured samples above describe this test setup. Automated
virtual-display checks establish the carrier feasibility gate; desktop-specific
rendering/input behavior still benefits from the interactive README walkthrough.
The acceptance suite does not establish every architecture-analysis scenario or
behavior on every kitty version and platform.

## 0.3 performance follow-up

These measurements precede the native-tab presentation change.
Two sequential 100-sample runs use Kitty 0.47.4, eight sessions, the same
hidden-output load and 20 ms screen polling. These are host-dependent measurements,
not a controlled comparison isolating the effect of geometry fields. The direct
SSH timings also differ from the 0.2 records. This update does not tune scheduling.

| Added transport RTT | Direct SSH p95 | Loaded carrier p95 | Added p95 | Carrier throughput |
| --- | ---: | ---: | ---: | ---: |
| 0 ms | 134.2 ms | 416.3 ms | 282.1 ms | 197.9 KiB/s |
| 100 ms | 290.0 ms | 293.5 ms | 3.5 ms | 218.4 KiB/s |

Raw records: [carrier v2 localhost](benchmarks/carrier-v2-localhost.json) and
[carrier v2 with 100 ms added RTT](benchmarks/carrier-v2-100ms.json). The no-delay
hidden-flood case exceeds the 100 ms additional-p95 target; the delayed case meets
it in this run. Neither these results nor the earlier 0.2 results guarantee
performance on another server. Physical display refresh was not measured.

## Deferred persistence follow-up

The deferred [detach analysis](detach-analysis.md) compares suspension, bounded
output history and hybrid approaches, records their evidence and limitations,
and lists the investigations needed before selecting an implementation.

The chosen initial policy is to end all sessions on transport/controller loss.
A later implementation must separately design a durable remote PTY owner,
authenticated reconnection, session discovery, and how fresh kitty screens recover
state without termlab acquiring an emulator. An existing kitty process retaining
views is different from reconnecting after those views have disappeared. Resolve
that distinction before promising detach/reattach; do not silently change the
current close-on-loss contract.
