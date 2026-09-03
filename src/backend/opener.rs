//! Platform open-with adapters (plan §15, Phase 8.5): open a saved
//! attachment with the OS file handler — `open` on macOS, `xdg-open` on
//! Linux. The opener program is spawned directly by argv, one path per
//! invocation; a shell is never involved, so paths with spaces or special
//! characters stay intact. On other platforms there is no opener in v1:
//! the request fails with a clear, typed error instead of guessing.

use std::io;
use std::path::Path;

/// Opens a file with the platform's handler.
pub trait PathOpener: Send + Sync {
    /// Hand `path` to the platform opener. Returns once the opener process
    /// is spawned; the handler app lives its own life from there. The
    /// spawned child is reaped by the async runtime, so no zombies linger.
    fn open(&self, path: &Path) -> io::Result<()>;
}

/// The system opener: `open` on macOS, `xdg-open` on Linux (plan §15).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemOpener;

impl PathOpener for SystemOpener {
    fn open(&self, path: &Path) -> io::Result<()> {
        let Some(program) = opener_program() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no platform opener is configured for this OS (v1 supports \
                 macOS `open` and Linux `xdg-open`)",
            ));
        };
        // tokio's child is reaped by the runtime's orphan reaper when the
        // handle drops, so a quickly-exiting `open`/`xdg-open` leaves no
        // zombie behind.
        tokio::process::Command::new(program)
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}

/// The opener program for the current platform (plan §15).
#[cfg(target_os = "macos")]
fn opener_program() -> Option<&'static str> {
    Some("open")
}

/// The opener program for the current platform (plan §15).
#[cfg(target_os = "linux")]
fn opener_program() -> Option<&'static str> {
    Some("xdg-open")
}

/// No opener on other platforms in v1: refusing beats guessing.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn opener_program() -> Option<&'static str> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_open() {
        assert_eq!(opener_program(), Some("open"));
    }

    // The spawn path itself is exercised with test doubles in
    // runtime::tasks (RecordingOpener / RefusingOpener): SystemOpener
    // always spawns the real platform program, so unit tests must not
    // call it.
}
