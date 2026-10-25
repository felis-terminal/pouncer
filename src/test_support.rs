//! Scaffolding for tests that depend on crossterm's colour policy.
//!
//! `Colored`'s `NO_COLOR` switch is one process-global flag, so a test that
//! pins it would otherwise change what every parallel test sees.

use crossterm::style::Colored;
use std::sync::{Mutex, MutexGuard, PoisonError};

static ANSI_COLOR: Mutex<()> = Mutex::new(());

/// Holds crossterm's global colour policy at a known value for as long as it
/// lives, excluding every other test that needs a known value.
pub(crate) struct AnsiColor {
    _guard: MutexGuard<'static, ()>,
}

impl AnsiColor {
    pub(crate) fn set(enabled: bool) -> Self {
        let guard = ANSI_COLOR.lock().unwrap_or_else(PoisonError::into_inner);
        Colored::set_ansi_color_disabled(!enabled);
        Self { _guard: guard }
    }
}

impl Drop for AnsiColor {
    fn drop(&mut self) {
        // Back to enabled rather than to the ambient env value, so the suite
        // behaves the same whether or not NO_COLOR is set in the environment.
        Colored::set_ansi_color_disabled(false);
    }
}
