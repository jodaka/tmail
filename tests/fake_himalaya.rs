//! Fake `himalaya` executable for backend contract tests (plan §19 Phase 2,
//! plan §20 "Backend adapter: fake `himalaya` executable").
//!
//! The fake is a small bash script written into a per-test temp directory
//! with its behavior and record-file paths baked in (no environment
//! variables, so tests never mutate process env). It:
//!
//! 1. records its exact argv (NUL-separated per invocation), letting
//!    contract tests assert precise, shell-free argument handling;
//! 2. records everything it receives on stdin (read-only commands must get
//!    none);
//! 3. emits canned JSON per subcommand, or deliberately broken output
//!    (malformed, non-UTF-8, exit 1 with JSON errors, stderr-only errors)
//!    to prove the adapter fails safely (ADR 0001 finding 1).
//!
//! This file doubles as the shared helper for `backend_contract.rs` via
//! `#[path]`, so everything here is exercised by its own smoke test too.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A fake `himalaya` executable plus its stub config, all inside one
/// temporary directory that is removed when this value is dropped.
pub struct FakeHimalaya {
    program: PathBuf,
    config: PathBuf,
    argv_log: PathBuf,
    stdin_record: PathBuf,
    _dir: TempDir,
}

impl FakeHimalaya {
    /// Spawn the fake with per-subcommand behavior modes:
    ///
    /// mailbox: `ok` | `error-json` | `error-stderr` | `slow`
    /// envelope: `ok` | `empty` | `partial` | `malformed` | `non-utf8`
    ///           | `error-json` | `error-stderr` | `slow`
    pub fn spawn(mailbox_mode: &'static str, envelope_mode: &'static str) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let program = dir.path().join("himalaya");
        let argv_log = dir.path().join("argv.log");
        let stdin_record = dir.path().join("stdin.record");
        let config = dir.path().join("config.toml");

        let script = SCRIPT
            .replace("@ARGV_LOG@", &argv_log.display().to_string())
            .replace("@STDIN_RECORD@", &stdin_record.display().to_string())
            .replace("@MAILBOX_MODE@", mailbox_mode)
            .replace("@ENVELOPE_MODE@", envelope_mode);

        let mut file = fs::File::create(&program).expect("write fake script");
        file.write_all(script.as_bytes())
            .expect("write fake script");
        drop(file);
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("make fake executable");
        fs::write(&config, "[post]\naccount = \"probe\"\n").expect("write stub config");

        Self {
            program,
            config,
            argv_log,
            stdin_record,
            _dir: dir,
        }
    }

    /// The fake with both subcommands behaving like a healthy himalaya.
    pub fn spawn_ok() -> Self {
        Self::spawn("ok", "ok")
    }

    /// Executable path to hand to the backend under test.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Stub config file whose path the backend must forward with `-c`.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Recorded argv per invocation, in call order.
    pub fn argv(&self) -> Vec<Vec<String>> {
        let bytes = fs::read(&self.argv_log).unwrap_or_default();
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|invocation| !invocation.is_empty())
            .map(|invocation| {
                invocation
                    .split(|byte| *byte == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect()
            })
            .collect()
    }

    /// Everything the fake received on stdin across all invocations.
    pub fn stdin_bytes(&self) -> Vec<u8> {
        fs::read(&self.stdin_record).unwrap_or_default()
    }
}

const SCRIPT: &str = r#"#!/usr/bin/env bash
# Fake himalaya for Post contract tests (generated; do not edit by hand).
set -u
ARGV_LOG="@ARGV_LOG@"
STDIN_RECORD="@STDIN_RECORD@"

# Exact argv record: NUL-separated args, one newline per invocation.
printf '%s\0' "$@" >> "$ARGV_LOG"
printf '\n' >> "$ARGV_LOG"

# Read-only commands must receive no stdin; record whatever arrives.
cat > "$STDIN_RECORD"

# Identify the subcommand anywhere in argv (global flags like `-c` come
# first on real invocations).
SUB=""
for a in "$@"; do
  case "$a" in
    mailbox) SUB="mailbox" ;;
    envelope) SUB="envelope" ;;
  esac
done

if [ "$SUB" = "mailbox" ]; then
  case "@MAILBOX_MODE@" in
    ok)
      printf '%s' '{"mailboxes":[{"id":"/root/maildir/INBOX","name":"INBOX","total":null,"unread":null},{"id":"/root/maildir/Archive","name":"Archive","total":null,"unread":null},{"id":"Sent","name":"Sent"}]}'
      ;;
    error-json)
      printf '%s' '{"error":"account not found","sources":["config"]}'
      exit 1
      ;;
    error-stderr)
      printf '%s' 'disk on fire' >&2
      exit 3
      ;;
    slow)
      # Long-running invocation for cancellation tests: hangs for 30s
      # unless killed. It never gets to print.
      sleep 30
      ;;
  esac
  exit 0
fi

if [ "$SUB" = "envelope" ]; then
  case "@ENVELOPE_MODE@" in
    ok)
      printf '%s' '{"envelopes":[{"id":"env-1","message-id":"1@post.local","in-reply-to":[],"flags":[{"raw":"\\Flagged","iana":"flagged"}],"subject":"Welcome","from":[{"name":"Ada","email":"ada@example.org"}],"to":[{"name":null,"email":"probe@post.local"}],"date":"2026-09-02T10:03:40+03:00","size":319,"has-attachment":false},{"id":"env-2","flags":[{"raw":"\\Seen","iana":"seen"}],"subject":"Plain","from":[{"name":null,"email":"bob@example.org"}],"date":"2026-09-02T10:03:40+03:00"},{"id":"env-3","flags":[],"subject":"Grüße 🎉","from":[],"date":null}]}'
      ;;
    empty)
      printf '%s' '{"envelopes":[]}'
      ;;
    partial)
      printf '%s' '{"envelopes":[{"id":"only-id"}]}'
      ;;
    malformed)
      printf '%s' 'not json at all'
      ;;
    non-utf8)
      printf '\377\376\001\002binary junk'
      ;;
    error-json)
      printf '%s' '{"error":"mailbox not found","sources":["maildir"]}'
      exit 1
      ;;
    error-stderr)
      printf '%s' 'boom' >&2
      exit 4
      ;;
    slow)
      # Long-running invocation for cancellation tests: hangs for 30s
      # unless killed. It never gets to print.
      sleep 30
      ;;
  esac
  exit 0
fi

printf '%s' '{"error":"unsupported subcommand"}'
exit 1
"#;

#[test]
fn fake_executes_records_argv_and_stdin() {
    let fake = FakeHimalaya::spawn_ok();
    assert!(fake.config().exists(), "stub config must exist");

    let output = Command::new(fake.program())
        .args(["-c", "/tmp/stub.toml", "mailbox", "list", "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("run fake");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"mailboxes\""));

    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            "/tmp/stub.toml".to_string(),
            "mailbox".to_string(),
            "list".to_string(),
            "--json".to_string(),
        ]]
    );
    assert!(fake.stdin_bytes().is_empty());
}

#[test]
fn fake_envelope_ok_emits_canned_envelopes() {
    let fake = FakeHimalaya::spawn_ok();
    let output = Command::new(fake.program())
        .args([
            "envelope", "list", "-m", "INBOX", "-p", "1", "-s", "20", "--json",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run fake");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Grüße 🎉"));
    assert_eq!(fake.argv().len(), 1);
}
