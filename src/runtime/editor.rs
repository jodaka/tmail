//! The external editor run (plan §14 steps 3–6, Phase 11).
//!
//! Pure process + file mechanics, no terminal or TUI knowledge: the caller
//! suspends the terminal before and resumes it after (steps 2 and 7). The
//! editor is spawned from argv — program plus arguments, never a shell
//! (step 4) — on a secure temporary file holding the body (step 3), and
//! the run waits for exit (step 5). The edited file is read back on
//! success (step 6); a non-zero exit imports nothing and reports why.
//!
//! Deliberately no timeout or cancellation (ticket tnc1 review): the run
//! is an interactive user session — minutes in an editor are normal — so
//! any timeout would destroy the user's work mid-edit. The TUI is
//! suspended while it runs, by design. A hung editor is user-recoverable:
//! Ctrl-C reaches the foreground process group, the default SIGINT
//! terminates both the editor and tmail, and the terminal guard's Drop
//! restores the screen on that exit path.

use std::fs;
use std::path::Path;

use anyhow::Context;

/// Run `program` (argv; `program[0]` is the executable) over the draft
/// body: write `body` into a secure temporary file, spawn the editor with
/// the file as its final argument, wait for exit, and read the file back.
/// `Ok` carries the edited text; `Err` is an `anyhow` chain (issue pjzr)
/// that keeps every `io::Error` source intact — only the *display* form is
/// flattened at the reducer boundary. The temporary file lives in a
/// private directory removed on return, success or failure.
pub async fn run(program: &[String], body: &str) -> anyhow::Result<String> {
    let Some((argv0, args)) = program.split_first() else {
        anyhow::bail!("no editor is configured");
    };

    // Step 3: a private temporary directory (0o700) holding the body file
    // (0o600). Plan §21: restrictive permissions, cleaned up on return —
    // the TempDir drop removes the whole tree whatever happens.
    let dir = tempfile::Builder::new()
        .prefix("tmail-editor-")
        .tempdir()
        .context("could not create a temporary directory")?;
    let path = dir.path().join("body.txt");
    write_secure(&path, body)?;

    // Steps 4/5: argv-only spawn (the path rides as the final argument) and
    // a wait for exit. `tokio::process` keeps the runtime — and any in-flight
    // draft save — alive while the editor runs.
    let status = tokio::process::Command::new(argv0)
        .args(args)
        .arg(&path)
        .status()
        .await
        .with_context(|| format!("could not run {argv0}"))?;
    if !status.success() {
        anyhow::bail!(match status.code() {
            Some(code) => format!("editor exited with code {code}"),
            None => String::from("editor was terminated by a signal"),
        });
    }

    // Step 6: read the edited text back.
    fs::read_to_string(&path).context("could not read the edited body back")
}

/// Write `body` to `path` with owner-only permissions (plan §21: temporary
/// editor files use restrictive permissions). Unix only; elsewhere the
/// platform default applies (Tmail's external-editor flow is macOS/Linux).
fn write_secure(path: &Path, body: &str) -> anyhow::Result<()> {
    use std::io::Write;
    // Create the file with mode 0o600 from the start (ticket tfxm):
    // creating first and chmodding after leaves a window in which the
    // draft body is readable by others. `create_new` also refuses to
    // follow a pre-planted symlink at the path.
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .context("create temporary file")?
    };
    #[cfg(not(unix))]
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("create temporary file")?;
    file.write_all(body.as_bytes())
        .context("write body to the temporary file")?;
    file.flush().context("flush the temporary file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_the_body_through_the_file() {
        // `true` exits 0 without touching the file: the read-back returns
        // exactly what was handed over.
        let program = vec![String::from("true")];
        let out = run(&program, "line one\nline two\n")
            .await
            .expect("editor run succeeds");
        assert_eq!(out, "line one\nline two\n");
    }

    #[tokio::test]
    async fn a_failing_editor_reports_and_imports_nothing() {
        let program = vec![String::from("false")];
        let err = run(&program, "body").await.expect_err("false exits 1");
        assert!(
            err.to_string().contains("exited with code 1"),
            "top-level display shows the exit: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_missing_editor_reports_instead_of_panicking() {
        let program = vec![String::from("no-such-editor-binary")];
        let err = run(&program, "body").await.expect_err("spawn fails");
        assert!(
            err.to_string().contains("could not run"),
            "top-level display names the editor: {err:#}"
        );
        // The io::Error stays chained (issue pjzr), not flattened away:
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("No such file or directory"),
            "source chain survives into the full display: {rendered}"
        );
    }

    #[test]
    fn temporary_files_are_owner_only() {
        let path = std::env::temp_dir().join("tmail-editor-perm-test");
        write_secure(&path, "x").expect("write");
        let meta = fs::metadata(&path).expect("file exists");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_file(&path);
    }
}
