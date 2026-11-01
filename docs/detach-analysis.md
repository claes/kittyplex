# Detach and reattachment analysis

Status: **deferred research, not an implementation commitment**. Recorded
2026-11-01. No persistence approach, CLI, protocol extension, buffer size or
suspension backend was selected. The user chose to set this work aside and retain
the analysis for a later investigation.

The implemented behavior remains unchanged: transport/controller loss closes the
group, and detach/reattach is not supported. See the
[current lifecycle contract](original-terminal-carrier.md#lifecycle-and-failures).
This document owns the deferred options and their tradeoffs; the
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
problem, outside ordinary live-daemon detach/reattach. Neither suspension nor a
recent-output buffer provides process checkpoint/restore.

## 2. Evidence and confidence

Distinguish established primitives, inspected implementation facts, an isolated
experiment, and proposed behavior. No complete persistence prototype was built.

| Evidence | What it establishes | What it does not establish |
| --- | --- | --- |
| Current source inspection | The remote controller owns application PTY masters; dropping sessions performs hangup/reaping cleanup; carrier/broker loss currently closes local views | That existing lifecycle code can already detach safely |
| Current relay credit handling | Credits follow writes into the local terminal endpoint | A Kitty parser checkpoint or rendered-frame acknowledgement |
| Small PTY suspension experiment during the discussion | A known child was confirmed stopped, generated no new output during the observation after draining, retained its PTY geometry, and resumed output with the same PID | Whole-workspace freezing, nonempty-backlog recovery, SSH disconnect handling, or display recovery |
| Linux signal and freezer documentation | Mechanisms and their limitations, discussed below | Their availability or correctness for a particular deployment/workload |
| Kitty API/source inspection against the implementation baseline | Text/format/cursor extraction and window launch metadata exist; no complete supported screen/parser/graphics checkpoint was established | That no future or private Kitty implementation could provide one |

The small PTY trial had no pending bytes in its post-stop drain. It must not be
treated as a validation of a real transport-drain protocol. Cgroup freezing was
not exercised; a usable writable delegated hierarchy was not available in the
analysis environment. Host-specific suspension and logout behavior remain to be
validated. The implementation baseline uses Kitty 0.47.4, with additional checks
on 0.49.1; upstream documentation links can evolve.

## 3. Shared foundation: persistent owner, temporary attachment

Both main approaches require separating responsibilities currently combined in
the remote controller:

```text
Persistent remote termlab server
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
port forwarding, or a terminal emulator in termlab.

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

## 6. Comparison

Complexity depends on the promised guarantee. A single-process stop and a plain
text ring buffer are both easy demonstrations; neither proves a reliable general
reattachment feature.

| Aspect | Suspend while detached | Keep running with limited history |
| --- | --- | --- |
| Persistent PTY owner | Required | Required |
| Prototype complexity | Low for known process targets | Low for ordinary text output |
| Robust implementation work | Containment, freeze completion, draining and resume semantics | Retention, replay isolation, delivery offsets and gap handling |
| Server setup | Strong freezing may require delegated cgroups | Buffering itself needs no special kernel setup |
| Detached progress | Workloads pause | Workloads can continue, subject to input/query/resource waits |
| CPU/resources | Application CPU largely stops; memory and resources remain | Normal workload resource use plus bounded history storage |
| Long disconnection | Little new output after confirmed freeze | Important history becomes more likely to be evicted |
| External connections | Peers may time out while application execution is stopped | Applications can continue servicing peers |
| User expectation | Pause and resume a workspace | Closer to leaving work running in tmux |
| No-emulator constraint | Compatible with process persistence | Compatible with byte retention; semantic replay policies require further design |

| Reattachment case | Suspension | Limited history |
| --- | --- | --- |
| Retained Kitty views, intentional detach | Promising: retain display and drain a finite tail | Stronger recovery if every missing byte remains available |
| Retained views, unexpected loss | Still needs failure detection and buffered traffic | Can recover missing ranges within the retention window |
| Fresh Kitty, recent shell/log output | Little display-recovery help by itself | Often useful context |
| Fresh Kitty, incremental full-screen application | Old display is still missing | May reconstruct fragments only |
| Fresh Kitty, arbitrary modes and graphics | Needs an additional state mechanism | Dependencies may lie outside retained history |
| Daemon crash or host reboot | Not solved | Not solved |

These are feasibility judgments, not measured implementation estimates. Retaining
views also requires investigating offline resize/font changes and the lifetimes
of the broker, relays, local PTYs and parser state. Keeping application PIDs alive
must never be described as equivalent to complete terminal restoration.

## 7. How the approaches can complement each other

1. **Suspend plus an in-flight buffer.** Even a pause design needs space for
   already queued output and data produced before failure detection. Suspension
   makes that backlog finite once it completes.
2. **Buffer first, then suspend before losing missing data.** Continue through
   short disconnections, then freeze as the undelivered range approaches capacity.
   Reserve headroom for freeze latency and in-flight data. This can protect a
   retained view's continuity; it does not turn a tail into a fresh-screen snapshot.
3. **Brief suspension during attachment.** Pause producers while establishing a
   replay/live boundary, then resume. This can simplify synchronization but does
   not repair missing history or remove replay side effects.
4. **Per-session policies.** A build could continue with bounded history while an
   editor pauses with its view retained. This adds configuration, containment and
   test combinations and was not selected as a requirement.
5. **Retained views plus missing-byte replay.** Preserve Kitty's existing state,
   send only the missing range, and optionally pause before that range outgrows
   storage. This is a promising continuity design, not a fresh-client guarantee.

The discussion identified two useful initial targets without choosing one:
pause-and-detach with retained views for stronger workspace continuity, or
bounded history for ongoing work with explicitly approximate fresh-view recovery.

## 8. Fresh-client recovery alternatives

| Option | Assessment |
| --- | --- |
| Reattach with application redraw | Relatively accessible, but application-dependent; not arbitrary restoration |
| Text/ANSI extraction plus selected metadata | Partial visual recovery, not a complete screen/parser/graphics checkpoint |
| Full output journal with geometry events | Plausible research path; history growth, replay effects, queries and external resources remain difficult |
| Complete native Kitty checkpoint/import | Potentially strong; no supported complete mechanism was established in the inspected baseline |
| Persistent terminal-state backend elsewhere | A larger architecture/dependency change; not part of the initial no-extra-services direction |

Kitty's documented text extraction includes formatting and optional cursor data;
it is not documented as a full checkpoint. Saved Kitty sessions recreate launch
arrangements and restart commands, rather than preserving the same remote
processes. See [text extraction](https://sw.kovidgoyal.net/kitty/rc_protocol/#get-text)
and [saved sessions](https://sw.kovidgoyal.net/kitty/sessions/#the-save_as_session-action).
Graphics can refer to previously transmitted image data, so placement commands
alone are insufficient; see [image placement](https://sw.kovidgoyal.net/kitty/graphics-protocol/#display-images-on-screen).

## 9. Further analysis before selecting an implementation

The order below is a proposed investigation order, not authorized implementation
work. No additional experiments were run when this analysis was archived.

| Priority | Question / experiment | Required evidence |
| --- | --- | --- |
| 1 | Define the intended guarantee | Retained versus fresh Kitty, pause versus continued execution, fidelity expectations, and supported hosts/workloads are explicit |
| 2 | Persistent server and deployment | Same PIDs and PTYs survive attachment exit/logout on target hosts; discovery, authorization and explicit termination work |
| 3 | Workspace containment and freezing | Test pipelines, background jobs, nested shells, debugger, concurrent forks and existing stopped jobs; incomplete freezes have a defined outcome |
| 4 | Delivery/resume boundaries | Detach during floods, partial UTF-8/control sequences and image transfer; demonstrate no duplication or omission where continuity is promised |
| 5 | Input and terminal replies | Interrupt attachment during input/query delivery; prevent unintended command repetition, lost pending replies and stale-client input |
| 6 | Buffer usefulness and limits | Test logs, partial shell commands, Vim, another full-screen application, incremental status fields, progress bars, colors and output faster than replay |
| 7 | Retained-view lifecycle | Rebind a new SSH carrier safely; exercise broker/relay failure, destroyed views, offline resize/font changes and gaps beyond retention |
| 8 | Fresh-view fidelity | Compare redraw, bounded replay and available Kitty export/import facilities against text, modes, graphics and geometry dependencies |
| 9 | Workload effects and hybrid policy | Test external timeouts, resource/lock retention, freeze thresholds and whether differing per-session policies are worthwhile |

Before a later implementation plan, resolve close-tab versus detach versus kill,
single-writer ownership and takeover, input acknowledgement behavior, buffer
overflow/gap policy, freeze-failure deadlines, capability/version negotiation,
and how users discover and recover an unattended paused workspace. Do not invent
these policies by silently changing the current lifecycle.

Useful existing code entry points are the
[controller](../src/remote.rs), [PTY/process owner](../src/session.rs),
[broker](../src/broker.rs), [relay](../src/view.rs), and
[Kitty lifecycle integration](../kitty/termlab.py). The
[existing verification record](kitty-compatibility.md#validation-record-2026-11-01-europestockholm)
describes current functionality only; it is not evidence that any proposed
detach behavior works.
