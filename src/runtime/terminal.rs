//! Terminal lifecycle: raw mode + alternate screen with panic-safe
//! restoration (plan §11: must survive normal exit, Ctrl+C, error return,
//! and panic).

use std::io::{self, Stdout};
use std::panic;

use crossterm::cursor::Show;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
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

/// Enter raw mode + alternate screen and install the panic hook. When
/// `mouse` is set (plan §10, `[tmail].mouse`), mouse capture is enabled so
/// click/wheel events reach the app; otherwise the terminal keeps its
/// native selection behavior and no mouse events arrive.
pub fn enable(mouse: bool) -> io::Result<TerminalGuard> {
    term::enable_raw_mode()?;
    execute!(io::stdout(), term::EnterAlternateScreen)?;
    set_mouse_capture(mouse)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    install_panic_hook();
    tracing::debug!(mouse, "terminal entered (raw mode + alternate screen)");
    Ok(TerminalGuard { terminal })
}

/// Turn mouse capture on or off at runtime (the `m` toggle, plan §10
/// feedback): idempotent, safe to call with the same mode repeatedly.
pub fn set_mouse_capture(enabled: bool) -> io::Result<()> {
    if enabled {
        execute!(io::stdout(), EnableMouseCapture)
    } else {
        execute!(io::stdout(), DisableMouseCapture)
    }
}

/// Best-effort, idempotent restoration. Safe to call multiple times and
/// from the panic hook. Mouse capture is always disabled: it is harmless
/// when never enabled and guarantees restoration after a mid-session
/// enable.
pub fn restore() {
    let _ = execute!(
        io::stdout(),
        term::LeaveAlternateScreen,
        DisableMouseCapture,
        Show
    );
    let _ = term::disable_raw_mode();
}

/// Suspend the TUI so a child program (the external editor, plan §14
/// step 2, Phase 11.2) owns the terminal: leave raw mode and the
/// alternate screen. Best-effort and idempotent, like [`restore`] — which
/// this is.
pub fn suspend() {
    restore();
    tracing::debug!("terminal suspended for the external editor");
}

/// Re-enter the TUI after [`suspend`]: raw mode, alternate screen, and
/// mouse capture per `mouse` — always called after the editor exits,
/// success or failure (plan §14 step 7, Phase 11.7). A *fresh* guard is
/// built on purpose: its ratatui buffers start empty, so the next draw
/// repaints the whole screen without a cursor-position query (which the
/// paused event reader would race for).
pub fn reenter(mouse: bool) -> io::Result<TerminalGuard> {
    term::enable_raw_mode()?;
    execute!(io::stdout(), term::EnterAlternateScreen)?;
    set_mouse_capture(mouse)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    tracing::debug!(mouse, "terminal re-entered after the external editor");
    Ok(TerminalGuard { terminal })
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
