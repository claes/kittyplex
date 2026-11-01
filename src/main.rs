#[cfg(not(target_os = "linux"))]
compile_error!("kittyplex currently supports Linux only");
mod broker;
mod control;
mod help;
mod launch;
mod protocol;
mod remote;
mod session;
mod terminal;
mod view;

use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::Duration;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kittyplex: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print!("{}", help::ROOT);
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("kittyplex {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("ctl") => control::client(&args[1..]),
        Some(mode @ ("start" | "run0" | "ssh" | "doctor" | "__start")) => {
            launch::run(mode, &args[1..])
        }
        Some("__launch-tab") => launch::tab(&args[1..]),
        Some("__broker") => broker::run(&args[1..]),
        Some("__view") => view::run(&args[1..]),
        None | Some("--heartbeat-timeout") => {
            if std::env::var_os("KITTYPLEX_SOCKET").is_some() {
                return Err(invalid("nested kittyplex is not supported"));
            }
            let timeout = if args.is_empty() {
                30
            } else {
                if args.len() != 2 {
                    return Err(invalid("expected --heartbeat-timeout SECONDS"));
                }
                args[1]
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n >= 10)
                    .ok_or_else(|| invalid("heartbeat timeout must be at least 10 seconds"))?
            };
            remote::run(Duration::from_secs(timeout))
        }
        _ => Err(invalid("unknown argument; see --help")),
    }
}
fn pollfd(fd: libc::c_int, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}
fn transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn flush(writer: &mut impl Write, buffer: &mut VecDeque<u8>) -> io::Result<()> {
    while !buffer.is_empty() {
        match writer.write(buffer.as_slices().0) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buffer.drain(..n);
            }
            Err(e) if transient(&e) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
