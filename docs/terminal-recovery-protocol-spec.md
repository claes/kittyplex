# Restore Display: experimental v0 specification

Status: **experimental, unregistered and not implemented**. Updated 2026-11-02.
Restore Display lets an application reconstruct its interface when a terminal
becomes available again. It is independent of Kittyplex and uses small ASCII
key/value messages. This specification replaces the earlier unimplemented draft;
there is no compatibility requirement with that draft.

The [demo and exerciser requirements](terminal-recovery-demo-spec.md) describe
reference integrations and tests. The [research document](terminal-recovery-protocol.md)
retains background and alternatives. No production detach implementation or
persistence strategy has been selected.

## 1. The application contract

An application that enables Restore Display promises to **reestablish the terminal
modes and resources it needs, then draw its current interface while preserving
application state and pending input**. This is more than an incremental refresh
or repaint. It does not require a saved terminal snapshot.

For example, a file manager restores its panels and active dialog; an editor
restores unsaved text and cursor positions. If the interface uses graphics, it
retransmits current image data and recreates placements. It must not assume that
images, colors or saved mode stacks from the old terminal still exist. If it
cannot meet this promise, it reports restoration unavailable.

MUST and MUST NOT state requirements; SHOULD describes a recommendation; MAY
permits optional behavior. V0 supports one display and one cooperating application
interface at a time. A **controller** owns the application PTY master and connects
it to a **display**, such as a terminal-emulator window. Keeping the process and
PTY alive is the controller's responsibility, not a service provided by this
protocol. An **enabled application** has opted in to receiving protocol input.

### Display status

```mermaid
---
title: Display status
---
stateDiagram-v2
    [*] --> initial
    initial --> attached: Display available
    initial --> detached: No display available
    attached --> detached: Display disengages or is lost
    detached --> attached: Display becomes available
```

- **initial**: The application has not yet established the display status through
  an accepted enable response. This is a local starting condition, never a wire
  value or a state to return to. An unsupported application may remain here
  without participating.
- **attached**: A display is available. Its contents may still need restoration;
  this status alone does not mean drawing or user input can resume.
- **detached**: No display is available. The process and its application data
  survive, and background computation may continue while drawing is paused.

The enable response establishes the first status; subsequent state messages
report availability changes. A restore request also establishes attached status,
without requiring a preceding attached notification. Preparing to detach and restoring are operations,
not additional display states. Track participation, the pending operation and its
transaction number separately. Preparation leaves the display attached until it
actually disengages. Restoration, including an unsuccessful attempt, leaves it
attached while the display remains available. Loss changes it to detached and
cancels the pending operation.

Disable, shutdown and child handoff end participation, not display availability.
They do not transition back to initial. A disabled participant's last-known status
is no longer authoritative; a later enable acceptance supplies the current status.

```mermaid
flowchart LR
    A[Application] <-->|Terminal input and output| P[Application PTY]
    P <-->|Control frames and terminal bytes| C[Controller]
    C <-->|Ordinary terminal traffic| D[Replaceable display]
    C -.->|Consumes application protocol frames| C
```

The controller consumes protocol frames and forwards unrelated terminal traffic.
The application recognizes protocol input before keybinding dispatch, including
in dialogs, search and terminal-query waits. Neither side maintains an extra
terminal screen model just to implement this protocol.

## 2. Escape sequence

All messages use this experimental vendor DCS string:

```text
ESC P @restore-display;action=ACTION[,key=value...] ESC \
```

Spaces in the framing notation are explanatory; none are transmitted.
For example, a restore request is the exact bytes:

```text
\x1bP@restore-display;action=restore,id=7\x1b\\
```

In this worked request, `action=restore` asks the application to reconstruct the
display, and `id=7` identifies this particular attempt. The application echoes
`id=7` in its `action=begin` and `action=complete` responses, or in an
`action=unavailable` response if it cannot finish. `ESC P` starts the DCS string,
`@restore-display;` selects this protocol, commas separate fields, and `ESC \`
ends the message. The `\x1b` notation above represents one escape byte, not four
literal characters. No spaces or newline are sent.

Only the seven-bit introducer `ESC P` and terminator `ESC \` are accepted. BEL
and eight-bit C1 forms are not used. The namespace is unregistered; implementations
must explicitly enable experimental v0 support. Graphics use their own terminal
protocol, not a payload inside Restore Display.

### Fields

| Key | Value |
| --- | --- |
| `action` | Required operation name from section 3 |
| `version` | Requested protocol version; `0` in the enable request |
| `id` | Transaction number, assigned by the controller |
| `status` | Operation result: `accepted`, `ready` or `unavailable`, as specified below |
| `display` | Display status: `attached` or `detached` |

Fields may be in any order. Keys are nonempty words of lowercase ASCII letters.
Values are nonempty ASCII words containing only letters, digits, `.`, `_`, `+`
or `-`. Known numeric
fields use unsigned decimal with no leading zeros except the value `0` itself.
Transaction numbers are `1..4294967295`. Empty fields, whitespace, repeated keys,
leading/trailing commas and invalid known values make a message invalid. Unknown
keys are ignored, allowing small future extensions. Unknown actions and invalid
messages are ignored; no generic error-response machinery is defined.

A frame, including introducer and terminator, must not exceed 512 bytes. Senders
serialize complete frames with their other terminal writes. Receivers handle
fragmented and adjacent frames, bound storage, and recognize this namespace only
at a control-string boundary, never inside another OSC/DCS/APC payload. Unrelated
strings pass through unchanged without unbounded buffering.

After recognizing the namespace, a receiver consumes the frame rather than
passing it to a display or keymap. An invalid/oversized frame is discarded through
its terminator. CAN (`0x18`) or SUB (`0x1a`) cancels it immediately. An ESC not
followed by `\` cancels it and is reconsidered as the next control introducer.
Incomplete frames remain bounded until completion, cancellation or connection
closure.

## 3. Operations

A = application; C = controller. Fields shown in the table are required.
There are no other required fields. Responses use the same action as their
request except restoration start/completion and unavailable notifications.

| Direction | Fields | Meaning |
| --- | --- | --- |
| A -> C | `action=enable,version=0` | Enable v0 participation for the current interface if supported |
| C -> A | `action=enable,status=accepted,display=detached` or `action=enable,status=accepted,display=attached` | Accepted; includes current display state |
| C -> A | `action=enable,status=unavailable` | Participation unavailable; no notifications enabled |
| A -> C | `action=disable` | Disable participation before shutdown or another reader takes over |
| C -> A | `action=disable,status=accepted` | Disabled; final protocol input for this participation |
| A -> C | `action=state` | Query current display state while enabled |
| C -> A | `action=state,display=detached` or `action=state,display=attached` | State response or unsolicited state notification |
| C -> A | `action=prepare,id=N` | Prepare to disengage the display |
| A -> C | `action=prepare,id=N,status=ready` | Ready: earlier output flushed and new drawing paused |
| C -> A | `action=restore,id=N` | Display is attached; restore it |
| A -> C | `action=begin,id=N` | Start of restoration output; earlier output is obsolete |
| A -> C | `action=complete,id=N` | Restoration output complete and flushed |
| A -> C | `action=unavailable,id=N` | Cannot complete this disengagement/restoration |

The controller never sends unsolicited input to a participant that has not
enabled the protocol. The application initiates a versioned enable request,
which combines support discovery and opt-in. It installs its decoder and prevents
terminal echo/line-discipline transformations before enabling.
An attached application draws normally after enable acceptance; a detached one
waits for a restore request. Unsupported controllers need not respond; absence
of a response means support was not established, not that restoration is impossible.

Application support is deliberately a single promise. It includes resources used
by that interface, not separate per-feature coverage flags. Screen dimensions
come from the existing terminal-size mechanism. Existing graphics, keyboard and
other terminal protocols supply their own capability queries when a display is
available. The controller updates actual PTY geometry before requesting restore.
The application may probe terminal features after its start marker, or report
unavailable rather than silently claim complete restoration with missing content.

Only one disengagement/restoration transaction is outstanding. The controller
uses increasing transaction numbers for the PTY lifetime, never reusing them;
it ends participation before exhausting the range. A new restore request replaces
an earlier transaction. The application stops the old drawing path before starting
the replacement; old requests are ignored. Only responses matching the current
transaction can advance the pending operation or release gated input. There are
no other IDs, tokens, global attachment generations or per-message counters.

## 4. Discovery and startup

```mermaid
sequenceDiagram
    participant A as Application
    participant C as Controller
    A->>C: action=enable,version=0
    C->>A: action=enable,status=accepted,display=attached
```

The application requests the version it implements; acceptance confirms that
version without a separate discovery exchange or final acknowledgement. The
controller must not accept an unsupported version. It may refuse with
`action=enable,status=unavailable`; an unsupported controller may remain silent.
Refusal or silence leaves support unestablished and ordinary operation available.

The accepted response reports `display=attached` or `display=detached`. The former
allows normal initial drawing; the latter requires waiting for a restore request.
The controller serializes acceptance before any subsequent protocol events on
the same input stream, so availability changes after acceptance arrive in order.
No new participation is enabled by a refused request. A repeated enable while
already enabled is refused and does not replace the active interface.
State queries are answered only while enabled. Unknown/disabled input is not a
place for controller probing.

Timeouts are local implementation policy, not fields in the protocol. Before
handing input to another reader, an application must resolve any outstanding
negotiation, including a late acceptance, through disable acknowledgement or
controller-confirmed revocation. Merely waiting a short time cannot prove that
no late terminal response will arrive. A decoder remains installed while resolving
that uncertainty. The exerciser explicitly tests a silent unsupported controller.

## 5. Display disengagement

```mermaid
sequenceDiagram
    participant A as Application
    participant C as Controller
    C->>A: action=prepare,id=7
    A->>C: action=prepare,id=7,status=ready
    Note over C: Finish earlier output delivery and detach
    C->>A: action=state,display=detached
```

The controller gates new user input before requesting preparation. The application
reaches a safe point, flushes earlier output and pauses drawing before replying
ready. Display status stays attached until actual disengagement, reported by the
final detached notification. The controller finishes ordered delivery of earlier
output before detaching.

Readiness is an output barrier, not a request to freeze the process. After it,
only protocol output is permitted until restoration. Pending input and application
data remain intact. An application that cannot prepare sends `action=unavailable,id=7`.
The controller cannot claim graceful disengagement without the matching readiness
response. On refusal or timeout it may keep the display or explicitly force loss;
that policy belongs to the controller. Before resuming user interaction after an
uncertain preparation, restore the display, since a late readiness response may
have paused drawing.

## 6. Restore display

```mermaid
sequenceDiagram
    participant A as Application
    participant C as Controller
    C->>A: action=restore,id=8
    A->>C: action=begin,id=8
    A->>C: Terminal initialization and current interface
    A->>C: action=complete,id=8
```

Before requesting restore, the controller connects the display, updates actual
PTY dimensions and gates user input. The restore request itself establishes
attached status; no separate attached notification is required. Attached status
alone, including a standalone state notification, does not resume drawing or
release user input. The application waits for restore, then stops obsolete writes,
flushes earlier output and finishes partial control strings before emitting begin.

Start and completion markers share the same serialized output path as drawing. The controller suppresses obsolete presentation
output to the replacement until the matching start marker, then forwards the
restoration output in order. The application emits completion only after flushing
all initialization and drawing output, never merely scheduling it. The controller
finishes ordered delivery before allowing normal interaction.

```text
Application output:   obsolete bytes | action=begin,id=8 | initialization + drawing | action=complete,id=8 | live output
Replacement display:     suppressed  |     consumed      |          forwarded       |       consumed       | forwarded
```

The application may report unavailable before or after the start marker using
`action=unavailable,id=8`. A failed or timed-out attempt leaves restoration visibly
incomplete and user input gated. A retry gets a new transaction number; only its
start marker reopens restoration output. Old completion messages cannot release input.

Restore means the same operation for an empty or previously used display:
reestablish owned modes/resources and redraw. Restoring saved stacks must include
the application-owned baseline needed for later return to a parent, not just the
current top value. A participant unable to do that must report unavailable.
Byte-perfect continuation of an intact retained view is a separate controller
mechanism, not another mode or capability in this protocol.

Completion establishes application output completion and ordered delivery. It
is not an acknowledgement that the terminal parsed everything or rendered a
frame. After reattachment, test subsequent input and drawing, not just a picture.

## 7. Unexpected loss and display changes

```mermaid
sequenceDiagram
    participant A as Application
    participant C as Controller
    Note over C: Display is lost
    C->>A: action=state,display=detached
    Note over A: Pause drawing and reconcile outstanding terminal queries
    Note over A,C: When a display becomes available, use the restore exchange
```

When a display becomes available, use the [restore exchange](#6-restore-display)
without a preceding attached notification. Pending output is retained or discarded
according to the controller's declared policy.

An application must support restoration without an earlier preparation handshake.
Detached notification causes it to stop new display queries/effects and drawing
when it can process the event. There is no extra quiescence acknowledgement for
unexpected loss; the later restoration start marker provides the required output
boundary. Delivery of detached notification alone does not prove drawing stopped.

A controller detects display failure and reports state; application-facing
heartbeats/leases are not part of v0. The application may query state after
uncertainty. State messages are ordered on the same input stream. A display loss
cancels the controller's pending transaction, so an old response cannot revive it.

If resize or replacement occurs during restoration, update actual PTY geometry
and issue a new restore transaction. The application serializes cancellation of
the old renderer with starting the new one. Ordinary resize handling while active
can continue to use existing terminal mechanisms. A restoration failure must not
be mistaken for successful ordinary resize.

## 8. Handoff and shutdown

```mermaid
sequenceDiagram
    participant P as Parent application
    participant C as Controller
    P->>C: action=disable
    C->>P: action=disable,status=accepted
    Note over P: Child runs, then parent resumes
    P->>C: action=enable,version=0
    C->>P: action=enable,status=accepted,display=attached
```

Before acknowledging disable, the controller cancels the pending transaction,
stops new events and serializes earlier input. The parent consumes the
acknowledgement before handing input to a child. The child may opt in separately
or remain unsupported. When the parent resumes, it uses the same versioned enable
exchange as startup, drawing normally if attached or waiting for restore if
detached. Participation ends on disable; last-known display status is retained.

Disable is idempotent: the controller acknowledges it even if participation has
already ended. This responds to an application request rather than notifying an
unknown reader. Disable acknowledgement is the last protocol input for that
participation. The application keeps decoding through it, discards cancelled
transaction events, and drains/reconciles earlier terminal replies before
transferring input. Normal
shutdown uses the same disable exchange; no separate goodbye operation is needed.
Applications can start or shut down while detached: enable reports
`display=detached`, and disable still works without a display.

If restoration fails or times out, display status remains attached while the
display is available, but user input stays gated until a new restore, disable or
explicit fallback resolves the attempt. After uncertain disengagement, a restore
is required before resuming interaction. Only one cooperating interface reads
input at a time; process names or shared foreground groups do not establish that.
The controller must revoke participation on known process death or invalidation.
An in-band protocol cannot atomically detect arbitrary uncooperative exec/reader
takeover or retract input already delivered. Integrations must document their
lifecycle monitoring and cooperate with handoff, rather than claim universal
unmodified-shell safety. This protocol is not an authentication boundary.

## 9. Implementation rules and examples

Controller policy handles deadlines, failure detection, buffering and fallback.
It must preserve the resources needed by a fallback before discarding them.
Failed restoration must not silently inject Ctrl-L or signals, replay old effects,
restart the application or terminate it. Raw PTY/control processing stays bounded
and responsive while detached; a fresh-restoration policy may drain obsolete
output without promising uninterrupted byte delivery.

User input is visibly rejected while disengaging/restoring rather than queued
for surprise execution. Previously delivered input is not replayed. Real terminal
replies and inserted protocol input are serialized without interleaving frames.
An outstanding query at loss must be cancelled/reconciled; stale replies must not
become keystrokes or answers to a new indistinguishable query. Fresh attachment
must not inherit replies from the discarded display. Clipboard writes, desktop
notifications and other one-time effects are not part of restoration.

Literal examples, with explanatory escape notation:

```text
Application -> Controller: \x1bP@restore-display;action=enable,version=0\x1b\\
Controller -> Application: \x1bP@restore-display;action=enable,status=accepted,display=attached\x1b\\
Controller -> Application: \x1bP@restore-display;action=restore,id=12\x1b\\
Application -> Controller: \x1bP@restore-display;action=begin,id=12\x1b\\
Application -> Controller: [ordinary terminal initialization, images and text]
Application -> Controller: \x1bP@restore-display;action=complete,id=12\x1b\\
```

Required parser/behavior tests include every frame split point, adjacent frames,
unknown keys/actions, invalid/repeated fields, the 512-byte bound, cancellation,
namespace text inside another control string, disabled participants, stale
transaction responses, completion before start, refusal, interrupted restoration
and acknowledged child handoff. A completion before start fails the transaction
and cannot release input. Unknown/invalid messages produce no generic error reply.

Framework adapters should isolate this small codec, schedule restoration below
keymaps at safe UI execution points, and expose application initialization/redraw
callbacks. Do not draw asynchronously from a signal handler. Passing protocol
checks is distinct from proving graphics, terminal modes and interactions work.
See the [exerciser](terminal-recovery-demo-spec.md#5-protocol-exerciser).

Existing protocol patterns, not an allocation or endorsement of Restore Display:
[Kitty graphics](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
[notification metadata](https://sw.kovidgoyal.net/kitty/desktop-notifications/),
[keyboard negotiation](https://sw.kovidgoyal.net/kitty/keyboard-protocol/), and
[Linux PTY behavior](https://man7.org/linux/man-pages/man7/pty.7.html).
