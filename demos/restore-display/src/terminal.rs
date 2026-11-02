use crate::invalid;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    os::fd::{AsRawFd, RawFd},
};
pub struct RawTerminal {
    fd: RawFd,
    attrs: libc::termios,
    flags: libc::c_int,
}
impl RawTerminal {
    pub fn new(fd: RawFd) -> io::Result<Self> {
        unsafe {
            let mut attrs = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut attrs) < 0 {
                return Err(io::Error::last_os_error());
            }
            let flags = libc::fcntl(fd, libc::F_GETFL);
            let mut raw = attrs;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { fd, attrs, flags })
        }
    }
}
impl Drop for RawTerminal {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.attrs);
            libc::fcntl(self.fd, libc::F_SETFL, self.flags);
        }
    }
}
pub fn nonblock(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
pub fn size(fd: RawFd) -> libc::winsize {
    unsafe {
        let mut w = std::mem::zeroed::<libc::winsize>();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut w) < 0 || w.ws_col == 0 || w.ws_row == 0 {
            w.ws_col = 80;
            w.ws_row = 24;
        }
        w
    }
}
pub fn resize(fd: RawFd, cols: u16, rows: u16, pixels: (u16, u16)) -> io::Result<()> {
    if cols == 0 || rows == 0 {
        return Err(invalid("dimensions must be positive"));
    }
    let w = libc::winsize {
        ws_col: cols,
        ws_row: rows,
        ws_xpixel: pixels.0,
        ws_ypixel: pixels.1,
    };
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &w) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub fn drain(writer: &mut impl Write, queue: &mut VecDeque<u8>) -> io::Result<()> {
    while !queue.is_empty() {
        match writer.write(queue.as_slices().0) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                queue.drain(..n);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
pub fn read(fd: RawFd, b: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, b.as_mut_ptr().cast(), b.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}
pub fn poll(fds: &mut [libc::pollfd], ms: i32) -> io::Result<()> {
    if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, ms) } < 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
    Ok(())
}
pub fn event(fd: RawFd, write: bool) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN | if write { libc::POLLOUT } else { 0 },
        revents: 0,
    }
}
pub fn write_fd(fd: RawFd, data: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}
pub fn fd(file: &File) -> RawFd {
    file.as_raw_fd()
}
