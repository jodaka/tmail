//! Platform open-with adapters (plan §15, Phase 8.5): open a saved
//! attachment, or a link from an HTML body (ticket hc9n), with the OS
//! handler — `open` on macOS, `xdg-open` on Linux, `cmd /C start` on
//! Windows. The opener program is
//! spawned directly by argv, one target per invocation; a shell is never
//! involved in target resolution, so paths with spaces or special
//! characters stay intact (Windows `start` goes through `cmd`, which
//! resolves the target it is handed, never interpolating it). On
//! other platforms there is no opener in v1: the request fails with a
//! clear, typed error instead of guessing.

use std::io;
use std::path::Path;
use std::process::Stdio;

/// Opens a file or a web link with the platform's handler.
///
/// The methods are `async` because the implementation spawns through
/// `tokio::process`, which requires an async runtime context: a sync
/// signature would hide that dependency and make the adapter unusable
/// outside a runtime (plan §3: the main runtime is single-threaded and
/// every process spawn belongs to the async layer).
#[async_trait::async_trait]
pub trait PathOpener: Send + Sync {
    /// Hand `path` to the platform opener. Returns once the opener process
    /// is spawned; the handler app lives its own life from there. The
    /// spawned child is reaped by the async runtime, so no zombies linger.
    async fn open(&self, path: &Path) -> io::Result<()>;

    /// Hand a web link to the platform opener (ticket hc9n) so it opens in
    /// the user's browser. Only [`crate::domain::url::is_openable_url`]
    /// targets are accepted; anything else is refused before an opener is
    /// even looked up, so untrusted HTML can never dispatch local files or
    /// application handlers.
    async fn open_url(&self, url: &str) -> io::Result<()>;
}

/// The system opener: `open` on macOS, `xdg-open` on Linux (plan §15).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemOpener;

/// No opener on the platform: v1 refuses rather than guessing (macOS
/// `open`, Linux `xdg-open`, and Windows `start` are the supported
/// handlers).
fn no_opener() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "no platform opener is configured for this OS (v1 supports \
         macOS `open` and Linux `xdg-open`)",
    )
}

#[async_trait::async_trait]
impl PathOpener for SystemOpener {
    async fn open(&self, path: &Path) -> io::Result<()> {
        let Some(program) = opener_program() else {
            return Err(no_opener());
        };
        // tokio's child is reaped by the runtime's orphan reaper when the
        // handle drops, so a quickly-exiting `open`/`xdg-open` leaves no
        // zombie behind.
        spawn_opener(program, &[path.as_os_str().to_owned()]).await
    }

    async fn open_url(&self, url: &str) -> io::Result<()> {
        if !crate::domain::url::is_openable_url(url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only http and https links can be opened",
            ));
        }
        let Some(program) = opener_program() else {
            return Err(no_opener());
        };
        // The URL travels as a single argv entry (no shell), so query
        // strings and fragments reach the browser byte-for-byte.
        spawn_opener(program, &[std::ffi::OsString::from(url)]).await
    }
}

/// Spawn the platform opener with the target as the trailing argument
/// (never a shell string on Unix). Windows is the one exception where a
/// shell *builtin* is required: `start` is not a program, so it rides on
/// `cmd /C start "" <target>`, the empty title guard keeping a
/// quoted/first-quoted target from being swallowed as the window title
/// (Windows port, issue y90w).
async fn spawn_opener(program: &'static str, args: &[std::ffi::OsString]) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // `program` (the Unix opener name) is only carried for the
        // Unix branch below.
        let _ = program;
        let mut command = tokio::process::Command::new("cmd");
        command.arg("/C").arg("start").arg("").args(args);
        spawn_reaped(&mut command).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = tokio::process::Command::new(program);
        command.arg(&args[0]);
        spawn_reaped(&mut command).await
    }
}

/// Spawn with all three stdio detached: the opener outlives Tmail in
/// every sense the user cares about, so nothing here needs the pipes.
async fn spawn_reaped(command: &mut tokio::process::Command) -> io::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
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

/// The opener program for the current platform (plan §15). On Windows the
/// name is only a marker for the availability check: the actual spawn
/// rides `cmd /C start` (see [`spawn_opener`]).
#[cfg(target_os = "windows")]
fn opener_program() -> Option<&'static str> {
    Some("cmd")
}

/// No opener on other platforms in v1: refusing beats guessing.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn opener_program() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod scheme_tests {
    use super::*;

    /// The scheme gate runs before any opener lookup, so this never
    /// spawns a program: untrusted HTML cannot reach the OS dispatcher
    /// with `file://`, `javascript:`, or a custom scheme (ticket hc9n).
    #[tokio::test]
    async fn system_opener_refuses_non_web_schemes_before_spawning() {
        for url in ["file:///etc/passwd", "javascript:alert(1)", "mailto:x@y.z"] {
            let err = SystemOpener
                .open_url(url)
                .await
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
