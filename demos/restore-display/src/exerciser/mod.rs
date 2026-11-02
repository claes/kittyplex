mod client;
mod server;
use crate::invalid;
pub use client::run;
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};
pub fn request(dir: &Path, value: Value) -> io::Result<Value> {
    let mut socket = UnixStream::connect(dir.join("control.sock"))?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    serde_json::to_writer(&mut socket, &value)?;
    socket.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line)?;
    let result: Value = serde_json::from_str(&line)?;
    if let Some(e) = result.get("error") {
        return Err(invalid(e.as_str().unwrap_or("controller error")));
    }
    Ok(result)
}
pub fn value<'a>(v: &'a Value, key: &str) -> io::Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| invalid(format!("missing {key}")))
}
