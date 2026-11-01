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
| `c` | Create and select a shell |
| `n` / `p` | Next / previous session |
| `1`–`9` | Select the session at that position, if present |
| `q` | Close the entire group |
| Escape | Cancel |
| Ctrl-Shift-M again | Send one literal Ctrl-Shift-M to the application |

Management mode ends after one action. Shortcuts activate only in session views.
Ordinary Enter/Ctrl-M, typing, paste, mouse reports, and terminal replies remain
application input. Change both `ctrl+shift+m` mappings to customize the prefix.

New sessions use the group's chosen interactive shell, initial directory and
environment. Launchers choose the account's configured shell; bare `kittyplex`
uses `$SHELL`, falling back to `/bin/sh`. Native Kitty focus changes are
synchronized, including tab clicks and native Kitty next/previous-tab navigation.
Kittyplex’s next/previous and numbered shortcuts stay within their own group and
use session creation order; manually reordering tabs does not renumber sessions.
Management commands identify their originating session.
Closing a session tab closes its remote session; exiting the last shell ends
the group. Quitting closes only kittyplex’s session views, preserving unrelated
tabs and ordinary windows you manually added.
A group supports up to eight simultaneous sessions.

**There is no detach/reattach or persistence after connection loss.** Closing the
carrier, losing SSH, or losing the controller closes the group. Quit hangs up
shells and foreground process groups and restores the original carrier termios.
Jobs deliberately ignoring hangup can survive; SIGKILL cannot run cleanup.

Hidden sessions continue receiving output. A slow display can apply backpressure
to its applications; see the [carrier flow-control contract](original-terminal-carrier.md#flow-control-and-scheduling).

## Command-line control

From a managed shell, `kittyplex ctl status` shows the group state, and
`kittyplex ctl new` requests another session. Use `kittyplex ctl --help` or
`man kittyplex` for all control commands and explicit socket selection.

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

