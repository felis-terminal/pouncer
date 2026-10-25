//! Termination signals that arrive while the overlay holds the terminal.
//!
//! Raw mode turns `Ctrl-C` and `Ctrl-\` into keys, but a signal sent from
//! outside (`timeout`, `kill`, a supervisor, the SIGHUP of a closed popup)
//! would end the process with the terminal still raw and on the alternate
//! screen. The handler only records the signal; the event loop sees it, the
//! overlay is torn down as on any other exit, and the signal is then delivered
//! again with its default action.

#[cfg(unix)]
pub(crate) use unix::Signals;

#[cfg(windows)]
pub(crate) use windows::Signals;

#[cfg(unix)]
mod unix {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    use signal_hook::flag;
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    pub(crate) struct Signals {
        caught: Arc<AtomicUsize>,
        released: Arc<AtomicBool>,
    }

    impl Signals {
        /// Unregistering would not bring the default action back: signal-hook
        /// keeps its handler installed, and with no actions left the signal
        /// would be ignored for the rest of the run. So the default action is
        /// registered too, and armed once the overlay is gone, or on drop
        /// for an exit that never gets to `release`.
        pub(crate) fn register() -> io::Result<Self> {
            let signals = Self {
                caught: Arc::new(AtomicUsize::new(0)),
                released: Arc::new(AtomicBool::new(false)),
            };
            for sig in [SIGTERM, SIGHUP, SIGINT, SIGQUIT] {
                flag::register_usize(sig, Arc::clone(&signals.caught), sig as usize)?;
                flag::register_conditional_default(sig, Arc::clone(&signals.released))?;
            }
            Ok(signals)
        }

        pub(crate) fn caught(&self) -> bool {
            self.caught.load(Ordering::SeqCst) != 0
        }

        /// Arms the default action, then delivers a signal caught before that
        /// with it, which ends the process. Call it only once the terminal is
        /// restored.
        pub(crate) fn release(self) {
            self.released.store(true, Ordering::SeqCst);
            let sig = self.caught.load(Ordering::SeqCst);
            if sig != 0 {
                let sig = i32::try_from(sig).unwrap_or(SIGTERM);
                let _ = signal_hook::low_level::emulate_default_handler(sig);
            }
        }
    }

    impl Drop for Signals {
        fn drop(&mut self) {
            self.released.store(true, Ordering::SeqCst);
        }
    }
}

/// The console's close and logoff events end the process without a signal a
/// handler could defer, so there is nothing to catch here.
#[cfg(windows)]
mod windows {
    pub(crate) struct Signals;

    // The stub keeps the Unix signature so the picker has one code path.
    #[expect(clippy::unnecessary_wraps, clippy::unused_self)]
    impl Signals {
        pub(crate) const fn register() -> std::io::Result<Self> {
            Ok(Self)
        }

        pub(crate) const fn caught(&self) -> bool {
            false
        }

        pub(crate) const fn release(self) {}
    }
}
