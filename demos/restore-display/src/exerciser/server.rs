use super::*;
use crate::{
    codec::{self, Decoder, Token},
    terminal,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::UnixListener,
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
const LIMIT: usize = 1024 * 1024;
const CAPTURE: u64 = 64 * 1024 * 1024;
struct Peer {
    socket: UnixStream,
    input: Vec<u8>,
    output: VecDeque<u8>,
    viewer: bool,
    inflight: usize,
    close: bool,
}
impl Peer {
    fn send(&mut self, v: &Value) {
        if self.output.len() > LIMIT {
            self.close = true;
            return;
        }
        self.output.extend(serde_json::to_vec(v).unwrap());
        self.output.push_back(b'\n');
    }
}
struct Capture {
    file: File,
    offset: u64,
    truncated: bool,
}
impl Capture {
    fn write(&mut self, b: &[u8]) -> io::Result<()> {
        let n = (CAPTURE.saturating_sub(self.offset) as usize).min(b.len());
        self.file.write_all(&b[..n])?;
        self.offset += b.len() as u64;
        self.truncated |= n < b.len();
        Ok(())
    }
}
struct State {
    dir: PathBuf,
    master: File,
    child: Child,
    peers: Vec<Peer>,
    to_app: VecDeque<u8>,
    decoder: Decoder,
    enabled: bool,
    support: bool,
    attached: bool,
    headless: bool,
    gate: bool,
    begun: bool,
    next: u32,
    current: Option<u32>,
    status: String,
    display: Value,
    snapshot: Value,
    snapshot_buf: Vec<u8>,
    capture_in: Capture,
    capture_out: Capture,
    events: File,
    started: Instant,
    deadline: Option<Instant>,
    valid: bool,
    stop: bool,
    stop_at: Option<Instant>,
    flow_stopped: bool,
    next_write: Instant,
    request_timeout: Duration,
    discovery_timeout: Duration,
    pty_eof: bool,
    cols: u16,
    rows: u16,
    fault: Value,
    last_input: Vec<u8>,
    delay: VecDeque<(Instant, Vec<u8>)>,
    disconnect_at: Option<u64>,
}
impl State {
    fn trace(&mut self, event: &str, data: Value) {
        let _ = writeln!(
            self.events,
            "{}",
            json!({"elapsed_ms":self.started.elapsed().as_millis(),"event":event,"id":self.current,"data":data,"input_offset":self.capture_in.offset,"output_offset":self.capture_out.offset})
        );
    }
    fn send_app(&mut self, b: &[u8]) {
        self.last_input = b.to_vec();
        if self.fault["drop_next_response"].as_bool() == Some(true) {
            self.fault["drop_next_response"] = json!(false);
            self.trace("dropped_input", json!(STANDARD.encode(b)));
            return;
        }
        if self.to_app.len() + b.len() > LIMIT {
            self.trace("input_overflow", Value::Null);
            self.status = "input_overflow".into();
            self.current = None;
            self.gate = true;
            return;
        }
        self.to_app.extend(b);
    }
    fn send_frame(&mut self, body: &str) {
        self.trace("protocol_input", json!(body));
        self.send_app(&codec::frame(body));
    }
    fn inspect(&self) -> Value {
        json!({"schema":0,"pid":self.child.id(),"process_start":self.display_start(),"controller_pid":std::process::id(),"enabled":self.enabled,"display_status":if self.attached{"attached"}else{"detached"},"display":self.display,"transaction":self.current,"last_transaction":self.next,"status":self.status,"begun":self.begun,"valid_restore_order":self.valid,"gated":self.gate,"request_timeout_ms":self.request_timeout.as_millis(),"flow_stopped":self.flow_stopped,"model":self.snapshot.get("model"),"snapshot":self.snapshot,"capture_complete":!self.capture_in.truncated&&!self.capture_out.truncated,"input_offset":self.capture_in.offset,"output_offset":self.capture_out.offset})
    }
    fn display_start(&self) -> String {
        fs::read_to_string(format!("/proc/{}/stat", self.child.id()))
            .ok()
            .and_then(|s| {
                s.rsplit_once(") ")
                    .and_then(|(_, x)| x.split_whitespace().nth(19).map(str::to_string))
            })
            .unwrap_or_default()
    }
    fn detach(&mut self) {
        self.attached = false;
        self.headless = false;
        self.gate = true;
        self.begun = false;
        self.current = None;
        self.deadline = None;
        self.status = "detached".into();
        for p in &mut self.peers {
            if p.viewer {
                p.send(&json!({"event":"detach"}));
                p.close = true;
            }
        }
        if self.enabled {
            self.send_frame("action=state,display=detached");
        }
        self.trace("detached", Value::Null);
    }
    fn restore(&mut self) -> io::Result<Value> {
        if !self.attached || !self.enabled {
            return Err(invalid(
                "restore needs an attached display and enabled application",
            ));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("transaction numbers exhausted"))?;
        self.current = Some(self.next);
        self.gate = true;
        self.begun = false;
        self.status = "restoring".into();
        self.trace("gates_closed", Value::Null);
        terminal::resize(
            self.master.as_raw_fd(),
            self.cols,
            self.rows,
            (
                self.display["xpixel"].as_u64().unwrap_or(0) as u16,
                self.display["ypixel"].as_u64().unwrap_or(0) as u16,
            ),
        )?;
        self.trace("TIOCSWINSZ", json!([self.cols, self.rows]));
        self.send_frame(&format!("action=restore,id={}", self.next));
        self.deadline = Some(Instant::now() + self.request_timeout);
        Ok(json!({"id":self.next}))
    }
    fn protocol(&mut self, f: codec::Fields) {
        self.trace("protocol_output", json!(f));
        match codec::get(&f, "action") {
            "enable" if codec::get(&f, "status").is_empty() => {
                if !self.support {
                    return;
                }
                if codec::get(&f, "version") == "0" && !self.enabled {
                    self.enabled = true;
                    self.send_frame(&format!(
                        "action=enable,status=accepted,display={}",
                        if self.attached {
                            "attached"
                        } else {
                            "detached"
                        }
                    ));
                    // Attached startup may draw normally, unlike reattachment.
                    self.gate = false;
                    self.status = "enabled".into();
                } else {
                    self.send_frame("action=enable,status=unavailable");
                }
            }
            "disable" if codec::get(&f, "status").is_empty() => {
                self.enabled = false;
                self.current = None;
                self.deadline = None;
                self.status = "disabled".into();
                self.gate = false;
                self.send_frame("action=disable,status=accepted");
            }
            "begin" | "complete" | "unavailable" => {
                let id = codec::get(&f, "id").parse::<u32>().ok();
                if id != self.current || self.status != "restoring" {
                    self.trace("ignored_stale", json!(f));
                    return;
                }
                match codec::get(&f, "action") {
                    "begin" if !self.begun => {
                        self.begun = true;
                    }
                    "complete" if self.begun => {
                        self.status = "delivering".into();
                        self.deadline = None;
                    }
                    "unavailable" => {
                        self.status = "unavailable".into();
                        self.begun = false;
                        self.deadline = None;
                    }
                    _ => {
                        self.status = "invalid_order".into();
                        self.valid = false;
                        self.begun = false;
                        self.deadline = None;
                    }
                }
            }
            _ => {}
        }
    }
    fn output(&mut self, b: &[u8]) {
        for token in self.decoder.feed(b) {
            match token {
                Token::Frame(f) => self.protocol(f),
                Token::Invalid => self.trace("invalid_frame", Value::Null),
                Token::Bytes(bytes) => {
                    if self.attached && (!self.gate || self.begun) {
                        for p in &mut self.peers {
                            if p.viewer && !p.close {
                                p.inflight += 1;
                                p.send(&json!({"event":"output","data":STANDARD.encode(&bytes)}));
                            }
                        }
                    }
                }
            }
        }
    }
    fn command(&mut self, index: usize, v: Value) -> io::Result<Value> {
        match value(&v, "op")? {
            "inspect" | "state" => Ok(self.inspect()),
            "viewer" => {
                if self.attached {
                    return Err(invalid("detach the existing display first"));
                }
                self.attached = true;
                self.headless = false;
                self.gate = self.enabled;
                self.display = v.clone();
                self.cols = v["cols"].as_u64().unwrap_or(80) as u16;
                self.rows = v["rows"].as_u64().unwrap_or(24) as u16;
                self.peers[index].viewer = true;
                self.trace("display_attached", v);
                Ok(json!({"event":"ready"}))
            }
            "delivery" if self.peers[index].viewer => {
                self.peers[index].inflight = self.peers[index]
                    .inflight
                    .saturating_sub(v["count"].as_u64().unwrap_or(0) as usize);
                Ok(json!({"ok": true}))
            }
            "headless" => {
                if self.attached {
                    return Err(invalid("detach the existing display first"));
                }
                self.attached = true;
                self.headless = true;
                self.gate = self.enabled;
                self.cols = 80;
                self.rows = 24;
                self.display = json!({"kind":"headless","identity":v["identity"],"fresh":false});
                Ok(json!({"simulated":true}))
            }
            "detach" | "lose-display" => {
                self.detach();
                Ok(json!({"ok":true}))
            }
            "restore" => self.restore(),
            "resize" => {
                self.gate = true;
                let cols = v["cols"].as_u64().ok_or_else(|| invalid("cols required"))?;
                let rows = v["rows"].as_u64().ok_or_else(|| invalid("rows required"))?;
                if cols == 0 || rows == 0 || cols > 65535 || rows > 65535 {
                    return Err(invalid("invalid dimensions"));
                }
                self.cols = cols as u16;
                self.rows = rows as u16;
                self.display["xpixel"] = v["pixel_width"].clone();
                self.display["ypixel"] = v["pixel_height"].clone();
                if self.enabled && self.attached {
                    self.restore()
                } else {
                    terminal::resize(self.master.as_raw_fd(), self.cols, self.rows, (0, 0))?;
                    Ok(json!({"ok":true}))
                }
            }
            "cancel" => {
                self.current = None;
                self.status = "cancelled".into();
                self.begun = false;
                self.gate = true;
                self.deadline = None;
                Ok(json!({"ok":true}))
            }
            "input" | "bytes" => {
                self.trace("terminal_input", v.clone());
                let data = STANDARD
                    .decode(value(&v, "data")?)
                    .map_err(|_| invalid("invalid base64"))?;
                let replies = v["op"] == "bytes";
                if (replies && !self.peers[index].viewer)
                    || (self.peers[index].viewer && self.peers[index].close)
                {
                    return Err(invalid("stale or unregistered display input"));
                }
                if !self.attached {
                    return Err(invalid("no display attached"));
                }
                if self.gate && !replies {
                    return Err(invalid("user input is gated"));
                }
                if self.gate && replies {
                    // The viewer only labels complete terminal replies as bytes.
                    if !(data.starts_with(b"\x1b_G")
                        || data.starts_with(b"\x1b[")
                        || data.starts_with(b"\x1b]"))
                    {
                        return Err(invalid("user input is gated"));
                    }
                }
                if self.to_app.len() + data.len() > LIMIT {
                    return Err(invalid("application input queue full"));
                }
                self.send_app(&data);
                Ok(json!({"ok":true}))
            }
            "fault" => {
                let kind = value(&v, "kind")?;
                match kind {
                    "duplicate_last_protocol" => self.to_app.extend(self.last_input.clone()),
                    "stale_transaction" => {
                        self.send_frame("action=restore,id=1");
                    }
                    "malformed_frame" => {
                        self.send_app(b"\x1bP@restore-display;action=restore,id=bad\x1b\\");
                    }
                    "unsupported_version" => {
                        self.send_app(&codec::frame("action=enable,status=unavailable"));
                    }
                    "disconnect_after_output_bytes" => {
                        self.disconnect_at = Some(
                            self.capture_out.offset
                                + v["value"]
                                    .as_u64()
                                    .ok_or_else(|| invalid("fault needs value"))?,
                        );
                    }
                    "fragment" | "delay_input_ms" | "delay_output_ms" => {
                        let n = v["value"]
                            .as_u64()
                            .ok_or_else(|| invalid("fault needs nonnegative value"))?;
                        if n > 60000 || kind == "fragment" && n == 0 {
                            return Err(invalid("invalid fault value"));
                        }
                        self.fault[kind] = json!(n);
                    }
                    "drop_next_response" => self.fault[kind] = json!(true),
                    _ => return Err(invalid("unknown fault")),
                }
                self.trace("fault", v);
                Ok(json!({"ok":true}))
            }
            "stop" => {
                self.stop = true;
                self.stop_at = Some(Instant::now() + Duration::from_secs(2));
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGTERM);
                }
                Ok(json!({"ok":true}))
            }
            _ => Err(invalid("unknown command")),
        }
    }
}
fn file(dir: &Path, name: &str) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name))
}
pub fn serve(
    dir: &Path,
    program: &[String],
    support: bool,
    diagnostics: bool,
    request_timeout: Duration,
    discovery_timeout: Duration,
) -> io::Result<()> {
    let listener = UnixListener::bind(dir.join("control.sock"))?;
    fs::set_permissions(dir.join("control.sock"), fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut m = -1;
    let mut s = -1;
    if unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let master = unsafe { File::from_raw_fd(m) };
    let slave = unsafe { File::from_raw_fd(s) };
    for fd in [m, s] {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    terminal::nonblock(m)?;
    terminal::resize(m, 80, 24, (0, 0))?;
    let mut pipe = [-1; 2]; // two descriptors, CLOEXEC until explicitly duplicated
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let observer = unsafe { File::from_raw_fd(pipe[0]) };
    let writer = unsafe { File::from_raw_fd(pipe[1]) };
    let mut cmd = Command::new(&program[0]);
    cmd.args(&program[1..])
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave);
    if diagnostics {
        cmd.env("RESTORE_DISPLAY_DEMO_STATE_FD", "198");
    }
    let writefd = writer.as_raw_fd();
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if diagnostics && libc::dup2(writefd, 198) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    // Command retains its configured Stdio handles for possible reuse. Close the
    // parent copies of the slave, otherwise PTY EOF is never observable.
    drop(cmd);
    drop(writer);
    let mut state = State {
        dir: dir.into(),
        master,
        child,
        peers: Vec::new(),
        to_app: VecDeque::new(),
        decoder: Decoder::default(),
        enabled: false,
        support,
        attached: false,
        headless: false,
        gate: true,
        begun: false,
        next: 0,
        current: None,
        status: "starting".into(),
        display: Value::Null,
        snapshot: Value::Null,
        snapshot_buf: Vec::new(),
        capture_in: Capture {
            file: file(dir, "input.raw")?,
            offset: 0,
            truncated: false,
        },
        capture_out: Capture {
            file: file(dir, "output.raw")?,
            offset: 0,
            truncated: false,
        },
        events: file(dir, "events.jsonl")?,
        started: Instant::now(),
        deadline: None,
        valid: true,
        stop: false,
        stop_at: None,
        flow_stopped: false,
        next_write: Instant::now(),
        request_timeout,
        discovery_timeout,
        pty_eof: false,
        cols: 80,
        rows: 24,
        fault: json!({}),
        last_input: Vec::new(),
        delay: VecDeque::new(),
        disconnect_at: None,
    };
    let mut packet = 1;
    let packet_mode = unsafe { libc::ioctl(m, libc::TIOCPKT, &mut packet) } == 0;
    fs::write(
        dir.join("ready.json"),
        serde_json::to_vec(
            &json!({"controller_pid":std::process::id(),"pid":state.child.id(),"started":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()}),
        )?,
    )?;
    loop {
        let mut fds = vec![
            terminal::event(listener.as_raw_fd(), false),
            terminal::event(
                if state.pty_eof { -1 } else { m },
                !state.to_app.is_empty() && Instant::now() >= state.next_write,
            ),
            terminal::event(observer.as_raw_fd(), false),
        ];
        for p in &state.peers {
            fds.push(terminal::event(p.socket.as_raw_fd(), !p.output.is_empty()));
        }
        fds[1].events |= libc::POLLPRI;
        terminal::poll(&mut fds, 10)?;
        if fds[0].revents & libc::POLLIN != 0 {
            while let Ok((socket, _)) = listener.accept() {
                if state.peers.len() >= 64 {
                    drop(socket);
                    continue;
                }
                socket.set_nonblocking(true)?;
                state.peers.push(Peer {
                    socket,
                    input: Vec::new(),
                    output: VecDeque::new(),
                    viewer: false,
                    inflight: 0,
                    close: false,
                });
            }
        }
        if fds[1].revents & (libc::POLLIN | libc::POLLPRI | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut b = [0; 65536];
            match terminal::read(m, &mut b) {
                Ok(0) => state.pty_eof = true,
                Ok(n) => {
                    if packet_mode && b[0] != 0 {
                        state.flow_stopped = if b[0] & 4 != 0 {
                            true
                        } else if b[0] & 8 != 0 {
                            false
                        } else {
                            state.flow_stopped
                        };
                        state.trace("pty_packet", json!(b[0]));
                        if b[0] & 3 != 0 && state.current.is_some() {
                            state.current = None;
                            state.status = "stream_disrupted".into();
                            state.gate = true;
                            state.begun = false;
                        }
                    } else {
                        let data = &b[usize::from(packet_mode)..n];
                        state.capture_out.write(data)?;
                        if let Some(ms) = state.fault["delay_output_ms"].as_u64().filter(|n| *n > 0)
                        {
                            if state.delay.iter().map(|(_, b)| b.len()).sum::<usize>() + data.len()
                                > LIMIT
                            {
                                state.delay.clear();
                                state.decoder = Decoder::default();
                                state.current = None;
                                state.status = "delay_overflow".into();
                                state.gate = true;
                                state.begun = false;
                                state.trace("delay_overflow", Value::Null);
                            }
                            state.delay.push_back((
                                Instant::now() + Duration::from_millis(ms),
                                data.to_vec(),
                            ));
                        } else {
                            state.output(data);
                        }
                        if state
                            .disconnect_at
                            .is_some_and(|at| state.capture_out.offset >= at)
                        {
                            state.disconnect_at = None;
                            state.detach();
                        }
                    }
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => state.pty_eof = true,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        while state
            .delay
            .front()
            .is_some_and(|(t, _)| *t <= Instant::now())
        {
            let (_, b) = state.delay.pop_front().unwrap();
            state.output(&b);
        }
        if fds[2].revents & libc::POLLIN != 0 {
            let mut b = [0; 8192];
            if let Ok(n) = terminal::read(observer.as_raw_fd(), &mut b) {
                state.snapshot_buf.extend_from_slice(&b[..n]);
                while let Some(end) = state.snapshot_buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = state.snapshot_buf.drain(..=end).collect();
                    if let Ok(v) = serde_json::from_slice(&line) {
                        state.snapshot = v;
                    }
                }
                if state.snapshot_buf.len() > LIMIT {
                    state.snapshot_buf.clear();
                }
            }
        }
        let old_len = fds.len() - 3;
        for i in 0..old_len {
            if i >= state.peers.len() {
                break;
            }
            if fds[i + 3].revents & libc::POLLIN != 0 {
                let mut b = [0; 65536];
                match terminal::read(state.peers[i].socket.as_raw_fd(), &mut b) {
                    Ok(0) => state.peers[i].close = true,
                    Ok(n) => state.peers[i].input.extend_from_slice(&b[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => state.peers[i].close = true,
                }
                if state.peers[i].input.len() > LIMIT {
                    state.peers[i].close = true;
                }
                while let Some(end) = state.peers[i].input.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = state.peers[i].input.drain(..=end).collect();
                    let result = serde_json::from_slice::<Value>(&line)
                        .map_err(io::Error::from)
                        .and_then(|v| state.command(i, v));
                    let reply = result.unwrap_or_else(|e| json!({"error":e.to_string()}));
                    state.peers[i].send(&reply);
                }
            }
        }
        if !state.to_app.is_empty() {
            let chunk = state.fault["fragment"].as_u64().unwrap_or(65536) as usize;
            let n = chunk.min(state.to_app.as_slices().0.len());
            if !state.pty_eof && Instant::now() >= state.next_write {
                match terminal::write_fd(m, &state.to_app.as_slices().0[..n]) {
                    Ok(n) => {
                        let b: Vec<_> = state.to_app.drain(..n).collect();
                        state.capture_in.write(&b)?;
                        state.next_write = Instant::now()
                            + Duration::from_millis(
                                state.fault["delay_input_ms"].as_u64().unwrap_or(0),
                            );
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        for p in &mut state.peers {
            if !p.output.is_empty() && terminal::drain(&mut p.socket, &mut p.output).is_err() {
                p.close = true;
                p.output.clear();
            }
        }
        if state.status == "delivering"
            && state
                .peers
                .iter()
                .filter(|p| p.viewer && !p.close)
                .all(|p| p.output.is_empty() && p.inflight == 0)
        {
            state.status = "complete".into();
            state.gate = false;
            state.trace("delivery_complete", Value::Null);
        }
        let lost =
            state.attached && !state.headless && state.peers.iter().any(|p| p.viewer && p.close);
        if lost {
            state.detach();
        }
        state.peers.retain(|p| !p.close || !p.output.is_empty());
        if !state.enabled
            && state.status == "starting"
            && state.started.elapsed() > state.discovery_timeout
        {
            state.status = "unsupported".into();
        }
        if state.deadline.is_some_and(|d| Instant::now() > d) {
            state.deadline = None;
            state.status = "timeout".into();
            state.current = None;
            state.begun = false;
            state.gate = true;
            state.trace("timeout", Value::Null);
        }
        if state.child.try_wait()?.is_some()
            && state.pty_eof
            && state
                .peers
                .iter()
                .filter(|p| p.viewer && !p.close)
                .all(|p| p.output.is_empty() && p.inflight == 0)
        {
            break;
        }
        if state.pty_eof && state.stop_at.is_none() {
            state.stop_at = Some(Instant::now() + Duration::from_secs(2));
        }
        if state.stop_at.is_some_and(|t| Instant::now() > t) {
            unsafe {
                if state.child.try_wait()?.is_none() {
                    libc::kill(-(state.child.id() as i32), libc::SIGKILL);
                }
            }
            let _ = state.child.wait();
            break;
        }
    }
    let _ = fs::write(
        state.dir.join("final.json"),
        serde_json::to_vec_pretty(&state.inspect())?,
    );
    let _ = fs::remove_file(state.dir.join("control.sock"));
    let _ = state.child.try_wait();
    Ok(())
}

impl Drop for State {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
        let _ = fs::remove_file(self.dir.join("control.sock"));
    }
}
