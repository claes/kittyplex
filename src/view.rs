//! One byte relay per kitty window. Kitty, rather than kittyplex, parses its output.
use crate::protocol::{self as p, Channel, Message};
use crate::terminal::{Signals, Terminal};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> io::Result<()> {
    if args.len() != 3 {
        return Err(crate::invalid("internal view arguments"));
    }
    let id = args[2]
        .parse()
        .map_err(|_| crate::invalid("invalid session id"))?;
    let group = &args[1];
    let stream = UnixStream::connect(format!("{}/views.sock", args[0]))?;
    stream.set_nonblocking(true)?;
    let mut channel = Channel::new(stream);
    let mut tty = Terminal::open()?;
    let _signals = Signals::install()?;
    let mut m = Message::new(group, "view_ready", id);
    let size = tty.size()?;
    m.rows = size.ws_row;
    m.cols = size.ws_col;
    m.pixel_width = size.ws_xpixel;
    m.pixel_height = size.ws_ypixel;
    channel.send(&m)?;
    let mut output = VecDeque::new();
    let mut credit_pending = 0;
    let mut last_credit = Instant::now();
    let mut input_credit = p::INPUT;
    let mut closing = false;
    let mut last_rx = Instant::now();
    loop {
        if Signals::terminating() {
            return Ok(());
        }
        if last_rx.elapsed() > Duration::from_secs(35) {
            return Err(io::Error::other("local broker lost"));
        }
        if Signals::resized() {
            let size = tty.size()?;
            let mut m = Message::new(group, "resize", id);
            m.rows = size.ws_row;
            m.cols = size.ws_col;
            m.pixel_width = size.ws_xpixel;
            m.pixel_height = size.ws_ypixel;
            channel.send(&m)?;
        }
        for m in channel.pump()? {
            if m.g != *group || m.id != id {
                return Err(crate::invalid("wrong view route"));
            }
            last_rx = Instant::now();
            match m.kind.as_str() {
                "output" => {
                    let bytes = m.binary()?;
                    if output.len() + bytes.len() > p::OUTPUT {
                        return Err(crate::invalid("view output overflow"));
                    }
                    output.extend(bytes);
                }
                "input_credit" => {
                    input_credit = input_credit
                        .checked_add(m.n)
                        .filter(|n| *n <= p::INPUT)
                        .ok_or_else(|| crate::invalid("invalid input credit"))?;
                }
                "drop" => closing = true,
                "heartbeat" => {}
                _ => return Err(crate::invalid("unknown view operation")),
            }
        }
        let before = output.len();
        crate::flush(&mut tty, &mut output)?;
        credit_pending += before - output.len();
        if credit_pending >= p::CHUNK
            || (credit_pending > 0 && last_credit.elapsed() >= Duration::from_millis(50))
        {
            let mut m = Message::new(group, "credit", id);
            m.n = credit_pending;
            channel.send(&m)?;
            credit_pending = 0;
            last_credit = Instant::now();
        }
        if closing && output.is_empty() && channel.output.is_empty() {
            return Ok(());
        }
        if channel.eof {
            tty.finish_output(&mut output)?;
            return Ok(());
        }
        let mut fds = [
            crate::pollfd(
                tty.as_raw_fd(),
                if input_credit > 0 && !closing {
                    libc::POLLIN
                } else {
                    0
                } | if output.is_empty() { 0 } else { libc::POLLOUT },
            ),
            crate::pollfd(
                channel.io.as_raw_fd(),
                libc::POLLIN
                    | if channel.output.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
            ),
        ];
        unsafe {
            libc::poll(fds.as_mut_ptr(), 2, 20);
        }
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return Ok(());
        }
        if fds[0].revents & libc::POLLIN != 0 {
            let mut buf = [0; p::CHUNK];
            let limit = input_credit.min(p::CHUNK);
            match tty.read(&mut buf[..limit]) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    input_credit -= n;
                    let mut m = Message::new(group, "input", id);
                    m.data = p::encode(&buf[..n]);
                    channel.send(&m)?;
                }
                Err(e) if crate::transient(&e) => {}
                Err(e) => return Err(e),
            }
        }
        channel.io.flush()?;
    }
}
