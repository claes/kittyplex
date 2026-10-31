# Kitty compatibility: termlab 0.3 / carrier v2

The remote application owns a PTY; the corresponding local Kitty window owns the
terminal parser, screen and graphics manager. Application bytes and terminal
replies pass through the original SSH connection. No terminal emulation or
filesystem/desktop bridge is implemented by termlab.

## Geometry fix and upgrade requirements

`kitten icat` first needs the PTY's pixel dimensions. Earlier termlab releases
copied rows and columns but set the two pixel fields to zero, producing the
reported “Terminal does not support reporting screen sizes in pixels” error.
Version 0.3 copies all four unsigned 16-bit `winsize` fields from each local view
into its remote PTY before starting the application. Every resize forwards the
complete size, even if only pixels changed. Zero remains unknown, rather than a
fabricated cell size. No `icat` override is needed when Kitty reports valid sizes.

Carrier v2 requires `pixel_width` and `pixel_height` on every envelope. Install
matching local and remote 0.3 binaries and replace **all four** integration files
(`termlab.conf`, `termlab.py`, `termlab_auth.py`, `termlab_watcher.py`). End existing
groups and start a **fresh Kitty process**. Config reload alone can retain cached
Python authorization code. Mixed versions are rejected; v1 is not negotiated.
Kitty's native RC version `[0,47,4]` is independent of carrier v2.

The initial session title is now a child title, rather than a fixed override,
so applications can update it using OSC 2. Each session has a separate native
Kitty tab whose title follows the window title; no fixed tab title overrides it.

## Automated evidence and remaining checks

`tests/compatibility.py` runs real Kitty and localhost SSH on a private Xvfb
display. Where applicable it compares an ordinary SSH window to a managed view.
A private encrypted test-controller capability reads native Kitty state; this
capability is absent from product configuration. These checks observe parsed
screen state, graphics storage and placements, not physical monitor output.

| Feature | Automated evidence | Remaining interactive check / qualification |
| --- | --- | --- |
| Native tabs | Separate tab per session; native and termlab navigation, creation-order numbering, title updates, tab closure and group cleanup | Tab-bar appearance and mouse clicks follow your Kitty settings |
| PTY geometry | All four fields match the corresponding local PTY; visible/hidden views resize; font and pixel-only changes propagate | Repeat on the real remote machine and desktop |
| Default `kitten icat image.png` | Exact command, stdout attached to terminal, image stored in native graphics manager; ordinary SSH baseline | Localhost shares filesystems; genuinely separate-host auto-detection is a separate check below |
| Streamed graphics | PNG larger than the 256 KiB credit window; native image dimensions checked | Large images remain subject to carrier bandwidth/latency |
| Session graphics isolation | Same image ID stores different images in separate windows, survives switching; clear removes first window's placements and preserves second | Visually inspect image placement after scrolling and switching |
| Graphics replies | Positive inline query and negative nonexistent-file query return to the originating PTY even while another view is selected | Separate-host file/shared-memory rejection and automatic stream fallback |
| Alternate screen | Transfer in alternate screen and native exit from alternate-screen mode | Visually inspect application-specific teardown |
| Animation | Two-frame GIF accepted and extra frame stored by native graphics manager | Timing, smoothness and final appearance require visual inspection |
| Unicode placeholders | Explicit streamed placeholder mode; image and placeholder character reach native Kitty | Visually inspect placeholder positioning, wrapping and scrolling |
| Terminal queries | Cursor position, device attributes, text-area/cell pixels and color replies over direct SSH and termlab | Not an exhaustive query-protocol conformance suite |
| Keyboard | Kitty keyboard protocol Ctrl-A and application-cursor Up arrive in raw remote input; prefix mappings tested separately | Physical Ctrl-Shift-M and literal-prefix action on your desktop |
| Paste | Native bracketed-paste delimiters arrive at remote reader in both paths | OS clipboard paste with your application |
| Focus and mouse | Focus-in/out bytes return to managed PTY; native mouse modes enabled/reset | Actual mouse coordinates, buttons, drag, wheel and selection |
| Colors | True-color SGR survives native ANSI screen extraction | Visual color profile and font rendering |
| Title / metadata | Native title, OSC 7 cwd, OSC 133 prompt marker and application user variable observed | Automatic shell integration requires remote shell setup; reserved `termlab_*` variables belong to termlab |
| OSC 8 hyperlinks | Native ANSI extraction retains OSC 8 URL and labeled text | Clicking and external URL/file handling require desktop checks |
| OSC 52 clipboard | Writes and explicitly permitted reads work in direct SSH and termlab; denied reads do not expose the private token | User Kitty clipboard policy remains authoritative; no permission is granted by termlab |
| OSC 99 notifications | Capability query receives native Kitty reply | Actual desktop delivery/buttons require a notification service and manual check |
| Job control, recovery, parser boundaries | Existing integration suite covers signals, alternate screen/Vim, fragmented UTF-8/control sequences, hidden output, independent groups and carrier/broker failure | Persistent reattachment is not implemented |

Features absent from this table are **not tested**, rather than implicitly
supported or unsupported. There is no claim of universal Kitty feature parity.

## Validation record: 2026-11-01 (Europe/Stockholm)

Linux x86_64: 16 Rust tests, 14 Kitty helper/configuration/authorization tests,
13 SSH/startup regression tests (including two native-tab cases) and all
9 feature-audit tests passed on Kitty 0.47.4.
Formatting and Clippy passed. The feature audit passed on pinned Kitty 0.47.4;
the same cases passed on Kitty 0.49.1 across the audit and focused reruns.
Three geometry/graphics cases also passed with 100 ms added transport RTT.
Nix package and Python checks passed; the current packaged integration
and demo scripts match the tested source, and the packaged version command runs.
The preceding graphics audit also checked its packaged binary’s feature-demo
and metadata cases. Historical 100-sample benchmark records are linked in
the [carrier reference](original-terminal-carrier.md#03-performance-follow-up);
the localhost hidden-flood latency target remains unmet.
Real desktop delivery, a truly separate
remote filesystem and aarch64 execution were not tested.

The native-tab follow-up reran the complete 52-test suite on Kitty 0.47.4.
Both new tab cases plus default/streamed graphics, keyboard/paste/focus and pixel
resize cases passed on Kitty 0.49.1. Both tab cases and streamed graphics/pixel
resize cases also passed with 100 ms added SSH RTT on Kitty 0.47.4. Nix checks
passed; no carrier protocol or Rust CLI change was required. Physical tab-bar
mouse clicks and desktop rendering remain interactive checks.

The audit also corrected test startup synchronization (atomic publication of the
controller key, native parser readiness before input injection, and no-reset
Xvfb displays). Some isolated X11 launches failed before bootstrap and required retry. Those
infrastructure failures are not counted as passes.

## Host and policy boundaries

Graphics file and shared-memory transfers refer to resources accessible to the
**local Kitty process**. A path existing only on the server is not made local by
termlab. Use `kitten icat --transfer-mode=stream image.png` for a deterministic
remote transport. Default `icat` can probe transfer methods and fall back, but
localhost success alone cannot establish that behavior for a separate server.
Applications that insist on local file/shared-memory access need their own stream
option; termlab does not provide a filesystem bridge.

Kitty sees a local relay process as the window's direct child. Forwarding OSC 7
and shell markers does not teach Kitty that child is an SSH process or install
remote shell integration. Host-aware actions (remote file links, cwd inheritance,
remote edit/open workflows, SSH kitten integration and desktop launch actions)
need separate setup and testing. They are not established as equivalent to a
native `kitten ssh` window. Interactive UI kittens are likewise workflow-specific.

Clipboard access, notification delivery, hyperlink opening and shell integration
retain their normal Kitty/desktop requirements. Some resources, such as the OS
clipboard, are naturally shared by Kitty windows. Termlab does not virtualize
those resources or relax permissions. Generic remote control remains restricted
by the existing scoped authorization callback.

## Reproduce and inspect interactively

```sh
nix develop --command ./scripts/test.sh
nix develop --command env TERMLAB_TEST_RTT_MS=100 python3 tests/compatibility.py
./scripts/demo.sh --features
```

In the demo's managed session run `termlab-features`. It prints all four PTY
size fields, shows a generated PNG using the ordinary `icat` command and provides
commands for a large streamed PNG, animated GIF, clear, and `kitten show-key`.
Create another session in its own tab, display a different image, switch back
using both native Kitty tab navigation and termlab shortcuts, resize and change
font size. Verify image retention, animation and independent clearing. The assets
and SSH configuration are temporary; only standard-library Python is added to the
existing demo tools. `nix run path:.#demo -- --features` is equivalent.

On an actually separate SSH server, create an image there and run both:

```sh
kitten icat server-only.png
kitten icat --transfer-mode=stream server-only.png
```

Confirm both show an image and that no path or shared-memory mapping is required
on the workstation. Run `kitten show-key -m kitty`, exercise paste, focus and mouse
in a suitable TUI, click an OSC 8 URL, and test desktop notifications and clipboard
only with your desired native permissions. Record Kitty version, server platform,
transfer mode and observations. Do not treat Xvfb checks as desktop-delivery tests.

## References

The [graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) describes
pixel geometry, inline/file/shared-memory transfers, replies and placements;
[`icat`](https://sw.kovidgoyal.net/kitty/kittens/icat/) documents transfer options.
Native test inspection uses Kitty's
[graphics manager implementation](https://github.com/kovidgoyal/kitty/blob/master/kitty/graphics.c).
See also [keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/),
[notifications](https://sw.kovidgoyal.net/kitty/desktop-notifications/) and
[shell integration](https://sw.kovidgoyal.net/kitty/shell-integration/).
