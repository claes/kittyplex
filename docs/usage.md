# Using kittyplex

First complete one of the [installation paths](integration-instructions.md#choose-a-setup-path).
Kitty retains each screen while kittyplex keeps its shell and applications running.
Each session appears in a native Kitty tab. Keep the original carrier tab open.

## Start a group

From an ordinary shell in your configured Kitty:

```sh
kittyplex doctor                      # Check the local integration
kittyplex start                       # Local group in the current directory
kittyplex run0 -u bob --pty            # Another local account, using run0
kittyplex ssh user@server              # Connect and start a remote group
```

Run0 and SSH start in the target account's home with its login environment and
shell. Complete normal authentication when prompted. For a different directory,
use `kittyplex run0 -u bob -D /srv/work`. Both run0 PTY modes are supported;
the default `--pty-late` leaves authentication on the source terminal until
permission is granted. Run0 without `-u` targets root.

You can also start after an existing SSH login:

```sh
ssh server
kittyplex
```

Within a managed view, use a [Kitty launch action](integration-instructions.md#kitty-settings-shared-by-all-setup-paths)
to open an independent group. Direct CLI nesting is rejected. CLI group exit
returns to the source shell. A Kitty action closes its carrier on success,
returns focus to the source window if available, and keeps failures visible
until Enter is pressed.

For every option, default, environment variable and exit status, use
`man kittyplex` or [the manual source](kittyplex.1). For a short summary, use
`kittyplex --help` or, for example, `kittyplex run0 --help`. Help never starts a
group or requires a terminal or authentication. Before installing the man page,
view it from the checkout with `man -l docs/kittyplex.1`.

## Controls and lifecycle

| After Ctrl-Shift-M | Action |
| --- | --- |
| `t` / `c` | Create and select a shell in a new tab |
| `v` | Vertical split: new session on the right |
| `h` | Horizontal split: new session below |
| `w` | Close this pane and its session |
| `d` | Detach this display; requires application Restore Display support |
| `?` | Show help; Escape dismisses it |
| `n` / `p` | Next / previous session |
| `1`–`9` | Select the session at that position, if present |
| `q` | Close the entire group |
| Escape | Cancel |
| Ctrl-Shift-M again | Send one literal Ctrl-Shift-M to the application |

Management mode ends after one action. Shortcuts activate only in session views.
Ordinary Enter/Ctrl-M, typing, paste, mouse reports, and terminal replies remain
application input. Change `ctrl+shift+m` in the entry, literal-prefix and help
mappings to customize the prefix.

Tabs and split panes each hold an independent session. Repeated `v` and `h`
actions create nested splits. Splitting enables Kitty’s splits layout in that
tab if needed, preserving its other enabled layouts. Help covers only the focused
pane; Escape closes the overlay without reaching the application. Use Up/Down or Page Up/Page Down
to scroll help in a small pane. Kitty’s native controls handle pane focus,
movement and resizing.

New sessions use the group's chosen interactive shell, initial directory and
environment. Launchers choose the account's configured shell; bare `kittyplex`
uses `$SHELL`, falling back to `/bin/sh`. Native Kitty focus changes are
synchronized, including tab clicks and native Kitty next/previous-tab navigation.
Kittyplex’s next/previous and numbered shortcuts stay within their own group and
use attached sessions in creation order; manually reordering tabs does not renumber them.
Detached sessions retain their stable IDs but have no navigation position.
Management commands identify their originating session.
Closing a pane closes its remote session; closing a tab closes its sessions; exiting the last shell ends
the group. Quitting closes only kittyplex’s session views, preserving unrelated
tabs and ordinary windows you manually added.
A group supports up to eight simultaneous sessions.

**Display detach/attach requires a cooperating application and a live carrier.**
It does not provide persistence after connection loss. Closing the
carrier, losing SSH, or losing the controller closes the group. Quit hangs up
shells and foreground process groups and restores the original carrier termios.
Jobs deliberately ignoring hangup can survive; SIGKILL cannot run cleanup.

Hidden sessions continue receiving output. A slow display can apply backpressure
to its applications; see the [carrier flow-control contract](original-terminal-carrier.md#flow-control-and-scheduling).

## Command-line control

From a managed shell, `kittyplex ctl status` shows the group state, and
`kittyplex ctl new` requests a session in a new tab.
`kittyplex ctl split right` and `kittyplex ctl split below` split the currently
selected session, even when the command runs in a different session. Use `kittyplex ctl --help` or
`man kittyplex` for all control commands and explicit socket selection.

## Detach and attach a display

Keep a control shell in one session and launch a cooperating application in another.
Press **Ctrl-Shift-M**, release it, then **d** in the application view to detach.
The view closes while its application, shell and PTY remain alive. An ordinary
pane close still terminates the session. Unsupported applications, including a
plain shell, cannot detach; the shortcut reports an error and leaves the view open.

From the control shell:

```sh
kittyplex ctl list
kittyplex ctl detach 2    # Command equivalent; use an ID from list
kittyplex ctl attach 2
```

`list` includes attached and detached sessions, sorted by stable session ID, with
Restore Display support, shell PID and pending restore/error information. The
`detach 2` example is an alternative to the shortcut, not an additional step.
`attach 2` asynchronously opens a fresh tab for the same session. It applies the
new dimensions and requests restoration before accepting user keys; terminal
replies remain enabled. Completion means ordered delivery into the terminal,
not an acknowledgement of pixels rendered on screen. Failed restoration closes
the replacement view, preserves the application and reports the error; retry
`attach` after resolving it. An already attached/attaching session is rejected.

Detached sessions count toward the eight-session limit. All views can be detached
while the carrier remains open. With no control shell visible, run commands in
another terminal on the same host/account using `kittyplex ctl --socket PATH`;
record `printf '%s\n' "$KITTYPLEX_SOCKET"` before detaching the last view.
Carrier/SSH loss still terminates the group. Split layout is not restored:
reattachment always creates a new tab.

### Try the image demo through Kittyplex

From the repository root in Kitty:

```sh
kittyplex start
```

Keep the first shell for management. Press Ctrl-Shift-M then `t` to open another
session, return to this repository directory if necessary, and run:

```sh
./scripts/image-demo.sh
```

The script builds and launches the image demo in the current terminal. It accepts
demo options, for example `./scripts/image-demo.sh --seed 7`.
Press `i` to generate images, note the displayed image number and application PID,
then Ctrl-Shift-M followed by `d`. In the control shell, run `kittyplex ctl list`
and `kittyplex ctl attach ID` using the demo session's ID. The same PID restores
the selected image; `i` generates another and `q` returns to the original shell.
A registration is revoked if its foreground terminal job changes, preventing
a crashed application from leaving its shell marked as supported. The binary and matching Kitty integration must both include this feature; after
installing updated integration, restart Kitty. For SSH, build/install the demo
on the target where its application runs.

## Try the isolated demo

From a graphical Linux desktop, run these commands from the project root:

```sh
./scripts/demo.sh
# Equivalent, including untracked working-tree files:
nix run path:.#demo
```

This builds a separate Kitty process and temporary localhost SSH server without
changing your installed Kitty/SSH configuration or system services. The terminal
prints fixture log locations. Failed runs retain their temporary directory;
successful runs remove it when the demo process exits. This demonstrates real
SSH locally, rather than performance on a separate server.

## Interactive walkthrough

Press **Ctrl-Shift-M**, release both keys, then press the action key.

1. In the initial session, run:

   ```sh
   echo "session=$KITTYPLEX_SESSION"; tty
   printf 'This is the first screen\n'
   ```

2. Press **Ctrl-Shift-M, c**. Run the same `echo`/`tty` command. The session ID
   and remote PTY must differ, and a new Kitty tab should appear. Switch back
   using the tab bar or Kitty’s native tab navigation, then use
   **Ctrl-Shift-M, 1** and verify that the first screen is still present.

3. Run `vim -Nu NONE -i NONE /tmp/kittyplex-demo-note` in the first session. Type
   `i`, enter a few lines, then Escape. Switch to session 2 and back using
   **Ctrl-Shift-M, n/p** or **1/2**. Vim's contents and cursor should persist.
   Exit Vim with `:q!` followed by Enter.

4. In session 2, run:

   ```sh
   for i in $(seq 1 100); do echo "counter-$i"; sleep .1; done
   ```

   Switch away immediately, wait a few seconds, and switch back. Output should
   have continued updating that session, without appearing in session 1.

5. Resize the kitty window and run `stty size` in each session. Try `sleep 30`,
   Ctrl-Z, `jobs`, `fg`, then Ctrl-C. These must affect the remote foreground
   job while leaving its shell usable. Close one session with `exit`; the
   remaining session should stay usable. Alternatively, close its native Kitty
   tab and verify that only that session ends.

6. Press **Ctrl-Shift-M, q**. You should return to the original SSH shell with
   working echo, Enter, and line editing. Run `echo restored`, then `exit` to
   close the demo and clean up its fixture.

## Kitty features and graphics

```sh
./scripts/demo.sh --features
# In a managed session:
kittyplex-features
```

This generates temporary PNG/GIF assets and prints commands for checking images,
large streamed transfers, animation, keyboard input, and sizing. Create two
sessions and display different images; switch using both the Kitty tab bar and
Ctrl-Shift-M shortcuts. Clear one image and check that the other remains, then
resize or change the font size and repeat.
The [Kitty compatibility reference](kitty-compatibility.md) records automated
coverage, manual checks, and remote filesystem and desktop integration limits.

