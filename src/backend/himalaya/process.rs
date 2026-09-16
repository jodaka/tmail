//! Child-process handling for the Himalaya CLI (ADR 0001 decision 2).
//!
//! Every invocation uses `tokio::process::Command` with argv arrays only —
//! never a shell string — with stdout/stderr piped and drained by dedicated
//! tasks so a full pipe can never deadlock. `kill_on_drop(true)` is set as
//! a safety net, and the request's cancellation token terminates the owned
//! child with an exact-pid SIGKILL (the pattern proven by the Phase 0
//! probe), so cancelling never leaks a running himalaya or widens the kill
//! to unrelated processes (plan §21).

use std::process::Stdio;

use crate::backend::traits::{BackendError, BackendResult};
use serde::de::DeserializeOwned;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Raw result of one child process run.
pub(crate) struct ChildOutput {
    /// The executable that was run, echoed into command failures so
    /// [`BackendError::Command`] names the configured program (issue 5ab7)
    /// instead of a backend name baked into the shared error type.
    pub program: String,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// How long the cancellation path waits for a SIGKILLed child to be
/// reaped before giving up on it (immediate in practice; the bound only
/// covers a child stuck in uninterruptible disk sleep).
const REAP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// SIGKILL the child's whole process group, falling back to the direct
/// child when the group kill fails, and log whatever could not be
/// killed (ticket 8s0g: the `kill(2)` result was silently ignored, so
/// an `EPERM` left the tree running with nothing said).
fn kill_child_tree(child: &mut tokio::process::Child, pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    // Negative pid targets the child's *process group* (the child was
    // spawned with `process_group(0)`, so its group id is its pid): the
    // SIGKILL reaches the child and everything it forked, closing every
    // pipe write end so the readers above finish immediately (ticket
    // 2b7m). No pid→pgid reuse race: the child is an unreaped zombie or
    // alive at this point — we never `wait` before the kill — so the
    // group it leads cannot have been recycled.
    // SAFETY: kill(2) with an int pgid/signal; the group was created by
    // this spawn and contains only our child tree.
    let sent = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    if sent == 0 {
        return;
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        // The group is already gone (the child exited between spawn and
        // cancellation): nothing left to kill.
        tracing::debug!(pid, "cancelled child's process group is already gone");
        return;
    }
    tracing::warn!(pid, %err, "process-group kill failed; killing the direct child");
    if let Err(kill_err) = child.start_kill() {
        tracing::warn!(pid, %kill_err, "direct-child kill failed too");
    }
}

/// Run `program args` capturing stdout/stderr separately. stdin is null:
/// Phase 2/3 operations are read-only. If `token` fires while the child
/// runs, the child is SIGKILLed by pid and [`BackendError::Cancelled`] is
/// returned.
pub(crate) async fn run(
    program: &str,
    args: &[String],
    token: &CancellationToken,
) -> BackendResult<ChildOutput> {
    run_with_stdin(program, args, None, token).await
}

/// [`run`] with a stdin payload: how serialized drafts/mail reach himalaya
/// (plan §11: "Pipe serialized mail to stdin when required").
pub(crate) async fn run_with_stdin(
    program: &str,
    args: &[String],
    input: Option<&[u8]>,
    token: &CancellationToken,
) -> BackendResult<ChildOutput> {
    tracing::debug!(program, args = ?args, stdin = input.is_some(), "spawning himalaya");
    let mut child = Command::new(program)
        .args(args)
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Own process group (ticket 2b7m): cancellation can then kill the
        // whole tree. Killing only the direct child would leave any
        // grandchild it forked alive *holding the pipe write ends*, and the
        // output readers below would block on EOF until that grandchild
        // exits on its own (observed as a 30s cancellation on CI).
        .process_group(0)
        .spawn()
        // Name the configured program here (issue 5ab7): the spawn failure
        // is the user-facing "executable not found" case, and the bare
        // "No such file or directory" alone tells nothing actionable.
        .map_err(|source| BackendError::Spawn {
            program: program.to_owned(),
            source,
        })?;

    // Write stdin from a dedicated task so a child that never reads cannot
    // block us either. The payload is owned so the writer task is 'static.
    let owned_input: Option<Vec<u8>> = input.map(<[u8]>::to_vec);
    let stdin_task = match (owned_input, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => Some(tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            pipe.write_all(&bytes).await?;
            pipe.shutdown().await
        })),
        _ => None,
    };

    // Drain pipes from separate tasks so a chatty child can never block us.
    let stdout_task = child.stdout.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        })
    });
    let stderr_task = child.stderr.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        })
    });

    // Exact-pid termination of a child we own; SIGKILL for determinism.
    let pid = child.id();
    tokio::select! {
        biased;
        _ = token.cancelled() => {
            kill_child_tree(&mut child, pid);
            // Abort, never await (ticket 8s0g): the kill is expected to
            // close every pipe and let these tasks finish, but a pipe
            // holder the kill could not reach (a D-state child, an
            // EPERM'd group) would otherwise stall cancellation exactly
            // when it must not. Aborting drops each future at its await
            // point and releases the pipe; the buffered output is
            // discarded anyway — a cancelled run reports `Cancelled`,
            // never data.
            if let Some(task) = stdin_task {
                task.abort();
            }
            if let Some(task) = stdout_task {
                task.abort();
            }
            if let Some(task) = stderr_task {
                task.abort();
            }
            // The reap itself is bounded (ticket 8s0g): the SIGKILL
            // above makes the exit immediate, so an expiry here can only
            // mean the signal never reached the process (uninterruptible
            // disk sleep). Dropping the wait future leaves the reaping
            // to tokio's driver, and dropping `child` re-arms
            // `kill_on_drop` for the direct child.
            if tokio::time::timeout(REAP_GRACE, child.wait())
                .await
                .is_err()
            {
                tracing::warn!(pid, "cancelled child did not exit within the reap grace");
            }
            Err(BackendError::Cancelled)
        }
        status = child.wait() => {
            let status = status?;
            if let Some(task) = stdin_task {
                // A write failure (e.g. child died early) surfaces through
                // the exit status; the payload is regenerable.
                let _ = task.await;
            }
            let stdout = match stdout_task {
                Some(task) => join_reader(task).await?,
                None => Vec::new(),
            };
            let stderr = match stderr_task {
                Some(task) => join_reader(task).await?,
                None => Vec::new(),
            };
            Ok(ChildOutput {
                program: program.to_owned(),
                code: status.code(),
                stdout,
                stderr,
            })
        }
    }
}

/// Await one pipe-reader task; a panicked reader is surfaced as I/O.
async fn join_reader(
    task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> BackendResult<Vec<u8>> {
    let bytes = task.await.unwrap_or_else(|join| {
        Err(std::io::Error::other(format!(
            "output reader failed: {join}"
        )))
    })?;
    Ok(bytes)
}

/// Decode one successful child run into the expected DTO (ADR 0001 finding
/// 1: exit status is authoritative; stdout is JSON in both success and
/// error cases). Non-zero exits become [`BackendError::Command`], and any
/// parse failure — including truncated or non-UTF-8 output — becomes a
/// non-panicking [`BackendError::InvalidOutput`].
pub(crate) fn decode<T: DeserializeOwned>(output: ChildOutput) -> BackendResult<T> {
    if output.code != Some(0) {
        return Err(command_error(&output));
    }
    serde_json::from_slice(&output.stdout).map_err(|err| {
        BackendError::InvalidOutput(format!(
            "invalid JSON ({}): {:?}",
            err,
            snippet(&output.stdout)
        ))
    })
}

/// [`decode`] on the blocking pool: parsing child output is CPU work over
/// bytes that can reach tens of megabytes (a full message dump, a large
/// listing), and the main runtime is single-threaded (plan §3) — the frame
/// loop must never spend itself on a serde parse.
pub(crate) async fn decode_on_pool<T: DeserializeOwned + Send + 'static>(
    output: ChildOutput,
) -> BackendResult<T> {
    match tokio::task::spawn_blocking(move || decode(output)).await {
        Ok(result) => result,
        Err(join) => Err(BackendError::Io(std::io::Error::other(format!(
            "decode task failed: {join}"
        )))),
    }
}

/// Build the error for a non-zero exit. Prefers the JSON error object on
/// stdout (ADR 0001 finding 1), then stderr, then a generic note.
fn command_error(output: &ChildOutput) -> BackendError {
    BackendError::Command {
        program: output.program.clone(),
        code: output.code,
        detail: error_detail(output),
    }
}

/// Human-facing diagnostic for a failed child run: the JSON error object on
/// stdout when present, else stderr, else a generic note. Shared with send
/// classification (Phase 7.2), which needs the detail without the error
/// wrapper.
pub(crate) fn error_detail(output: &ChildOutput) -> String {
    detail_from_stdout_json(&output.stdout)
        .or_else(|| nonempty_lossy(&output.stderr))
        .unwrap_or_else(|| "no diagnostic output".to_string())
}

/// Extract `{"error": …, "sources": […]}` text from stdout, if present.
fn detail_from_stdout_json(stdout: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(stdout).ok()?;
    let mut detail = value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)?;
    if let Some(sources) = value.get("sources").and_then(serde_json::Value::as_array) {
        let sources: Vec<&str> = sources
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        if !sources.is_empty() {
            detail.push_str(&format!(" ({})", sources.join("; ")));
        }
    }
    Some(detail)
}

fn nonempty_lossy(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Lossy, bounded preview for error messages; never panics on bad bytes.
fn snippet(bytes: &[u8]) -> String {
    const MAX: usize = 200;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX)]);
    if bytes.len() > MAX {
        format!("{text}…")
    } else {
        text.into_owned()
    }
}
