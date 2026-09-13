//! New-mail notification adapters (`[tmail].notifications`, ticket b28p):
//! ring the terminal bell or show a desktop notification through
//! `notify-rust`. Desktop delivery blocks while the platform service
//! answers, so the operation manager runs it on the blocking thread pool
//! and the UI loop never waits.

use std::io;

/// Delivers one new-mail notification found by the background refresh.
pub trait Notifier: Send + Sync {
    /// Ring the terminal bell (`\x07`). Flushes before returning.
    fn bell(&self) -> io::Result<()>;

    /// Show a desktop notification with `summary` as the title and `body`
    /// as the detail text. Blocks until the platform service accepted it.
    fn notify(&self, summary: &str, body: &str) -> Result<(), NotifyError>;
}

/// A desktop-notification failure. The platform adapter's message is kept
/// opaque (its only consumer logs it at warn level), but the type keeps
/// the failure from being smuggled around as user-facing text.
#[derive(Debug, Clone)]
pub struct NotifyError(pub String);

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotifyError {}

/// The system notifier: `\x07` on stdout and `notify-rust` desktop
/// notifications (ticket b28p).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn bell(&self) -> io::Result<()> {
        // `execute!` flushes the escape byte, so the bell rings even
        // while the alternate screen owns the terminal.
        crossterm::execute!(io::stdout(), crossterm::style::Print("\x07"))
    }

    fn notify(&self, summary: &str, body: &str) -> Result<(), NotifyError> {
        notify_rust::Notification::new()
            .summary(summary)
            .body(body)
            .show()
            .map(|_| ())
            .map_err(|err| NotifyError(err.to_string()))
    }
}
