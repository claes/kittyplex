//! Local broker: private kitty-control FD, datagram callbacks, and stream relays.
use crate::protocol::{self as p, Channel, Message};
use crate::terminal::Signals;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Broker {
    group: String,
    directory: PathBuf,
    carrier: u64,
    helper: String,
    rc: File,
    wire: VecDeque<u8>,
    remote: VecDeque<Message>,
    seq: u64,
    views: HashMap<u64, Channel<UnixStream>>,
    connecting: Vec<(Channel<UnixStream>, Instant)>,
    events: UnixDatagram,
    listener: UnixListener,
    last_rx: Instant,
    heartbeat: Instant,
    timeout: Duration,
    closing: Option<Instant>,
    finishing: bool,
    awaiting_finish: bool,
    round: usize,
}
pub fn run(args: &[String]) -> io::Result<()> {
    if args.len() != 5 {
        return Err(crate::invalid("internal broker arguments"));
    }
    let descriptor = std::env::var("KITTY_LISTEN_ON")
        .ok()
        .and_then(|s| s.strip_prefix("fd:").and_then(|s| s.parse::<i32>().ok()))
        .filter(|n| *n >= 3)
        .ok_or_else(|| crate::invalid("broker requires kitty's private control descriptor"))?;
    let rc = unsafe { File::from_raw_fd(descriptor) };
    unsafe {
        libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(descriptor, libc::F_SETFL, libc::O_NONBLOCK);
    }
    let directory = PathBuf::from(&args[0]);
    let mut broker = Broker {
        events: UnixDatagram::bind(directory.join("events.sock"))?,
        listener: UnixListener::bind(directory.join("views.sock"))?,
        directory,
        group: args[1].clone(),
        carrier: args[2]
            .parse()
            .map_err(|_| crate::invalid("invalid carrier id"))?,
        helper: args[3].clone(),
        rc,
        wire: VecDeque::new(),
        remote: VecDeque::new(),
        seq: 0,
        views: HashMap::new(),
        connecting: Vec::new(),
        last_rx: Instant::now(),
        heartbeat: Instant::now(),
        timeout: Duration::from_secs(
            args[4]
                .parse()
                .map_err(|_| crate::invalid("invalid timeout"))?,
        ),
        closing: None,
        finishing: false,
        awaiting_finish: false,
        round: 0,
    };
    broker.events.set_nonblocking(true)?;
    broker.listener.set_nonblocking(true)?;
    let _signals = Signals::install()?;
    broker.send("ready", 0);
    let result = broker.run();
    let _ = broker.local("cleanup", 0);
    let deadline = Instant::now() + Duration::from_millis(300);
    while !broker.wire.is_empty() && Instant::now() < deadline {
        let _ = crate::flush(&mut broker.rc, &mut broker.wire);
        std::thread::sleep(Duration::from_millis(5));
    }
    broker.views.clear();
    result
}
impl Broker {
    fn send(&mut self, kind: &str, id: u64) {
        self.remote.push_back(Message::new(&self.group, kind, id));
    }
    fn local(&mut self, kind: &str, id: u64) -> io::Result<()> {
        let m = Message::new(&self.group, kind, id);
        self.local_message(&m)
    }
    fn local_message(&mut self, m: &Message) -> io::Result<()> {
        self.wire.extend(p::remote_command("kitten",serde_json::json!({"kitten":self.helper,"match":format!("id:{}",self.carrier),"args":["_local",String::from_utf8(m.bytes()?).unwrap()]}),false)?);
        Ok(())
    }
    fn run(&mut self) -> io::Result<()> {
        loop {
            if Signals::terminating() {
                return Ok(());
            }
            if self.last_rx.elapsed() > self.timeout {
                return Err(io::Error::other("remote carrier heartbeat lost"));
            }
            if self.finishing
                && self
                    .closing
                    .is_some_and(|t| t.elapsed() > Duration::from_millis(300))
                && self.wire.is_empty()
            {
                return Ok(());
            }
            if !self.awaiting_finish
                && !self.finishing
                && self.heartbeat.elapsed() > Duration::from_secs(5)
            {
                self.send("heartbeat", 0);
                self.local("check", 0)?;
                for (id, view) in &mut self.views {
                    view.send(&Message::new(&self.group, "heartbeat", *id))?;
                }
                self.heartbeat = Instant::now();
            }
            // Cap how much is pushed into kitty's source PTY at once. Backpressure
            // continues through the private socket to the originating relays.
            let mut batch = Vec::new();
            for _ in 0..16 {
                if self.wire.len() > p::OUTPUT {
                    break;
                }
                let Some(mut m) = self.remote.front().cloned() else {
                    break;
                };
                m.seq = self.seq + 1;
                let bytes = p::carrier(&m)?;
                if !batch.is_empty() && batch.len() + bytes.len() > p::CHUNK {
                    break;
                }
                self.remote.pop_front();
                self.seq += 1;
                batch.extend(bytes);
            }
            if !batch.is_empty() {
                self.wire.extend(p::remote_command("send-text",serde_json::json!({"match":format!("id:{}",self.carrier),"data":format!("base64:{}",p::encode(&batch))}),false)?);
            }
            crate::flush(&mut self.rc, &mut self.wire)?;
            let mut ignored = [0; 8192];
            match self.rc.read(&mut ignored) {
                Ok(0) => return Err(io::Error::other("kitty control descriptor closed")),
                Ok(_) => {}
                Err(e) if crate::transient(&e) => {}
                Err(e) => return Err(e),
            }
            // The callback socket preserves datagram boundaries and sender order.
            for _ in 0..16 {
                let mut buf = [0; p::FRAME];
                match self.events.recv(&mut buf) {
                    Ok(n) => self.event(Message::parse(&buf[..n])?)?,
                    Err(e) if crate::transient(&e) => break,
                    Err(e) => return Err(e),
                }
            }
            if self.connecting.len() < p::MAX_SESSIONS {
                match self.listener.accept() {
                    Ok((s, _)) => {
                        s.set_nonblocking(true)?;
                        self.connecting.push((Channel::new(s), Instant::now()));
                    }
                    Err(e) if crate::transient(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            let mut index = 0;
            while index < self.connecting.len() {
                let messages = self.connecting[index].0.pump()?;
                if messages.is_empty() {
                    index += 1;
                    continue;
                }
                let mut messages = messages.into_iter();
                let m = messages.next().unwrap();
                if m.g != self.group || m.kind != "view_ready" || self.views.contains_key(&m.id) {
                    return Err(crate::invalid("invalid relay handshake"));
                }
                let id = m.id;
                let mut followup = Vec::new();
                for m in messages {
                    if m.g != self.group
                        || m.id != id
                        || !matches!(m.kind.as_str(), "input" | "resize" | "credit")
                    {
                        return Err(crate::invalid("invalid message after relay handshake"));
                    }
                    followup.push(m);
                }
                let (c, _) = self.connecting.remove(index);
                self.views.insert(m.id, c);
                self.remote.push_back(m);
                self.remote.extend(followup);
            }
            self.connecting
                .retain(|(c, t)| !c.eof && t.elapsed() < Duration::from_secs(5));
            let mut incoming = Vec::new();
            let mut dead = Vec::new();
            let mut ids: Vec<_> = self.views.keys().copied().collect();
            ids.sort_unstable();
            let count = ids.len();
            for offset in 0..count {
                let id = &ids[(self.round + offset) % count];
                let view = self.views.get_mut(id).unwrap();
                if self.remote.len() < 128 {
                    for m in view.pump()? {
                        if m.g != self.group
                            || m.id != *id
                            || !matches!(m.kind.as_str(), "input" | "credit" | "resize")
                        {
                            return Err(crate::invalid("invalid view message"));
                        }
                        incoming.push(m);
                    }
                }
                if view.eof {
                    dead.push(*id);
                }
            }
            self.round = self.round.wrapping_add(1);
            self.remote.extend(incoming);
            for id in dead {
                self.views.remove(&id);
                self.send("close", id);
            }
            if self.closing.is_some()
                && self.views.is_empty()
                && self.connecting.is_empty()
                && !self.awaiting_finish
            {
                self.send("closed", 0);
                self.awaiting_finish = true;
            }
            let queued = self.wire.len()
                + self
                    .remote
                    .iter()
                    .map(|m| m.data.len() + 128)
                    .sum::<usize>()
                + self.views.values().map(|v| v.output.len()).sum::<usize>();
            if queued > p::GROUP {
                return Err(io::Error::other("broker queue limit exceeded"));
            }
            let mut fds = vec![
                crate::pollfd(self.events.as_raw_fd(), libc::POLLIN),
                crate::pollfd(self.listener.as_raw_fd(), libc::POLLIN),
                crate::pollfd(
                    self.rc.as_raw_fd(),
                    libc::POLLIN
                        | if self.wire.is_empty() {
                            0
                        } else {
                            libc::POLLOUT
                        },
                ),
            ];
            fds.extend(self.views.values().map(|v| {
                crate::pollfd(
                    v.io.as_raw_fd(),
                    libc::POLLIN
                        | if v.output.is_empty() {
                            0
                        } else {
                            libc::POLLOUT
                        },
                )
            }));
            unsafe {
                libc::poll(fds.as_mut_ptr(), fds.len() as _, 10);
            }
        }
    }
    fn event(&mut self, m: Message) -> io::Result<()> {
        if m.g != self.group {
            return Err(crate::invalid("wrong broker group"));
        }
        if self.awaiting_finish && !matches!(m.kind.as_str(), "finish" | "abort" | "heartbeat") {
            if m.seq > 0 {
                self.last_rx = Instant::now();
            }
            return Ok(());
        }
        match m.kind.as_str() {
            "finish" => {
                if !self.awaiting_finish {
                    return Err(crate::invalid("unexpected finish"));
                }
                self.local("cleanup", 0)?;
                self.finishing = true;
                self.closing = Some(Instant::now());
            }
            "create" => self.local_message(&m)?,
            "select" => self.local("select", m.id)?,
            "output" | "input_credit" => {
                if let Some(v) = self.views.get_mut(&m.id) {
                    v.send(&m)?;
                }
            }
            "drop" => {
                if let Some(v) = self.views.get_mut(&m.id) {
                    v.send(&m)?;
                }
            }
            "heartbeat" => {}
            "shutdown" | "abort" => {
                if self.closing.is_none() {
                    for (id, view) in &mut self.views {
                        view.send(&Message::new(&self.group, "drop", *id))?;
                    }
                    self.closing = Some(Instant::now());
                }
            }
            "lost" => return Err(io::Error::other("carrier window closed")),
            "command" | "focus" | "close" => self.remote.push_back(m.clone()),
            _ => return Err(crate::invalid("unknown broker event")),
        }
        if m.seq > 0 {
            self.last_rx = Instant::now();
        }
        Ok(())
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.directory.join("events.sock"));
        let _ = fs::remove_file(self.directory.join("views.sock"));
        let _ = fs::remove_dir(&self.directory);
    }
}
