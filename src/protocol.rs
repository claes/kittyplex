//! Bounded carrier and local IPC framing. Application bytes are never interpreted.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{self, Read, Write};

pub const CHUNK: usize = 8192;
pub const PIPELINE: usize = 8;
pub const FRAME: usize = 32768;
pub const OUTPUT: usize = 256 * 1024;
pub const INPUT: usize = 64 * 1024;
pub const GROUP: usize = 4 * 1024 * 1024;
pub const MAX_SESSIONS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub v: u8,
    pub g: String,
    pub seq: u64,
    pub kind: String,
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub n: usize,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}
impl Message {
    pub fn new(group: &str, kind: &str, id: u64) -> Self {
        Self {
            v: 2,
            g: group.into(),
            seq: 0,
            kind: kind.into(),
            id,
            data: String::new(),
            n: 0,
            rows: 0,
            cols: 0,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
    pub fn bytes(&self) -> io::Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > FRAME {
            return Err(crate::invalid("frame too large"));
        }
        Ok(bytes)
    }
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > FRAME {
            return Err(crate::invalid("frame too large"));
        }
        let message: Self = serde_json::from_slice(bytes)?;
        if message.v != 2
            || message.g.len() != 32
            || !message.g.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(crate::invalid("incompatible protocol or invalid group"));
        }
        Ok(message)
    }
    pub fn binary(&self) -> io::Result<Vec<u8>> {
        let bytes = decode(&self.data)?;
        if bytes.len() > CHUNK {
            return Err(crate::invalid("data chunk too large"));
        }
        Ok(bytes)
    }
}
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
pub fn decode(text: &str) -> io::Result<Vec<u8>> {
    STANDARD
        .decode(text)
        .map_err(|_| crate::invalid("invalid base64"))
}
pub fn ipc(message: &Message) -> io::Result<Vec<u8>> {
    let mut bytes = message.bytes()?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub fn remote_command(
    command: &str,
    payload: serde_json::Value,
    response: bool,
) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(&serde_json::json!({
        "cmd":command,"version":[0,47,4],"no_response":!response,"payload":payload
    }))?;
    let mut frame = b"\x1bP@kitty-cmd".to_vec();
    frame.extend(json);
    frame.extend(b"\x1b\\");
    Ok(frame)
}
pub fn carrier(message: &Message) -> io::Result<Vec<u8>> {
    let mut frame = b"\x1bP@kittyplex:".to_vec();
    frame.extend(message.bytes()?);
    frame.extend(b"\x1b\\");
    Ok(frame)
}

/// Incremental DCS decoder, also accepting kitty's RPC responses.
#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> io::Result<Vec<(bool, Vec<u8>)>> {
        self.bytes.extend(bytes);
        if self.bytes.len() > FRAME * 2 {
            return Err(crate::invalid("carrier frame exceeds limit"));
        }
        let mut result = Vec::new();
        while let Some(end) = self.bytes.windows(2).position(|s| s == b"\x1b\\") {
            let frame: Vec<_> = self.bytes.drain(..end + 2).collect();
            let start = frame
                .windows(2)
                .position(|s| s == b"\x1bP")
                .ok_or_else(|| crate::invalid("invalid carrier framing"))?;
            let body = &frame[start + 2..end];
            if let Some(json) = body.strip_prefix(b"@kittyplex:") {
                result.push((false, json.to_vec()));
            } else if let Some(json) = body.strip_prefix(b"@kitty-cmd") {
                result.push((true, json.to_vec()));
            } else {
                return Err(crate::invalid("unknown carrier frame"));
            }
        }
        // Ignore only leading terminal noise before the first DCS (e.g. keystrokes during bootstrap).
        if !self.bytes.starts_with(b"\x1bP") {
            if let Some(start) = self.bytes.windows(2).position(|s| s == b"\x1bP") {
                self.bytes.drain(..start);
            } else if self.bytes.last() == Some(&27) {
                self.bytes = vec![27];
            } else {
                self.bytes.clear();
            }
        }
        Ok(result)
    }
}

pub struct Channel<T> {
    pub io: T,
    input: Vec<u8>,
    pub output: VecDeque<u8>,
    pub eof: bool,
}
impl<T: Read + Write> Channel<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            input: Vec::new(),
            output: VecDeque::new(),
            eof: false,
        }
    }
    pub fn send(&mut self, m: &Message) -> io::Result<()> {
        let bytes = ipc(m)?;
        if self.output.len() + bytes.len() > OUTPUT * 2 {
            return Err(crate::invalid("local IPC queue limit exceeded"));
        }
        self.output.extend(bytes);
        Ok(())
    }
    pub fn pump(&mut self) -> io::Result<Vec<Message>> {
        match crate::flush(&mut self.io, &mut self.output) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                self.eof = true;
                self.output.clear();
                return Ok(Vec::new());
            }
            result => result?,
        }
        let mut buffer = [0; FRAME];
        match self.io.read(&mut buffer) {
            Ok(0) => self.eof = true,
            Ok(n) => self.input.extend(&buffer[..n]),
            Err(e) if crate::transient(&e) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) =>
            {
                self.eof = true
            }
            Err(e) => return Err(e),
        }
        let mut result = Vec::new();
        while let Some(end) = self.input.iter().position(|b| *b == b'\n') {
            result.push(Message::parse(&self.input[..end])?);
            self.input.drain(..end + 1);
        }
        if self.input.len() > FRAME {
            return Err(crate::invalid("IPC frame too large"));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const G: &str = "0123456789abcdef0123456789abcdef";
    #[test]
    fn binary_and_every_split_roundtrip() {
        let mut m = Message::new(G, "output", 9);
        let bytes: Vec<u8> = (0..=255).collect();
        m.data = encode(&bytes);
        let frame = carrier(&m).unwrap();
        for split in 0..frame.len() {
            let mut d = Decoder::default();
            let mut frames = d.push(&frame[..split]).unwrap();
            frames.extend(d.push(&frame[split..]).unwrap());
            assert_eq!(frames.len(), 1);
            assert_eq!(
                Message::parse(&frames[0].1).unwrap().binary().unwrap(),
                bytes
            );
        }
    }
    #[test]
    fn renamed_carrier_namespace_rejects_legacy_frames() {
        let message = Message::new(G, "heartbeat", 0);
        let frame = carrier(&message).unwrap();
        assert!(frame.starts_with(b"\x1bP@kittyplex:"));
        let legacy = [
            b"\x1bP@termlab:".as_slice(),
            &message.bytes().unwrap(),
            b"\x1b\\",
        ]
        .concat();
        assert!(Decoder::default().push(&legacy).is_err());
        assert_eq!(Decoder::default().push(&frame).unwrap().len(), 1);
    }

    #[test]
    fn rejects_oversized_and_incompatible_frames() {
        assert!(Decoder::default().push(&vec![b'x'; FRAME * 2 + 1]).is_err());
        let mut m = Message::new(G, "output", 1);
        m.v = 1;
        assert!(Message::parse(&m.bytes().unwrap()).is_err());
        m.data = encode(&vec![0; CHUNK + 1]);
        assert!(m.binary().is_err());
    }
    #[test]
    fn fragmented_ipc_and_short_writes_preserve_messages() {
        struct Fragmented {
            input: VecDeque<u8>,
            written: Vec<u8>,
        }
        impl Read for Fragmented {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.input.is_empty() {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let n = bytes.len().min(11).min(self.input.len());
                for byte in &mut bytes[..n] {
                    *byte = self.input.pop_front().unwrap();
                }
                Ok(n)
            }
        }
        impl Write for Fragmented {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let n = bytes.len().min(7);
                self.written.extend_from_slice(&bytes[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut a = Message::new(G, "input", 1);
        a.data = encode(&[0, 27, 255, 13]);
        let b = Message::new(G, "resize", 2);
        let wire = [ipc(&a).unwrap(), ipc(&b).unwrap()].concat();
        let mut c = Channel::new(Fragmented {
            input: wire.clone().into(),
            written: Vec::new(),
        });
        c.send(&a).unwrap();
        c.send(&b).unwrap();
        let mut messages = Vec::new();
        for _ in 0..wire.len() {
            messages.extend(c.pump().unwrap());
        }
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].binary().unwrap(), [0, 27, 255, 13]);
        assert_eq!(messages[1].id, 2);
        assert_eq!(c.io.written, wire);
    }
    #[test]
    fn batched_carrier_messages_preserve_order_across_fragmentation() {
        let a = Message::new(G, "credit", 1);
        let b = Message::new(G, "input", 2);
        let wire = [carrier(&a).unwrap(), carrier(&b).unwrap()].concat();
        for split in 0..wire.len() {
            let mut decoder = Decoder::default();
            let mut messages = decoder.push(&wire[..split]).unwrap();
            messages.extend(decoder.push(&wire[split..]).unwrap());
            assert_eq!(messages.len(), 2);
            assert_eq!(Message::parse(&messages[0].1).unwrap().id, 1);
            assert_eq!(Message::parse(&messages[1].1).unwrap().id, 2);
        }
    }
    #[test]
    fn v2_geometry_is_required_and_roundtrips_without_truncation() {
        let mut m = Message::new(G, "resize", 9);
        m.rows = 34;
        m.cols = 100;
        m.pixel_width = 1927;
        m.pixel_height = 1089;
        let decoded = Message::parse(&m.bytes().unwrap()).unwrap();
        assert_eq!(
            (
                decoded.rows,
                decoded.cols,
                decoded.pixel_width,
                decoded.pixel_height
            ),
            (34, 100, 1927, 1089)
        );
        let mut value = serde_json::to_value(&m).unwrap();
        value.as_object_mut().unwrap().remove("pixel_width");
        assert!(Message::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        m.v = 1;
        assert!(Message::parse(&m.bytes().unwrap()).is_err());
    }
}
