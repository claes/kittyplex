# kittyplex

Terminal multiplexing with **Kitty-owned screens**. Kittyplex manages shells and
application PTYs; Kitty keeps their independent screens, scrollback and terminal
modes. Switching tabs preserves both without replaying output or emulating a
terminal inside kittyplex.

Works locally, through run0 as another user, and over one ordinary SSH connection.
This is experimental. **Detach/reattach and persistence after connection loss are
not supported.**

## Try it

From a graphical Linux desktop with Nix and flakes enabled:

```sh
./scripts/demo.sh
```

The isolated demo opens a separate Kitty process and temporary localhost SSH
server. Follow the [interactive walkthrough](docs/usage.md#interactive-walkthrough)
to try session creation, switching, Vim, job control and cleanup. The
[graphics demo](docs/usage.md#kitty-features-and-graphics) exercises images and
other Kitty features.

## Install

Choose how to configure your desktop Kitty:

| Setup | Instructions and example |
| --- | --- |
| Kittyplex's NixOS/Home Manager modules | [Module setup](docs/integration-instructions.md#install-with-nixos-and-home-manager) |
| Standard NixOS/Home Manager options, without kittyplex modules | [Explicit Nix setup](docs/integration-instructions.md#explicit-nix-configuration-without-kittyplex-modules) |
| Your existing `kitty.conf`, with or without Nix | [Manual Kitty setup](docs/integration-instructions.md#manual-kitty-configuration) |

Install a compatible binary and terminfo on SSH targets too. For source builds,
see [building from source](docs/integration-instructions.md#building-from-source).
Start a fresh Kitty process after installation, then run:

```sh
kittyplex doctor
kittyplex start
# Or:
kittyplex run0 -u bob --pty
kittyplex ssh user@server
```

## Use

Press **Ctrl-Shift-M**, release it, then `c` to create a session, `n`/`p` to
switch, or `q` to close the group. Kitty's native tabs work too. Run0 uses the
target account's home and login shell; it defaults to root if `-u` is omitted.

The [usage guide](docs/usage.md) owns the full shortcut table, lifecycle rules and
demo walkthrough. `kittyplex COMMAND --help` gives a short command summary;
`man kittyplex` is the complete command reference. From a checkout, use
`man -l docs/kittyplex.1`.

## Documentation

| Question | Reference |
| --- | --- |
| How do I install, configure or troubleshoot it? | [Integration instructions](docs/integration-instructions.md) |
| How do I start and manage sessions? | [Usage guide](docs/usage.md) and [kittyplex(1)](docs/kittyplex.1) |
| Which processes and PTYs communicate, and how? | [How kittyplex works](docs/how-it-works.md) |
| Which Kitty features have been tested? | [Compatibility and validation](docs/kitty-compatibility.md) |
| What are the wire protocol and failure contracts? | [Carrier reference](docs/original-terminal-carrier.md) |
| How do I build, test or benchmark changes? | [Development guide](docs/development.md) |

The [architecture analysis](docs/remote-session-architecture.md) is historical
design research. The [detach analysis](docs/detach-analysis.md) records deferred
options; neither document promises additional implemented features.
