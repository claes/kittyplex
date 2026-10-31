use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Session {
    pub id: u64,
    master: File,
    child: Child,
    pub input: VecDeque<u8>,
    pub hung_up: bool,
}

impl Session {
    pub fn spawn(
        id: u64,
        shell: &Path,
        socket: &Path,
        original: &libc::termios,
        size: libc::winsize,
    ) -> io::Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both fds; termios and winsize are valid.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                original,
                &size,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut command = Command::new(shell);
        command
            .arg("-i")
            .env("TERMLAB_SOCKET", socket)
            .env("TERMLAB_SESSION", id.to_string())
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                // Only async-signal-safe operations run between fork and exec.
                for signal in [
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGHUP,
                    libc::SIGWINCH,
                    libc::SIGPIPE,
                    libc::SIGTSTP,
                    libc::SIGTTIN,
                    libc::SIGTTOU,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok(Self {
            id,
            master,
            child,
            input: VecDeque::new(),
            hung_up: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn exited(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    pub fn resize(&self, size: libc::winsize) -> io::Result<()> {
        if unsafe { libc::ioctl(self.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EIO) {
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn flush_input(&mut self) -> io::Result<()> {
        if self.hung_up {
            self.input.clear();
            return Ok(());
        }
        match crate::flush(&mut self.master, &mut self.input) {
            Err(error) if matches!(error.raw_os_error(), Some(libc::EIO | libc::EPIPE)) => {
                self.input.clear();
                Ok(())
            }
            result => result,
        }
    }

    pub fn read_chunk(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.master.read(buffer) {
            Ok(0) => {
                self.hung_up = true;
                Ok(0)
            }
            Err(e) if e.raw_os_error() == Some(libc::EIO) => {
                self.hung_up = true;
                Ok(0)
            }
            result => result,
        }
    }

    #[cfg(test)]
    pub fn read_output(&mut self, mut visible: Option<&mut VecDeque<u8>>) -> io::Result<bool> {
        let mut buffer = [0; 8192];
        for _ in 0..8 {
            match self.master.read(&mut buffer) {
                Ok(0) => {
                    self.hung_up = true;
                    return Ok(true);
                }
                Ok(n) => {
                    if let Some(output) = visible.as_mut() {
                        output.extend(&buffer[..n]);
                    }
                }
                Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                    self.hung_up = true;
                    return Ok(true);
                }
                Err(error) if crate::transient(&error) => return Ok(true),
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }
}

impl AsRawFd for Session {
    fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Foreground jobs usually have a different group from their shell.
        // Signal both before closing the PTY; continue stopped jobs to let HUP
        // take effect. The shell remains responsible for its background jobs.
        let foreground = unsafe { libc::tcgetpgrp(self.as_raw_fd()) };
        for group in [foreground, self.child.id() as libc::pid_t] {
            if group > 0 {
                unsafe {
                    libc::kill(-group, libc::SIGHUP);
                    libc::kill(-group, libc::SIGCONT);
                }
            }
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            // Bound shutdown even when a shell ignores hangup.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Session {
        let mut master = -1;
        let mut slave = -1;
        let mut attrs: libc::termios = unsafe { std::mem::zeroed() };
        unsafe {
            assert_eq!(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null()
                ),
                0
            );
            assert_eq!(libc::tcgetattr(slave, &mut attrs), 0);
            libc::close(master);
            libc::close(slave);
        }
        attrs.c_lflag &= !(libc::ECHO | libc::ECHONL);
        Session::spawn(
            1,
            Path::new("/bin/sh"),
            Path::new("/tmp/test-control-unused"),
            &attrs,
            libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap()
    }

    fn send(session: &mut Session, bytes: &[u8]) {
        session.input.extend(bytes);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !session.input.is_empty() {
            session.flush_input().unwrap();
            assert!(Instant::now() < deadline, "PTY input stalled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn until(session: &mut Session, marker: &str) -> String {
        let mut output = VecDeque::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            session.read_output(Some(&mut output)).unwrap();
            let text = String::from_utf8_lossy(output.make_contiguous()).into_owned();
            if text.contains(marker) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {marker:?}: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn shell_has_controlling_terminal_and_exits() {
        let mut session = fresh();
        send(
            &mut session,
            b"test -t 0 && test -t 1 && test -t 2 && printf 'TTY_%s\\n' OK\n",
        );
        until(&mut session, "TTY_OK");
        assert!(unsafe { libc::tcgetpgrp(session.as_raw_fd()) } > 0);
        send(&mut session, b"exit\n");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !session.exited().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn full_winsize_survives_resize() {
        let session = fresh();
        let size = libc::winsize {
            ws_row: 34,
            ws_col: 100,
            ws_xpixel: 1007,
            ws_ypixel: 689,
        };
        session.resize(size).unwrap();
        let mut actual: libc::winsize = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::ioctl(session.as_raw_fd(), libc::TIOCGWINSZ, &mut actual) },
            0
        );
        assert_eq!(
            (
                actual.ws_row,
                actual.ws_col,
                actual.ws_xpixel,
                actual.ws_ypixel
            ),
            (34, 100, 1007, 689)
        );
    }

    #[test]
    fn resize_is_visible_to_child() {
        let mut session = fresh();
        session
            .resize(libc::winsize {
                ws_row: 37,
                ws_col: 101,
                ws_xpixel: 0,
                ws_ypixel: 0,
            })
            .unwrap();
        send(&mut session, b"stty size\n");
        until(&mut session, "37 101");
    }

    #[test]
    fn raw_input_preserves_control_bytes() {
        let mut session = fresh();
        send(
            &mut session,
            b"stty raw -echo; printf 'RAW_%s\\n' READY; od -An -tx1 -N4; stty sane\n",
        );
        until(&mut session, "RAW_READY");
        send(&mut session, &[2, 13, 0, 27]);
        until(&mut session, "02 0d 00 1b");
    }

    #[test]
    fn ctrl_c_interrupts_foreground_job_not_shell() {
        let mut session = fresh();
        send(&mut session, b"printf 'START_%s\\n' JOB; sleep 30\n");
        until(&mut session, "START_JOB");
        std::thread::sleep(Duration::from_millis(50));
        send(&mut session, b"\x03");
        send(&mut session, b"printf 'AFTER_%s\\n' INTERRUPT\n");
        until(&mut session, "AFTER_INTERRUPT");
        assert!(!session.exited().unwrap());
    }

    #[test]
    fn ctrl_z_stops_job_and_fg_resumes_it() {
        let mut session = fresh();
        send(&mut session, b"printf 'START_%s\\n' JOB; sleep 30\n");
        until(&mut session, "START_JOB");
        std::thread::sleep(Duration::from_millis(50));
        send(&mut session, b"\x1a");
        send(&mut session, b"jobs; printf 'CHECK_%s\\n' STOPPED\n");
        let output = until(&mut session, "CHECK_STOPPED");
        assert!(output.contains("Stopped"), "{output:?}");
        send(&mut session, b"fg\n");
        std::thread::sleep(Duration::from_millis(50));
        send(&mut session, b"\x03");
        send(&mut session, b"printf 'RESUMED_%s\\n' OK\n");
        until(&mut session, "RESUMED_OK");
    }

    #[test]
    fn captures_all_output_without_visibility_filtering() {
        let mut session = fresh();
        send(
            &mut session,
            b"head -c 1048576 /dev/zero; printf 'END_%s\\n' OUTPUT\n",
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output: Vec<u8> = Vec::new();
        loop {
            let mut bytes = [0; 8192];
            match session.read_chunk(&mut bytes) {
                Ok(n) => output.extend(&bytes[..n]),
                Err(e) if crate::transient(&e) => {}
                Err(e) => panic!("{e}"),
            }
            if output[output.len().saturating_sub(64)..]
                .windows(10)
                .any(|w| w == b"END_OUTPUT")
            {
                break;
            }
            assert!(Instant::now() < deadline, "output stalled");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output.iter().filter(|b| **b == 0).count(), 1048576);
    }

    #[test]
    fn dropping_session_reaps_shell() {
        let session = fresh();
        let pid = session.pid() as libc::pid_t;
        drop(session);
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
}
