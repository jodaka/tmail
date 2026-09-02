//! Terminal lifecycle: raw mode + alternate screen with panic-safe
//! restoration (plan §11: must survive normal exit, Ctrl+C, error return,
//! and panic).

use std::io::{self, Stdout};
use std::panic;

use crossterm::cursor::Show;
use crossterm::{execute, terminal as term};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Owns the terminal while the app runs. Restoring on drop covers normal
/// exits, `?` error returns, and panics (via the hook installed in
/// [`enable`]).
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

/// Enter raw mode + alternate screen and install the panic hook.
pub fn enable() -> io::Result<TerminalGuard> {
    term::enable_raw_mode()?;
    execute!(io::stdout(), term::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    install_panic_hook();
    tracing::debug!("terminal entered (raw mode + alternate screen)");
    Ok(TerminalGuard { terminal })
}

/// Best-effort, idempotent restoration. Safe to call multiple times and
/// from the panic hook.
pub fn restore() {
    let _ = execute!(io::stdout(), term::LeaveAlternateScreen, Show);
    let _ = term::disable_raw_mode();
}

fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Restore the terminal first so the panic report is readable and
        // the user's shell is intact, then defer to the previous hook.
        restore();
        previous(info);
    }));
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
        tracing::debug!("terminal restored");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_idempotent_outside_a_tty() {
        // In a non-tty test environment these calls fail harmlessly; the
        // contract under test is "never panics, never corrupts state".
        restore();
        restore();
    }
}
