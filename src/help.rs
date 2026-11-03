//! Short, command-specific help; the complete reference is kittyplex(1).
pub const ROOT: &str = "kittyplex - terminal multiplexing with Kitty-owned screens

Usage:
  kittyplex [--heartbeat-timeout SECONDS]
  kittyplex start [OPTIONS]       Start a local group
  kittyplex run0 [OPTIONS]        Start a group as another local user
  kittyplex ssh [OPTIONS] HOST [-- SSH_OPTIONS...]
  kittyplex ctl [--socket PATH] COMMAND
  kittyplex doctor               Check the local Kitty integration

Options:
  -h, --help                     Show help without starting a group
  -V, --version                  Show the version
      --heartbeat-timeout SECONDS
                                 Controller loss timeout (default 30; minimum 10)

With no subcommand, start the controller here, including after an SSH login.
Install the Kitty integration locally and kittyplex on each target first.
Ctrl-Shift-M, then ? shows shortcuts; t/c opens a tab, v/h splits, w closes.
Use n/p/1-9 to select, q to quit; repeat the prefix to send it.
Connection loss closes the group. Cooperative sessions can detach/attach displays.

Run 'kittyplex COMMAND --help' for command help, or 'man kittyplex' for details.
Installation: https://github.com/claes/kittyplex/blob/master/docs/integration-instructions.md
";

const START: &str = "Start a local group with Kitty-owned screens.

Usage: kittyplex start [--login] [--cwd DIRECTORY]

Options:
      --login          Load the account's login environment; default to its home
      --cwd DIRECTORY  Initial directory (default: current directory, or home
                       with --login); ~ and ~/ resolve to the account's home
  -h, --help           Show this help

Uses the executing account's shell. With --login, relative directories are
relative to its home. Each new session inherits the group's initial directory.
Run from an ordinary Kitty shell; use a Kitty action inside a managed view.

Example: kittyplex start --cwd /srv/work
See kittyplex(1) and the installation guide linked by 'kittyplex --help'.
";

const RUN0: &str = "Start a group as another local user, using run0 authentication.

Usage: kittyplex run0 [OPTIONS]

Options:
  -u, --user USER       Target account (default: root)
  -g, --group GROUP     Target primary group (default: account's primary group)
  -D, --chdir DIRECTORY Initial directory (default: target home)
      --cwd DIRECTORY  Alias for --chdir
      --pty            Allocate the transport PTY before authentication finishes
      --pty-late       Allocate it after authorization (default)
      --no-ask-password
                       Do not prompt for authentication; does not grant access
  -h, --help           Show this help

Requires systemd >=258 and normal Polkit authorization. The target gets its
own login environment and shell. --pty and --pty-late cannot be combined.
Quit with Ctrl-Shift-M, then q to return to your source shell.
Inside a managed view, use the Kitty kittyplex-run0 action instead.

Example: kittyplex run0 -u bob --pty
See kittyplex(1) for directory semantics, environment and configuration.
";

const SSH: &str = "Connect with SSH and automatically start kittyplex on the target.

Usage: kittyplex ssh [OPTIONS] DESTINATION [-- SSH_OPTIONS...]

Options:
      --remote-binary PATH  Target executable (default: kittyplex on SSH PATH)
      --cwd DIRECTORY      Initial directory (default: target home)
  -h, --help               Show this help

Put connection options such as -p, -i and -J after --. The target must already
have kittyplex >=0.4 and xterm-kitty terminfo installed. Uses its login shell.
The launcher supplies the remote command and forces a foreground PTY;
-N, -f, -T, -n, -O, -W, -G and conflicting -o settings are rejected.
Inside a managed view, use the Kitty kittyplex-ssh action instead.

Example: kittyplex ssh user@server -- -p 2222 -J jump-host
See kittyplex(1); targets are not bootstrapped or uploaded automatically.
";

const CTL: &str = "Manage an existing group on the same host and account.

Usage: kittyplex ctl [--socket PATH] COMMAND

Commands:
  new          Create and select a shell in a new tab (asynchronous)
  split right  Create a session to the right of the selected session
  split below  Create a session below the selected session
  next         Select the next session
  previous     Select the previous session
  select N     Select a session by its current 1-based position
  status       Print selection, ready sessions, IDs and shell PIDs
  list         List all sessions, including detached ones, by stable ID
  detach ID    Detach a session display (requires Restore Display support)
  attach ID    Open a fresh tab and restore the detached session display
  quit         Close the entire group

Options:
      --socket PATH  Controller socket (default: KITTYPLEX_SOCKET)
  -h, --help         Show this help without connecting to a socket

Run from a managed shell, or supply its controller socket explicitly.
Positions follow attached sessions in creation order; IDs remain stable.
Detach/attach requires the carrier connection to stay open.
Example: kittyplex ctl status
See kittyplex(1) for environment and lifecycle details.
";

const DOCTOR: &str = "Check the local Kitty integration without starting a group.

Usage: kittyplex doctor

Options:
  -h, --help  Show this help without probing a terminal

Run inside the configured Kitty terminal, including inside a managed view.
Reports the scoped callback, desktop API, carrier version, selected local
binary, run0 version and SSH availability. Does not connect to a remote host
or change user. Missing optional tools are reported separately from Kitty
probe failures. Start fresh Kitty after installing or updating the helpers.

See kittyplex(1) and the installation guide linked by 'kittyplex --help'.
";

pub fn command(mode: &str) -> Option<&'static str> {
    match mode {
        "start" => Some(START),
        "run0" => Some(RUN0),
        "ssh" => Some(SSH),
        "ctl" => Some(CTL),
        "doctor" => Some(DOCTOR),
        _ => None,
    }
}

/// Match help options, not option values or arguments after SSH's separator.
pub fn requested(mode: &str, args: &[String]) -> bool {
    let values: &[&str] = match mode {
        "start" => &["--cwd"],
        "run0" => &["-u", "--user", "-g", "--group", "-D", "--chdir", "--cwd"],
        "ssh" => &["--remote-binary", "--cwd"],
        "ctl" => &["--socket"],
        _ => &[],
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--" => break,
            "-h" | "--help" => return true,
            value if values.contains(&value) => i += 1,
            _ => {}
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn help_respects_option_values_and_ssh_passthrough() {
        assert!(requested("ctl", &args(&["--socket", "/missing", "--help"])));
        assert!(requested("run0", &args(&["-u", "bob", "--help"])));
        assert!(!requested("start", &args(&["--cwd", "--help"])));
        assert!(!requested("ssh", &args(&["host", "--", "--help"])));
        assert!(!requested(
            "ssh",
            &args(&["--remote-binary=--help", "host"])
        ));
        assert!(command("__broker").is_none());
    }
}
