//! The terminal device the overlay and OSC 52 are written to. Never stdout:
//! that is where the print action's values go, and it may be a pipe.

#[cfg(unix)]
pub(crate) const NAME: &str = "/dev/tty";

#[cfg(unix)]
pub(crate) type Terminal = std::fs::File;

#[cfg(unix)]
pub(crate) fn open() -> std::io::Result<Terminal> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(NAME)
}

#[cfg(windows)]
pub(crate) use console::{NAME, Terminal, open};

#[cfg(windows)]
mod console;
