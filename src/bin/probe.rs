//! Phase 0 probe: verifies that Post can drive the Himalaya CLI through
//! tokio::process::Command (no shell), capture stdout/stderr separately, pipe
//! stdin, and cancel a running child process.
//!
//! Usage: cargo run --bin probe -- <himalaya-config> [all|argv|stdin|cancel]

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

struct Outcome {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs `program args` with argv-style arguments (never a shell), capturing
/// stdout/stderr separately, piping stdin when provided, and terminating the
/// child if the token fires while it is still running.
async fn run_cancellable(
    program: &str,
    args: &[&str],
    stdin_bytes: Option<&[u8]>,
    token: &CancellationToken,
) -> anyhow::Result<Outcome> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn {program}"))?;

    // Drain pipes from separate tasks so a full pipe can never deadlock us.
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        })
    });
    let stdin_writer = stdin_bytes.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut stdin = child.stdin.take();
        tokio::spawn(async move {
            if let Some(stdin) = stdin.as_mut() {
                stdin.write_all(&bytes).await?;
                stdin.shutdown().await?;
            }
            Ok::<_, std::io::Error>(())
        })
    });

    let pid = child.id();
    let cancelled = token.clone();

    tokio::select! {
        _ = cancelled.cancelled() => {
            // Exact-pid termination of a child we own; SIGKILL for determinism.
            if let Some(pid) = pid {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            }
            let _ = child.wait().await;
            if let Some(t) = stdout_reader {
                let _ = t.await;
            }
            if let Some(t) = stderr_reader {
                let _ = t.await;
            }
            if let Some(t) = stdin_writer {
                let _ = t.await;
            }
            bail!("cancelled before completion")
        }
        status = child.wait() => {
            let status = status?;
            token.cancel();
            let stdout = match stdout_reader {
                Some(t) => t.await??,
                None => Vec::new(),
            };
            let stderr = match stderr_reader {
                Some(t) => t.await??,
                None => Vec::new(),
            };
            if let Some(t) = stdin_writer {
                t.await??;
            }
            Ok(Outcome { code: status.code(), stdout, stderr })
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut argv = std::env::args().skip(1);
    let config = argv
        .next()
        .context("usage: probe <config> [all|argv|stdin|cancel]")?;
    let step = argv.next().unwrap_or_else(|| "all".into());

    let run_all = step == "all";

    // ── 1. argv invocation + JSON capture (no shell) ────────────────────────
    if run_all || step == "argv" {
        let token = CancellationToken::new();
        let started = Instant::now();
        let out = run_cancellable(
            "himalaya",
            &[
                "-c", &config, "-a", "probe", "envelope", "list", "-m", "INBOX", "--json",
            ],
            None,
            &token,
        )
        .await?;
        assert_eq!(
            out.code,
            Some(0),
            "himalaya exited nonzero: {:?}",
            out.stderr
        );
        let parsed: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let count = parsed["envelopes"].as_array().map_or(0, |a| a.len());
        println!(
            "1. argv+json    OK   envelopes={count} elapsed={:?}",
            started.elapsed()
        );
    }

    // ── 2. stdin piping (draft add path used by composer) ───────────────────
    if run_all || step == "stdin" {
        let token = CancellationToken::new();
        let raw = b"Message-ID: <probe-stdin@post.local>\r\nFrom: probe@post.local\r\n\
             To: alice@example.org\r\nSubject: stdin probe\r\n\r\nbody\r\n";
        let out = run_cancellable(
            "himalaya",
            &[
                "-c", &config, "-a", "probe", "message", "add", "-m", "Drafts", "--flag", "draft",
                "--json",
            ],
            Some(raw),
            &token,
        )
        .await?;
        assert_eq!(out.code, Some(0));
        let parsed: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let id = parsed["id"]
            .as_str()
            .context("no id in add output")?
            .to_string();
        println!("2. stdin pipe   OK   draft_id={id}");
    }

    // ── 3. cancellation kills a slow child before it finishes ──────────────
    if run_all || step == "cancel" {
        let token = CancellationToken::new();
        let token_clone = token.clone();
        let started = Instant::now();
        let worker =
            tokio::spawn(
                async move { run_cancellable("sleep", &["30"], None, &token_clone).await },
            );
        tokio::time::sleep(Duration::from_millis(150)).await;
        token.cancel();
        let joined = worker.await?;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "cancel took too long: {elapsed:?}"
        );
        println!(
            "3. cancellation OK   elapsed={elapsed:?} (child killed, task errored as expected: {})",
            joined.is_err()
        );
    }

    println!("probe: all steps passed");
    Ok(())
}
