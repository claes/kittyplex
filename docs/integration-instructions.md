# Desktop integration

Kittyplex provides launchers for local shells, SSH and switching user with run0.
Kitty owns the screens; kittyplex owns the application PTYs. These launchers automate
starting the existing original-terminal carrier design. They do not add another
network service, SSH connection or terminal emulator.

This document owns installation and launch configuration. The
[process guide](how-it-works.md) explains the transport and process roles;
[compatibility](kitty-compatibility.md) covers terminal features and their limits.
Detach and reattach remain unimplemented; see [detach analysis](detach-analysis.md).

## Choose a setup path

Choose one path for configuring Kitty; avoid duplicate includes or two tools
managing the same files. All paths use the same packaged integration assets.

| Setup | Who configures Kitty? | Complete examples |
| --- | --- | --- |
| [Kittyplex modules](#install-with-nixos-and-home-manager) | The project's NixOS and Home Manager modules | [Module-based flake](../examples/flake-with-modules.nix) |
| [Explicit Nix configuration](#explicit-nix-configuration-without-kittyplex-modules) | Your own standard Home Manager settings | [Manual Home Manager flake](../examples/flake-manual-home-manager.nix) |
| [Manual Kitty configuration](#manual-kitty-configuration) | You edit your existing `kitty.conf` | [Kitty config fragment](../examples/kitty.conf) and optional [NixOS package-only flake](../examples/flake-manual.nix) |

The Nix examples use your existing `configuration.nix`, including users and
hardware imports. Replace `myhost`, `alice` and the architecture, and retain your
existing state versions. Merge their settings into your current flake rather
than replacing your host configuration.

## Install with NixOS and Home Manager

Use [examples/flake-with-modules.nix](../examples/flake-with-modules.nix) as a
complete consumer flake example. Copy its inputs and modules into your existing
`flake.nix`; retain your host's `configuration.nix`, hardware imports, users and
existing state versions. Replace `myhost`, `alice` and the system architecture.
Use `github:claes/kittyplex` for published code or
`path:/absolute/path/to/kittyplex` for a local checkout. Git-backed local flake
URLs include only tracked files; add new files to Git or use `path:` to include
them.

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

The two `programs.kittyplex` blocks are in different scopes:

| Scope | Purpose |
| --- | --- |
| NixOS `programs.kittyplex` | Install the CLI and terminfo for all users, plus optional run0 prerequisites |
| `home-manager.users.alice.programs.kittyplex` | Configure Alice's Kitty integration and helper files |

Importing a module makes its options available; setting `enable = true` activates
it. The Home Manager module must be imported through `home-manager.sharedModules`
or the user's Home Manager imports, separately from the NixOS module.

If your user configuration is in a separate file, flake inputs are not automatic
module arguments. For example, pass `home-manager.extraSpecialArgs = { inherit inputs; };`
and request `{ inputs, pkgs, ... }` in that file, then use
`inputs.kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default`. A direct
`kittyplex` argument instead requires passing it explicitly in `extraSpecialArgs`.

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

## Explicit Nix configuration without kittyplex modules

[examples/flake-manual-home-manager.nix](../examples/flake-manual-home-manager.nix)
shows the complete NixOS flake, including inputs and explicit Kitty configuration.
It imports standard Home Manager only: it does not import either of this
project's integration modules or use any `programs.kittyplex` options.

The example explicitly installs the kittyplex package and Kitty terminfo using
`environment.systemPackages`, and enables normal run0 authentication using
`security.polkit.enable`. For run0, the host must have systemd >=258; these manual
settings do not add the version assertions supplied by the kittyplex module.
Kitty itself is installed for the desktop user by `programs.kitty.enable`.

Within `home-manager.users.alice`, it sets
`programs.kitty.settings.allow_remote_control = "password"`, adds
`include kittyplex.conf` through `programs.kitty.extraConfig`, and declares all
five `xdg.configFile` sources individually. The sources point into
`${kittyplexPackage}/share/kittyplex/kitty`, keeping helpers and the binary from
the same package. No shell copy/link step is needed for this path.

Merge the example's `programs.kitty` settings with your existing Home Manager
settings. Back up existing manually maintained files at the five managed paths
before activation. Keep your other Kitty settings and include the integration
only once. Optional launch bindings are commented out in the example.

Apply the configuration using your normal `nixos-rebuild switch --flake .#myhost`,
then follow [Verify the setup](#verify-the-setup).

## Building from source

Linux and Kitty 0.47.4 are the implementation baseline, with additional checks
on Kitty 0.49.1. A source build requires Rust 1.85 or newer:

```sh
cargo build --release --locked
install -Dm755 target/release/kittyplex "$HOME/.local/bin/kittyplex"
```

Make that directory available on PATH before starting Kitty, or set
`KITTYPLEX_BINARY` to the absolute binary path in Kitty's startup environment.
The four source helpers and `kittyplex.conf` are in `kitty/`. Alternatively,
`nix build path:.` produces `result/bin/kittyplex`, packaged integration assets
and the manual page. Cargo builds the executable only; to install the manual
separately, copy `docs/kittyplex.1` into your installation's `share/man/man1/`.

Install compatible binaries locally and on SSH targets. Each host can build for
its own architecture. Targets need `xterm-kitty` terminfo; no Nix installation
is required if you supply a compatible binary and terminfo yourself.

## Manual Kitty configuration

This path works with an existing `kitty.conf` and needs no Home Manager or
kittyplex integration module. Follow these steps as your desktop user.

### 1. Install the binary and integration assets

On NixOS, [examples/flake-manual.nix](../examples/flake-manual.nix) installs the
package, Kitty, terminfo and run0's Polkit prerequisite using standard NixOS
options only. It does **not** configure Kitty or install files in your home.
Merge it into your host flake and rebuild before continuing. Its `environment.pathsToLink` setting
exposes the packaged files at `/run/current-system/sw/share/kittyplex`; it does
not modify your shell's PATH or configure Kitty. Direct Home Manager package
references in the previous example do not need that setting. Polkit enables
run0 authorization under the existing host policy, without granting passwordless
access. Run0 requires
systemd >=258 and normal Polkit authorization.

For a source build, use the [source build instructions](#building-from-source).
Install the binary on your local PATH; SSH targets also need a compatible binary
and `xterm-kitty` terminfo. The four Python helpers and `kittyplex.conf` are in
`kitty/` in a checkout, or `share/kittyplex/kitty/` in a Nix package output.

### 2. Place all five files in Kitty's configuration directory

With the NixOS package-only example, create links to the system package:

```sh
kittyplex_kitty_dir="${KITTY_CONFIG_DIRECTORY:-${XDG_CONFIG_HOME:-$HOME/.config}/kitty}"
mkdir -p "$kittyplex_kitty_dir"
for file in kittyplex.py kittyplex_auth.py kittyplex_watcher.py kittyplex_launch.py kittyplex.conf; do
  ln -s "/run/current-system/sw/share/kittyplex/kitty/$file" "$kittyplex_kitty_dir/$file"
done
```

Alternatively, copy files from a checkout or package output. From the project
root, choose the source directory and run:

```sh
kittyplex_assets=kitty
# For a package built with `nix build path:.`, use instead:
# kittyplex_assets=result/share/kittyplex/kitty
kittyplex_kitty_dir="${KITTY_CONFIG_DIRECTORY:-${XDG_CONFIG_HOME:-$HOME/.config}/kitty}"
mkdir -p "$kittyplex_kitty_dir"
for file in kittyplex.py kittyplex_auth.py kittyplex_watcher.py kittyplex_launch.py kittyplex.conf; do
  cp -i "$kittyplex_assets/$file" "$kittyplex_kitty_dir/$file"
done
```

Back up existing files at those names first. The linking commands fail rather
than overwrite an existing file; the copying commands ask before overwriting.
For a custom Kitty configuration directory or explicit `--config`, use the actual
config directory recognized by Kitty rather than assuming the default path.
The bare filename `kittyplex.py` must resolve there: an `action_alias` cannot
replace that requirement for terminal-originated requests.

For the default location, the result is:

```text
~/.config/kitty/
├── kitty.conf                 # Your existing settings, plus the include below
├── kittyplex.conf             # Management mappings, scoped authorization, aliases
├── kittyplex.py
├── kittyplex_auth.py
├── kittyplex_watcher.py
└── kittyplex_launch.py
```

### 3. Merge the include into your existing Kitty configuration

[examples/kitty.conf](../examples/kitty.conf) is a copyable fragment. Merge its
lines into your current file; do not replace your existing configuration with it.
The required addition is:

```conf
include kittyplex.conf
```

### Verify the setup

For either manual path or explicit Nix configuration, start a **fresh Kitty
process** after installation; a config reload alone can retain cached Python
code. From a shell in that Kitty, run:

```sh
kittyplex doctor
kittyplex start
```

Doctor should report desktop API 1 and carrier v2. In the new session, press
Ctrl-Shift-M then `c` to create another tab, switch between the tabs, and press
Ctrl-Shift-M then `q` to close the group and return to your shell. For user
switching, try `kittyplex run0 -u bob --pty` with an existing target account.

## Kitty settings shared by all setup paths

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
With explicit Home Manager configuration, keep the edited file in your own
configuration repository and replace only that file's source:

```nix
xdg.configFile."kitty/kittyplex.conf".source = ./kittyplex.conf;
```

Keep the four helper sources pointing to the package. Review the copied config
when upgrading. Packaged helpers pin their local executable
to the matching Nix store path, so Kitty need not inherit a particular shell PATH.
`KITTYPLEX_BINARY`, set before Kitty starts, remains an explicit override for source
development. See [upgrading](#upgrading-an-existing-installation) when replacing components.

## Upgrading an existing installation

End existing groups, update local and target binaries and all five integration
files together, then start a **fresh Kitty process**. Reloading configuration can
retain cached Python authorization code. Carrier v2 is incompatible with v1;
see the [protocol contract](original-terminal-carrier.md#protocol-v2-kittyplex-03).
Automatic SSH startup requires the target `start` command from version 0.4 or
newer. The original login-then-run workflow remains supported.

## Launch and use

After installation, follow the [usage guide](usage.md) for local, run0 and SSH
workflows, controls and lifecycle. Use `kittyplex COMMAND --help` for a short
summary or `man kittyplex` for all options and defaults.

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

For test commands, the separate cross-user run0 VM, documentation validation and
benchmark reproduction, see the [development guide](development.md).
