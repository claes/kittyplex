# Desktop integration

Kittyplex 0.4 adds launchers for local shells, SSH and switching user with run0.
Kitty owns the screens; kittyplex owns the application PTYs. These launchers automate
starting the existing original-terminal carrier design. They do not add another
network service, SSH connection or terminal emulator.

This document owns installation and launch configuration. The
[process guide](how-it-works.md) explains the transport and process roles;
[compatibility](kitty-compatibility.md) covers terminal features and their limits.
Detach and reattach remain unimplemented; see [detach analysis](detach-analysis.md).

## Install with NixOS and Home Manager

Use [examples/flake-with-modules.nix](../examples/flake-with-modules.nix) as a
complete consumer flake example. Copy its inputs and modules into your existing
`flake.nix`; retain your host's `configuration.nix`, hardware imports, users and
existing state versions. Replace `myhost`, `alice` and the system architecture.
The repository URL is `github:claes/kittyplex`. Until these changes are published,
use `path:/absolute/path/to/kittyplex` instead. Git-backed local flake URLs include only tracked files; add new project files
to Git or use `path:` to include untracked files.

The essential imports are:

```nix
inputs.kittyplex = {
  url = "github:claes/kittyplex";
  inputs.nixpkgs.follows = "nixpkgs";
};
# In nixosSystem.modules:
# kittyplex.nixosModules.default
# home-manager.nixosModules.home-manager
# In your host module:
programs.kittyplex.enable = true;
programs.kittyplex.run0.enable = true;
home-manager.sharedModules = [ kittyplex.homeManagerModules.default ];
home-manager.users.alice.programs.kittyplex.enable = true;
```

The complete example explicitly selects the same package for both modules. The
NixOS module installs kittyplex and Kitty terminfo for all users, and links the
integration under `/run/current-system/sw/share/kittyplex/`. Enabling `run0` enables
Polkit with normal authentication policy and asserts systemd >=258. NixOS supplies
the systemd-run0 PAM service. The module does not grant passwordless privilege
changes. A desktop Polkit agent or run0's terminal authentication must be available.

The Home Manager module installs Kitty, four Python helpers and `kittyplex.conf`,
and adds its include to Kitty's configuration. It preserves other Kitty settings.
Its options are `programs.kittyplex.enable`, `.package`, `.kitty.enable` and
`.kitty.prefix`. Set `.kitty.enable = false` to install only the package and manage
Kitty yourself. Existing files at those managed paths must first be backed up or
moved into your declarative configuration. Avoid duplicate includes. The module
asserts password-mode remote control; remove an existing unrestricted setting.

Apply your normal configuration:

```sh
sudo nixos-rebuild switch --flake .#myhost
```

Start a **fresh Kitty process**, then run `kittyplex doctor`. Reloading configuration
alone can keep cached Python authorization code. No daemon, user service or global
Kitty listening socket needs to be started.

For an SSH-only NixOS target, import just the NixOS module, enable kittyplex, and
leave `.run0.enable = false`. No Kitty GUI or Home Manager configuration is needed
on that target. Other Linux targets can install a matching binary themselves and
must have `xterm-kitty` terminfo available. Run0 executes the caller's kittyplex executable by its absolute path; the Nix store
provides a path readable by the target user, without relying on that user's PATH.
A source build under a private home directory may be inaccessible to another user.
Target installation is a prerequisite for SSH;
launchers do not upload binaries or bootstrap Nix.

## Incorporate into an existing Kitty configuration, without module options

[examples/flake-manual.nix](../examples/flake-manual.nix) is a complete consumer
example with no `programs.kittyplex` options or kittyplex module imports. It installs
the package, Kitty, terminfo and run0's Polkit prerequisite. Merge it into your
host flake, rebuild, then install the integration links as your desktop user:

```sh
kittyplex_kitty_dir="${KITTY_CONFIG_DIRECTORY:-${XDG_CONFIG_HOME:-$HOME/.config}/kitty}"
mkdir -p "$kittyplex_kitty_dir"
for file in kittyplex.py kittyplex_auth.py kittyplex_watcher.py kittyplex_launch.py kittyplex.conf; do
  ln -s "/run/current-system/sw/share/kittyplex/kitty/$file" "$kittyplex_kitty_dir/$file"
done
```

Back up existing files at those names first; these commands intentionally fail
rather than overwrite them. If Kitty uses an explicit `--config` or custom
configuration directory, install the helpers in its actual config directory.
The bare filename `kittyplex.py` must resolve there: an `action_alias` cannot replace
that requirement for terminal-originated requests.

Add this line to your **existing** `kitty.conf`:

```conf
include kittyplex.conf
```

The include contains the scoped Ctrl-Shift-M management mappings, authorization
callback and optional action aliases. Replace conflicting `allow_remote_control`
and empty-password authorization entries; use `allow_remote_control password`,
never `yes` for this integration. Other empty-password rules granting broad
permissions would also bypass the intended restriction. Existing authenticated
remote-control rules are your separate policy choice.

No additional global shortcuts are installed. You can optionally add bindings to
unused chords in your own configuration, for example:

```conf
map ctrl+alt+f8 kittyplex-run0 -u bob --pty
map ctrl+alt+f9 kittyplex-ssh user@server
map ctrl+alt+f10 kittyplex-local
```

Choose chords that fit your desktop and applications. The aliases are thin
custom kittens running inside Kitty, available only as trusted local actions.
They open a separate carrier tab even when invoked from an existing kittyplex view.
They neither wrap nor modify Kitty's bundled `kitten ssh`. You can also bind
`kitten kittyplex_launch.py run0 -u bob --pty` directly.

For a custom management prefix, copy `kittyplex.conf` instead of symlinking it and
change **both** occurrences of `ctrl+shift+m` (entry and literal-prefix forwarding).
Review the copied file when upgrading. Packaged helpers pin their local executable
to the matching Nix store path, so Kitty need not inherit a particular shell PATH.
`KITTYPLEX_BINARY`, set before Kitty starts, remains an explicit override for source
development. End old groups, update targets and helpers together, and start fresh
Kitty after upgrading. Carrier protocol stays v2; automatic launch requires 0.4's
new target `start` command.

## Launch and use

From an ordinary shell in the configured Kitty:

```sh
kittyplex doctor
kittyplex start                         # Local group, current directory.
kittyplex start --login                 # Account login environment and home.
kittyplex run0 -u bob                   # Target home and login shell; --pty-late.
kittyplex run0 --user=bob --pty          # Explicit early PTY allocation.
kittyplex run0 -u bob -g work -D /srv/work --pty
kittyplex run0 --user=bob --cwd='/srv/a directory' --pty-late
kittyplex run0                          # Root, with normal run0 authentication.
kittyplex ssh user@server
kittyplex ssh --cwd=/srv/work server -- -p 2222 -J jump-host
kittyplex ssh --remote-binary=/opt/kittyplex/bin/kittyplex server
```

`run0` supports `-u`/`--user`, `-g`/`--group`, `-D`/`--chdir`/`--cwd`, including
`--option=value` and attached short values such as `-ubob`. `--pty` and
`--pty-late` are mutually exclusive; the default is `--pty-late`. Both require
systemd >=258 in this launcher. Late allocation leaves authentication on the
original terminal until authorization succeeds. `--no-ask-password` is available
for already-authorized environments; it does not grant permission. This is a
purpose-built launcher, not a complete pass-through of every run0 option.

SSH uses your normal OpenSSH configuration, keys, host verification and prompts.
Put connection options after `--`, after the destination. The launcher forces a
foreground PTY and supplies the remote command; options such as `-N`, `-f`, `-T`,
`-n`, `-O`, `-W` and conflicting `-o` settings are rejected. Remote paths are
shell-quoted; `~` and `~/...` for `--cwd` are resolved by the target bootstrap,
not the desktop shell. Relative explicit directories are relative to target home
when launching with a login environment. `--remote-binary` names a single target
executable, not a shell command. SSH servers with forced commands may disallow it.

The bootstrap obtains the executing account's shell and home from the target's
account database, runs its shell as a login shell to load environment settings,
then starts the controller. Applications get that shell interactively, with job
control. Login profiles must allow noninteractive commands; a profile that exits,
executes another program or prints unbounded output can break startup. A session
started for bob gets bob's home and shell, rather than inheriting the caller's
`$SHELL`. Explicit `--cwd` is applied after login initialization. New sessions
inherit the controller's initial directory, not the last foreground shell's cwd.

Use Ctrl-Shift-M then `c`, `n`, `p`, a number or `q` as described in the
[README controls](../README.md#controls-and-lifecycle). Automatically started groups
label tabs `user@host · application title`; application title changes preserve the
identity prefix. Identity labels are useful context, not an authentication proof:
terminal applications can emit title sequences and target code is trusted.

Quitting a CLI launch returns to the source shell. Kitty actions close their
carrier on success and return focus to the source window if it still exists.
Launch failures remain in their carrier until Enter is pressed, with diagnostics
visible. Closing a carrier ends that group. Direct CLI nesting inside an existing
managed view is rejected; use the Kitty action to create an independent group.
There is no detach or reconnect after connection loss.

## Processes and privilege boundaries

```text
Desktop UID (alice)                       Target UID (bob)
Kitty + trusted action                   systemd service started by run0
  carrier: kittyplex __launch-tab             target login shell
             -> kittyplex run0                  -> kittyplex controller
                  -> run0 <--- PTY bytes --->      -> PTY -> bob's shell/apps
  kittyplex __broker
  kittyplex __view -> Kitty session screen
```

Run0 asks systemd/Polkit to authorize a new service under the requested user.
The target controller and its application PTYs belong to that user. Kitty, the
broker and display relays keep the desktop UID. For SSH, replace run0 and its
service with the SSH client, encrypted connection and server-side SSH processes;
the controller runs under the SSH login account. For local launch, both sides
use the desktop UID.

The original terminal carries bounded bootstrap requests and all session traffic.
The probe verifies the scoped Kitty callback before changing user or connecting;
it creates no group or tabs. Its nonce binds the response to the request. The
private Kitty control FD stays local to the broker; source session sockets,
D-Bus addresses and arbitrary environment variables are not passed as run0
arguments. Normal systemd service environment and target login configuration
apply. OpenSSH's own environment-forwarding settings remain under your control.
For detailed per-byte routing, use the [process guide](how-it-works.md).

A dedicated kitten is useful for independent tabs and readable error handling;
the Rust CLI owns argument parsing, preflight, transport execution and target
bootstrap so behavior also works from an ordinary configured Kitty shell. A shell
alias alone would miss those independent-tab and privilege-boundary requirements.
No shell startup hooks, SSH aliases or custom Kitty build are required.

## Troubleshooting and verification

- `kittyplex doctor` checks the Kitty callback, selected local binary, run0 version
  and SSH availability. Run it inside Kitty; it needs a real terminal. It does
  not change user, connect to a server or start a group.
- No compatible Kitty response: check the include, helper directory, password
  mode and executable override; start a fresh Kitty process. Generic terminal
  multiplexers between Kitty and kittyplex can intercept terminal control traffic.
- Target reports unknown `start` command or command not found: install kittyplex
  >=0.4 on that target's SSH command PATH, or supply SSH `--remote-binary`.
  SSH command PATH can differ from an interactive login PATH; the launcher must
  find the executable before it can initialize the target login environment.
- Run0 fails authorization: check normal Polkit policy and agent availability.
  Do not copy test-only passwordless rules from the isolated VM onto a desktop.
- A group opens but graphics fail: consult the [compatibility reference](kitty-compatibility.md).
  Target terminfo and streamed graphics matter independently of authentication.

```sh
nix develop --command ./scripts/test.sh
nix flake check path:.
# Separate, potentially substantial NixOS VM build and actual multi-user test:
nix build path:.#desktop-test -L
```

The normal suite tests desktop actions, preflight, automatic SSH startup, missing
packages and cleanup in isolated Kitty/Xvfb. Real multi-user run0 tests live in
`nix/desktop-test.nix`: target login shells (Bash, Zsh, Fish), both PTY modes,
identity/home, controller socket ownership, local relay UIDs, explicit group/cwd
and job control, plus authorization rejection. Its passwordless
rule exists only inside that test VM. The VM requires a machine able to run NixOS
VM tests; a skipped or unexecuted VM is not evidence of working authentication
on your desktop. Module checks evaluate NixOS and Home Manager configurations,
reject unrestricted remote control and parse generated Kitty configurations.
