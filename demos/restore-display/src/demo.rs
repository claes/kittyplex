//! Application hooks: `detached` changes only the model; `draw` reconstructs the
//! presentation. Protocol framing and scheduling live in the adapter loop below.
use crate::{
    codec::{self, Decoder, Token},
    invalid, terminal,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::fd::FromRawFd,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static EXIT: AtomicBool = AtomicBool::new(false);
static RESIZE: AtomicBool = AtomicBool::new(false);
extern "C" fn signal(s: i32) {
    if s == libc::SIGWINCH {
        RESIZE.store(true, Ordering::Relaxed);
    } else {
        EXIT.store(true, Ordering::Relaxed);
    }
}
const HELP: &str = "Restore Display minimal image demo\n\nUsage: restore-display-demo-rust-direct [--image FILE.png] [--seed N]\n  [--protocol=auto|off] [--features text,graphics] [--log-file PATH]\n  [--restore-behavior normal|delay|refuse|stall] [--restore-delay-ms N] [--guided]\n\nKeys: i generate a new image, d arm generation on the next detach, q quit.\nWith --guided, Enter advances the walkthrough; d is disabled and detach preserves the image.\nDefault: numbered 64x64 image of colored boxes, seed 1. PNG bytes are transmitted in-band.\nWith a PNG, i/d generate a new image of colored boxes. No image decoder is needed.\nProfiles: text, graphics. Other profiles are not implemented.\nRun directly in Kitty with --protocol=off, or under restore-display-exerciser.\n";
struct Options {
    png: Option<Vec<u8>>,
    seed: u32,
    protocol: bool,
    graphics: bool,
    guided: bool,
    log: Option<File>,
    behavior: String,
    delay: u64,
}
fn options(args: &[String]) -> io::Result<Option<Options>> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(None);
    }
    let mut o = Options {
        png: None,
        seed: 1,
        protocol: true,
        graphics: true,
        guided: false,
        log: None,
        behavior: "normal".into(),
        delay: 250,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--guided" {
            o.guided = true;
            continue;
        }
        let (k, inline) = a
            .split_once('=')
            .map_or((a.as_str(), None), |(k, v)| (k, Some(v)));
        let v = if let Some(v) = inline {
            v
        } else {
            it.next()
                .ok_or_else(|| invalid(format!("missing value for {k}")))?
        };
        match k {
            "--image" => {
                let b = fs::read(v)?;
                if b.len() > 32 * 1024 * 1024 || !b.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err(invalid("expected PNG, maximum 32 MiB"));
                }
                o.png = Some(b);
            }
            "--seed" => o.seed = v.parse().map_err(|_| invalid("invalid seed"))?,
            "--protocol" => {
                o.protocol = match v {
                    "auto" => true,
                    "off" => false,
                    _ => return Err(invalid("protocol must be auto or off")),
                }
            }
            "--features" => {
                let f: Vec<_> = v.split(',').collect();
                if f.is_empty() || f.iter().any(|x| !matches!(*x, "text" | "graphics")) {
                    return Err(invalid("implemented profiles: text,graphics"));
                }
                o.graphics = f.contains(&"graphics");
            }
            "--log-file" => o.log = Some(OpenOptions::new().create(true).append(true).open(v)?),
            "--restore-behavior" => {
                if !matches!(v, "normal" | "delay" | "refuse" | "stall") {
                    return Err(invalid("invalid restore behavior"));
                }
                o.behavior = v.into();
            }
            "--restore-delay-ms" => {
                o.delay = v.parse().map_err(|_| invalid("invalid delay"))?;
                if o.delay > 60000 {
                    return Err(invalid("delay must be 0..60000"));
                }
            }
            _ => return Err(invalid(format!("unknown option {k}"))),
        }
    }
    Ok(Some(o))
}
#[derive(Default)]
struct Model {
    image: u64,
    armed: bool,
    revision: u64,
    advance: u64, // Observer-only UI events, excluded from the image model snapshot.
    png: Option<Vec<u8>>,
}
impl Model {
    fn change(&mut self) {
        if self.png.take().is_none() {
            self.image += 1;
        }
        self.revision += 1;
    }
    fn detached(&mut self) {
        if self.armed {
            self.armed = false;
            self.change();
        }
    }
    fn name(&self) -> String {
        if self.png.is_some() {
            "PNG".into()
        } else {
            format!("#{}", self.image + 1)
        }
    }
}
/// Seed and image number reproduce the same sixteen colored boxes on any display.
pub fn pixels(seed: u32, image: u64) -> Vec<u8> {
    let colors: Vec<[u8; 3]> = (0..16u64)
        .map(|index| {
            let mut value = u64::from(seed)
                .wrapping_add(image.wrapping_mul(0x9e3779b97f4a7c15))
                .wrapping_add((index + 1).wrapping_mul(0xbf58476d1ce4e5b9));
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
            value ^= value >> 31;
            [
                48 + (value as u8 % 192),
                48 + ((value >> 8) as u8 % 192),
                48 + ((value >> 16) as u8 % 192),
            ]
        })
        .collect();
    let mut pixels = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64usize {
        for x in 0..64usize {
            let rgb = if (2..14).contains(&(x % 16)) && (2..14).contains(&(y % 16)) {
                colors[(y / 16) * 4 + x / 16]
            } else {
                [24, 24, 28]
            };
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    pixels
}
fn graphics(model: &Model, seed: u32, cols: u16, rows: u16) -> Vec<u8> {
    let (format, data) = if let Some(p) = &model.png {
        ("f=100".to_string(), p.clone())
    } else {
        ("f=32,s=64,v=64".to_string(), pixels(seed, model.image))
    };
    let encoded = STANDARD.encode(data);
    let mut out = Vec::new();
    let len = encoded.len();
    for (i, chunk) in encoded.as_bytes().chunks(4096).enumerate() {
        let more = u8::from((i + 1) * 4096 < len);
        let control = if i == 0 {
            format!("a=t,t=d,{format},i=1,q=2,m={more}")
        } else {
            format!("q=2,m={more}")
        };
        out.extend_from_slice(format!("\x1b_G{control};").as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out.extend_from_slice(
        format!(
            "\x1b[5;3H\x1b_Ga=p,i=1,p=1,c={},r={},C=1,q=2;\x1b\\",
            cols.saturating_sub(4).clamp(1, 32),
            rows.saturating_sub(7).clamp(1, 16)
        )
        .as_bytes(),
    );
    out
}
fn draw(model: &Model, o: &Options, display: &str, id: u32, supported: bool) -> Vec<u8> {
    let size = terminal::size(0);
    let mut out=format!("\x1b[0m\x1b[2J\x1b[HRestore Display image demo\r\nPID {} | display {} | restore {} | image {} | armed {}\r\ni: new image  d: new image on next detach  q: quit\r\n",std::process::id(),display,id,model.name(),model.armed).into_bytes();
    if o.guided {
        out.extend_from_slice(
            b"\x1b[3;1H\x1b[2KEnter: next walkthrough step  i: new image  q: quit\r\n",
        );
    }
    if o.graphics && supported {
        out.extend_from_slice(&graphics(model, o.seed, size.ws_col, size.ws_row));
    } else {
        out.extend_from_slice(b"\x1b[5;3H[graphics unavailable or text-only profile]");
    }
    out
}
// Consume terminal responses before dispatching keys. Restore Display frames have
// already been extracted by Decoder; unrelated CSI/OSC/APC responses never become keys.
#[derive(Default)]
struct Input {
    esc: Vec<u8>,
    string: bool,
    oversized: bool,
    previous_escape: bool,
}
impl Input {
    fn feed(&mut self, raw: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in raw {
            if self.esc.is_empty() {
                if b == 27 {
                    self.esc.push(b);
                    self.previous_escape = true;
                } else {
                    out.push(vec![b]);
                }
                continue;
            }
            if self.esc.len() < 4096 {
                self.esc.push(b);
            } else {
                self.oversized = true;
            }
            if self.esc.len() == 2 {
                self.string = matches!(b, b'_' | b']' | b'P' | b'^');
                self.previous_escape = false;
                if !self.string && b != b'[' {
                    self.esc.clear();
                }
                continue;
            }
            let done = if self.string {
                (self.previous_escape && b == b'\\') || b == 7
            } else {
                (0x40..=0x7e).contains(&b)
            };
            self.previous_escape = b == 27;
            if done {
                if !self.oversized {
                    out.push(std::mem::take(&mut self.esc));
                } else {
                    self.esc.clear();
                }
                self.oversized = false;
                self.previous_escape = false;
            }
        }
        out
    }
}
fn snapshot(
    fd: Option<i32>,
    m: &Model,
    enabled: bool,
    display: &str,
    id: u32,
    operation: bool,
    count: u64,
) {
    if let Some(fd) = fd {
        let mut v=serde_json::to_vec(&json!({"schema":0,"pid":std::process::id(),"model":{"revision":m.revision,"image":m.name(),"armed":m.armed},"walkthrough":{"advance":m.advance},"protocol":{"enabled":enabled,"display":display,"operation":if operation{Some("restore")}else{None},"transaction":id,"restorations":count}})).unwrap();
        v.push(b'\n');
        let _ = terminal::write_fd(fd, &v);
    }
}
pub fn run(args: &[String]) -> io::Result<()> {
    let Some(mut o) = options(args)? else {
        return Ok(());
    };
    let _raw = terminal::RawTerminal::new(0)?;
    let old_stdout = unsafe { libc::fcntl(1, libc::F_GETFL) };
    terminal::nonblock(1)?;
    struct Flags(i32);
    impl Drop for Flags {
        fn drop(&mut self) {
            unsafe {
                libc::fcntl(1, libc::F_SETFL, self.0);
            }
        }
    }
    let _flags = Flags(old_stdout);
    for s in [libc::SIGTERM, libc::SIGINT, libc::SIGWINCH] {
        unsafe {
            libc::signal(s, signal as *const () as usize);
        }
    }
    let diagnostic = std::env::var("RESTORE_DISPLAY_DEMO_STATE_FD")
        .ok()
        .and_then(|s| s.parse::<i32>().ok());
    if let Some(fd) = diagnostic {
        terminal::nonblock(fd)?;
    }
    let mut model = Model {
        png: o.png.take(),
        ..Model::default()
    };
    let mut decoder = Decoder::default();
    let mut input = Input::default();
    let mut queue = VecDeque::new();
    let mut stdout = unsafe { std::fs::File::from_raw_fd(libc::dup(1)) };
    let mut enabled = false;
    let mut display = "initial";
    let mut last = 0;
    let mut pending: Option<(u32, Instant)> = None;
    let mut probe: Option<(Option<u32>, Instant)> = None;
    let mut support = !o.graphics;
    let mut completion = None;
    let mut restorations = 0;
    let start = Instant::now();
    let mut initial = !o.protocol;
    let mut exiting: Option<Instant> = None;
    let mut dirty = false;
    if o.protocol {
        queue.extend(codec::frame("action=enable,version=0"));
    }
    snapshot(
        diagnostic,
        &model,
        enabled,
        display,
        last,
        false,
        restorations,
    );
    loop {
        let mut fds = [
            terminal::event(0, false),
            terminal::event(1, !queue.is_empty()),
        ];
        terminal::poll(&mut fds, 20)?;
        if fds[0].revents & libc::POLLIN != 0 {
            let mut b = [0; 8192];
            let n = terminal::read(0, &mut b)?;
            if n == 0 {
                break;
            }
            for token in decoder.feed(&b[..n]) {
                match token {
                    Token::Frame(f) => {
                        if let Some(log) = o.log.as_mut() {
                            writeln!(log, "input {f:?}")?;
                        }
                        match codec::get(&f, "action") {
                            "enable" if codec::get(&f, "status") == "accepted" => {
                                enabled = true;
                                display = if codec::get(&f, "display") == "detached" {
                                    "detached"
                                } else {
                                    "attached"
                                };
                                initial = display == "attached";
                            }
                            "enable" if codec::get(&f, "status") == "unavailable" => {
                                initial = true;
                            }
                            "state" if enabled => {
                                display = "detached";
                                pending = None;
                                probe = None;
                                model.detached();
                                dirty = false;
                            }
                            "restore" if enabled => {
                                let id = codec::get(&f, "id").parse().unwrap();
                                if id > last {
                                    last = id;
                                    display = "attached";
                                    pending = Some((
                                        id,
                                        Instant::now()
                                            + Duration::from_millis(if o.behavior == "delay" {
                                                o.delay
                                            } else {
                                                0
                                            }),
                                    ));
                                    probe = None;
                                }
                            }
                            "disable" if codec::get(&f, "status") == "accepted" => {
                                enabled = false;
                                if exiting.is_some() {
                                    queue.extend(
                                        b"\x1b_Ga=d,d=I,i=1,q=2;\x1b\\\x1b[0m\x1b[?25h\x1b[?1049l",
                                    );
                                    exiting = Some(Instant::now());
                                }
                            }
                            _ => {}
                        }
                    }
                    Token::Bytes(raw) => {
                        for event in input.feed(&raw) {
                            if let Some(log) = o.log.as_mut() {
                                writeln!(
                                    log,
                                    "terminal {event:?}, probe={probe:?}, elapsed={:?}",
                                    start.elapsed()
                                )?;
                            }
                            if let Some((id, _)) = probe {
                                if event.starts_with(b"\x1b_Gi=2147483647;") {
                                    support = event.windows(3).any(|w| w == b";OK");
                                    probe = None;
                                    queue.extend(draw(&model, &o, display, last, support));
                                    if let Some(id) = id {
                                        queue.extend(codec::frame(&format!(
                                            "action={},id={id}",
                                            if support { "complete" } else { "unavailable" }
                                        )));
                                        if support {
                                            completion = Some(id);
                                        }
                                    }
                                    continue;
                                }
                                if event.starts_with(b"\x1b[") && event.ends_with(b"c") {
                                    probe = None;
                                    support = false;
                                    queue.extend(draw(&model, &o, display, last, false));
                                    if let Some(id) = id {
                                        queue.extend(codec::frame(&format!(
                                            "action=unavailable,id={id}"
                                        )));
                                    }
                                    continue;
                                }
                            }
                            if event.len() == 1 && exiting.is_none() {
                                match event[0] {
                                    b'i' if display != "detached" => {
                                        model.change();
                                        dirty = true;
                                    }
                                    b'd' if display != "detached" && !o.guided => {
                                        model.armed = !model.armed;
                                        model.revision += 1;
                                        dirty = true;
                                    }
                                    b'\r' | b'\n' if o.guided && display != "detached" => {
                                        model.advance += 1;
                                    }
                                    b'q' | 3 => {
                                        EXIT.store(true, Ordering::Relaxed);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Token::Invalid => {}
                }
            }
            snapshot(
                diagnostic,
                &model,
                enabled,
                display,
                last,
                pending.is_some() || probe.is_some() || completion.is_some(),
                restorations,
            );
        }
        if o.protocol
            && !enabled
            && display == "initial"
            && start.elapsed() > Duration::from_secs(1)
            && probe.is_none()
        {
            initial = true;
            display = "attached";
        }
        if EXIT.swap(false, Ordering::Relaxed) && exiting.is_none() {
            pending = None;
            probe = None;
            if enabled {
                queue.extend(codec::frame("action=disable"));
                exiting = Some(Instant::now() + Duration::from_secs(5));
            } else {
                queue.extend(b"\x1b_Ga=d,d=I,i=1,q=2;\x1b\\\x1b[0m\x1b[?25h\x1b[?1049l");
                exiting = Some(Instant::now());
            }
        }
        if exiting.is_none() {
            if RESIZE.swap(false, Ordering::Relaxed) && display != "detached" {
                dirty = true;
            }
            if let Some((id, when)) = pending {
                if when <= Instant::now() && queue.is_empty() {
                    pending = None;
                    dirty = false;
                    match o.behavior.as_str() {
                        "refuse" => {
                            queue.extend(codec::frame(&format!("action=unavailable,id={id}")))
                        }
                        "stall" => {}
                        _ => {
                            queue.extend(codec::frame(&format!("action=begin,id={id}")));
                            initial = true;
                            probe = Some((Some(id), Instant::now() + Duration::from_secs(3)));
                        }
                    }
                }
            }
            if initial && queue.is_empty() {
                initial = false;
                if display == "initial" {
                    display = "attached";
                }
                queue.extend(b"\x1b[?1049h\x1b[?25l");
                let id = probe.and_then(|p| p.0);
                if o.graphics {
                    queue.extend(b"\x1b_Gi=2147483647,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c");
                    probe = Some((id, Instant::now() + Duration::from_secs(3)));
                } else {
                    probe = None;
                    queue.extend(draw(&model, &o, display, last, true));
                    if let Some(id) = id {
                        queue.extend(codec::frame(&format!("action=complete,id={id}")));
                        completion = Some(id);
                    }
                }
            }
            if let Some((id, deadline)) = probe {
                if Instant::now() > deadline {
                    probe = None;
                    support = false;
                    queue.extend(draw(&model, &o, display, last, false));
                    if let Some(id) = id {
                        queue.extend(codec::frame(&format!("action=unavailable,id={id}")));
                    }
                }
            }
            if dirty && probe.is_none() && pending.is_none() && queue.is_empty() {
                queue.extend(draw(&model, &o, display, last, support));
                dirty = false;
            }
        }
        terminal::drain(&mut stdout, &mut queue)?;
        if queue.is_empty() {
            if completion.take().is_some() {
                restorations += 1;
                snapshot(
                    diagnostic,
                    &model,
                    enabled,
                    display,
                    last,
                    false,
                    restorations,
                );
            }
            if let Some(deadline) = exiting {
                if !enabled || Instant::now() > deadline {
                    break;
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_image_sequence() {
        let images: std::collections::HashSet<Vec<u8>> =
            (0..64).map(|number| pixels(1, number)).collect();
        assert_eq!(
            images.len(),
            64,
            "new images must not alternate between two patterns"
        );
        assert_eq!(pixels(1, 7), pixels(1, 7));
        assert_ne!(pixels(1, 7), pixels(2, 7));
        let first = pixels(1, 0);
        assert_eq!(first.len(), 16384);
        assert_eq!(&first[..4], &[24, 24, 28, 255]);
        assert!(first.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }
    #[test]
    fn detach_preserves_the_current_image_without_explicit_arming() {
        let mut model = Model::default();
        for _ in 0..7 {
            model.change();
        }
        assert_eq!(model.name(), "#8");
        let before = pixels(1, model.image);
        model.detached();
        assert_eq!(model.name(), "#8");
        assert_eq!(before, pixels(1, model.image));
    }
    #[test]
    fn detached_changes_only_model() {
        let mut m = Model {
            armed: true,
            ..Model::default()
        };
        m.detached();
        assert_eq!(m.name(), "#2");
        assert!(!m.armed);
    }
    #[test]
    fn oversized_terminal_reply_does_not_become_keys() {
        let mut input = Input::default();
        let bytes = [b"\x1b_G".as_slice(), &vec![b'q'; 10000], b"\x1b\\i"].concat();
        assert_eq!(input.feed(&bytes), vec![vec![b'i']]);
    }
}
