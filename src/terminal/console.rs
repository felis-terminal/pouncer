use crossterm_winapi::{Console, ConsoleMode, Handle};
use std::io::{self, Write};
use windows_sys::Win32::System::Console::ENABLE_VIRTUAL_TERMINAL_PROCESSING;

pub(crate) const NAME: &str = "CONOUT$";

/// A `File` on `CONOUT$` would `WriteFile` the UTF-8 bytes, which the
/// console decodes with its output code page (CP932, CP437, ...), so this
/// hands text to `WriteConsoleW` instead.
///
/// There is no flush on drop: a panic hook restores the screen through its
/// own handle before unwinding drops this one, and flushing a pending
/// overlay then would put the alternate screen back. Drop does put back the
/// output mode `open` changed, since the screen buffer outlives pouncer.
pub(crate) struct Terminal {
    console: Console,
    mode: ConsoleMode,
    previous_mode: Option<u32>,
    pending: Vec<u8>,
}

/// Without VT processing crossterm falls back to console API calls, and its
/// alternate screen is then a new screen buffer that this handle, opened
/// on the old one, would not follow. crossterm's own `supports_ansi` is
/// not that guarantee: it also answers yes for any `TERM` but `dumb`.
pub(crate) fn open() -> io::Result<Terminal> {
    let handle = Handle::current_out_handle()?;
    let mode = ConsoleMode::from(handle.clone());
    let current = mode.mode()?;
    let previous_mode = if current & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0 {
        mode.set_mode(current | ENABLE_VIRTUAL_TERMINAL_PROCESSING)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "the console does not accept escape sequences",
                )
            })?;
        Some(current)
    } else {
        None
    };
    Ok(Terminal {
        console: Console::from(handle),
        mode,
        previous_mode,
        pending: Vec::new(),
    })
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(previous) = self.previous_mode {
            let _ = self.mode.set_mode(previous);
        }
    }
}

impl Write for Terminal {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Every write here is a whole `str`, so a buffer that is not valid
    /// UTF-8 at a flush is a bug, reported rather than held back.
    fn flush(&mut self) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        let text = String::from_utf8(pending)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if !text.is_empty() {
            self.console.write_char_buffer(text.as_bytes())?;
        }
        Ok(())
    }
}
