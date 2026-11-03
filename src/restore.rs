//! Controller-side Restore Display v0. The demo and runtime share the wire codec.
#[path = "../demos/restore-display/src/codec.rs"]
pub mod codec;
use codec::{Decoder, Token, frame, get};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Display {
    decoder: Decoder,
    pub enabled: bool,
    next: u32,
    pub operation: Option<Operation>,
    pub error: Option<String>,
}
pub struct Operation {
    id: u32,
    begun: bool,
    pub complete: bool,
    pub target: u64,
    started: Instant,
}
impl Display {
    pub fn start(&mut self, input: &mut VecDeque<u8>) -> Result<(), &'static str> {
        self.next = self.next.checked_add(1).ok_or("restore IDs exhausted")?;
        self.error = None;
        self.operation = Some(Operation {
            id: self.next,
            begun: false,
            complete: false,
            target: 0,
            started: Instant::now(),
        });
        input.extend(frame(&format!("action=restore,id={}", self.next)));
        Ok(())
    }
    pub fn detach(&mut self, input: &mut VecDeque<u8>) {
        self.operation = None;
        if self.enabled {
            input.extend(frame("action=state,display=detached"));
        }
    }
    pub fn feed(
        &mut self,
        bytes: &[u8],
        attached: bool,
        input: &mut VecDeque<u8>,
        output: &mut VecDeque<u8>,
        sent: u64,
    ) {
        for token in self.decoder.feed(bytes) {
            match token {
                Token::Bytes(bytes) => {
                    if attached && self.operation.as_ref().is_none_or(|o| o.begun) {
                        output.extend(bytes);
                    }
                }
                Token::Frame(f) => match get(&f, "action") {
                    "enable" if get(&f, "status").is_empty() => {
                        if get(&f, "version") == "0" && !self.enabled {
                            self.enabled = true;
                            input.extend(frame(&format!(
                                "action=enable,status=accepted,display={}",
                                if attached { "attached" } else { "detached" }
                            )));
                        } else {
                            input.extend(frame("action=enable,status=unavailable"));
                        }
                    }
                    "disable" if get(&f, "status").is_empty() => {
                        self.enabled = false;
                        if self.operation.is_some() {
                            self.error = Some("application disabled Restore Display".into());
                        }
                        self.operation = None;
                        input.extend(frame("action=disable,status=accepted"));
                    }
                    "begin" | "complete" | "unavailable" => {
                        if let Some(o) = self
                            .operation
                            .as_mut()
                            .filter(|o| get(&f, "id").parse::<u32>().ok() == Some(o.id))
                        {
                            match get(&f, "action") {
                                "begin" if !o.begun && !o.complete => o.begun = true,
                                "complete" if o.begun && !o.complete => {
                                    o.complete = true;
                                    o.target = sent + output.len() as u64;
                                }
                                "unavailable" => {
                                    self.error = Some("application declined restore".into());
                                }
                                _ => self.error = Some("invalid restore ordering".into()),
                            }
                        }
                    }
                    _ => {}
                },
                Token::Invalid => {}
            }
        }
        debug_assert!(self.decoder.buffered() <= codec::MAX_FRAME);
    }
    pub fn delivered(&mut self, count: u64) -> bool {
        if self
            .operation
            .as_ref()
            .is_some_and(|o| o.complete && count >= o.target)
        {
            self.operation = None;
            return true;
        }
        false
    }
    pub fn check_timeout(&mut self) {
        if self
            .operation
            .as_ref()
            .is_some_and(|o| o.started.elapsed() > Duration::from_secs(5))
        {
            self.error = Some("restore timed out (application preserved)".into());
        }
    }
}

/// While restoring, pass complete terminal replies but discard user keys.
/// Matches the reference exerciser's CSI/APC/OSC reply classification.
#[derive(Default)]
pub struct Replies(Vec<u8>);
impl Replies {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.0.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let b = &self.0;
            if b.is_empty() {
                break;
            }
            let (n, reply) = if b[0] != 27 {
                (1, false)
            } else if b.len() < 2 {
                break;
            } else if matches!(b[1], b'_' | b']' | b'P') {
                match b.windows(2).position(|b| b == b"\x1b\\") {
                    Some(p) => (p + 2, true),
                    None => break,
                }
            } else if b[1] == b'[' {
                match b
                    .iter()
                    .enumerate()
                    .skip(2)
                    .find(|(_, b)| (0x40..=0x7e).contains(*b))
                {
                    Some((p, last)) => (p + 1, matches!(last, b'c' | b'n' | b'R' | b't')),
                    None => break,
                }
            } else {
                (2, false)
            };
            if reply {
                out.extend_from_slice(&self.0[..n]);
            }
            self.0.drain(..n);
        }
        if self.0.len() > 8192 {
            self.0.clear();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_order_delivery_and_stale_ids() {
        let mut d = Display::default();
        let mut input = VecDeque::new();
        let mut out = VecDeque::new();
        d.feed(
            &frame("action=enable,version=0"),
            true,
            &mut input,
            &mut out,
            0,
        );
        assert!(d.enabled);
        assert_eq!(
            input.drain(..).collect::<Vec<_>>(),
            frame("action=enable,status=accepted,display=attached")
        );
        d.detach(&mut input);
        d.feed(b"obsolete", false, &mut input, &mut out, 0);
        assert!(out.is_empty());
        d.start(&mut input).unwrap();
        for (bytes, expected) in [
            (b"old".to_vec(), 0),
            (frame("action=begin,id=9"), 0),
            (frame("action=complete,id=9"), 0),
            (frame("action=begin,id=1"), 0),
            (b"new screen".to_vec(), 10),
            (frame("action=complete,id=1"), 10),
        ] {
            d.feed(&bytes, true, &mut input, &mut out, 0);
            assert_eq!(out.len(), expected);
        }
        assert!(!d.delivered(9));
        assert!(d.delivered(10));
        d.feed(&frame("action=disable"), true, &mut input, &mut out, 10);
        assert!(!d.enabled);
    }
    #[test]
    fn refuses_invalid_order_and_keeps_reply_fragments() {
        let mut d = Display::default();
        let mut input = VecDeque::new();
        d.start(&mut input).unwrap();
        d.feed(
            &frame("action=complete,id=1"),
            true,
            &mut input,
            &mut VecDeque::new(),
            0,
        );
        assert!(d.error.is_some());
        let mut r = Replies::default();
        assert!(r.feed(b"keys\x1b[").is_empty());
        assert_eq!(r.feed(b"?1;2c\x1b[Ahello\x1b_Gi=1;OK\x1b"), b"\x1b[?1;2c");
        assert_eq!(r.feed(b"\\"), b"\x1b_Gi=1;OK\x1b\\");
    }
}
