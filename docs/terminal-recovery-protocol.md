# Cooperative terminal recovery protocol research

Status: **proposed research protocol, not implemented or assigned an encoding**.
Updated 2026-11-02. This document owns the detailed cooperative-recovery analysis;
the [detach analysis](detach-analysis.md) compares persistence and display-recovery
alternatives. No persistence strategy, adapter implementation or wire format is
selected. Current kittyplex behavior is described by the
[carrier lifecycle contract](original-terminal-carrier.md#lifecycle-and-failures).

The design directions below distinguish application capabilities, intentional
disengagement, unexpected loss and reattachment. Detailed framing, ownership
proof, fallback priorities and timing policies remain open. Existing terminal
protocols and library APIs are evidence for building blocks, not support for this
proposed operation in Kitty or the example applications. No protocol prototype or
recovery experiments were run.

## 1. Purpose and recovery guarantee

Instead of reconstructing an application's display from earlier output,
kittyplex would notify a cooperating application that its old terminal display
has been replaced. The application would recreate its interface from its own
surviving state. An editor already knows its documents, splits and cursor; a file
manager knows its panels and active dialog. Kittyplex could avoid retaining a
second cell grid for the interface covered by that cooperation.

Distinguish two capabilities: **repaint only**, which assumes appropriate terminal
state already exists, and **fresh-terminal recovery**, which reestablishes required
terminal modes and fully redraws. Only the latter is a candidate for recovery
without a separate terminal-state model. Its guarantee must say which interface
state it covers, including graphics and inactive buffers if applicable. Earlier
shell output and unsupported applications still need another mechanism. The
persistent owner and attachment foundation in the
[detach analysis](detach-analysis.md#3-shared-foundation-persistent-owner-temporary-attachment)
remains necessary.

## 2. Communication over the existing PTY

```text
Application output -> remote kittyplex -> local relay -> Kitty
                    [protocol requests and acknowledgements intercepted]
Application input  <- remote kittyplex <- local relay <- Kitty
                    [recovery notifications inserted after negotiation]
```

Application requests would travel in terminal output; kittyplex notifications
would enter the application's input through its PTY master. This is per-session
application traffic, distinct from the shared SSH carrier's management framing.
It needs no additional SSH connection or application socket. Linux documents
this bidirectional [PTY behavior](https://man7.org/linux/man-pages/man7/pty.7.html).

There is precedent for negotiated terminal-to-application events: enabled focus
reporting sends `CSI I` and `CSI O`, and Kitty notification interactions can send
structured escape-code responses. Neither means that terminal state was lost or
requires a full recovery. See [focus reporting](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-FocusIn_FocusOut)
and [Kitty notification responses](https://sw.kovidgoyal.net/kitty/desktop-notifications/).

The application or its terminal library must recognize the event before ordinary
keybinding dispatch, including in prompts, search and menus. Unknown bytes are
not an independent OS event channel: an unsupported reader could treat them as
input. Explicit opt-in is essential. Kittyplex would need bounded protocol
framing, but interpreting this small protocol need not imply interpreting all
terminal output into a screen model.

## 3. Negotiation, capabilities and fallback

The design direction is application-initiated negotiation. A participant announces
supported versions and capabilities in its output; kittyplex responds only after
that opt-in, and the exchange establishes accepted support for the current owner.
Kittyplex must not probe an unknown input reader with unsolicited escape codes.
No response means support was not established, rather than proof of inability to
recover. Neither process-name detection nor an earlier owner's registration is a
substitute for this exchange.

Capabilities describe guarantees, not whether an implementation uses a key,
signal, library or application callback:

| Capability | Proposed meaning |
| --- | --- |
| Repaint | Recreate visible content while relying on existing terminal modes |
| Fresh-display recovery | Reestablish required modes and recreate the supported interface in a replacement display |
| Disengagement handling | Prepare for intentional display loss and report an agreed output boundary; optionally suppress drawing afterward |
| Recovery completion | Identify when the requested generation's initialization/redraw output is complete |

### Candidate logical operations

These names describe semantics only; they are not wire messages or assigned
escape sequences. The negotiation sequence, encoding and capability representation
remain to be specified.

| Operation | Direction | Proposed purpose |
| --- | --- | --- |
| Announce support / negotiate | Application -> kittyplex / return | Application initiates version and capability discovery; kittyplex establishes compatible support |
| Enable recovery / accepted | Application -> kittyplex / return | Register the current interface owner and its repaint or fresh-terminal capability |
| Disable recovery | Application -> kittyplex | Relinquish support or ownership before leaving the interface |
| Prepare disengagement / prepared | Kittyplex -> application / return | Allow a capable owner to reach a safe point and acknowledge an output boundary before intentional detach |
| Attachment-state notification | Kittyplex -> application | Report the current display state and its generation to a negotiated participant |
| Query attachment state / state response | Application -> kittyplex / return | Reconcile current attachment state after negotiation or uncertainty |
| Recover display | Kittyplex -> application | Identify a recovery generation, replacement geometry and relevant terminal capabilities |
| Recovery complete | Application -> kittyplex | Mark completion of that generation's initialization and redraw output |
| Recovery unavailable | Application -> kittyplex | Report that the requested recovery cannot currently be performed |

An application must not assume that emitting an enable request establishes
support without receiving acceptance. Capability discovery must also work when
the application starts while no display is attached: the persistent server would
answer with the current attachment state, without pretending a view exists.

### Fallback needs previously preserved state

Kittyplex needs to distinguish established support, unknown or incompatible
support, and a capable owner that temporarily cannot recover. Repaint-only
support must not be treated as fresh-display recovery. Loss of ownership or a
failed generation invalidates assumptions about the current recovery attempt.

Graceful fallback is a design goal, not a selected priority order. Possible
mechanisms include retained Kitty state with missing-byte delivery, retained
output history, an independently maintained terminal-state model, or an explicitly
limited fresh view. The [detach analysis](detach-analysis.md#9-fresh-client-recovery-alternatives)
discusses their different guarantees. A fallback must have preserved the state it
requires before it is needed: failed cooperation cannot recreate discarded output
or a screen model that was never maintained. Determine the fallback promise before
discarding display updates. No automatic key or signal injection is implied.

## 4. Application responsibilities

On a recovery request, the application would schedule work at a safe event-loop
point, preserve its data and active interaction, apply the supplied geometry,
reestablish terminal presentation and input modes, invalidate its cached physical
screen, and draw the complete supported interface. It would then acknowledge the
same recovery generation. It must not execute pending command text, cancel a
search or alter a selection merely to reach its redraw function.

Receiving a dedicated event avoids reliance on a configurable redraw key, but
does not automatically make every application state recoverable. Blocking input
paths, nested event loops and library assumptions need investigation. A redraw
acknowledgement must represent completion of output, not merely scheduling a
later render. Deferral, refusal and timeout behavior remain open.

## 5. Output boundaries, input and replies

A candidate attachment sequence is: prepare the fresh view, establish its actual
dimensions and capabilities, coordinate an output boundary with the owner, request
recovery, deliver initialization and redraw, then resume normal interaction.

Old queued cursor movements or partial sequences must not arrive after the new
redraw and corrupt it. A generation identifier correlates requests and responses;
it does not by itself identify which PTY bytes are obsolete. A later design must
coordinate flushing of application-side output, PTY/server/relay queues and
partial parser state, and serialize recovery markers with the output they bound.
Multiple writers to the same PTY complicate this boundary. Do not silently discard
pending bytes under a guarantee that promises continuity.

While detached, buffering, backpressure, application-suppressed output and selective
discarding have different consequences. Discarding display updates is not a
license to discard terminal queries, replies or side effects indiscriminately.
Define reply ownership, outstanding requests, stale attachment input and input
gating during recovery. A replacement with different terminal capabilities may
require renegotiation or refusal; new geometry alone does not describe it.

Recovery complete would indicate application output completion at an agreed
boundary. It would not prove that Kitty has parsed or rendered a frame. This
distinction also applies to the existing delivery-credit mechanism.

## 6. Ownership, lifecycle and liveness

Application lifetime and display lifetime are independent. An application can
start, exit or change its interface owner while detached. The initial research
assumes one active attachment; simultaneous displays and competing input owners
are a separate extension, not a solved part of this protocol.

### Main lifecycle scenarios

| Scenario | Proposed responsibility / question |
| --- | --- |
| Application startup | Negotiate version/capabilities and ownership; obtain current state whether attached or detached |
| Application shutdown | Relinquish ownership and invalidate outstanding requests without requiring a visible display; crashes may provide no message |
| Intentional display disengagement | Prepare a capable owner, establish a boundary and acknowledge readiness before detaching |
| Unexpected display loss | Detect failure without assuming advance notice; define output and outstanding-query handling |
| Display re-engagement | Distinguish retained terminal state from a fresh display; supply geometry/capabilities and request recovery only as needed |
| Recovery completion, refusal or failure | Correlate the current generation; handle deferral, timeout, stale messages and another disconnect |
| Interface ownership handoff | Yield and reclaim around foreground jobs, external editors, suspension/resumption and nested programs |
| Attached display changes | Coordinate real resize and capability changes without confusing them with lost display state |
| Unsupported or incompatible participant | Keep ordinary interaction usable and select only a fallback whose required state was preserved |

### Intentional disengagement and unexpected loss

For intentional detach, a candidate exchange is prepare disengagement, application
preparation and acknowledgement at an agreed output boundary, then attachment
removal. This can allow drawing to stop while computation continues. It does not
mean freezing the process, abandoning pending input or closing its PTY. A redraw
generation and its acknowledgement are separate from this preparation boundary.

The disengagement capability is advertised explicitly; behavior for refusal,
deferral, timeout or an unsupported owner remains a policy question. Unexpected
loss cannot depend on a successful handshake. The persistent server may detect
it only afterward and notify the application that the display is absent. Later
recovery must remain possible without a preceding prepared acknowledgement.

On re-engagement, a retained view may need only ordered missing output; a fresh
view needs initialization and reconstruction. Geometry and capability information
must correspond to the replacement view, not a stale attachment. An application's
support for recovering its current interface does not recover earlier shell output.

### Interface ownership

Cooperation belongs to the current interface owner, not permanently to a PTY or
executable name. Bash must yield before a foreground application reads input;
Midnight Commander must yield when launching an external editor and reclaim
ownership when it resumes. Process identity or foreground-group membership alone
does not establish support: pipelines and nested applications can share groups.

A later specification must define handoff, scoped enable/disable lifetimes and
invalidation on exit, exec or an uncooperative child. An old parent's registration
must not authorize sending protocol bytes to an unsupported reader. Failure
between yielding and reclaiming must have a defined outcome. Retained-view
reattachment should remain distinguishable from fresh-view reconstruction.

Intentional detach, unexpected loss, takeover and a second disconnect during
recovery also need explicit transitions. Old completion messages must not complete
a newer recovery generation. No timeout, retry, automatic key/signal fallback or
unsupported-application policy is selected here.

### Attachment state and liveness

The initial design direction uses explicit attachment-state events, an
application-accessible current-state query, generation identifiers for rejecting
stale notifications, and server-side display failure detection. A query lets a
participant reconcile state after negotiation or communication uncertainty; it
does not independently detect a failed server that cannot answer. Lease and query
timeout policies are not specified.

The persistent kittyplex server would monitor its display attachment and notify
the current cooperating owner when that state changes. The display/server link
and the application/server protocol are different liveness relationships. A
responsive attachment does not prove that Kitty rendered the latest output or
that a person is watching. An application that misses server messages also cannot
infer a successful detach merely from silence.

Periodic application-facing heartbeats or attachment leases remain optional
future work, not an initial requirement. If later added, lease expiry should mean
attachment is no longer confirmed, not permission to kill the process or discard
application data. Scheduling delays, stopped processes and blocked event loops
can delay handling; renewed communication should reconcile the current generation
and attachment state before acting on old notifications. No renewal interval,
timeout or server-side heartbeat mechanism is selected.

## 7. Encoding and compatibility questions

Before assigning codes, investigate a namespace and framing that avoid existing
terminal protocols, bounded message lengths and parser storage, fragmentation,
cancellation, version negotiation and unknown-message handling. Define which
endpoint consumes each message, and how unrelated output remains byte-for-byte
transparent. Intermediate multiplexers can consume, transform or forward control
strings; pass-through and nested negotiation cannot be assumed.

Opt-in and recovery markers are application traffic, not authorization to execute
commands on the local host. Output from other writers or echoed content must not
silently replace the owner or create a false completion boundary. The discovery
and ownership mechanisms need to be evaluated together. This analysis deliberately
does not allocate a CSI/OSC/DCS identifier, choose payload syntax, or reserve a
terminal mode number.

## 8. Examples and proposed validation

| Example | Proposed integration | Evidence still needed |
| --- | --- | --- |
| Bash / Readline | A recovery event invokes terminal setup and redisplay without executing pending input | Multiline prompts/commands, cursor and input preservation, incremental search, completion, and foreground-job handoff; earlier command output remains separate |
| Go TUI using tcell | Recognize recovery below keybindings, restore presentation and request full screen synchronization | Modes, dialogs, selection, inactive state and subsequent input behave correctly; synchronization alone is not complete terminal initialization |
| Go TUI using Bubble Tea | Decode a typed recovery event and invalidate renderer assumptions while preserving the model | Full initialization/redraw, active overlays, asynchronous updates and completion ordering |
| File manager launching an editor | Yield ownership to the editor, then reclaim and redraw panels | Unsupported children receive no recovery bytes; crash and return paths do not leave stale ownership |

Readline exposes forced redisplay, tcell exposes `Screen.Sync()`, and Bubble Tea
translates terminal input events into typed messages. These are inspected building
blocks, not implementations of this proposed protocol. See the
[Readline redisplay API](https://gnu.googlesource.com/binutils-gdb/+/FSF/readline/doc/readline.html#3307),
[tcell screen interface](https://raw.githubusercontent.com/gdamore/tcell/main/screen.go)
and [Bubble Tea input translation](https://raw.githubusercontent.com/charmbracelet/bubbletea/main/input.go).
Upstream framework sources may differ from deployed versions.

Future tests should include unchanged and changed dimensions, complete and
fragmented messages, continuous output, queries pending at disconnect, redraw
deferral/refusal, unavailable peers, timeout and duplicate generations, interrupted
recovery, and nested applications. Include startup/shutdown while detached,
disengagement refusal or failure, unknown/incompatible capabilities, fallback
after failed recovery, and querying state after missed notifications. Verify that
the chosen fallback's required data was preserved before cooperation failed.
Compare the recovered interface and its next updates/input behavior.
Demonstrating a repaint in one normal mode is insufficient
evidence for context-independent fresh-terminal recovery.

The [shared ncurses adapter](#9-shared-ncurses-recovery-adapter) below is another
candidate integration, with library-specific validation requirements.

## 9. Shared ncurses recovery adapter

The protocol and an ncurses adapter would serve complementary purposes: the
protocol communicates the recovery request, while the adapter performs recovery
inside the application using ncurses' existing screen representation. Several
compatible applications could share that implementation without kittyplex
maintaining another cell grid. This is a proposed integration, not an existing
adapter or an established fresh-terminal recovery guarantee.

Ncurses normally composes application windows into a desired screen and compares
that with its model of the physical display to minimize output. After a Kitty
view is replaced, the physical-display assumption is stale even though the
application's windows survive. A full repaint could reuse those windows rather
than reconstructing them from earlier escape codes. The library documents its
[window composition and refresh operations](https://invisible-island.net/ncurses/man/curs_refresh.3x.html),
including `wrefresh(curscr)` for clearing and repainting the screen. Repaint is a
building block; it does not establish complete terminal reinitialization.

### Proposed recovery flow

1. The adapter negotiates protocol support for the active ncurses interface and
   advertises only the recovery capability it can provide.
2. Its input integration recognizes a recovery notification before application
   keybinding dispatch, including relevant prompt and dialog input paths.
3. It schedules work at a safe point in the application's UI execution, checks
   ownership, and coordinates the generation and output boundary with kittyplex.
4. It reestablishes terminal presentation and required input modes, invalidates
   physical-screen assumptions, and repaints the composed interface while
   preserving windows, selections and cursor state.
5. It acknowledges completion only after the corresponding output boundary is
   established. Partial or deferred recovery must not be reported as complete.

The adapter remains subject to the ordering, reply ownership and lifecycle rules
in [section 5](#5-output-boundaries-input-and-replies) and
[section 6](#6-ownership-lifecycle-and-liveness). It does not independently decide
how to buffer or discard output while detached, and a repaint does not make old
queued bytes safe to deliver afterward.

### Integration options

| Option | Potential benefit | Work and limits to investigate |
| --- | --- | --- |
| Ncurses extension | Recognize the protocol within library input/update paths and share recovery machinery across applications | Requires an extended library, compatible versions and application lifecycle information where the library cannot infer ownership |
| Application hooks using a shared helper | Provide explicit safe points, interface ownership and special-state handling with limited application changes | Requires adoption; the helper still needs a defined recovery contract |
| Preloaded adapter using `LD_PRELOAD` | Prototype interception in some dynamically linked applications without changing their executables | Symbol/version differences, macros and internal library calls can bypass wrappers; static binaries and secure-execution restrictions limit coverage |

Preloading and symbol interposition are established
[dynamic-loader mechanisms](https://man7.org/linux/man-pages/man8/ld.so.8.html),
not evidence that a transparent ncurses recovery adapter works. Wrapping only
`refresh()` would miss applications using `wrefresh()` or
`wnoutrefresh()`/`doupdate()`. Recognition of a new event would also need input
integration, not just output interception.

### Reusing primitives without erasing application state

`endwin()` temporarily relinquishes terminal management without destroying the
screen objects; a later refresh/update resumes it. That lifecycle is a candidate
for investigation, not a proven recipe for restoring every mode on a replacement
Kitty view. See [ncurses initialization and suspension](https://invisible-island.net/ncurses/man/curs_initscr.3x.html).
Likewise, saved program-mode restoration concerns terminal-driver settings and
must not be equated with restoring all escape-controlled display/input state;
see [terminal-mode routines](https://invisible-island.net/ncurses/man/curs_kernel.3x.html).

Invalidate the physical display without erasing the application's desired
contents. In particular, `clear()`/`wclear()` erase window contents as well as
requesting a clear/repaint. A redraw experiment must not accidentally replace
the interface with blanks or repaint only `stdscr` when the application composed
multiple windows. See [clearing semantics](https://invisible-island.net/ncurses/man/curs_clear.3x.html).

### Coverage and proposed experiments

A static application blocked waiting for input must still be able to receive and
service recovery without an injected editing command. A background receiver
thread cannot simply call curses concurrently; safe scheduling and the deployed
library's [threading support](https://invisible-island.net/ncurses/man/curs_threads.3x.html)
need validation. Multiple screen contexts, nested input loops and resize-driven
application layout also require explicit handling.

State written outside ncurses, including direct escape sequences and graphics,
may be absent from its model. External subprocesses require ownership handoff;
earlier shell output is not supplied by the application's windows. Programs using
other frameworks need separate integrations. Consequently, the adapter must not
advertise complete recovery merely because an application links ncurses or uses
terminfo.

Start future experiments with a text-only, single-threaded ncurses application
at unchanged dimensions. Recover static and updating screens, then test multiple
windows, open dialogs, blocking input, resized layouts, nested/external programs
and interrupted recovery. Verify terminal modes, preserved interaction state,
cursor and subsequent input/output as well as the initial image. No adapter
implementation, supported-application set or protocol encoding is selected, and
no experiments were run for this subsection.

