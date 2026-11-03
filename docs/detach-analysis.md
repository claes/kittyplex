# Detach and reattachment analysis

Status: **historical research; broader persistence remains deferred**. Recorded
2026-11-01, updated 2026-11-02. No strategy for surviving carrier/controller loss,
retaining terminal history or suspending applications has been selected.

Update: cooperative session-display detach/attach is now implemented while the
carrier remains open; see the [usage guide](usage.md#detach-and-attach-a-display).
Transport/controller loss still closes the group. The broader persistence and
fallback approaches below remain research rather than implemented guarantees. See the
[current lifecycle contract](original-terminal-carrier.md#lifecycle-and-failures).
This document owns the deferred options and their tradeoffs; detailed cooperative
recovery research lives in the [protocol analysis](terminal-recovery-protocol.md). The
[running-system guide](how-it-works.md) explains today's processes and connections,
and the [architecture analysis](remote-session-architecture.md) preserves the
earlier requirements and display-backend alternatives.

## 1. Separate the persistence guarantees

| Guarantee | What must survive or be recovered |
| --- | --- |
| Process persistence | Remote shells, application memory, open resources and their application PTYs |
| Display persistence | Screen buffers, cursor, terminal modes, parser state, scrollback and graphics currently held by Kitty |
| Reattachment | An authenticated route from a new attachment to the existing remote workspace, with defined input/output continuity |

Keeping a program alive does not recover its old terminal screen. Conversely,
retaining a Kitty screen does not preserve a remote process after its PTY owner
has exited. Two distinct reattachment cases must be specified:

- **Retained views:** the original Kitty process, session views and relevant local
  endpoints remain alive. They already hold terminal state and may need only
  missing output.
- **Fresh views:** Kitty has exited, a view was destroyed, or attachment happens
  from another computer. Surviving processes must be connected to newly created
  screens with some defined recovery mechanism.

Survival of the remote session server's own crash or a server reboot is a further
problem, outside ordinary live-daemon detach/reattach. None of suspension,
recent-output history or output backpressure provides process checkpoint/restore.

## 2. Evidence and confidence

Distinguish established primitives, inspected implementation facts, an isolated
experiment, and proposed behavior. No complete persistence prototype was built.

| Evidence | What it establishes | What it does not establish |
| --- | --- | --- |
| Source inspection at the research baseline | The remote controller owns application PTY masters; dropping sessions performs hangup/reaping cleanup; carrier/broker loss currently closes local views | That existing lifecycle code can already detach safely |
| Delivery-credit handling at the research baseline | The controller gates application-PTY reads on available credit; the relay returns credit after writes into the local terminal endpoint | Safe detach, pending-data preservation across attachment loss, a Kitty parser checkpoint or rendered-frame acknowledgement |
| Small PTY suspension experiment during the discussion | A known child was confirmed stopped, generated no new output during the observation after draining, retained its PTY geometry, and resumed output with the same PID | Whole-workspace freezing, nonempty-backlog recovery, SSH disconnect handling, or display recovery |
| Linux signal, freezer, terminal and write documentation | Mechanisms and their limitations, discussed below | Their availability or correctness for a particular deployment/workload |
| Kitty API/source inspection against the implementation baseline | Text/format/cursor extraction and window launch metadata exist; no complete supported screen/parser/graphics checkpoint was established | That no future or private Kitty implementation could provide one |

The small PTY trial had no pending bytes in its post-stop drain. It must not be
treated as a validation of a real transport-drain protocol. Cgroup freezing was
not exercised; a usable writable delegated hierarchy was not available in the
analysis environment. Host-specific suspension and logout behavior remain to be
validated. The implementation baseline uses Kitty 0.47.4, with additional checks
on 0.49.1; upstream documentation links can evolve.

## 3. Shared foundation: persistent owner, temporary attachment

All three approaches require separating responsibilities currently combined in
the remote controller:

```text
Persistent remote kittyplex server
├── Owns application PTY masters and starts session shells
├── Tracks workspaces, sessions and attachment ownership
└── Accepts temporary attachment connections over private remote IPC
    └── Attachment process bridges the original SSH terminal carrier

On detach: attachment exits; server, application PTYs and processes remain.
On reattach: a new attachment connects to that same server and workspace.
```

Applications would retain their existing PIDs and in-memory state rather than be
restarted from saved commands. The existing single-SSH carrier can remain the
transport: this does not inherently require another SSH connection per session,
port forwarding, or a terminal emulator in kittyplex.

The persistent server must not depend on the attachment's terminal, standard
streams or lifetime. Simply suppressing one hangup signal is insufficient: the
current session destructor deliberately sends SIGHUP/SIGCONT and bounds shell
reaping. Detach must release attachment resources without dropping the server's
application sessions. Local broker, relay, registration and heartbeat lifetimes
also need a defined detached state rather than their current cleanup behavior.

Server logout policy is a deployment dependency. A background daemon can still
be terminated with its login-session scope; a suitable user-service arrangement
may be needed. This cannot be guaranteed by `setsid()` alone. See
[session creation](https://man7.org/linux/man-pages/man2/setsid.2.html) and
[systemd logout policy](https://www.freedesktop.org/software/systemd/man/latest/logind.conf.html#KillUserProcesses=).

Other shared design work includes workspace discovery, same-user authorization,
binding the correct server/workspace to retained local views, stale attachment
rejection, and explicit separation of detach from terminate. One controlling
attachment at a time is a simplifying candidate, not a selected requirement.
Multiple clients would add input ownership, sizing and reply-routing questions.

## 4. Option A: suspend applications while detached

This changes the workflow to **pause a workspace and resume it later**. Once the
workload has actually stopped, only a finite output tail remains to be delivered.
The persistent server and attachment machinery must stay outside the paused
workload and continue handling control traffic.

A candidate intentional-detach sequence is: block new user input, request and
confirm suspension, drain pending output, establish a delivery boundary, and then
disconnect. Reattachment would restore routing, reconcile remaining traffic and
dimensions, and resume the appropriate jobs. Each step requires failure handling;
this is an outline for investigation, not an implemented handshake.

### Suspension mechanisms

| Mechanism | Benefit | Limitation |
| --- | --- | --- |
| Ctrl-Z / SIGTSTP | Familiar application-aware job suspension | Usually targets the foreground job; applications may handle it, alter the screen and return control to the shell; background jobs remain |
| SIGSTOP/SIGCONT on known processes or groups | Few setup requirements; direct control over known targets | Descendant/group coverage, fork races, existing job-control stops and observable stop/continue events complicate a reliable workspace freeze |
| Dedicated cgroup freezer | A stronger workload boundary with a completion indication | Requires suitable delegation, workload placement and host support; the server itself must not be frozen |

SIGSTOP cannot be caught, blocked or ignored, but its effects can be observed by
parents; SIGCONT can invoke application behavior. The kernel documents why these
cycles can affect shells and debuggers. They are not a transparent substitute for
freezing. See [signal semantics](https://man7.org/linux/man-pages/man7/signal.7.html)
and the [freezer discussion](https://docs.kernel.org/admin-guide/cgroup-v1/freezer-subsystem.html).

Cgroup v2 can freeze a group and its descendants and reports completion through
`cgroup.events`. Freezing takes time, and delegation must permit the necessary
operations. Workload membership must be established before descendants can
escape the intended boundary. A signal fallback must not silently claim the same
guarantees. See the [cgroup v2 interface and delegation rules](https://docs.kernel.org/admin-guide/cgroup-v2.html).

### Benefits and costs

- Output-storage pressure stops growing once all relevant producers are stopped
  and already queued output has drained. Intentional detach is easier to bound.
- Application CPU use largely stops, but memory, files, locks and connections
  remain allocated. Background builds and downloads also stop unless excluded.
- The outside world does not pause. Network peers, deadlines and external
  services may time out. Resume cannot promise that every connection is usable.
- Jobs already stopped by the user must remain stopped after workspace resume;
  blindly sending SIGCONT to everything would change their state.
- Unexpected disconnect still requires buffering: applications can produce data
  before failure detection and before suspension completes.
- Suspension prevents further application changes but does not recover a lost
  Kitty screen. It also does not freeze local Kitty animation or rendering.

The strongest initial use case is retained Kitty views. Fresh-view recovery would
still require redraw, replay or another display-state mechanism.

## 5. Option B: keep running with bounded output history

The persistent server would continue reading PTY output while detached, retaining
a configurable recent history and evicting older data at its limit. A fresh
attachment could replay that history before joining live output. Existing
delivery-credit queues are not a retained history log.

This supports **leave work running and recover recent context**. It is attractive
for logs, builds and command output, and potentially for applications that
regularly repaint their whole display. It does not provide a general screen
snapshot.

### Output history is not rendered screen state

Consider an editor that drew its document earlier and now updates one status
field. The retained history might contain only cursor movement and `42%`; the
document and borders were drawn before the oldest retained byte. Replaying the
tail into a blank terminal cannot recover them. Increasing capacity helps in
practice but cannot guarantee that all currently visible content is represented.

"A few screens" must therefore be defined carefully:

| Retained representation | Meaning |
| --- | --- |
| Recent bytes or lines | Activity history, without a terminal screen model |
| Actual rendered screens | A terminal-state representation maintained by Kitty or another engine |

A byte cap gives predictable storage bounds. UTF-8, frequent updates to one line,
control sequences and image payloads make a screen-count estimate unreliable.
No default cap or history format was selected.

### Replay options and difficulties

| Variant | Expected capability | Additional work |
| --- | --- | --- |
| Recent text history | Useful shell/log context | Define treatment of control sequences and how history is presented |
| Recent terminal-byte history | Kitty interprets retained formatting and cursor commands | Safe truncation, replay effects and approximate recovery |
| History plus redraw assistance | Better recovery for supported interactive applications | Define application-dependent assistance; resize or Ctrl-L is not universally appropriate |

Key questions remain:

- **Truncation:** the retained tail can start inside UTF-8, a control sequence or
  an image transfer. Lightweight framing could retain complete sequences without
  becoming a screen emulator, but complete commands may still depend on lost state.
- **Replay effects:** clipboard operations and notifications should not run again.
  Replies to historical terminal queries must not become duplicate live input.
  Queries still awaiting answers must be distinguished from already answered ones.
- **Replay/live ordering:** establish a cutover position while producers continue
  running, then join live output without omissions or duplicates.
- **Slow clients:** detect when retained data has been evicted before delivery.
  History gaps must be explicit, not silently treated as uninterrupted output.
- **Detached queries:** a buffer does not answer terminal requests. Processes can
  remain alive yet wait for input or a reply, or encounter application timeouts.
- **Graphics and modes:** image placements, keyboard modes, palettes and other
  state may depend on earlier output. Image data can be larger than a text-history
  budget and file/shared-memory references may no longer be usable on reattachment.

For retained views, replay only the missing range identified by an output
position, not the entire recent buffer. If that range is still available, existing
Kitty state supplies the older context. If it was evicted, recognize a continuity
failure and apply an explicitly limited recovery policy.

## 6. Option C: stop consuming terminal output while detached

This supports **keep running until terminal output applies backpressure**. The
persistent owner would keep application PTY masters open but stop reading them
while detached, either immediately or after a bounded buffer fills. This is a
proposal inferred from existing credit handling, not tested detach behavior.

### Terminal flow control versus stopping PTY reads

Ctrl-S normally requests an output stop and Ctrl-Q restarts output when the
terminal's `IXON` setting and configured STOP/START characters permit it.
Applications can disable or change these settings; `IXANY` can also allow other
input to restart output. Injecting Ctrl-S is therefore not a reliable detach
protocol. Programmatically, `tcflow(fd, TCOOFF)` suspends terminal output and
`tcflow(fd, TCOON)` restarts it. These control output transmission at the relevant
terminal; they do not suspend processes. See Linux
[terminal settings](https://man7.org/linux/man-pages/man3/termios.3.html) and
[tcflow](https://man7.org/linux/man-pages/man3/tcflow.3.html).

Stopping reads from an application PTY master is a different mechanism: it leaves
terminal flow-control settings alone and lets unread output accumulate until
kernel queues exert backpressure. Output already readable at the master is not
removed by suspending terminal output. Continuing to read and discard bytes would
lose continuity. The relevant endpoint is the application PTY, not the shared SSH
carrier, whose management traffic must remain responsive. See the Linux
[PTY interface](https://man7.org/linux/man-pages/man7/pty.7.html).

### Natural extension of delivery-credit backpressure

In the inspected controller, application-PTY reads require positive delivery
credit and room in the message queue. The relay returns credit after writing
bytes into the local terminal endpoint. Extending that gate with attachment state
or detached-buffer capacity is a natural candidate: stop consuming output without
closing the PTY, while continuing to service management IPC, attachment requests,
termination and unrelated sessions. It does not require an application-aware
suspension signal or delegated cgroup.

This still requires the persistent owner and all attachment lifecycle changes in
section 3. Credit exhaustion alone does not preserve the current workspace after
transport loss. Queued bytes in the PTY, server, transport and local relay must be
accounted for and preserved across attachment changes. A delivery boundary must
identify which bytes reached a retained view and which remain pending; terminal
endpoint writes are not rendered-frame acknowledgements. Reattachment must deliver
the pending range in order before joining newly read output, without omissions,
duplicates or flushing queues. Unexpected loss and partial delivery remain design
problems, not guarantees supplied by the existing credit counters.

### Workload behavior and limits

- Processes continue executing until a terminal write blocks as available queue
  space runs out. Quiet computation, file output and network activity may continue
  indefinitely; this is not a workspace freeze or a CPU-saving guarantee.
- Stderr shares this backpressure when it refers to the same terminal. Redirected
  output follows its own destination. A blocked terminal consumer in a pipeline
  can eventually block upstream pipe writers, while independent jobs keep running.
- Nonblocking terminal writers may receive `EAGAIN` instead of waiting, and writes
  may be partial. Applications can retry, drop output or fail according to their
  own handling. See Linux [write semantics](https://man7.org/linux/man-pages/man2/write.2.html).
- A process blocked in terminal output may stop servicing peers, retaining locks
  and resources while external deadlines expire. The effect depends on where it
  writes, rather than a confirmed stop of every process.
- Terminal queries can remain queued or unanswered while detached. Input and reply
  ownership still need explicit rules; backpressure supplies no terminal replies.

Preserving pending bytes can protect continuity for retained Kitty views if every
missing byte survives. Fresh-view screen recovery remains unresolved: unread
output does not recreate the earlier screen, modes, parser state or graphics.
No flow-control backend, buffer capacity or detach/resume policy was selected,
and neither terminal flow control nor detached PTY-read gating was experimented
with for this analysis.

## 7. Comparison

Complexity depends on the promised guarantee. A known-process stop, a text ring
buffer and a PTY-read gate are simple demonstrations; none proves a reliable
general reattachment feature.

| Aspect | Suspend while detached | Keep running with limited history | Output backpressure |
| --- | --- | --- | --- |
| Persistent PTY owner | Required | Required | Required |
| Prototype complexity | Low for known process targets | Low for ordinary text output | Low for gating PTY reads |
| Robust implementation work | Containment, freeze completion, draining and resume semantics | Retention, replay isolation, delivery offsets and gap handling | Pending-data preservation, delivery boundaries, management responsiveness and ordered resume |
| Server setup | Strong freezing may require delegated cgroups | Buffering itself needs no special kernel setup | Read gating needs no special kernel setup |
| Detached progress | Workloads pause | Workloads can continue, subject to input/query/resource waits | Continues until terminal writes block; quiet work may continue |
| CPU/resources | Application CPU largely stops; memory and resources remain | Normal workload resource use plus bounded history storage | CPU may continue; resources and bounded pending output remain |
| Long disconnection | Little new output after confirmed freeze | Important history becomes more likely to be evicted | Writers may remain blocked; preserve pending bytes rather than evict them |
| External connections | Peers may time out while application execution is stopped | Applications can continue servicing peers | Can continue until blocked output prevents servicing peers |
| User expectation | Pause and resume a workspace | Closer to leaving work running in tmux | Run until output capacity is exhausted, then wait for a reader |
| No-emulator constraint | Compatible with process persistence | Compatible with byte retention; semantic replay policies require further design | Compatible with PTY backpressure; no screen recovery supplied |

| Reattachment case | Suspension | Limited history | Output backpressure |
| --- | --- | --- | --- |
| Retained Kitty views, intentional detach | Promising: retain display and drain a finite tail | Stronger recovery if every missing byte remains available | Promising if all pending bytes survive and resume in order |
| Retained views, unexpected loss | Still needs failure detection and buffered traffic | Can recover missing ranges within the retention window | Still needs failure detection, delivery boundaries and preservation of in-flight bytes |
| Fresh Kitty, recent shell/log output | Little display-recovery help by itself | Often useful context | Pending output may offer context, but no earlier history is implied |
| Fresh Kitty, incremental full-screen application | Old display is still missing | May reconstruct fragments only | Old display is still missing |
| Fresh Kitty, arbitrary modes and graphics | Needs an additional state mechanism | Dependencies may lie outside retained history | Needs an additional state mechanism |
| Daemon crash or host reboot | Not solved | Not solved | Not solved |

These are feasibility judgments, not measured implementation estimates. Retaining
views also requires investigating offline resize/font changes and the lifetimes
of the broker, relays, local PTYs and parser state. Keeping application PIDs alive
must never be described as equivalent to complete terminal restoration.

## 8. How the approaches can complement each other

1. **Suspend plus an in-flight buffer.** Even a pause design needs space for
   already queued output and data produced before failure detection. Suspension
   makes that backlog finite once it completes.
2. **Buffer first, then suspend before losing missing data.** Continue through
   short disconnections, then freeze as the undelivered range approaches capacity.
   Reserve headroom for freeze latency and in-flight data. This can protect a
   retained view's continuity; it does not turn a tail into a fresh-screen snapshot.
3. **Bounded buffering, then stop PTY reads.** Allow short disconnections to
   consume a bounded pending-output budget, then apply backpressure before any
   missing bytes are evicted. Preserve queued and in-flight bytes and reserve
   capacity for them. Resume delivery in order as a new attachment consumes the
   backlog. Quiet work can continue; this does not promise a paused workspace or
   fresh-screen recovery.
4. **Brief suspension during attachment.** Pause producers while establishing a
   replay/live boundary, then resume. This can simplify synchronization but does
   not repair missing history or remove replay side effects.
5. **Per-session policies.** A build could continue with bounded history while an
   editor pauses with its view retained. This adds configuration, containment and
   test combinations and was not selected as a requirement.
6. **Retained views plus missing-byte replay.** Preserve Kitty's existing state,
   send only the missing range, and optionally suspend or stop PTY reads before
   that range outgrows storage. This is a promising continuity design, not a
   fresh-client guarantee.

Three candidate targets remain open: pause-and-detach with retained views,
bounded history for ongoing work with explicitly approximate fresh-view recovery,
and output backpressure with preserved pending data for retained-view continuity.
No persistence strategy or hybrid policy was selected.

## 9. Fresh-client recovery alternatives

| Option | Assessment |
| --- | --- |
| Reattach with application redraw | Application-dependent; an explicit recovery protocol could provide a stronger contract than a redraw key; see [section 10](#10-restore-display-protocol) |
| Text/ANSI extraction plus selected metadata | Partial visual recovery, not a complete screen/parser/graphics checkpoint |
| Full output journal with geometry events | Plausible research path; history growth, replay effects, queries and external resources remain difficult |
| Compact terminal-state model | Could retain current cells and continuation state without a renderer or full output journal; still requires terminal emulation and an explicit compatibility boundary; see [section 9.1](#91-compact-terminal-state-for-screen-recovery) |
| Complete native Kitty checkpoint/import | Potentially strong; no supported complete mechanism was established in the inspected baseline |
| Persistent terminal-state backend elsewhere | A larger architecture/dependency change; not part of the initial no-extra-services direction |

Kitty's documented text extraction includes formatting and optional cursor data;
it is not documented as a full checkpoint. Saved Kitty sessions recreate launch
arrangements and restart commands, rather than preserving the same remote
processes. See [text extraction](https://sw.kovidgoyal.net/kitty/rc_protocol/#get-text)
and [saved sessions](https://sw.kovidgoyal.net/kitty/sessions/#the-save-as-session-action).
Graphics can refer to previously transmitted image data, so placement commands
alone are insufficient; see [image placement](https://sw.kovidgoyal.net/kitty/graphics-protocol/#display-images-on-screen).

### 9.1. Compact terminal state for screen recovery

The research question is whether a small state machine between each application
PTY and Kitty could condense output into enough state to reconstruct a fresh view.
**A compact screen model is plausible; a tiny tracker of changed regions alone
cannot guarantee arbitrary screen recovery.** This is a design inference from
terminal semantics, not a tested kittyplex implementation.

Terminal interpretation and graphical rendering are separate responsibilities.
A remote model would not need fonts, GPU drawing or a graphical window. It could
interpret output into cells and metadata, then generate a repaint for Kitty.
[libvterm](https://www.leonerd.org.uk/code/libvterm/) demonstrates terminal
emulation without a particular graphics toolkit or output system; this is evidence
that the separation is possible, not a dependency recommendation or proof of Kitty
compatibility.

#### What can be condensed?

| Representation | What it can recover | What it still needs |
| --- | --- | --- |
| Sequence framing and a small parser state | Complete UTF-8/control-sequence boundaries for replay | Earlier screen contents and the meaning of commands |
| Dirty rows or rectangles | Which regions changed relative to a known screen | The contents of those regions; a fresh view needs the unchanged regions too |
| Dependency-preserving command replay | State reconstructed by executing retained commands | All still-relevant dependencies; determining which commands can be discarded requires semantic interpretation |
| Cell grid plus terminal metadata | A candidate bounded representation of current text screens and their continuation state | Supported control semantics, serialization and agreement with Kitty |
| Complete compatible terminal checkpoint | Potentially the strongest continuation guarantee | All relevant parser, mode, screen and graphics state, and a compatible importer |

For example, an editor may draw a document once and subsequently change only its
status field. Remembering that the status row is dirty cannot reconstruct the
document. Keeping the last bytes written at each location also requires knowing
where they took effect: relative cursor movement, scrolling, insertion, erasure
and mode changes can move or alter earlier contents. A clear-screen command is
not necessarily a complete reset of terminal state. The
[xterm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
documents these operations, saved state and alternate-screen behavior.

Consequently, removing old output safely is more than recognizing escape-sequence
boundaries. A dependency-preserving journal may retain arbitrarily old commands;
there is no established general tiny-state compaction scheme here. Converting
those dependencies into current cells and metadata is already terminal emulation.

#### A small display model still has continuation state

Repainting the same visible characters is a weaker guarantee than making future
output and input behave as if the original terminal had survived. A candidate
text-terminal model would need to account for at least:

- Primary and alternate screen cells, character widths and combining text,
  attributes, colors, and line-wrap metadata.
- Current and saved cursors and attributes, scrolling margins, tab stops, active
  character sets, origin/insert/autowrap modes and pending wrap at the last column.
- Input-affecting state such as keyboard, mouse and bracketed-paste modes, plus
  any supported palette, hyperlink and other terminal extensions.
- Geometry and ordered resize events, and partial UTF-8/control-sequence parser
  state at the checkpoint boundary.

For fixed geometry and a bounded character/attribute representation, text grids
scale with screen area rather than the duration of output. Identical attributes
can be shared and a snapshot compressed. This can be substantially smaller than a
long journal, but it is not constant-sized state independent of screen contents.
Scrollback adds a separate retention budget. Arbitrary combining text, control
strings and extension payloads also need limits before claiming bounded memory.
Graphics require image data, placements and potentially animation state;
retaining only visible text or image identifiers cannot reconstruct them.

A deliberately smaller terminal subset could reduce implementation work. Its
supported semantics and recovery guarantee would have to be explicit. Unsupported
sequences cannot simply be ignored while claiming exact recovery: their effects
could invalidate the model. Detecting such uncertainty and defining a fallback
would be part of a later design. No subset or fallback is selected here.

#### Tracking and reattachment boundaries

The model would normally track output continuously from session creation, even
while attached. Starting only at detach loses the earlier screen unless a
sufficient initial checkpoint is available. Kitty's documented text/ANSI/cursor
extraction does not establish such a complete checkpoint. A persistent local
Kitty view could instead remain the state owner, but that retains the local
process dependency and does not solve recovery after losing it.

A proposed recovery sequence is to capture model state at an output position,
retain the subsequent ordered tail, restore a fresh view, then deliver that tail
before joining live output. Correctness requires an exact boundary and consistent
geometry. Partial sequences must be preserved or the checkpoint deferred to a
suitable parser boundary; ordinary repaint commands are not established as a way
to import arbitrary internal parser state, saved state or pending wrap. A resize
on reattachment introduces an additional reflow/application-redraw question.

Terminal replies also need one owner. If both Kitty and a shadow model answer the
same query, duplicate responses can reach the application. If neither answers
while detached, an application may wait. Historical queries and side effects must
not be replayed as new live requests. Answering queries remotely would add another
part of terminal emulation, beyond retaining a picture.

Finally, a shadow model can disagree with Kitty about Unicode widths, wrapping,
resizing, modes or extensions. Even a visually successful initial repaint can
diverge on the next character. Validation must compare subsequent behavior as
well as the restored screen. A remote model would reconsider the existing
[no-emulator ownership model](remote-session-architecture.md#1-goal-and-ownership-model);
it is an architectural alternative, not a small change to opaque-byte forwarding.
The promising investigation is bounded screen state without a renderer, with
fidelity measured explicitly. Neither a full Kitty clone nor a limited model has
been selected or prototyped.

## 10. Restore Display protocol

A cooperating application could reconstruct its current interface when a new
display is available. The experimental **Restore Display** protocol uses small
ASCII escape-code messages for versioned opt-in, detached notification,
restoration and acknowledged opt-out. Intentional and unexpected loss use the
same notification; v0 has no preparation handshake or display-status query. Participation promises to reestablish required terminal modes
and resources as well as redraw, including graphics used by the interface.
Unsupported applications and earlier shell output still need another mechanism;
fallback depends on state preserved beforehand.

The [research](terminal-recovery-protocol.md) records alternatives and adapter
ideas. The [v0 specification](terminal-recovery-protocol-spec.md) defines the
compact wire format, lifecycle and diagrams; the
[demo and exerciser requirements](terminal-recovery-demo-spec.md) define staged
implementation and evidence. These remain experimental documents, not an
implementation of production persistence. The [minimal image demo](restore-display-demo.md)
provides a raw-mode reference and isolated restoration tests; it does not establish
Bash, MC or Helix support. The persistent-owner requirements in section 3 still apply.

## 11. Further analysis before selecting an implementation

The order below is a proposed investigation order, not authorized implementation
work. No additional experiments were run when this analysis was archived.

| Priority | Question / experiment | Required evidence |
| --- | --- | --- |
| 1 | Define the intended guarantee | Retained versus fresh Kitty, pause versus continued execution versus output-limited progress, fidelity expectations, and supported hosts/workloads are explicit |
| 2 | Persistent server and deployment | Same PIDs and PTYs survive attachment exit/logout on target hosts; discovery, authorization and explicit termination work |
| 3 | Workspace containment and freezing | Test pipelines, background jobs, nested shells, debugger, concurrent forks and existing stopped jobs; incomplete freezes have a defined outcome |
| 4 | Delivery/resume boundaries | Detach during floods, partial UTF-8/control sequences and image transfer; demonstrate no duplication or omission where continuity is promised |
| 5 | Input and terminal replies | Interrupt attachment during input/query delivery; prevent unintended command repetition, lost pending replies and stale-client input |
| 6 | Buffer usefulness and limits | Test logs, partial shell commands, Vim, another full-screen application, incremental status fields, progress bars, colors and output faster than replay |
| 7 | Retained-view lifecycle | Rebind a new SSH carrier safely; exercise broker/relay failure, destroyed views, offline resize/font changes and gaps beyond retention |
| 8 | Fresh-view fidelity | Compare redraw, bounded replay and available Kitty export/import facilities against text, modes, graphics and geometry dependencies |
| 9 | Compact terminal-state feasibility ([section 9.1](#91-compact-terminal-state-for-screen-recovery)) | Compare restored screens and subsequent behavior against Kitty for incremental status updates, scrolling/insertion/erasure, primary/alternate screens, saved cursors, pending wrap, Unicode, fragmented sequences, resize, queries and graphics; measure retained state under long output and identify unsupported semantics and model divergence |
| 10 | Restore Display ([section 10](#10-restore-display-protocol)) | Validate negotiation, application ownership and handoff, redraw across input contexts, terminal initialization, output/reply boundaries and interrupted recovery without a kittyplex cell grid; define unsupported-client behavior before claiming compatibility |
| 11 | Output backpressure and ordered resumption | Propose output floods exceeding queue/buffer capacity, quiet CPU/file/network work, pipelines and stderr on the same PTY, nonblocking writers with partial writes/EAGAIN, and terminal queries; verify bounded queues, responsive management, workload progress/blocking, pending-reply handling and ordered delivery without loss or duplication on resume |
| 12 | Workload effects and hybrid policy | Test external timeouts, resource/lock retention, freeze and stop-reading thresholds and whether differing per-session policies are worthwhile |

Before a later implementation plan, resolve close-tab versus detach versus kill,
single-writer ownership and takeover, input acknowledgement behavior, buffer
overflow/gap policy, stop-reading/resume thresholds, freeze-failure deadlines,
capability/version negotiation, and how users discover and recover an unattended paused workspace. Do not invent
these policies by silently changing the current lifecycle.

Useful existing code entry points are the
[controller](../src/remote.rs), [PTY/process owner](../src/session.rs),
[broker](../src/broker.rs), [relay](../src/view.rs), and
[Kitty lifecycle integration](../kitty/kittyplex.py). The
[existing verification record](kitty-compatibility.md#validation-record-native-tab-release-2026-11-01-europestockholm)
describes current functionality only; it is not evidence that any proposed
detach behavior works.
