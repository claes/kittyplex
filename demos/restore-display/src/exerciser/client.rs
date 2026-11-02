use super::*;
use crate::terminal;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
const HELP: &str = "Restore Display exerciser (private PTY; independent of Kittyplex)\n\nUsage: restore-display-exerciser COMMAND --session-dir DIR [OPTIONS]\n  start [--demo-state] [--support on|off] -- PROGRAM ARGS...\n  attach --display kitty|terminal|headless [--fresh]\n  ctl state|detach|lose-display|restore|resize|cancel|input [--data TEXT]\n      resize requires --cols N --rows N [--pixel-width N --pixel-height N]\n  scenario --file CASE.json --report REPORT.json\n  inspect [--json]\n  console\n  stop\n\nAttach does not restore automatically: follow it with ctl restore.\nHeadless is simulated and supplies no graphics or fabricated terminal replies.\nDefault deadlines: enable 1000ms, restore 5000ms, display loss 2000ms.\n";
fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .take_while(|s| s.as_str() != "--")
        .enumerate()
        .find_map(|(i, a)| {
            if a == name {
                args.get(i + 1).cloned()
            } else {
                a.strip_prefix(&format!("{name}=")).map(str::to_string)
            }
        })
}
fn has(args: &[String], name: &str) -> bool {
    args.iter()
        .take_while(|s| s.as_str() != "--")
        .any(|a| a == name)
}
fn timeout_option(args: &[String], key: &str, default: u64) -> io::Result<u64> {
    let n = option(args, key)
        .map(|s| s.parse::<u64>())
        .transpose()
        .map_err(|_| invalid("invalid timeout"))?
        .unwrap_or(default);
    if !(1..=60000).contains(&n) {
        return Err(invalid("timeouts must be 1..60000 milliseconds"));
    }
    Ok(n)
}
fn validate(args: &[String]) -> io::Result<()> {
    let mut i = 1;
    while i < args.len() && args[i] != "--" {
        let k = args[i].split('=').next().unwrap();
        if matches!(k, "--demo-state" | "--fresh" | "--json" | "--retain-view") {
            i += 1;
            continue;
        }
        if matches!(
            k,
            "--session-dir"
                | "--support"
                | "--display"
                | "--cols"
                | "--rows"
                | "--pixel-width"
                | "--pixel-height"
                | "--data"
                | "--file"
                | "--report"
                | "--kind"
                | "--identity"
                | "--discovery-timeout-ms"
                | "--request-timeout-ms"
                | "--display-loss-timeout-ms"
        ) {
            if !args[i].contains('=') {
                i += 1;
                if i >= args.len() || args[i] == "--" {
                    return Err(invalid("option value required"));
                }
            }
            i += 1;
            continue;
        }
        if args[0] == "ctl"
            && matches!(
                k,
                "state" | "detach" | "lose-display" | "restore" | "resize" | "cancel" | "input"
            )
        {
            i += 1;
            continue;
        }
        return Err(invalid(format!("unknown argument {}", args[i])));
    }
    if let Some(s) = option(args, "--support") {
        if !matches!(s.as_str(), "on" | "off") {
            return Err(invalid("support must be on or off"));
        }
    }
    for (k, n) in [
        ("--request-timeout-ms", 5000),
        ("--discovery-timeout-ms", 1000),
        ("--display-loss-timeout-ms", 2000),
    ] {
        timeout_option(args, k, n)?;
    }
    Ok(())
}
fn unique() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}
fn wait(dir: &Path, test: impl Fn(&Value) -> bool, ms: u64) -> io::Result<Value> {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        if let Ok(v) = request(dir, json!({"op":"inspect"})) {
            if test(&v) {
                return Ok(v);
            }
            if matches!(
                v["status"].as_str(),
                Some("unavailable" | "invalid_order" | "timeout" | "stream_disrupted")
            ) {
                return Err(invalid(format!("restoration {}", v["status"])));
            }
        }
        if Instant::now() > end {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "controller wait timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn rc(dir: &Path, args: &[&str]) -> io::Result<String> {
    let address = format!("unix:{}", dir.join("kitty.sock").display());
    let out = Command::new("kitten")
        .args(["@", "--to", &address])
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(invalid(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn observe(dir: &Path) -> io::Result<Value> {
    let mut v = request(dir, json!({"op":"inspect"}))?;
    if v["display"]["kind"] == "kitty" {
        v["text"] = json!(rc(
            dir,
            &["get-text", "--match", "all", "--extent", "screen"]
        )?);
        let probe = dir.join("probe.py");
        fs::write(&probe, include_str!("../../assets/kitty_probe.py"))?;
        v["graphics"] = serde_json::from_str(&rc(
            dir,
            &[
                "kitten",
                "--match",
                "all",
                probe
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF8 session path"))?,
                "graphics",
                "1",
            ],
        )?)?;
    }
    Ok(v)
}
fn attach(dir: &Path, args: &[String]) -> io::Result<Value> {
    let display = option(args, "--display").unwrap_or("kitty".into());
    let identity = unique();
    match display.as_str() {
        "headless" => request(dir, json!({"op":"headless","identity":identity})),
        "terminal" => {
            if has(args, "--fresh") {
                return Err(invalid(
                    "terminal adapter cannot establish fresh emulator state",
                ));
            }
            viewer(dir, "terminal", &identity, false)?;
            Ok(json!({"ok":true}))
        }
        "kitty" => {
            if request(dir, json!({"op":"inspect"}))?["display_status"] == "attached" {
                return Err(invalid("detach current display first"));
            }
            let _ = fs::remove_file(dir.join("kitty.sock"));
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(dir.join(format!("kitty-{identity}.log")))?;
            let mut cmd = Command::new("kitty");
            // Both configuration and cache writes belong to this private display.
            for (key, subdir) in [
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
            ] {
                let path = dir.join(subdir);
                fs::create_dir_all(&path)?;
                cmd.env(key, path);
            }
            cmd.env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-restore-display-bus",
            );
            cmd.args([
                "--config",
                "NONE",
                "--listen-on",
                &format!("unix:{}", dir.join("kitty.sock").display()),
                "--override",
                "allow_remote_control=socket-only",
                "--override",
                "confirm_os_window_close=0",
                "--override",
                "initial_window_width=80c",
                "--override",
                "initial_window_height=24c",
                "--override",
                "remember_window_size=no",
            ]);
            if std::env::var_os("DISPLAY").is_some() {
                cmd.args(["--override", "linux_display_server=x11"]);
            }
            cmd.arg(std::env::current_exe()?)
                .args(["__view", "--session-dir"])
                .arg(dir)
                .args(["--kind", "kitty", "--identity", &identity]);
            cmd.stdout(log.try_clone()?)
                .stderr(log)
                .stdin(Stdio::null());
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn()?;
            let end = Instant::now() + Duration::from_secs(15);
            loop {
                let s = request(dir, json!({"op":"inspect"}))?;
                if s["display"]["identity"] == identity {
                    return Ok(s);
                }
                if child.try_wait()?.is_some() {
                    return Err(invalid("Kitty exited; see private kitty log"));
                }
                if Instant::now() > end {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(invalid("Kitty attach timed out"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        _ => Err(invalid("display must be kitty, terminal or headless")),
    }
}
fn viewer(dir: &Path, kind: &str, identity: &str, fresh: bool) -> io::Result<()> {
    let _raw = terminal::RawTerminal::new(0)?;
    let old = unsafe { libc::fcntl(1, libc::F_GETFL) };
    terminal::nonblock(1)?;
    struct Restore(i32);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                libc::fcntl(1, libc::F_SETFL, self.0);
            }
        }
    }
    let _flags = Restore(old);
    let mut sock = UnixStream::connect(dir.join("control.sock"))?;
    sock.set_nonblocking(true)?;
    let w = terminal::size(0);
    let mut outgoing: VecDeque<u8> = VecDeque::new();
    let mut incoming = Vec::new();
    let mut output = VecDeque::new();
    let mut stdout = unsafe { std::fs::File::from_raw_fd(libc::dup(1)) };
    let hello = json!({"op":"viewer","kind":kind,"identity":identity,"fresh":fresh,"pid":std::process::id(),"emulator_pid":unsafe{libc::getppid()},"cols":w.ws_col,"rows":w.ws_row,"xpixel":w.ws_xpixel,"ypixel":w.ws_ypixel});
    outgoing.extend(serde_json::to_vec(&hello)?);
    outgoing.push_back(b'\n');
    // Only recognized complete replies bypass the controller's user-input gate.
    let mut input = Vec::new();
    let mut deliveries = 0usize;
    let mut relay_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("relay.jsonl"))?;
    let relay_start = Instant::now();
    loop {
        let mut fds = [
            terminal::event(sock.as_raw_fd(), !outgoing.is_empty()),
            terminal::event(0, false),
            terminal::event(1, !output.is_empty()),
        ];
        terminal::poll(&mut fds, 20)?;
        if fds[0].revents & libc::POLLIN != 0 {
            let mut b = [0; 65536];
            let n = terminal::read(sock.as_raw_fd(), &mut b)?;
            if n == 0 {
                break;
            }
            incoming.extend_from_slice(&b[..n]);
            while let Some(end) = incoming.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = incoming.drain(..=end).collect();
                let v: Value = serde_json::from_slice(&line)?;
                if v["event"] == "detach" {
                    return Ok(());
                }
                if v["event"] == "output" {
                    deliveries += 1;
                    output.extend(
                        STANDARD
                            .decode(value(&v, "data")?)
                            .map_err(|_| invalid("invalid output encoding"))?,
                    );
                }
            }
            if incoming.len() > 1024 * 1024 {
                return Err(invalid("viewer message too large"));
            }
            if output.len() > 1024 * 1024 {
                return Err(invalid(
                    "display output queue exceeded 1 MiB; consumer stalled",
                ));
            }
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut b = [0; 8192];
            let n = terminal::read(0, &mut b)?;
            if n == 0 {
                break;
            }
            writeln!(
                relay_log,
                "{}",
                json!({"ms":relay_start.elapsed().as_millis(),"read":STANDARD.encode(&b[..n])})
            )?;
            input.extend_from_slice(&b[..n]);
            loop {
                if input.is_empty() {
                    break;
                }
                let (n, reply) = if input[0] != 27 {
                    (
                        input.iter().position(|b| *b == 27).unwrap_or(input.len()),
                        false,
                    )
                } else if input.len() < 2 {
                    break;
                } else if input[1] == b'_' || input[1] == b']' {
                    if let Some(p) = input.windows(2).position(|b| b == b"\x1b\\") {
                        (p + 2, true)
                    } else {
                        break;
                    }
                } else if input[1] == b'[' {
                    if let Some(p) = input
                        .iter()
                        .enumerate()
                        .skip(2)
                        .find(|(_, b)| (0x40..=0x7e).contains(*b))
                        .map(|(i, _)| i)
                    {
                        let reply = matches!(input[p], b'c' | b'n' | b'R' | b't');
                        (p + 1, reply)
                    } else {
                        break;
                    }
                } else {
                    (2, false)
                };
                let data: Vec<_> = input.drain(..n).collect();
                outgoing.extend(serde_json::to_vec(
                    &json!({"op":if reply{"bytes"}else{"input"},"data":STANDARD.encode(data)}),
                )?);
                outgoing.push_back(b'\n');
            }
            if input.len() > 8192 {
                input.clear();
            }
        }
        let queued = output.len();
        terminal::drain(&mut stdout, &mut output)?;
        if queued > 0 {
            writeln!(
                relay_log,
                "{}",
                json!({"ms":relay_start.elapsed().as_millis(),"written":queued-output.len()})
            )?;
        }
        if output.is_empty() && deliveries > 0 {
            outgoing.extend(serde_json::to_vec(
                &json!({"op":"delivery","count":deliveries}),
            )?);
            outgoing.push_back(b'\n');
            deliveries = 0;
        }
        terminal::drain(&mut sock, &mut outgoing)?;
        if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            break;
        }
    }
    Ok(())
}
fn ctl(dir: &Path, args: &[String]) -> io::Result<Value> {
    let action = args
        .iter()
        .find(|s| {
            matches!(
                s.as_str(),
                "state" | "detach" | "lose-display" | "restore" | "resize" | "cancel" | "input"
            )
        })
        .ok_or_else(|| invalid("ctl action required"))?;
    if action == "detach" && has(args, "--retain-view") {
        return Err(invalid(
            "retained-view transport is not implemented; use fresh attachment",
        ));
    }
    let mut v = json!({"op":action});
    if action == "resize" {
        for (flag, key) in [
            ("--cols", "cols"),
            ("--rows", "rows"),
            ("--pixel-width", "pixel_width"),
            ("--pixel-height", "pixel_height"),
        ] {
            if let Some(n) = option(args, flag) {
                v[key] = json!(n.parse::<u16>().map_err(|_| invalid("invalid dimension"))?);
            }
        }
    }
    if action == "input" {
        v["data"] = json!(
            STANDARD.encode(option(args, "--data").ok_or_else(|| invalid("--data required"))?)
        );
    }
    let result = request(dir, v)?;
    if action == "restore" {
        let ms = request(dir, json!({"op":"inspect"}))?["request_timeout_ms"]
            .as_u64()
            .unwrap_or(5000)
            + 1000;
        return wait(dir, |s| s["status"] == "complete", ms);
    }
    Ok(result)
}
fn scenario(dir: &Path, path: &Path, report: &Path) -> io::Result<i32> {
    let case: Value = serde_json::from_slice(&fs::read(path)?)?;
    if case["schema"] != 0 {
        return Err(invalid("scenario schema must be 0"));
    }
    let mut checks = Vec::new();
    let mut checkpoints = std::collections::BTreeMap::<String, Value>::new();
    let baseline = request(dir, json!({"op":"inspect"}))?;
    let started = Instant::now();
    let mut status = 0;
    let requirements = case["requires"].as_array().cloned().unwrap_or_default();
    if requirements
        .iter()
        .any(|v| !matches!(v.as_str(), Some("text" | "graphics")))
    {
        fs::write(
            report,
            serde_json::to_vec_pretty(
                &json!({"schema":0,"case":case["case"],"checks":[{"id":"requirements","layer":"application_state","status":"unsupported","reason":"milestone 1 implements text and graphics only"}]}),
            )?,
        )?;
        return Ok(3);
    }
    let allowed_top = ["schema", "case", "requires", "steps"];
    if case
        .as_object()
        .is_none_or(|m| m.keys().any(|k| !allowed_top.contains(&k.as_str())))
    {
        return Err(invalid("unknown scenario field"));
    }
    let steps = case["steps"]
        .as_array()
        .ok_or_else(|| invalid("steps required"))?;
    for (index, step) in steps.iter().enumerate() {
        let op = value(step, "op")?;
        let extra: &[&str] = match op {
            "wait_enabled" | "restore" | "inspect_state" | "lose_display" => &[],
            "attach" => &["display", "fresh"],
            "detach" => &["retain_view"],
            "resize" => &["cols", "rows", "pixel_width", "pixel_height"],
            "input" => &["data", "encoding"],
            "checkpoint" => &["name"],
            "assert" => &["check", "checkpoint", "value"],
            "fault" => &["kind", "value"],
            _ => return Err(invalid(format!("unknown scenario operation {op}"))),
        };
        if step.as_object().is_none_or(|m| {
            m.keys()
                .any(|k| !matches!(k.as_str(), "op" | "timeout_ms") && !extra.contains(&k.as_str()))
        }) {
            return Err(invalid("unknown scenario step field"));
        }
        let ms = step["timeout_ms"].as_u64().unwrap_or(5000);
        let result: io::Result<Option<Value>> = (|| {
            match op {
                "wait_enabled" => {
                    wait(dir, |s| s["enabled"] == true, ms)?;
                }
                "attach" => {
                    let mut a = vec![
                        "--display".into(),
                        step["display"].as_str().unwrap_or("kitty").into(),
                    ];
                    if step["fresh"] == true {
                        a.push("--fresh".into());
                    }
                    attach(dir, &a)?;
                }
                "detach" | "lose_display" => {
                    if step["retain_view"] == true {
                        return Err(invalid("retained view unsupported"));
                    }
                    request(dir, json!({"op":"detach"}))?;
                }
                "restore" => {
                    request(dir, json!({"op":"restore"}))?;
                    wait(dir, |s| s["status"] == "complete", ms)?;
                }
                "resize" => {
                    let mut v = step.clone();
                    v["op"] = json!("resize");
                    request(dir, v)?;
                }
                "inspect_state" => {
                    request(dir, json!({"op":"inspect"}))?;
                }
                "input" => {
                    let text = value(step, "data")?;
                    let b = match step["encoding"].as_str().unwrap_or("utf8") {
                        "utf8" => text.as_bytes().to_vec(),
                        "base64" => STANDARD
                            .decode(text)
                            .map_err(|_| invalid("invalid base64 input"))?,
                        _ => return Err(invalid("unknown input encoding")),
                    };
                    request(dir, json!({"op":"input","data":STANDARD.encode(b)}))?;
                }
                "fault" => {
                    let mut command = step.clone();
                    command["op"] = json!("fault");
                    request(dir, command)?;
                }
                "checkpoint" => {
                    if checkpoints.contains_key(value(step, "name")?) {
                        return Err(invalid("checkpoint names must be unique"));
                    }
                    checkpoints.insert(value(step, "name")?.into(), observe(dir)?);
                }
                "assert" => {
                    let current = observe(dir)?;
                    let check = value(step, "check")?;
                    let prior = step["checkpoint"].as_str().and_then(|s| checkpoints.get(s));
                    let passed = match check {
                        "same_process" => {
                            current["pid"] == baseline["pid"]
                                && current["process_start"] == baseline["process_start"]
                        }
                        "fresh_display" => {
                            current["display"]["kind"] == "kitty"
                                && current["display"]["fresh"] == true
                                && prior.is_none_or(|p| {
                                    p["display"]["emulator_pid"]
                                        != current["display"]["emulator_pid"]
                                })
                        }
                        "valid_restore_order" => {
                            current["valid_restore_order"] == true
                                && current["status"] == "complete"
                                && current["capture_complete"] == true
                        }
                        "model_equal" => {
                            let p = prior.ok_or_else(|| invalid("checkpoint required"))?;
                            if p["model"].is_null() || current["model"].is_null() {
                                return Ok(Some(
                                    json!({"status":"not_run","reason":"application has no model observer"}),
                                ));
                            }
                            p["model"] == current["model"]
                        }
                        "text_equal" => {
                            let previous = prior.ok_or_else(|| invalid("checkpoint required"))?;
                            if previous["text"].is_null() || current["text"].is_null() {
                                return Ok(Some(
                                    json!({"status":"not_run","reason":"no screen observer"}),
                                ));
                            }
                            let normalized = |v: &Value| {
                                v.as_str()
                                    .unwrap_or("")
                                    .lines()
                                    .filter(|line| !line.starts_with("PID "))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            };
                            normalized(&previous["text"]) == normalized(&current["text"])
                        }
                        "graphics_equal" => {
                            let p = prior.ok_or_else(|| invalid("checkpoint required"))?;
                            if current["graphics"]["sha256"].is_null()
                                || p["graphics"]["sha256"].is_null()
                            {
                                return Ok(Some(
                                    json!({"status":"not_run","reason":"no graphics observer"}),
                                ));
                            }
                            p["graphics"]["sha256"] == current["graphics"]["sha256"]
                        }
                        "input_effect" => {
                            let fields = step["value"]
                                .as_object()
                                .ok_or_else(|| invalid("value must be an object"))?;
                            fields.iter().all(|(k, v)| current["model"][k] == *v)
                        }
                        "status" => current["status"] == step["value"],
                        _ => {
                            return Ok(Some(
                                json!({"status":"unsupported","reason":format!("observer {check} is not implemented")}),
                            ));
                        }
                    };
                    if !passed {
                        return Err(invalid(format!("assertion {check} failed")));
                    }
                }
                _ => return Err(invalid(format!("unknown scenario operation {op}"))),
            }
            Ok(None)
        })();
        let entry = match result {
            Ok(Some(v)) => {
                status = 3;
                v
            }
            Ok(None) => json!({"status":"pass"}),
            Err(e) => {
                status = 1;
                json!({"status":"fail","reason":e.to_string()})
            }
        };
        let failed = entry["status"] == "fail";
        checks.push(json!({"id":format!("{}.{}",case["case"].as_str().unwrap_or("case"),index),"layer":if op=="assert"{"application_state"}else{"protocol"},"operation":op,"status":entry["status"],"reason":entry.get("reason"),"result":entry,"elapsed_ms":started.elapsed().as_millis()}));
        if failed {
            break;
        }
    }
    if status == 0 && requirements.iter().any(|v| v == "graphics") {
        let observed = observe(dir)?;
        if observed["graphics"]["sha256"].is_null() {
            status = 3;
            checks.push(json!({"id":"requirements.graphics","layer":"rendering","status":"unsupported","reason":"required image observer unavailable"}));
        }
    }
    fs::write(
        report,
        serde_json::to_vec_pretty(
            &json!({"schema":0,"case":case["case"],"tool":"restore-display-exerciser","protocol_version":0,"checks":checks,"elapsed_ms":started.elapsed().as_millis(),"artifacts":dir,"final":request(dir,json!({"op":"inspect"})).ok()}),
        )?,
    )?;
    Ok(status)
}
pub fn run(args: &[String]) -> io::Result<i32> {
    if args.is_empty() || has(args, "--help") || has(args, "-h") {
        print!("{HELP}");
        return Ok(0);
    }
    validate(args)?;
    let mode = &args[0];
    let dir = PathBuf::from(
        option(args, "--session-dir").ok_or_else(|| invalid("--session-dir required"))?,
    );
    if mode == "__serve" {
        let split = args
            .iter()
            .position(|s| s == "--")
            .ok_or_else(|| invalid("program required"))?;
        super::server::serve(
            &dir,
            &args[split + 1..],
            option(args, "--support").as_deref() != Some("off"),
            has(args, "--demo-state"),
            Duration::from_millis(timeout_option(args, "--request-timeout-ms", 5000)?),
            Duration::from_millis(timeout_option(args, "--discovery-timeout-ms", 1000)?),
        )?;
        return Ok(0);
    }
    if mode == "__view" {
        viewer(
            &dir,
            &option(args, "--kind").unwrap_or("terminal".into()),
            &option(args, "--identity").unwrap_or_else(unique),
            true,
        )?;
        return Ok(0);
    }
    let result = match mode.as_str() {
        "start" => {
            let split = args
                .iter()
                .position(|s| s == "--")
                .ok_or_else(|| invalid("program required after --"))?;
            if args.len() == split + 1 {
                return Err(invalid("program required"));
            }
            fs::create_dir(&dir)?;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            let dir = fs::canonicalize(dir)?;
            let log = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(dir.join("controller.log"))?;
            let mut cmd = Command::new(std::env::current_exe()?);
            cmd.arg("__serve").args(&args[1..]);
            cmd.stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn()?;
            let end = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(v) = request(&dir, json!({"op":"inspect"})) {
                    break v;
                }
                if child.try_wait()?.is_some() {
                    return Err(invalid("controller exited; see controller.log"));
                }
                if Instant::now() > end {
                    let _ = child.kill();
                    return Err(invalid("controller startup timed out"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "attach" => attach(&dir, args)?,
        "ctl" => ctl(&dir, args)?,
        "inspect" => request(&dir, json!({"op":"inspect"}))?,
        "stop" => request(&dir, json!({"op":"stop"}))?,
        "scenario" => {
            return scenario(
                &dir,
                Path::new(&option(args, "--file").ok_or_else(|| invalid("--file required"))?),
                Path::new(&option(args, "--report").ok_or_else(|| invalid("--report required"))?),
            );
        }
        "console" => {
            for line in io::stdin().lock().lines() {
                let line = line?;
                let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
                if words.first().is_some_and(|s| s == "exit") {
                    break;
                }
                match ctl(&dir, &words) {
                    Ok(v) => println!("{v}"),
                    Err(e) => eprintln!("{e}"),
                }
            }
            return Ok(0);
        }
        _ => return Err(invalid("unknown command; see --help")),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(0)
}
