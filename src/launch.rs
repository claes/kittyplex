//! Desktop launchers. Transport clients remain foreground children of their TTY.
use crate::protocol as p;
use crate::terminal::{Signals, Terminal};
use std::ffi::CStr;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const HELP: &str = "Desktop launchers:
  termlab start [--login] [--cwd DIRECTORY]
  termlab run0 [-u USER|--user USER] [-g GROUP|--group GROUP]
               [-D DIRECTORY|--chdir DIRECTORY|--cwd DIRECTORY]
               [--pty|--pty-late] [--no-ask-password]
  termlab ssh [--remote-binary PATH] [--cwd DIRECTORY] DESTINATION [-- SSH_OPTIONS...]
  termlab doctor

run0 defaults to root, target home/login environment and --pty-late (systemd >=258).
SSH targets need termlab >=0.4 installed on PATH, or --remote-binary PATH.
Quit finishes the launch. Use Kitty actions to open independent groups from a
managed view. See docs/integration-instructions.md.
";

fn value(args: &[String], i: &mut usize, names: &[&str]) -> io::Result<Option<String>> {
    let arg = &args[*i];
    for name in names {
        if arg == name {
            *i += 1;
            return args
                .get(*i)
                .filter(|s| !s.is_empty())
                .cloned()
                .map(Some)
                .ok_or_else(|| crate::invalid(&format!("{name} requires a value")));
        }
        if let Some(v) = arg.strip_prefix(&format!("{name}=")) {
            if v.is_empty() {
                return Err(crate::invalid("empty option value"));
            }
            return Ok(Some(v.into()));
        }
        if name.len() == 2 && name.starts_with('-') && arg.starts_with(name) && arg.len() > 2 {
            return Ok(Some(arg[2..].into()));
        }
    }
    Ok(None)
}

pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub fn run0_args(args: &[String], executable: &Path) -> io::Result<Vec<String>> {
    let (mut user, mut group, mut cwd) = ("root".to_string(), None, "~".to_string());
    let mut pty: Option<&str> = None;
    let mut no_ask = false;
    let mut i = 0;
    while i < args.len() {
        if let Some(v) = value(args, &mut i, &["-u", "--user"])? {
            user = v;
        } else if let Some(v) = value(args, &mut i, &["-g", "--group"])? {
            group = Some(v);
        } else if let Some(v) = value(args, &mut i, &["-D", "--chdir", "--cwd"])? {
            cwd = v;
        } else {
            match args[i].as_str() {
                "--pty" | "--pty-late" => {
                    let selected = if args[i] == "--pty" {
                        "--pty"
                    } else {
                        "--pty-late"
                    };
                    if pty.is_some_and(|old| old != selected) {
                        return Err(crate::invalid("choose either --pty or --pty-late"));
                    }
                    pty = Some(selected);
                }
                "--no-ask-password" => no_ask = true,
                other => {
                    return Err(crate::invalid(&format!(
                        "unsupported run0 option: {other}; see termlab run0 --help"
                    )));
                }
            }
        }
        i += 1;
    }
    let mut result = vec![
        pty.unwrap_or("--pty-late").into(),
        // run0 otherwise probes Kitty with OSC 11; delayed replies can reach the
        // target after startup and corrupt the strictly framed carrier input.
        "--background=".into(),
        format!("--user={user}"),
        "--chdir=~".into(),
    ];
    if let Some(g) = group {
        result.push(format!("--group={g}"));
    }
    if no_ask {
        result.push("--no-ask-password".into());
    }
    result.extend([
        executable.to_string_lossy().into_owned(),
        "start".into(),
        "--login".into(),
        "--cwd".into(),
        cwd,
    ]);
    Ok(result)
}

pub fn ssh_args(args: &[String]) -> io::Result<Vec<String>> {
    let (mut binary, mut cwd, mut destination) = ("termlab".to_string(), "~".to_string(), None);
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        if let Some(v) = value(args, &mut i, &["--remote-binary"])? {
            binary = v;
        } else if let Some(v) = value(args, &mut i, &["--cwd"])? {
            cwd = v;
        } else if !args[i].starts_with('-') && destination.is_none() {
            destination = Some(args[i].clone());
        } else {
            return Err(crate::invalid(
                "expected one SSH destination; put connection options after --",
            ));
        }
        i += 1;
    }
    let destination: String =
        destination.ok_or_else(|| crate::invalid("SSH destination required"))?;
    if destination.chars().any(char::is_control) || binary.starts_with('-') {
        return Err(crate::invalid("invalid SSH destination or remote binary"));
    }
    let extra = if i < args.len() { &args[i + 1..] } else { &[] };
    validate_ssh_options(extra)?;
    let mut result = vec!["-tt".into()];
    for option in [
        "RemoteCommand=none",
        "RequestTTY=force",
        "StdinNull=no",
        "ForkAfterAuthentication=no",
        "SessionType=default",
    ] {
        result.extend(["-o".into(), option.into()]);
    }
    result.extend_from_slice(extra);
    let command = format!(
        "exec {} start --login --cwd {}",
        quote(&binary),
        quote(&cwd)
    );
    result.extend(["--".into(), destination, command]);
    Ok(result)
}

fn validate_ssh_options(args: &[String]) -> io::Result<()> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let bytes = arg.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'-' {
            return Err(crate::invalid(
                "SSH passthrough accepts connection options, not remote commands",
            ));
        }
        let option = bytes[1] as char;
        if "p i J F l o S c m E B b e L R D I w"
            .split_whitespace()
            .any(|s| s.starts_with(option))
        {
            let v = if bytes.len() > 2 {
                arg[2..].to_string()
            } else {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| crate::invalid("missing SSH option value"))?
            };
            if option == 'o' {
                let key = v
                    .split(['=', ' '])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if [
                    "remotecommand",
                    "requesttty",
                    "stdinnull",
                    "forkafterauthentication",
                    "sessiontype",
                ]
                .contains(&key.as_str())
                {
                    return Err(crate::invalid(
                        "SSH option conflicts with the termlab foreground carrier",
                    ));
                }
            }
        } else if !arg[1..].chars().all(|c| "46AaCqvXxYyKkg".contains(c)) || arg == "-G" {
            return Err(crate::invalid(&format!(
                "SSH option incompatible with the carrier: {arg}"
            )));
        }
        i += 1;
    }
    Ok(())
}

pub fn not_nested() -> io::Result<()> {
    if std::env::var_os("TERMLAB_SOCKET").is_some() {
        return Err(crate::invalid(
            "nested CLI launch is not supported; use a Kitty termlab-local/run0/ssh action for an independent group",
        ));
    }
    Ok(())
}

struct Account {
    name: String,
    home: PathBuf,
    shell: PathBuf,
}
fn account() -> io::Result<Account> {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0; 65536];
    let error = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    if result.is_null() {
        return Err(crate::invalid("executing account has no passwd entry"));
    }
    let field = |ptr| unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    Ok(Account {
        name: field(entry.pw_name),
        home: field(entry.pw_dir).into(),
        shell: field(entry.pw_shell).into(),
    })
}

fn directory(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.into()
    } else if let Some(tail) = path.strip_prefix("~/") {
        home.join(tail)
    } else {
        path.into()
    }
}

pub fn start(args: &[String], after_login: bool) -> io::Result<()> {
    not_nested()?;
    let mut login = false;
    let mut cwd = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--login" {
            login = true;
        } else if let Some(v) = value(args, &mut i, &["--cwd"])? {
            cwd = Some(v);
        } else {
            return Err(crate::invalid("expected start [--login] [--cwd DIRECTORY]"));
        }
        i += 1;
    }
    let user = account()?;
    if login && !after_login {
        let path = directory(cwd.as_deref().unwrap_or("~"), &user.home);
        let path = if path.is_absolute() {
            path
        } else {
            user.home.join(path)
        };
        let command = format!(
            "exec {} __start --cwd {}",
            quote(&std::env::current_exe()?.to_string_lossy()),
            quote(&path.to_string_lossy())
        );
        let name = user.shell.file_name().unwrap_or_default().to_string_lossy();
        let error = Command::new(&user.shell)
            .arg0(format!("-{name}"))
            .args(["-c", &command])
            .current_dir(&user.home)
            .env("HOME", &user.home)
            .env("USER", &user.name)
            .env("LOGNAME", &user.name)
            .env("SHELL", &user.shell)
            .exec();
        return Err(error);
    }
    if let Some(path) = cwd {
        std::env::set_current_dir(directory(&path, &user.home))?;
    }
    let mut hostname = [0u8; 256];
    if unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len() - 1) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let host =
        String::from_utf8_lossy(&hostname[..hostname.iter().position(|b| *b == 0).unwrap_or(255)]);
    // Single-threaded initialization, before any subprocess or event loop starts.
    unsafe {
        std::env::set_var("SHELL", &user.shell);
        std::env::set_var("TERMLAB_CONTEXT", format!("{}@{}", user.name, host));
    }
    crate::remote::run(Duration::from_secs(30))
}

pub fn probe() -> io::Result<serde_json::Value> {
    let mut tty = Terminal::open()?;
    let _signals = Signals::install()?;
    let mut random = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let nonce: String = random.iter().map(|n| format!("{n:02x}")).collect();
    let frame = p::remote_command(
        "kitten",
        serde_json::json!({"kitten":"termlab.py","args":["probe",nonce]}),
        true,
    )?;
    tty.write_all(&frame)?;
    let mut decoder = p::Decoder::default();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !Signals::terminating() {
        let mut fds = [crate::pollfd(tty.as_raw_fd(), libc::POLLIN)];
        unsafe {
            libc::poll(fds.as_mut_ptr(), 1, 50);
        }
        let mut buffer = [0; p::FRAME];
        match tty.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                for (rpc, body) in decoder.push(&buffer[..n])? {
                    if rpc {
                        let response: serde_json::Value = serde_json::from_slice(&body)?;
                        if response["ok"] == true {
                            let data = &response["data"];
                            let info: serde_json::Value = if let Some(s) = data.as_str() {
                                serde_json::from_str(s)?
                            } else {
                                data.clone()
                            };
                            if info["nonce"] == nonce && info["desktop_api"] == 1 {
                                return Ok(info);
                            }
                        }
                        return Err(crate::invalid(
                            "Kitty rejected the desktop probe; install matching integration and start a fresh Kitty process",
                        ));
                    }
                }
            }
            Err(e) if crate::transient(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Err(crate::invalid(
        "no compatible Kitty response; install the integration, start fresh Kitty, and run from a terminal",
    ))
}

fn run0_version() -> io::Result<()> {
    let output = Command::new("run0").arg("--version").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .split_whitespace()
        .nth(1)
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0);
    if !output.status.success() || version < 258 {
        return Err(crate::invalid("run0 requires systemd >=258"));
    }
    Ok(())
}

pub fn run(mode: &str, args: &[String]) -> io::Result<()> {
    if args.iter().any(|s| s == "--help" || s == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    match mode {
        "start" => start(args, false),
        "__start" => start(args, true),
        "doctor" => {
            if !args.is_empty() {
                return Err(crate::invalid("doctor takes no arguments"));
            }
            let info = probe()?;
            println!(
                "Kitty integration: {} (carrier v{}, desktop API 1)",
                info["version"], info["carrier"]
            );
            println!("Local binary: {}", info["binary"]);
            match run0_version() {
                Ok(()) => println!("run0: available (systemd >=258)"),
                Err(e) => println!("run0: {e}"),
            }
            println!(
                "SSH: {}",
                if Command::new("ssh").arg("-V").output().is_ok() {
                    "available"
                } else {
                    "not found"
                }
            );
            Ok(())
        }
        "run0" | "ssh" => {
            not_nested()?;
            let command_args = if mode == "run0" {
                run0_args(args, &std::env::current_exe()?)?
            } else {
                ssh_args(args)?
            };
            if mode == "run0" {
                run0_version()?;
            }
            probe()?;
            let error = Command::new(if mode == "run0" { "run0" } else { "ssh" })
                .args(command_args)
                .exec();
            Err(error)
        }
        _ => Err(crate::invalid("unknown desktop command")),
    }
}

// The wrapper shares its child's foreground process group. Let Ctrl-C reach the
// transport, then display its failure instead of killing the error-holding tab.
struct TabInterrupts(Vec<(libc::c_int, libc::sigaction)>);
impl TabInterrupts {
    fn install() -> io::Result<Self> {
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGQUIT] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old = action;
            action.sa_sigaction = libc::SIG_IGN;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(signal, &action, &mut old) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            guard.0.push((signal, old));
        }
        Ok(guard)
    }
}
impl Drop for TabInterrupts {
    fn drop(&mut self) {
        for (signal, old) in self.0.iter().rev() {
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}

pub fn tab(args: &[String]) -> io::Result<()> {
    let Some((mode, rest)) = args.split_first() else {
        return Err(crate::invalid("missing tab launch command"));
    };
    if !["start", "run0", "ssh"].contains(&mode.as_str()) {
        return Err(crate::invalid("unsupported tab launch command"));
    }
    let _interrupts = TabInterrupts::install()?;
    let mut child = Command::new(std::env::current_exe()?);
    // Only async-signal-safe operations may run between fork and exec.
    unsafe {
        child.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
    let status = child
        .arg(mode)
        .args(rest)
        .env_remove("TERMLAB_SOCKET")
        .env_remove("TERMLAB_SESSION")
        .env_remove("TERMLAB_CONTEXT")
        .status()?;
    if !status.success() {
        eprintln!(
            "termlab: launch failed ({}). Targets require termlab >=0.4; see integration-instructions.md. Press Enter to close.",
            status.code().map_or_else(
                || format!("signal {}", status.signal().unwrap_or(0)),
                |n| n.to_string()
            )
        );
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn run0_short_long_and_pty_modes() {
        let exe = Path::new("/nix/store/example/bin/termlab");
        let short =
            run0_args(&a(&["-ubob", "-g", "work", "-D", "/tmp/a b", "--pty"]), exe).unwrap();
        let long = run0_args(
            &a(&["--user=bob", "--group=work", "--cwd=/tmp/a b", "--pty"]),
            exe,
        )
        .unwrap();
        assert_eq!(short, long);
        assert_eq!(short[0], "--pty");
        assert!(short.iter().any(|s| s == "--background="));
        assert!(!short.iter().any(|s| s == "--pty-late"));
        assert_eq!(run0_args(&[], exe).unwrap()[0], "--pty-late");
        assert!(run0_args(&a(&["--pty", "--pty-late"]), exe).is_err());
        assert!(run0_args(&a(&["-u"]), exe).is_err());
    }
    #[test]
    fn ssh_connection_options_and_conflicts() {
        let args = ssh_args(&a(&[
            "--cwd", "/srv/a'b", "host", "--", "-p2222", "-J", "jump",
        ]))
        .unwrap();
        assert!(args.iter().any(|s| s == "-p2222"));
        for flag in ["-f", "-N", "-T", "-n", "-O", "-W", "-G"] {
            assert!(ssh_args(&a(&["host", "--", flag])).is_err());
        }
        assert!(ssh_args(&a(&["host", "--", "-oRemoteCommand=sh"])).is_err());
        assert!(ssh_args(&a(&["host", "unexpected command"])).is_err());
    }
    #[test]
    fn quoted_remote_arguments_are_literal_in_a_shell() {
        let text = "a ' b $(printf INJECTED) `printf INJECTED` ; x\nend";
        let result = Command::new("sh")
            .args(["-c", &format!("printf %s {}", quote(text))])
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, text.as_bytes());
    }
}
