use crate::control::{Action, Control};
use crate::protocol::{self as p, Message};
use crate::restore::{Display, Replies};
use crate::session::Session;
use crate::terminal::{Signals, Terminal};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const STARTUP_INSTRUCTIONS: &str = concat!(
    "kittyplex keeps this connection open; shells run in other Kitty tabs.\r\n",
    "Keep this tab open. Select a session tab to work.\r\n",
    "In a session tab, with default bindings, press Ctrl-Shift-M,\r\n",
    "release it, then press:\r\n",
    "  t / c   Create and select a session tab\r\n",
    "  v / h   Split right / below; w closes this pane\r\n",
    "  d       Detach this display (application must support Restore Display)\r\n",
    "  ?       Show help (Escape dismisses it)\r\n",
    "  n / p   Next / previous session\r\n",
    "  1-9     Select an existing session by position\r\n",
    "  q       Close the group and its sessions\r\n",
    "Escape cancels; repeat the prefix to send it. Kitty tab selection works too.\r\n",
);

struct Managed {
    session: Session,
    credit: usize,
    // A fresh route ID for every attachment; never reused as a generation.
    view: Option<u64>,
    display: Display,
    output: VecDeque<u8>,
    sent: u64,
    delivered: u64,
    replies: Replies,
    input_owed: usize,
    owner: Option<libc::pid_t>,
}
impl Managed {
    fn readable(&self) -> bool {
        (self.view.is_none() || self.output.len() < p::OUTPUT)
            // Reserve room for an entire credited user batch plus protocol replies.
            // A large paste alone must never stop reading application output.
            && self.session.input.len() < 2 * p::INPUT
    }
}
struct Remote {
    sessions: Vec<Managed>,
    terminal: Terminal,
    control: Control,
    _signals: Signals,
    group: String,
    next: u64,
    active: u64,
    reattaching: HashMap<u64, u64>,
    shell: PathBuf,
    pending: HashMap<u64, (Instant, u64)>,
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
        reattaching: HashMap::new(),
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
            serde_json::json!({"kitten":"kittyplex.py","args":["carrier",String::from_utf8(m.bytes()?).unwrap()]}),
            m.kind != "abort",
        )
    }
    fn event_loop(&mut self) -> io::Result<()> {
        let mut bootstrap = Message::new(&self.group, "bootstrap", 0);
        bootstrap.n = self.timeout.as_secs() as usize;
        bootstrap.data = std::env::var("KITTYPLEX_CONTEXT").unwrap_or_default();
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
                        "kitty integration did not become ready; install kittyplex.py and scoped authorization locally (see README)"
                    },
                ));
            }
            if self.ready && !self.closing && self.heartbeat.elapsed() >= Duration::from_secs(5) {
                self.enqueue("heartbeat", 0);
                self.heartbeat = Instant::now();
            }
            let expired: Vec<_> = self
                .pending
                .iter()
                .filter(|(_, (t, _))| t.elapsed() > self.timeout)
                .map(|(id, _)| *id)
                .collect();
            for view in expired {
                self.pending.remove(&view);
                if let Some(id) = self.reattaching.remove(&view) {
                    self.fail_restore(id, "replacement view failed to become ready");
                    self.enqueue("detach", view);
                } else {
                    self.enqueue("drop", view);
                    self.notice(view, "view failed to become ready");
                }
            }
            let mut failed = Vec::new();
            for managed in &mut self.sessions {
                managed.display.check_timeout();
                if managed.view.is_some() {
                    if let Some(error) = managed.display.error.clone() {
                        failed.push((managed.session.id, error));
                    }
                }
            }
            for (id, error) in failed {
                self.fail_restore(id, &error);
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
                    if m.readable() && !self.closing && self.messages.len() < 128 {
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
                let foreground = managed.session.foreground();
                if managed.owner.is_some_and(|owner| owner != foreground) {
                    managed.owner = None;
                    managed.display.enabled = false;
                    if managed.view.is_none() || managed.display.operation.is_some() {
                        managed.display.error =
                            Some("registered application is no longer foreground".into());
                    }
                    managed.display.operation = None;
                    managed.session.input.clear();
                }
                managed.session.flush_input()?;
                if managed.session.input.is_empty() && managed.input_owed > 0 {
                    if let Some(view) = managed.view {
                        let mut m = Message::new(&self.group, "input_credit", view);
                        m.n = managed.input_owed;
                        self.messages.push_front(m);
                    }
                    managed.input_owed = 0;
                }
                if managed.readable()
                    && !self.closing
                    && self.messages.len() < 128
                    && !managed.session.hung_up
                {
                    let mut buf = [0; p::CHUNK];
                    let limit = p::CHUNK;
                    match managed.session.read_chunk(&mut buf[..limit]) {
                        Ok(n) if n > 0 => {
                            let was_enabled = managed.display.enabled;
                            managed.display.feed(
                                &buf[..n],
                                managed.view.is_some(),
                                &mut managed.session.input,
                                &mut managed.output,
                                managed.sent,
                            );
                            if managed.display.enabled && !was_enabled {
                                managed.owner = Some(managed.session.foreground());
                            } else if !managed.display.enabled {
                                managed.owner = None;
                            }
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
                if let Some(view) = managed.view {
                    while managed.credit > 0
                        && !managed.output.is_empty()
                        && self.messages.len() < 128
                    {
                        let n = managed.credit.min(p::CHUNK).min(managed.output.len());
                        let bytes: Vec<_> = managed.output.drain(..n).collect();
                        managed.credit -= n;
                        managed.sent += n as u64;
                        let mut m = Message::new(&self.group, "output", view);
                        m.data = p::encode(&bytes);
                        self.messages.push_back(m);
                    }
                    managed.display.delivered(managed.delivered);
                }
            }
            self.round = self.round.wrapping_add(1);
            let mut dead = Vec::new();
            for m in &mut self.sessions {
                if m.session.exited()? && m.session.hung_up && m.output.is_empty() {
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
                "kitty rejected carrier (requires matching carrier v2 binaries and Kitty integration): {}",
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
                    // The carrier is raw: use CRLF and preserve queued frame ordering.
                    self.wire.extend(STARTUP_INSTRUCTIONS.as_bytes());
                }
            }
            "view_ready" => {
                if let Some((_, origin)) = self.pending.remove(&m.id) {
                    if let Some(id) = self.reattaching.remove(&m.id) {
                        if let Some(managed) = self.sessions.iter_mut().find(|s| s.session.id == id)
                        {
                            if !managed.display.enabled {
                                self.fail_restore(
                                    id,
                                    "application no longer supports Restore Display",
                                );
                                self.enqueue("drop", m.id);
                                return Ok(());
                            }
                            managed.session.resize(size(&m)?)?;
                            managed.view = Some(m.id);
                            managed.credit = p::OUTPUT;
                            managed.sent = 0;
                            managed.delivered = 0;
                            managed.output.clear();
                            managed
                                .output
                                .extend(b"Restoring display... keyboard input paused.\r\n");
                            managed.replies = Replies::default();
                            managed.input_owed = 0;
                            managed
                                .display
                                .start(&mut managed.session.input)
                                .map_err(crate::invalid)?;
                            self.active = m.id;
                        } else {
                            self.enqueue("drop", m.id);
                        }
                        return Ok(());
                    }
                    if self.closing
                        || (origin != 0 && !self.sessions.iter().any(|s| s.view == Some(origin)))
                    {
                        self.enqueue("drop", m.id);
                        return Ok(());
                    }
                    let session = Session::spawn(
                        m.id,
                        &self.shell,
                        self.control.path(),
                        self.terminal.original(),
                        size(&m)?,
                    )?;
                    self.sessions.push(Managed {
                        session,
                        credit: p::OUTPUT,
                        view: Some(m.id),
                        display: Display::default(),
                        output: VecDeque::new(),
                        sent: 0,
                        delivered: 0,
                        replies: Replies::default(),
                        input_owed: 0,
                        owner: None,
                    });
                    self.sessions.sort_by_key(|s| s.session.id);
                    self.active = m.id;
                }
            }
            "input" => {
                let bytes = m.binary()?;
                if let Some(s) = self.sessions.iter_mut().find(|s| s.view == Some(m.id)) {
                    let received = bytes.len();
                    let bytes = if s.display.operation.is_some() {
                        s.replies.feed(&bytes)
                    } else {
                        bytes
                    };
                    if s.session.input.len() + bytes.len() <= 4 * p::INPUT {
                        s.session.input.extend(bytes);
                        s.input_owed += received;
                    } else {
                        return Err(crate::invalid("input credit exceeded"));
                    }
                }
            }
            "credit" => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.view == Some(m.id)) {
                    s.credit = s
                        .credit
                        .checked_add(m.n)
                        .filter(|n| *n <= p::OUTPUT)
                        .ok_or_else(|| crate::invalid("output credit exceeded"))?;
                    s.delivered += m.n as u64;
                    s.display.delivered(s.delivered);
                }
            }
            "resize" => {
                if let Some(s) = self.sessions.iter().find(|s| s.view == Some(m.id)) {
                    s.session.resize(size(&m)?)?
                }
            }
            "focus" => {
                if self.sessions.iter().any(|s| s.view == Some(m.id)) {
                    self.active = m.id;
                }
            }
            "close" => {
                if let Some(id) = self
                    .sessions
                    .iter()
                    .find(|s| s.view == Some(m.id))
                    .map(|s| s.session.id)
                {
                    self.remove(id);
                } else if self.pending.remove(&m.id).is_some() {
                    if let Some(id) = self.reattaching.remove(&m.id) {
                        self.fail_restore(id, "replacement view closed");
                    }
                }
            }
            "command" => {
                let action = Action::parse(&m.data)?;
                if !self.sessions.iter().any(|s| s.view == Some(m.id)) {
                    return Ok(());
                }
                self.active = m.id;
                if let Err(error) = self.action(action) {
                    self.notice(m.id, &error.to_string());
                }
            }
            "heartbeat" => {}
            "closed" => self.closed = true,
            "lost" => return Err(io::Error::other("local carrier or broker closed")),
            _ => return Err(crate::invalid("unknown carrier operation")),
        }
        Ok(())
    }
    fn create(&mut self) -> io::Result<()> {
        self.create_at(0, "")
    }
    fn create_at(&mut self, origin: u64, placement: &str) -> io::Result<()> {
        if self.closing {
            return Err(crate::invalid("group is closing"));
        }
        if self.sessions.len() + self.pending.len() - self.reattaching.len() >= p::MAX_SESSIONS {
            return Err(crate::invalid("maximum 8 sessions per group"));
        }
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, (Instant::now(), origin));
        let mut message = Message::new(&self.group, "create", id);
        message.n = origin as usize;
        message.data = placement.into();
        // Keep creation ahead of bulk output without reversing rapid requests.
        let position = self
            .messages
            .iter()
            .position(|m| m.kind == "output")
            .unwrap_or(self.messages.len());
        self.messages.insert(position, message);
        Ok(())
    }
    fn remove(&mut self, id: u64) {
        self.pending.remove(&id);
        if let Some(view) = self
            .sessions
            .iter()
            .find(|s| s.session.id == id)
            .and_then(|s| s.view)
        {
            self.enqueue("drop", view);
        }
        self.sessions.retain(|s| s.session.id != id);
        let pending: Vec<_> = self
            .reattaching
            .iter()
            .filter(|(_, target)| **target == id)
            .map(|(view, _)| *view)
            .collect();
        for view in pending {
            self.reattaching.remove(&view);
            self.pending.remove(&view);
            self.enqueue("drop", view);
        }
        self.select_remaining();
        if self.sessions.is_empty() && self.pending.is_empty() {
            self.shutdown();
        }
    }
    fn select_remaining(&mut self) {
        if !self.sessions.iter().any(|s| s.view == Some(self.active)) {
            self.active = self.sessions.iter().find_map(|s| s.view).unwrap_or(0);
            if self.active != 0 {
                self.enqueue("select", self.active);
            }
        }
    }
    fn notice(&mut self, view: u64, text: &str) {
        let mut m = Message::new(&self.group, "notice", view);
        m.data = text.into();
        self.messages.push_front(m);
    }
    fn detach(&mut self, id: u64) -> io::Result<()> {
        let s = self
            .sessions
            .iter_mut()
            .find(|s| s.session.id == id)
            .ok_or_else(|| crate::invalid("session ID does not exist"))?;
        if !s.display.enabled {
            return Err(crate::invalid(
                "application has not enabled Restore Display",
            ));
        }
        let view = s
            .view
            .take()
            .ok_or_else(|| crate::invalid("session is already detached"))?;
        s.display.detach(&mut s.session.input);
        s.output.clear();
        // Already transmitted bytes retain their ordering; unsent presentation is obsolete.
        self.messages
            .retain(|m| m.id != view || !matches!(m.kind.as_str(), "output" | "input_credit"));
        self.enqueue("detach", view);
        self.select_remaining();
        Ok(())
    }
    fn fail_restore(&mut self, id: u64, error: &str) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.session.id == id) {
            let view = s.view.take();
            s.display.detach(&mut s.session.input);
            s.display.error = Some(error.into());
            s.output.clear();
            if let Some(view) = view {
                self.messages.retain(|m| {
                    m.id != view || !matches!(m.kind.as_str(), "output" | "input_credit")
                });
                self.enqueue("detach", view);
            }
            self.notice(0, &format!("session {id}: {error}"));
        }
        self.select_remaining();
    }
    fn attach(&mut self, id: u64) -> io::Result<()> {
        let s = self
            .sessions
            .iter_mut()
            .find(|s| s.session.id == id)
            .ok_or_else(|| crate::invalid("session ID does not exist"))?;
        if s.view.is_some() || self.reattaching.values().any(|target| *target == id) {
            return Err(crate::invalid("session is already attached or attaching"));
        }
        if !s.display.enabled {
            return Err(crate::invalid(
                "application has not enabled Restore Display",
            ));
        }
        s.display.error = None;
        let view = self.next;
        self.next += 1;
        self.pending.insert(view, (Instant::now(), 0));
        self.reattaching.insert(view, id);
        let mut message = Message::new(&self.group, "create", view);
        // With no split placement, n identifies the existing stable session.
        message.n = id as usize;
        self.messages.push_back(message);
        Ok(())
    }
    fn shutdown(&mut self) {
        if !self.closing {
            self.closing = true;
            self.pending.clear();
            self.reattaching.clear();
            self.sessions.clear();
            self.enqueue("shutdown", 0);
        }
    }
    fn action(&mut self, a: Action) -> io::Result<String> {
        match a {
            Action::List => {
                let mut rows =
                    vec!["ID  DISPLAY  RESTORE-DISPLAY  SHELL-PID  OPERATION".to_string()];
                for s in &self.sessions {
                    let state = if self.reattaching.values().any(|id| *id == s.session.id) {
                        "attaching"
                    } else if s.view.is_some() {
                        "attached"
                    } else {
                        "detached"
                    };
                    rows.push(format!(
                        "{}  {}  {}  {}  {}",
                        s.session.id,
                        state,
                        if s.display.enabled {
                            "supported"
                        } else {
                            "unsupported"
                        },
                        s.session.pid(),
                        s.display
                            .error
                            .as_deref()
                            .unwrap_or(if s.display.operation.is_some() {
                                "restoring"
                            } else {
                                "-"
                            })
                    ));
                }
                return Ok(rows.join("\n"));
            }
            Action::Detach(id) => {
                let id = id
                    .or_else(|| {
                        self.sessions
                            .iter()
                            .find(|s| s.view == Some(self.active))
                            .map(|s| s.session.id)
                    })
                    .ok_or_else(|| crate::invalid("no focused session"))?;
                self.detach(id)?;
            }
            Action::Attach(id) => self.attach(id)?,
            Action::New => self.create()?,
            Action::SplitRight | Action::SplitBelow => {
                if !self.sessions.iter().any(|s| s.view == Some(self.active)) {
                    return Err(crate::invalid("no ready session to split"));
                }
                let placement = if a == Action::SplitRight {
                    "right"
                } else {
                    "below"
                };
                self.create_at(self.active, placement)?;
            }
            Action::Quit => self.shutdown(),
            Action::Status => {}
            Action::Next | Action::Previous | Action::Select(_) => {
                let attached: Vec<_> = self.sessions.iter().filter_map(|s| s.view).collect();
                let count = attached.len();
                if count == 0 {
                    return Err(crate::invalid("no ready sessions"));
                }
                let current = attached
                    .iter()
                    .position(|view| *view == self.active)
                    .unwrap_or(0);
                let index = match a {
                    Action::Next => (current + 1) % count,
                    Action::Previous => (current + count - 1) % count,
                    Action::Select(n) if n > 0 && n <= count => n - 1,
                    _ => return Err(crate::invalid("session position does not exist")),
                };
                self.active = attached[index];
                self.enqueue("select", self.active);
            }
        }
        Ok(format!(
            "active={} sessions={} ids={} pids={}",
            self.sessions
                .iter()
                .filter(|s| s.view.is_some())
                .position(|s| s.view == Some(self.active))
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
        ws_xpixel: m.pixel_width,
        ws_ypixel: m.pixel_height,
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
