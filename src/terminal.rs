use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static TERMINATE: AtomicBool = AtomicBool::new(false);
static RESIZE: AtomicBool = AtomicBool::new(false);

extern "C" fn signal_handler(signal: libc::c_int) {
    if signal == libc::SIGWINCH {
        RESIZE.store(true, Ordering::Relaxed);
    } else {
        TERMINATE.store(true, Ordering::Relaxed);
    }
}

pub struct Signals(Vec<(libc::c_int, libc::sigaction)>);

impl Signals {
    pub fn install() -> io::Result<Self> {
        let mut guard = Self(Vec::new());
        for signal in [
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGWINCH,
        ] {
            // SAFETY: sigaction is a C POD; the mask is initialized below.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old = action;
            action.sa_sigaction = signal_handler as *const () as usize;
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

    pub fn terminating() -> bool {
        TERMINATE.load(Ordering::Relaxed)
    }
    pub fn resized() -> bool {
        RESIZE.swap(false, Ordering::Relaxed)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for (signal, old) in self.0.iter().rev() {
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}

pub struct Terminal {
    file: File,
    original: libc::termios,
}

impl Terminal {
    pub fn open() -> io::Result<Self> {
        if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } {
            return Err(io::Error::other("stdin and stdout must be a terminal"));
        }
        // Opening independently avoids changing the parent's shared fd flags.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/tty")?;
        let mut original = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut original) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file, original })
    }

    pub fn original(&self) -> &libc::termios {
        &self.original
    }

    pub fn size(&self) -> io::Result<libc::winsize> {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(self.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } < 0 {
            return Err(io::Error::last_os_error());
        }
        size.ws_row = size.ws_row.max(1);
        size.ws_col = size.ws_col.max(1);
        Ok(size)
    }

    fn write_bounded(&mut self, mut bytes: &[u8], timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while !bytes.is_empty() {
            match self.file.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => bytes = &bytes[n..],
                Err(error) if crate::transient(&error) => {
                    if Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    let mut fd = crate::pollfd(self.as_raw_fd(), libc::POLLOUT);
                    unsafe {
                        libc::poll(&mut fd, 1, 10);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn finish_output(&mut self, output: &mut VecDeque<u8>) -> io::Result<()> {
        self.write_bounded(output.make_contiguous(), Duration::from_millis(200))?;
        output.clear();
        Ok(())
    }
}

impl AsRawFd for Terminal {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}
impl Read for Terminal {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}
impl Write for Terminal {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.as_raw_fd(), libc::TCSANOW, &self.original);
        }
    }
}
