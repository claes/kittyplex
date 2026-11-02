//! The wire codec is independent of the demo renderer and the PTY controller.
pub mod codec;
pub mod demo;
pub mod exerciser;
pub mod terminal;

pub fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}
