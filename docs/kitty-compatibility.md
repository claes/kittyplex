# Kitty compatibility: carrier v2

This document owns feature coverage, validation evidence and host/policy limits.
For the process and communication model, read [How termlab works](how-it-works.md);
for exact wire rules, use the [carrier reference](original-terminal-carrier.md).

## Geometry fix and upgrade requirements

`kitten icat` first needs the PTY's pixel dimensions. Earlier termlab releases
copied rows and columns but set the two pixel fields to zero, producing the
reported “Terminal does not support reporting screen sizes in pixels” error.
Version 0.3 preserves pixel dimensions; the geometry cases below check this
against each view's actual PTY. No `icat` override is needed when Kitty reports
valid sizes. Exact fields, unknown-size semantics and resize handling belong to
the [geometry contract](original-terminal-carrier.md#protocol-v2-termlab-03).
Use the [README upgrade procedure](../README.md#upgrading-an-existing-installation)
when replacing an older installation.

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

## Validation record: native-tab release, 2026-11-01 (Europe/Stockholm)

The complete native-tab suite passed on Linux x86_64 with Kitty 0.47.4:

| Checks | Passed |
| --- | ---: |
| Rust codec/PTY tests | 16 |
| Kitty helper, configuration and authorization tests | 14 |
| SSH/startup regression tests, including two native-tab cases | 13 |
| Feature-audit tests | 9 |
| Total | 52 |

Formatting, Clippy, Nix package and Python checks passed. The packaged
integration and demo scripts matched the tested source, and the packaged version
command ran. The preceding graphics audit also checked its packaged binary's
feature-demo and metadata cases.

The original graphics audit exercised the feature cases on Kitty 0.49.1 across
the audit and focused reruns, and three geometry/graphics cases with 100 ms added
SSH RTT. After native tabs were introduced, both new tab cases and the default/
streamed graphics, keyboard/paste/focus and pixel-resize cases passed on Kitty
0.49.1. Both tab cases and streamed graphics/pixel-resize cases also passed with
100 ms added RTT on Kitty 0.47.4. These focused runs do not imply a complete rerun
of every case on each version/transport combination.

Real desktop delivery, physical tab-bar mouse clicks, a truly separate remote
filesystem and aarch64 execution were not tested. The
[historical benchmark records](original-terminal-carrier.md#03-performance-follow-up)
precede native tabs; the localhost hidden-flood latency target remains unmet.

The audit corrected startup synchronization (atomic publication of the controller
key, native parser readiness before input injection, and no-reset Xvfb displays).
Some isolated X11 launches failed before bootstrap and required retry. Those
infrastructure failures are not counted as passes.

Desktop launchers added in 0.4 have their own installation and validation commands
in the [integration instructions](integration-instructions.md). They preserve the
carrier v2 geometry and byte-routing contracts. The native-tab release counts
above are a historical validation record, not counts for the later desktop tests.

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
remote edit/open workflows and SSH kitten integration)
need separate setup and testing. They are not established as equivalent to a
native `kitten ssh` window. Interactive UI kittens are likewise workflow-specific.

Clipboard access, notification delivery, hyperlink opening and shell integration
retain their normal Kitty/desktop requirements. Some resources, such as the OS
clipboard, are naturally shared by Kitty windows. Termlab does not virtualize
those resources or relax permissions. Generic remote control remains restricted
by the existing scoped authorization callback.

## Reproduce and inspect interactively

Use the [README verification commands](../README.md#automated-verification) for
the complete suite and its delayed-transport mode, or run just this audit:

```sh
nix develop --command env TERMLAB_TEST_RTT_MS=100 python3 tests/compatibility.py
```

The [guided graphics demo](../README.md#kitty-features-and-graphics) supplies
temporary images and the basic switching/resize walkthrough. The following
additional checks establish behavior that the localhost fixture cannot prove.

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
