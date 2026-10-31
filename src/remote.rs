use crate::control::{Action, Control};
use crate::protocol::{self as p, Message};
use crate::session::Session;
use crate::terminal::{Signals, Terminal};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Managed {
    session: Session,
    credit: usize,
}
struct Remote {
    sessions: Vec<Managed>,
    terminal: Terminal,
    control: Control,
    _signals: Signals,
    group: String,
    next: u64,
    active: u64,
    shell: PathBuf,
    pending: HashMap<u64, Instant>,
    messages: VecDeque<Message>,
    wire: VecDeque<u8>,
    outstanding: VecDeque<Message>,
    seq: u64,
    received: u64,
    decoder: p::Decoder,
    last_rx: Instant,
    heartbeat: Instant,
    timeout: Duration,
    round: usize,
    last_background: u64,
    background_tokens: usize,
    background_refill: Instant,
    ready: bool,
    closing: bool,
    closed: bool,
    finish_sent: bool,
    retry: bool,
    last_retry: Instant,
}
pub fn run(timeout: Duration) -> io::Result<()> {
    let mut random = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let group = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut remote = Remote {
        sessions: Vec::new(),
        terminal: Terminal::open()?,
        control: Control::bind()?,
        _signals: Signals::install()?,
        group,
        next: 1,
        active: 0,
        shell: std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| "/bin/sh".into()),
        pending: HashMap::new(),
        messages: VecDeque::new(),
        wire: VecDeque::new(),
        outstanding: VecDeque::new(),
        seq: 0,
        received: 0,
        decoder: p::Decoder::default(),
        last_rx: Instant::now(),
        heartbeat: Instant::now(),
        timeout,
        round: 0,
        last_background: 0,
        background_tokens: 4,
        background_refill: Instant::now(),
        ready: false,
        closing: false,
        closed: false,
        finish_sent: false,
        retry: false,
        last_retry: Instant::now(),
    };
    let result = remote.event_loop();
    // Best effort cleanup even on timeout, signal, or protocol failure.
    if !remote.closed {
        let mut m = Message::new(&remote.group, "abort", 0);
        m.seq = remote.seq + 1;
        let frame = remote.rpc(&m)?;
        remote.wire.clear();
        remote.wire.extend(frame);
        let _ = remote.terminal.finish_output(&mut remote.wire);
    }
    remote.sessions.clear();
    result
}
impl Remote {
    fn enqueue(&mut self, kind: &str, id: u64) {
        let m = Message::new(&self.group, kind, id);
        if matches!(kind, "heartbeat" | "select" | "create") {
            self.messages.push_front(m);
        } else {
            self.messages.push_back(m);
        }
    }
    fn rpc(&self, m: &Message) -> io::Result<Vec<u8>> {
        p::remote_command(
            "kitten",
            serde_json::json!({"kitten":"termlab.py","args":["carrier",String::from_utf8(m.bytes()?).unwrap()]}),
            m.kind != "abort",
        )
    }
    fn event_loop(&mut self) -> io::Result<()> {
        let mut bootstrap = Message::new(&self.group, "bootstrap", 0);
        bootstrap.n = self.timeout.as_secs() as usize;
        self.messages.push_back(bootstrap);
        loop {
            if Signals::terminating() && !self.closing {
                self.shutdown();
            }
            if self.closed
                && self.messages.is_empty()
                && self.wire.is_empty()
                && self.outstanding.is_empty()
            {
                if self.finish_sent {
                    return Ok(());
                }
                // Wait for all carrier receipts, then acknowledge the broker's
                // final frame. No protocol input may follow the finish receipt.
                self.enqueue("finish", 0);
                self.finish_sent = true;
            }
            if self.last_rx.elapsed() > self.timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    if self.ready {
                        "carrier heartbeat lost; group closed"
                    } else {
                        "kitty integration did not become ready; install termlab.py and scoped authorization locally (see README)"
                    },
                ));
            }
            if self.ready && !self.closing && self.heartbeat.elapsed() >= Duration::from_secs(5) {
                self.enqueue("heartbeat", 0);
                self.heartbeat = Instant::now();
            }
            if self.pending.values().any(|t| t.elapsed() > self.timeout) {
                return Err(io::Error::other("kitty view failed to become ready"));
            }
            if self.background_refill.elapsed() >= Duration::from_millis(100) {
                self.background_tokens = 4;
                self.background_refill = Instant::now();
            }
            // Bounded RPC pipeline; receipt is not permission to send more PTY bytes.
            while self.outstanding.len() < p::PIPELINE {
                let Some(mut m) = next_message(
                    &mut self.messages,
                    &self.outstanding,
                    self.active,
                    &mut self.last_background,
                    self.background_tokens > 0,
                ) else {
                    break;
                };
                if m.kind == "output" && m.id != self.active {
                    self.background_tokens -= 1;
                }
                self.seq += 1;
                m.seq = self.seq;
                self.wire.extend(self.rpc(&m)?);
                self.outstanding.push_back(m);
            }
            if self.retry
                && self.wire.is_empty()
                && self.last_retry.elapsed() > Duration::from_millis(50)
            {
                for m in &self.outstanding {
                    self.wire.extend(self.rpc(m)?);
                }
                self.retry = false;
                self.last_retry = Instant::now();
            }
            crate::flush(&mut self.terminal, &mut self.wire)?;
            let mut fds = vec![
                crate::pollfd(
                    self.terminal.as_raw_fd(),
                    libc::POLLIN
                        | if self.wire.is_empty() {
                            0
                        } else {
                            libc::POLLOUT
                        },
                ),
                crate::pollfd(self.control.as_raw_fd(), libc::POLLIN),
            ];
            for m in &self.sessions {
                fds.push(crate::pollfd(
                    if m.session.hung_up {
                        -1
                    } else {
                        m.session.as_raw_fd()
                    },
                    if m.credit > 0 && !self.closing && self.messages.len() < 128 {
                        libc::POLLIN
                    } else {
                        0
                    } | if m.session.input.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
                ));
            }
            self.control.append_peers(&mut fds);
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 20) } < 0 {
                let e = io::Error::last_os_error();
                if !crate::transient(&e) {
                    return Err(e);
                }
            }
            if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                return Err(io::Error::other("carrier disconnected"));
            }
            if fds[0].revents & libc::POLLIN != 0 {
                let mut buf = [0; p::FRAME];
                match self.terminal.read(&mut buf) {
                    Ok(0) => return Err(io::Error::other("carrier closed")),
                    Ok(n) => {
                        for (rpc, bytes) in self.decoder.push(&buf[..n])? {
                            if rpc {
                                self.receipt(&bytes)?
                            } else {
                                self.receive(Message::parse(&bytes)?)?
                            }
                        }
                    }
                    Err(e) if crate::transient(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            let count = self.sessions.len();
            for offset in 0..count {
                let managed = &mut self.sessions[(self.round + offset) % count];
                let exited = managed.session.exited()?;
                let before = managed.session.input.len();
                managed.session.flush_input()?;
                let used = before - managed.session.input.len();
                if used > 0 {
                    let mut m = Message::new(&self.group, "input_credit", managed.session.id);
                    m.n = used;
                    self.messages.push_front(m);
                }
                if managed.credit > 0
                    && !self.closing
                    && self.messages.len() < 128
                    && !managed.session.hung_up
                {
                    let mut buf = [0; p::CHUNK];
                    let limit = managed.credit.min(p::CHUNK);
                    match managed.session.read_chunk(&mut buf[..limit]) {
                        Ok(n) if n > 0 => {
                            managed.credit -= n;
                            let mut m = Message::new(&self.group, "output", managed.session.id);
                            m.data = p::encode(&buf[..n]);
                            self.messages.push_back(m);
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock && exited => {
                            // Descendants may retain the slave after the shell exits.
                            // Drain what was already queued, then close the session.
                            managed.session.hung_up = true;
                        }
                        Err(e) if crate::transient(&e) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            self.round = self.round.wrapping_add(1);
            let mut dead = Vec::new();
            for m in &mut self.sessions {
                if m.session.exited()? && m.session.hung_up {
                    dead.push(m.session.id);
                }
            }
            for id in dead {
                self.remove(id);
            }
            self.control.accept()?;
            for (mut peer, request) in self.control.requests() {
                let result = request.and_then(|a| self.action(a));
                let response = match result {
                    Ok(s) => format!("OK {s}\n"),
                    Err(e) => format!("ERR {e}\n"),
                };
                let _ = peer.write_all(response.as_bytes());
            }
            let queued: usize = self.messages.iter().map(|m| m.data.len() + 128).sum();
            if queued + self.wire.len() > p::GROUP {
                return Err(io::Error::other("carrier queue limit exceeded"));
            }
        }
    }
    fn receipt(&mut self, bytes: &[u8]) -> io::Result<()> {
        let v: serde_json::Value = serde_json::from_slice(bytes)?;
        if v["ok"] != true {
            return Err(io::Error::other(format!(
                "kitty rejected carrier: {}",
                v["error"]
            )));
        }
        if let Some(data) = v["data"].as_str() {
            if let Some(seq) = data
                .strip_prefix("accepted:")
                .and_then(|s| s.parse::<u64>().ok())
            {
                while self.outstanding.front().is_some_and(|m| m.seq <= seq) {
                    self.outstanding.pop_front();
                }
                if self.outstanding.is_empty() {
                    self.retry = false;
                }
            } else if data == "busy" {
                // SSH is reliable: retry only an explicit busy response, never
                // merely because a receipt is slow on a high-latency link.
                self.retry = true;
            } else {
                return Err(io::Error::other(format!("invalid carrier receipt: {data}")));
            }
        }
        Ok(())
    }
    fn receive(&mut self, m: Message) -> io::Result<()> {
        if m.g != self.group {
            return Err(crate::invalid("wrong carrier group"));
        }
        if m.seq <= self.received {
            return Ok(());
        }
        if m.seq != self.received + 1 {
            return Err(crate::invalid("carrier sequence gap"));
        }
        self.received = m.seq;
        self.last_rx = Instant::now();
        if self.closing && matches!(m.kind.as_str(), "close" | "command" | "view_ready") {
            return Ok(());
        }
        match m.kind.as_str() {
            "ready" => {
                if !self.ready {
                    self.ready = true;
                    self.create()?;
                }
            }
            "view_ready" => {
                if self.pending.remove(&m.id).is_some() && !self.closing {
                    let s = Session::spawn(
                        m.id,
                        &self.shell,
                        self.control.path(),
                        self.terminal.original(),
                        size(&m)?,
                    )?;
                    self.sessions.push(Managed {
                        session: s,
                        credit: p::OUTPUT,
                    });
                    self.active = m.id;
                }
            }
            "input" => {
                let bytes = m.binary()?;
                if let Some(s) = self.sessions.iter_mut().find(|s| s.session.id == m.id) {
                    if s.session.input.len() + bytes.len() > p::INPUT {
                        return Err(crate::invalid("input credit exceeded"));
                    }
                    s.session.input.extend(bytes);
                }
            }
            "credit" => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.session.id == m.id) {
                    s.credit = s
                        .credit
                        .checked_add(m.n)
                        .filter(|n| *n <= p::OUTPUT)
                        .ok_or_else(|| crate::invalid("output credit exceeded"))?;
                }
            }
            "resize" => {
                if let Some(s) = self.sessions.iter().find(|s| s.session.id == m.id) {
                    s.session.resize(size(&m)?)?
                }
            }
            "focus" => {
                if self.sessions.iter().any(|s| s.session.id == m.id) {
                    self.active = m.id;
                }
            }
            "close" => {
                self.remove(m.id);
            }
            "command" => {
                if self.sessions.iter().any(|s| s.session.id == m.id) {
                    self.active = m.id;
                }
                // Out-of-range numbered selection is a harmless user error.
                let _ = self.action(Action::parse(&m.data)?);
            }
            "heartbeat" => {}
            "closed" => self.closed = true,
            "lost" => return Err(io::Error::other("local carrier or broker closed")),
            _ => return Err(crate::invalid("unknown carrier operation")),
        }
        Ok(())
    }
    fn create(&mut self) -> io::Result<()> {
        if self.closing {
            return Err(crate::invalid("group is closing"));
        }
        if self.sessions.len() + self.pending.len() >= p::MAX_SESSIONS {
            return Err(crate::invalid("maximum 8 sessions per group"));
        }
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, Instant::now());
        self.enqueue("create", id);
        Ok(())
    }
    fn remove(&mut self, id: u64) {
        self.pending.remove(&id);
        self.sessions.retain(|s| s.session.id != id);
        self.enqueue("drop", id);
        if self.active == id {
            if let Some(s) = self.sessions.first() {
                self.active = s.session.id;
                self.enqueue("select", self.active);
            }
        }
        if self.sessions.is_empty() && self.pending.is_empty() {
            self.shutdown();
        }
    }
    fn shutdown(&mut self) {
        if !self.closing {
            self.closing = true;
            self.pending.clear();
            self.sessions.clear();
            self.enqueue("shutdown", 0);
        }
    }
    fn action(&mut self, a: Action) -> io::Result<String> {
        match a {
            Action::New => self.create()?,
            Action::Quit => self.shutdown(),
            Action::Status => {}
            Action::Next | Action::Previous | Action::Select(_) => {
                let count = self.sessions.len();
                if count == 0 {
                    return Err(crate::invalid("no ready sessions"));
                }
                let current = self
                    .sessions
                    .iter()
                    .position(|s| s.session.id == self.active)
                    .unwrap_or(0);
                let index = match a {
                    Action::Next => (current + 1) % count,
                    Action::Previous => (current + count - 1) % count,
                    Action::Select(n) if n > 0 && n <= count => n - 1,
                    _ => return Err(crate::invalid("session position does not exist")),
                };
                self.active = self.sessions[index].session.id;
                self.enqueue("select", self.active);
            }
        }
        Ok(format!(
            "active={} sessions={} ids={} pids={}",
            self.sessions
                .iter()
                .position(|s| s.session.id == self.active)
                .map_or(0, |n| n + 1),
            self.sessions.len(),
            self.sessions
                .iter()
                .map(|s| s.session.id.to_string())
                .collect::<Vec<_>>()
                .join(","),
            self.sessions
                .iter()
                .map(|s| s.session.pid().to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
    }
}
fn size(m: &Message) -> io::Result<libc::winsize> {
    if m.rows == 0 || m.cols == 0 {
        return Err(crate::invalid("zero terminal dimensions"));
    }
    Ok(libc::winsize {
        ws_row: m.rows,
        ws_col: m.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    })
}

/// Schedule opaque output fairly, preserving each session's byte order. Reserve
/// half the RPC pipeline for foreground echo/replies. Management has priority
/// within the remaining budget; foreground traffic never resets the background cursor.
fn next_message(
    queue: &mut VecDeque<Message>,
    outstanding: &VecDeque<Message>,
    active: u64,
    last_background: &mut u64,
    background_ready: bool,
) -> Option<Message> {
    let foreground = outstanding
        .iter()
        .filter(|m| m.kind == "output" && m.id == active)
        .count();
    let other = outstanding.len() - foreground;
    let has_background = queue.iter().any(|m| m.kind == "output" && m.id != active);
    let control = queue
        .iter()
        .enumerate()
        .find(|(index, m)| match m.kind.as_str() {
            "output" => false,
            "shutdown" | "finish" => *index == 0,
            "drop" => !queue
                .iter()
                .take(*index)
                .any(|p| p.kind == "output" && p.id == m.id),
            _ => true,
        })
        .map(|(i, _)| i);
    if other < p::PIPELINE / 2 {
        if let Some(i) = control {
            return queue.remove(i);
        }
    }
    if let Some(i) = queue
        .iter()
        .position(|m| m.kind == "output" && m.id == active)
        .filter(|_| foreground < p::PIPELINE / 2 || !has_background)
    {
        return queue.remove(i);
    }
    if other >= p::PIPELINE / 2 || !background_ready {
        return None;
    }
    let ids = queue
        .iter()
        .filter(|m| m.kind == "output" && m.id != active)
        .map(|m| m.id);
    let next = ids
        .clone()
        .filter(|id| *id > *last_background)
        .min()
        .or_else(|| ids.min())?;
    *last_background = next;
    let i = queue
        .iter()
        .position(|m| m.kind == "output" && m.id == next)?;
    queue.remove(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    const G: &str = "0123456789abcdef0123456789abcdef";
    #[test]
    fn hidden_flood_cannot_fill_foreground_capacity() {
        let mut q: VecDeque<_> = (0..64).map(|_| Message::new(G, "output", 1)).collect();
        let inflight = (0..p::PIPELINE / 2)
            .map(|_| Message::new(G, "output", 1))
            .collect::<VecDeque<_>>();
        let mut cursor = 0;
        assert!(next_message(&mut q, &inflight, 8, &mut cursor, true).is_none());
        q.push_back(Message::new(G, "output", 8));
        assert_eq!(
            next_message(&mut q, &inflight, 8, &mut cursor, true)
                .unwrap()
                .id,
            8
        );
        q.push_back(Message::new(G, "heartbeat", 0));
        assert!(next_message(&mut q, &inflight, 8, &mut cursor, true).is_none());
        assert_eq!(
            next_message(
                &mut q,
                &VecDeque::from([Message::new(G, "output", 1)]),
                8,
                &mut cursor,
                true
            )
            .unwrap()
            .kind,
            "heartbeat"
        );
        let mut limited =
            VecDeque::from([Message::new(G, "output", 1), Message::new(G, "output", 8)]);
        assert_eq!(
            next_message(&mut limited, &VecDeque::new(), 8, &mut cursor, false)
                .unwrap()
                .id,
            8
        );
        assert!(next_message(&mut limited, &VecDeque::new(), 8, &mut cursor, false).is_none());
        limited.push_back(Message::new(G, "heartbeat", 0));
        assert_eq!(
            next_message(&mut limited, &VecDeque::new(), 8, &mut cursor, false)
                .unwrap()
                .kind,
            "heartbeat"
        );
        // Foreground floods must in turn leave capacity for other sessions.
        let active = (0..p::PIPELINE / 2)
            .map(|_| Message::new(G, "output", 8))
            .collect::<VecDeque<_>>();
        let mut q = VecDeque::from([Message::new(G, "output", 8), Message::new(G, "output", 1)]);
        cursor = 1;
        assert_eq!(
            next_message(&mut q, &active, 8, &mut cursor, true)
                .unwrap()
                .id,
            1
        );
    }
    #[test]
    fn background_round_robin_preserves_final_output_order() {
        let mut q = VecDeque::from([
            Message::new(G, "output", 1),
            Message::new(G, "output", 1),
            Message::new(G, "output", 2),
            Message::new(G, "drop", 1),
            Message::new(G, "shutdown", 0),
        ]);
        let inflight = VecDeque::new();
        let mut cursor = 0;
        let mut result = Vec::new();
        while let Some(m) = next_message(&mut q, &inflight, 8, &mut cursor, true) {
            result.push((m.kind, m.id));
        }
        assert_eq!(
            result,
            vec![
                ("output".into(), 1),
                ("output".into(), 2),
                ("output".into(), 1),
                ("drop".into(), 1),
                ("shutdown".into(), 0)
            ]
        );
        let mut q = VecDeque::from([
            Message::new(G, "output", 8),
            Message::new(G, "output", 2),
            Message::new(G, "output", 1),
        ]);
        let mut cursor = 1;
        assert_eq!(
            next_message(&mut q, &inflight, 8, &mut cursor, true)
                .unwrap()
                .id,
            8
        );
        assert_eq!(
            next_message(&mut q, &inflight, 8, &mut cursor, true)
                .unwrap()
                .id,
            2
        );
    }
}
