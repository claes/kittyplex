#[cfg(not(target_os = "linux"))]
compile_error!("termlab currently supports Linux only");

mod control;
mod session;
mod terminal;

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Duration;

use control::{Action, Control};
use session::Session;
use terminal::{Signals, Terminal};

const HELP: &str = "termlab — local PTY session switcher (Linux)

Usage:
  termlab
  termlab ctl [--socket PATH] new|next|previous|select N|status|quit
  termlab --help

Starts $SHELL -i (or /bin/sh -i). Install kitty/termlab.conf and the
kitty/termlab.py helper to enable the optional Ctrl-Shift-M prefix:
  c: new shell   n/p: next/previous   1–9: select   q: quit
  Escape: cancel   Ctrl-Shift-M again: send the literal chord

No keyboard bytes are reserved by termlab. Without kitty integration,
use `termlab ctl` from another terminal with an explicit socket path.
Inside a session, ctl uses $TERMLAB_SOCKET automatically.

Only the selected session is displayed. Inactive output is discarded.
Display restoration is best effort; some programs require manual redraw.
No panes, stored scrollback, daemon, or detach/reattach support.
Exiting termlab hangs up its sessions.
";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("termlab: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print!("{HELP}");
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("termlab {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("ctl") => control::client(&args[1..]),
        Some(_) => Err(io::Error::other("unknown argument; see --help")),
        None => {
            if std::env::var_os("TERMLAB_SOCKET").is_some() {
                return Err(io::Error::other("nested termlab is not supported"));
            }
            App::new()?.run()
        }
    }
}

struct App {
    // Drop sessions before restoring the outer terminal and signal handlers.
    sessions: Vec<Session>,
    terminal: Terminal,
    control: Control,
    _signals: Signals,
    active: usize,
    next_id: u64,
    shell: PathBuf,
    output: VecDeque<u8>,
}

impl App {
    fn new() -> io::Result<Self> {
        let signals = Signals::install()?;
        let mut terminal = Terminal::open()?;
        let control = Control::bind()
            .map_err(|e| io::Error::new(e.kind(), format!("bind control socket: {e}")))?;
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let first = Session::spawn(
            1,
            &shell,
            control.path(),
            terminal.original(),
            terminal.size()?,
        )?;
        terminal.register(control.path())?;
        Ok(Self {
            sessions: vec![first],
            terminal,
            control,
            _signals: signals,
            active: 0,
            next_id: 2,
            shell,
            output: VecDeque::new(),
        })
    }

    fn run(mut self) -> io::Result<()> {
        loop {
            if Signals::terminating() {
                break;
            }
            if Signals::resized() {
                let size = self.terminal.size()?;
                for session in &self.sessions {
                    session.resize(size)?;
                }
            }
            self.reap()?;
            if self.sessions.is_empty() {
                break;
            }

            // Bounded queues and per-session read budgets keep a busy session
            // from starving keyboard input, management commands, or other PTYs.
            let tty_events = if self.sessions[self.active].input.len() < 65536 {
                libc::POLLIN
            } else {
                0
            } | if self.output.is_empty() {
                0
            } else {
                libc::POLLOUT
            };
            let mut fds = vec![pollfd(self.terminal.as_raw_fd(), tty_events)];
            fds.push(pollfd(self.control.as_raw_fd(), libc::POLLIN));
            for (index, session) in self.sessions.iter().enumerate() {
                let events = if index != self.active || self.output.len() < 65536 {
                    libc::POLLIN
                } else {
                    0
                } | if session.input.is_empty() {
                    0
                } else {
                    libc::POLLOUT
                };
                fds.push(pollfd(
                    if session.hung_up {
                        -1
                    } else {
                        session.as_raw_fd()
                    },
                    events,
                ));
            }
            self.control.append_peers(&mut fds);
            // A finite timeout also bounds partial control requests and closes
            // the signal-before-poll race without doing I/O in signal handlers.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 50) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }

            if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                break;
            }
            if fds[0].revents & libc::POLLIN != 0 {
                let mut input = [0; 8192];
                match self.terminal.read(&mut input) {
                    Ok(0) => break,
                    Ok(n) => self.sessions[self.active].input.extend(&input[..n]),
                    Err(e) if transient(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            if fds[0].revents & libc::POLLOUT != 0 {
                flush(&mut self.terminal, &mut self.output)?;
            }
            for (index, session) in self.sessions.iter_mut().enumerate() {
                let events = fds[index + 2].revents;
                if events & libc::POLLOUT != 0 {
                    session.flush_input()?;
                }
                if events & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
                    && (index != self.active || self.output.len() < 65536)
                {
                    session.read_output((index == self.active).then_some(&mut self.output))?;
                }
            }

            self.control.accept()?;
            let requests = self.control.requests();
            for (mut peer, request) in requests {
                let result = request.and_then(|action| self.action(action));
                let quit = matches!(&result, Ok((true, _)));
                let response = match result {
                    Ok((_, message)) => format!("OK {message}\n"),
                    Err(error) => format!("ERR {error}\n"),
                };
                // One small response on a fresh connection fits the socket's
                // send buffer; a disconnected control client is harmless.
                let _ = peer.write_all(response.as_bytes());
                if quit {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn action(&mut self, action: Action) -> io::Result<(bool, String)> {
        match action {
            Action::New => {
                let session = Session::spawn(
                    self.next_id,
                    &self.shell,
                    self.control.path(),
                    self.terminal.original(),
                    self.terminal.size()?,
                )?;
                self.next_id += 1;
                self.sessions.push(session);
                self.activate(self.sessions.len() - 1)?;
            }
            Action::Next => self.activate((self.active + 1) % self.sessions.len())?,
            Action::Previous => {
                self.activate((self.active + self.sessions.len() - 1) % self.sessions.len())?
            }
            Action::Select(index) => {
                if index == 0 || index > self.sessions.len() {
                    return Err(io::Error::other("session position does not exist"));
                }
                self.activate(index - 1)?;
            }
            Action::Quit => return Ok((true, "quitting".into())),
            Action::Status => {}
        }
        let pids: Vec<String> = self.sessions.iter().map(|s| s.pid().to_string()).collect();
        let ids: Vec<String> = self.sessions.iter().map(|s| s.id.to_string()).collect();
        Ok((
            false,
            format!(
                "active={} sessions={} ids={} pids={}",
                self.active + 1,
                self.sessions.len(),
                ids.join(","),
                pids.join(",")
            ),
        ))
    }

    fn activate(&mut self, index: usize) -> io::Result<()> {
        if index == self.active {
            return Ok(());
        }
        self.active = index;
        self.output.clear();
        self.output.extend(b"\x1b[0m\x1b[2J\x1b[H");
        self.sessions[index].redraw(self.terminal.size()?)
    }

    fn reap(&mut self) -> io::Result<()> {
        let active_id = self.sessions.get(self.active).map(|s| s.id);
        let old_position = self.active;
        let mut index = 0;
        while index < self.sessions.len() {
            if self.sessions[index].exited()? {
                // Preserve the final output of an exiting visible shell.
                let visible = Some(self.sessions[index].id) == active_id;
                if visible && self.output.len() >= 65536 {
                    index += 1;
                    continue;
                }
                let drained =
                    self.sessions[index].read_output(visible.then_some(&mut self.output))?;
                if visible && !drained {
                    index += 1;
                    continue;
                }
                self.sessions.remove(index);
            } else {
                index += 1;
            }
        }
        if self.sessions.is_empty() {
            self.terminal.finish_output(&mut self.output)?;
        } else if let Some(index) = self.sessions.iter().position(|s| Some(s.id) == active_id) {
            self.active = index;
        } else {
            // Force redraw even when the replacement has the same position.
            self.active = usize::MAX;
            self.activate(old_position % self.sessions.len())?;
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.sessions.clear();
        // Terminal and socket RAII guards run while signal handlers are active.
    }
}

fn pollfd(fd: libc::c_int, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn flush(writer: &mut impl Write, buffer: &mut VecDeque<u8>) -> io::Result<()> {
    while !buffer.is_empty() {
        match writer.write(buffer.as_slices().0) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buffer.drain(..n);
            }
            Err(error) if transient(&error) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
