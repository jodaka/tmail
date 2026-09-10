//! Platform open-with adapters (plan §15, Phase 8.5): open a saved
//! attachment, or a link from an HTML body (ticket hc9n), with the OS
//! handler — `open` on macOS, `xdg-open` on Linux. The opener program is
//! spawned directly by argv, one target per invocation; a shell is never
//! involved, so paths with spaces or special characters stay intact. On
//! other platforms there is no opener in v1: the request fails with a
//! clear, typed error instead of guessing.

use std::io;
use std::path::Path;

/// Opens a file or a web link with the platform's handler.
pub trait PathOpener: Send + Sync {
    /// Hand `path` to the platform opener. Returns once the opener process
    /// is spawned; the handler app lives its own life from there. The
    /// spawned child is reaped by the async runtime, so no zombies linger.
    fn open(&self, path: &Path) -> io::Result<()>;

    /// Hand a web link to the platform opener (ticket hc9n) so it opens in
    /// the user's browser. Only [`crate::domain::url::is_openable_url`]
    /// targets are accepted; anything else is refused before an opener is
    /// even looked up, so untrusted HTML can never dispatch local files or
    /// application handlers.
    fn open_url(&self, url: &str) -> io::Result<()>;
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

    fn open_url(&self, url: &str) -> io::Result<()> {
        if !crate::domain::url::is_openable_url(url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only http and https links can be opened",
            ));
        }
        let Some(program) = opener_program() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no platform opener is configured for this OS (v1 supports \
                 macOS `open` and Linux `xdg-open`)",
            ));
        };
        // The URL travels as a single argv entry (no shell), so query
        // strings and fragments reach the browser byte-for-byte.
        tokio::process::Command::new(program)
            .arg(url)
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

#[cfg(test)]
mod scheme_tests {
    use super::*;

    /// The scheme gate runs before any opener lookup, so this never
    /// spawns a program: untrusted HTML cannot reach the OS dispatcher
    /// with `file://`, `javascript:`, or a custom scheme (ticket hc9n).
    #[test]
    fn system_opener_refuses_non_web_schemes_before_spawning() {
        for url in ["file:///etc/passwd", "javascript:alert(1)", "mailto:x@y.z"] {
            let err = SystemOpener
                .open_url(url)
                .expect_err("non-web scheme must be refused");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{url}");
        }
    }
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

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn linux_uses_xdg_open() {
        assert_eq!(opener_program(), Some("xdg-open"));
    }
}
