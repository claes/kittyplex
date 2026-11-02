//! A bounded streaming DCS recognizer. Unrelated strings are forwarded as bytes,
//! never interpreted as nested Restore Display frames.
use std::collections::BTreeMap;
const PREFIX: &[u8] = b"\x1bP@restore-display;";
pub const MAX_FRAME: usize = 512;
pub type Fields = BTreeMap<String, String>;
#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Bytes(Vec<u8>),
    Frame(Fields),
    Invalid,
}
#[derive(Default)]
pub struct Decoder {
    state: u8, // 0 text, 1 ESC, 2 candidate DCS, 3 opaque string, 4 protocol
    buf: Vec<u8>,
    osc: bool,
    escape: bool,
    oversized: bool,
}
impl Decoder {
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut raw = Vec::new();
        for &byte in bytes {
            match self.state {
                0 => {
                    if byte == 27 {
                        self.state = 1;
                    } else {
                        raw.push(byte);
                    }
                }
                1 => match byte {
                    b'P' => {
                        self.buf.extend_from_slice(b"\x1bP");
                        self.state = 2;
                    }
                    b']' | b'_' | b'^' | b'X' => {
                        raw.extend_from_slice(&[27, byte]);
                        self.osc = byte == b']';
                        self.escape = false;
                        self.state = 3;
                    }
                    27 => raw.push(27),
                    _ => {
                        raw.extend_from_slice(&[27, byte]);
                        self.state = 0;
                    }
                },
                2 => {
                    self.buf.push(byte);
                    if !PREFIX.starts_with(&self.buf) {
                        raw.extend_from_slice(&self.buf);
                        self.buf.clear();
                        self.state = if byte == 0x18 || byte == 0x1a { 0 } else { 3 };
                        self.osc = false;
                        self.escape = byte == 27;
                    } else if self.buf.len() == PREFIX.len() {
                        if !raw.is_empty() {
                            tokens.push(Token::Bytes(std::mem::take(&mut raw)));
                        }
                        self.state = 4;
                        self.escape = false;
                        self.oversized = false;
                    }
                }
                3 => {
                    raw.push(byte);
                    if (self.escape && byte == b'\\')
                        || (self.osc && byte == 7)
                        || byte == 0x18
                        || byte == 0x1a
                    {
                        self.state = 0;
                    }
                    self.escape = byte == 27;
                }
                4 => {
                    if byte == 0x18 || byte == 0x1a {
                        tokens.push(Token::Invalid);
                        self.reset();
                        continue;
                    }
                    if self.escape {
                        if byte == b'\\' {
                            let result = if self.oversized || self.buf.len() + 1 > MAX_FRAME {
                                None
                            } else {
                                parse(&self.buf[PREFIX.len()..self.buf.len() - 1])
                            };
                            tokens.push(result.map_or(Token::Invalid, Token::Frame));
                            self.reset();
                            continue;
                        }
                        tokens.push(Token::Invalid);
                        self.reset();
                        self.state = 1;
                        // Reconsider the interrupted ESC as a new introducer.
                        if byte == b'P' {
                            self.buf.extend_from_slice(b"\x1bP");
                            self.state = 2;
                        } else if matches!(byte, b']' | b'_' | b'^' | b'X') {
                            raw.extend_from_slice(&[27, byte]);
                            self.state = 3;
                            self.osc = byte == b']';
                        } else if byte != 27 {
                            raw.extend_from_slice(&[27, byte]);
                            self.state = 0;
                        }
                        continue;
                    }
                    if self.buf.len() < MAX_FRAME {
                        self.buf.push(byte);
                    } else {
                        self.oversized = true;
                    }
                    self.escape = byte == 27;
                }
                _ => unreachable!(),
            }
        }
        if !raw.is_empty() {
            tokens.push(Token::Bytes(raw));
        }
        tokens
    }
    fn reset(&mut self) {
        self.state = 0;
        self.buf.clear();
        self.escape = false;
        self.oversized = false;
    }
}
fn number(value: &str) -> Option<u32> {
    if value.len() > 1 && value.starts_with('0') {
        return None;
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
fn parse(bytes: &[u8]) -> Option<Fields> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut fields = Fields::new();
    for item in text.split(',') {
        let (k, v) = item.split_once('=')?;
        if k.is_empty()
            || !k.bytes().all(|b| b.is_ascii_lowercase())
            || v.is_empty()
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            || fields.insert(k.into(), v.into()).is_some()
        {
            return None;
        }
    }
    if let Some(v) = fields.get("id") {
        if number(v)? == 0 {
            return None;
        }
    }
    if let Some(v) = fields.get("version") {
        number(v)?;
    }
    if let Some(v) = fields.get("status") {
        if !matches!(v.as_str(), "accepted" | "unavailable") {
            return None;
        }
    }
    if let Some(v) = fields.get("display") {
        if !matches!(v.as_str(), "attached" | "detached") {
            return None;
        }
    }
    let valid = match get(&fields, "action") {
        "enable" => match get(&fields, "status") {
            "accepted" => fields.contains_key("display"),
            "unavailable" => true,
            "" => fields.contains_key("version"),
            _ => false,
        },
        "disable" => matches!(get(&fields, "status"), "" | "accepted"),
        "state" => get(&fields, "display") == "detached",
        "restore" | "begin" | "complete" | "unavailable" => fields.contains_key("id"),
        _ => false,
    };
    valid.then_some(fields)
}
pub fn get<'a>(f: &'a Fields, key: &str) -> &'a str {
    f.get(key).map(String::as_str).unwrap_or("")
}
pub fn frame(body: &str) -> Vec<u8> {
    format!("\x1bP@restore-display;{body}\x1b\\").into_bytes()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_splits() {
        let b = frame("action=restore,id=42");
        for split in 0..=b.len() {
            let mut d = Decoder::default();
            let mut t = d.feed(&b[..split]);
            t.extend(d.feed(&b[split..]));
            assert!(matches!(t.as_slice(),[Token::Frame(f)] if get(f,"id")=="42"));
        }
    }
    #[test]
    fn invalid_and_bounded() {
        for s in [
            "action=restore,id=0",
            "action=restore,id=01",
            "action=restore,id=2,id=3",
            "action=restore,id= 2",
            "action=unknown",
            "action=restore,id=4294967296",
        ] {
            assert_eq!(Decoder::default().feed(&frame(s)), vec![Token::Invalid]);
        }
        let mut d = Decoder::default();
        d.feed(&[PREFIX, &vec![b'x'; 10000]].concat());
        assert!(d.buffered() <= 512);
        assert_eq!(d.feed(b"\x1b\\"), vec![Token::Invalid]);
        assert!(matches!(
            d.feed(&frame("action=restore,id=1"))[0],
            Token::Frame(_)
        ));
    }
    #[test]
    fn opaque_strings_and_cancel() {
        let input = b"hello\x1b]title @restore-display;action=restore,id=1\x07world";
        let out = Decoder::default().feed(input);
        assert_eq!(out, vec![Token::Bytes(input.to_vec())]);
        let mut d = Decoder::default();
        let t = d.feed(
            &[
                b"\x1bP@restore-display;action=rest\x18".as_slice(),
                &frame("action=restore,id=9"),
            ]
            .concat(),
        );
        assert!(matches!(t.as_slice(), [Token::Invalid, Token::Frame(_)]));
    }
}
