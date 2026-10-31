use std::fs::{self, DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{CONTROL_TIMEOUT, invalid};

#[derive(Debug, PartialEq)]
pub enum Action {
    New,
    Next,
    Previous,
    Select(usize),
    Status,
    Quit,
}

impl Action {
    pub fn parse(request: &str) -> io::Result<Self> {
        let parts: Vec<_> = request.split_whitespace().collect();
        match parts.as_slice() {
            ["new"] => Ok(Self::New),
            ["next"] => Ok(Self::Next),
            ["previous"] => Ok(Self::Previous),
            ["status"] => Ok(Self::Status),
            ["quit"] => Ok(Self::Quit),
            ["select", number] => number
                .parse()
                .ok()
                .filter(|n| *n > 0)
                .map(Self::Select)
                .ok_or_else(|| invalid("select requires a positive session position")),
            _ => Err(invalid(
                "expected new, next, previous, select N, status, or quit",
            )),
        }
    }
}

struct Peer {
    stream: UnixStream,
    input: Vec<u8>,
    started: Instant,
}

pub struct Control {
    listener: UnixListener,
    directory: PathBuf,
    path: PathBuf,
    peers: Vec<Peer>,
}

impl Control {
    pub fn bind() -> io::Result<Self> {
        let mut random = [0; 8];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let name = format!(
            "termlab-{}-{:016x}",
            std::process::id(),
            u64::from_ne_bytes(random)
        );
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir() && p.as_os_str().len() < 60)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let directory = base.join(name);
        DirBuilder::new().mode(0o700).create(&directory)?;
        let path = directory.join("control.sock");
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(error);
            }
        };
        let control = Self {
            listener,
            directory,
            path,
            peers: Vec::new(),
        };
        control.listener.set_nonblocking(true)?;
        Ok(control)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append_peers(&self, fds: &mut Vec<libc::pollfd>) {
        fds.extend(
            self.peers
                .iter()
                .map(|p| crate::pollfd(p.stream.as_raw_fd(), libc::POLLIN)),
        );
    }

    pub fn accept(&mut self) -> io::Result<()> {
        for _ in 0..32 {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if self.peers.len() >= 32 {
                        continue;
                    }
                    stream.set_nonblocking(true)?;
                    self.peers.push(Peer {
                        stream,
                        input: Vec::new(),
                        started: Instant::now(),
                    });
                }
                Err(error) if crate::transient(&error) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn requests(&mut self) -> Vec<(UnixStream, io::Result<Action>)> {
        let mut ready = Vec::new();
        let mut pending = Vec::new();
        for mut peer in self.peers.drain(..) {
            let mut buffer = [0; 256];
            match peer.stream.read(&mut buffer) {
                Ok(0) => continue,
                Ok(n) => peer.input.extend(&buffer[..n]),
                Err(error) if crate::transient(&error) => {}
                Err(_) => continue,
            }
            let request = if let Some(end) = peer.input.iter().position(|b| *b == b'\n') {
                Some(
                    std::str::from_utf8(&peer.input[..end])
                        .map_err(|_| invalid("command must be UTF-8"))
                        .and_then(Action::parse),
                )
            } else if peer.input.len() >= 256 {
                Some(Err(invalid("command too long")))
            } else if peer.started.elapsed() >= CONTROL_TIMEOUT {
                Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "incomplete command",
                )))
            } else {
                None
            };
            match request {
                Some(action) => ready.push((peer.stream, action)),
                None => pending.push(peer),
            }
        }
        self.peers = pending;
        ready
    }
}

impl AsRawFd for Control {
    fn as_raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}

pub fn client(args: &[String]) -> io::Result<()> {
    let (socket, command) = if args.first().map(String::as_str) == Some("--socket") {
        let path = args
            .get(1)
            .ok_or_else(|| invalid("--socket requires a path"))?;
        (PathBuf::from(path), &args[2..])
    } else {
        (
            std::env::var_os("TERMLAB_SOCKET")
                .map(PathBuf::from)
                .ok_or_else(|| invalid("set TERMLAB_SOCKET or pass --socket PATH"))?,
            args,
        )
    };
    let request = command.join(" ");
    Action::parse(&request)?;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    writeln!(stream, "{request}")?;
    let mut response = String::new();
    stream.take(4096).read_to_string(&mut response)?;
    if let Some(message) = response.strip_prefix("OK ") {
        print!("{message}");
        Ok(())
    } else {
        Err(io::Error::other(
            response
                .trim()
                .strip_prefix("ERR ")
                .unwrap_or("invalid control response"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_control_commands() {
        for request in [
            "",
            "select",
            "select 0",
            "select -1",
            "select 1 extra",
            "new extra",
            "next\nquit",
        ] {
            assert!(Action::parse(request).is_err(), "{request:?}");
        }
        assert_eq!(Action::parse("select 9").unwrap(), Action::Select(9));
        assert_eq!(Action::parse("next").unwrap(), Action::Next);
    }
}
