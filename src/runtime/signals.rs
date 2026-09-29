//! External-kill signal handling (ticket y6s5).
//!
//! Ctrl+C through the tty is a key event and quits cleanly, but `kill
//! -TERM`, a closing terminal emulator (`SIGHUP`), or a user `SIGTSTP`
//! terminates/suspends the process without Rust unwinding: `TerminalGuard`'s
//! `Drop` and the panic hook never run, and the shell is left in raw mode /
//! alternate screen — the classic broken-terminal state recoverable only
//! with `reset`.
//!
//! The watcher here catches those signals so the event loop can restore the
//! terminal first and then exit (or suspend, for `SIGTSTP`, the way
//! well-behaved TUI programs do: restore, re-raise with the default
//! disposition so the kernel actually stops the process, and re-enter after
//! `SIGCONT` resumes it).

#[cfg(unix)]
use tokio::signal::unix::{Signal, SignalKind, signal};

/// What one caught signal asks the event loop to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// Restore and exit with the POSIX shell convention `128 + signal`.
    Exit(u8),
    /// Suspend the process (`SIGTSTP`): restore the terminal first, then
    /// re-raise the signal under its default disposition.
    Suspend,
}

/// The signals the event loop watches (ticket y6s5). On Unix: `SIGTERM`
/// (external kill), `SIGHUP` (the terminal emulator closed), and `SIGTSTP`
/// (user suspend). On other platforms there is nothing to watch and the
/// action future never fires. The registrations sit behind `Option` so the
/// suspend path can drop them in place (a live registration would swallow
/// the re-raised `SIGTSTP` and the process would never stop).
pub struct ExitSignals {
    #[cfg(unix)]
    term: Option<Signal>,
    #[cfg(unix)]
    hup: Option<Signal>,
    #[cfg(unix)]
    tstp: Option<Signal>,
}

impl ExitSignals {
    /// Install the watchers. Idempotent at the process level (tokio
    /// replaces the sigaction), safe before or after the terminal enters
    /// raw mode.
    pub fn install() -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                term: Some(signal(SignalKind::terminate())?),
                hup: Some(signal(SignalKind::hangup())?),
                // `from_raw`: no `SignalKind` constructor spells SIGTSTP.
                tstp: Some(signal(SignalKind::from_raw(libc::SIGTSTP))?),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    /// Wait for the next caught signal (ticket y6s5): a `SIGTERM` or
    /// `SIGHUP` maps to an exit code, a `SIGTSTP` to a suspend. On
    /// platforms without Unix signals this never resolves.
    pub async fn next(&mut self) -> SignalAction {
        #[cfg(unix)]
        {
            let term = self.term.as_mut().expect("term watcher installed");
            let hup = self.hup.as_mut().expect("hup watcher installed");
            let tstp = self.tstp.as_mut().expect("tstp watcher installed");
            tokio::select! {
                _ = term.recv() => SignalAction::Exit(128 + 15),
                _ = hup.recv() => SignalAction::Exit(128 + 1),
                _ = tstp.recv() => SignalAction::Suspend,
            }
        }
        #[cfg(not(unix))]
        {
            std::future::pending().await
        }
    }

    /// Suspend the process after [`crate::runtime::terminal::restore`] has
    /// put the terminal back: the tokio registrations are dropped, the
    /// `SIGTSTP` disposition is reset to the kernel default, and the
    /// signal is re-raised — so the process actually stops (a caught
    /// signal alone would never stop it). When the user resumes with
    /// `fg`/`SIGCONT`, this call returns and the caller re-enters the TUI
    /// with a fresh watcher (a fresh [`ExitSignals::install`], which
    /// re-registers every stream) before the next wait.
    #[cfg(unix)]
    pub fn suspend_after_restore(&mut self) {
        // Drop the registrations first: a live tokio `Signal` holds a
        // sigaction that would swallow the re-raised signal below.
        self.term = None;
        self.hup = None;
        self.tstp = None;
        // SAFETY: signal()/raise() with libc constants; this is the
        // standard TUI suspend dance (restore → SIG_DFL → raise).
        unsafe {
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::raise(libc::SIGTSTP);
        }
        // The process stopped above and was resumed (SIGCONT): the tokio
        // registration is re-established by the fresh watcher the caller
        // installs before the next wait.
    }

    /// Non-Unix builds never suspend.
    #[cfg(not(unix))]
    pub fn suspend_after_restore(self) {
        let _ = self;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Self-sent signals resolve the watcher with the shell's exit-code
    /// convention (ticket y6s5): the handlers must be installed so the
    /// default kills never happen. One sequential test on purpose —
    /// parallel tests would cross-deliver each other's signals (the
    /// process-wide signal source broadcasts to every watcher).
    #[tokio::test]
    async fn self_sent_sigterm_and_sighup_map_to_the_posix_exit_codes() {
        let mut signals = ExitSignals::install().expect("signal watchers install");
        // A real SIGTERM to self: the watcher owns the disposition now, so
        // the process survives and the arm fires.
        let sent = unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
        assert_eq!(sent, 0, "SIGTERM sent to self");
        let action = tokio::time::timeout(std::time::Duration::from_secs(5), signals.next())
            .await
            .expect("the signal watcher resolves");
        assert_eq!(action, SignalAction::Exit(143), "SIGTERM → 128+15");

        // SIGHUP (a closing terminal emulator) maps to its own code.
        let sent = unsafe { libc::kill(libc::getpid(), libc::SIGHUP) };
        assert_eq!(sent, 0, "SIGHUP sent to self");
        let action = tokio::time::timeout(std::time::Duration::from_secs(5), signals.next())
            .await
            .expect("the signal watcher resolves");
        assert_eq!(action, SignalAction::Exit(129), "SIGHUP → 128+1");
    }
}
