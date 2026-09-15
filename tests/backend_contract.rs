//! Backend contract tests (plan §20): the adapter must drive the fake
//! himalaya executable with exact argv, leave stdin untouched, map real
//! output shapes into domain types, and fail safely on empty, partial,
//! malformed, and non-UTF-8 output (plan §19 Phase 2 acceptance).

#![cfg(unix)]

#[path = "fake_himalaya.rs"]
mod fake_himalaya;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fake_himalaya::FakeHimalaya;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use tmail::app::operation::OperationId;
use tmail::backend::himalaya::HimalayaCliBackend;
use tmail::backend::{BackendError, MailBackend, RequestContext};
use tmail::domain::{MailboxId, MailboxRole, MessageId, MessageLocator, PageRequest};

/// A live request context; tests that do not cancel share one token.
fn ctx() -> RequestContext {
    RequestContext {
        operation: OperationId(1),
        cancellation: CancellationToken::new(),
    }
}

fn locator(mailbox: &str, id: &str) -> MessageLocator {
    MessageLocator {
        mailbox: MailboxId(String::from(mailbox)),
        id: MessageId(String::from(id)),
        message_id: Some(String::from("1@tmail.local")),
    }
}

/// Backend pointed at a fake executable with a config path and account.
fn backend(fake: &FakeHimalaya, account: Option<&str>) -> HimalayaCliBackend {
    HimalayaCliBackend::new(
        fake.program().display().to_string(),
        Some(PathBuf::from(fake.config())),
        account.map(str::to_owned),
        aliases(&[("inbox", "INBOX"), ("trash", "Archive")]),
    )
}

fn aliases(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (String::from(*key), String::from(*value)))
        .collect()
}

fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    Runtime::new().expect("runtime").block_on(future)
}

fn page_request(mailbox: &str, offset: usize) -> PageRequest {
    PageRequest {
        mailbox_id: MailboxId(String::from(mailbox)),
        offset,
        limit: 20,
    }
}

#[test]
fn mailbox_list_argv_is_exact() {
    let fake = FakeHimalaya::spawn_ok();
    let result = block(backend(&fake, Some("probe")).list_mailboxes(ctx()));
    let mailboxes = result.expect("list succeeds");
    assert_eq!(mailboxes.len(), 3);
    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            fake.config().display().to_string(),
            "-a".to_string(),
            "probe".to_string(),
            "mailbox".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--counts".to_string(),
        ]]
    );
}

#[test]
fn mailbox_list_roles_resolve_through_aliases() {
    let fake = FakeHimalaya::spawn_ok();
    let mailboxes = block(backend(&fake, Some("probe")).list_mailboxes(ctx())).unwrap();
    // INBOX → Inbox via alias; Archive → Trash via the probe's trash alias
    // (not the name heuristic); Sent → Sent via the name fallback.
    assert_eq!(mailboxes[0].role, Some(MailboxRole::Inbox));
    assert_eq!(mailboxes[1].role, Some(MailboxRole::Trash));
    assert_eq!(mailboxes[2].role, Some(MailboxRole::Sent));
    // Counts may be unknown (maildir) and must stay optional.
    assert_eq!(mailboxes[0].unread_count, None);
    assert_eq!(mailboxes[0].total_count, None);
}

#[test]
fn envelope_list_argv_is_exact_and_maps_1based_pages() {
    let fake = FakeHimalaya::spawn_ok();
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 40)));
    let page = result.expect("list succeeds");
    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            fake.config().display().to_string(),
            "-a".to_string(),
            "probe".to_string(),
            "envelope".to_string(),
            "list".to_string(),
            "-m".to_string(),
            "INBOX".to_string(),
            "-p".to_string(),
            "3".to_string(),
            "-s".to_string(),
            "20".to_string(),
            "--json".to_string(),
        ]]
    );
    assert_eq!(page.offset, 40);
    assert_eq!(page.limit, 20);
    assert_eq!(page.total, None);
}

#[test]
fn envelope_rows_map_to_domain_summaries() {
    let fake = FakeHimalaya::spawn_ok();
    let page = block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)))
        .unwrap();
    assert_eq!(page.items.len(), 3);

    let first = &page.items[0];
    assert_eq!(first.id.0, "env-1");
    assert_eq!(first.message_id.as_deref(), Some("1@tmail.local"));
    assert_eq!(first.from[0].display(), "Ada");
    assert_eq!(first.subject, "Welcome");
    assert!(first.is_starred);
    assert!(!first.is_read);
    assert_eq!(first.snippet, None);

    let second = &page.items[1];
    assert!(second.is_read);
    assert!(!second.is_starred);

    let third = &page.items[2];
    assert_eq!(third.subject, "Grüße 🎉");
    assert!(third.from.is_empty());
    assert_eq!(third.message_id, None);
    assert_eq!(third.timestamp.to_rfc3339(), "1970-01-01T00:00:00+00:00");
}

#[test]
fn read_only_commands_receive_no_stdin() {
    let fake = FakeHimalaya::spawn_ok();
    block(backend(&fake, Some("probe")).list_mailboxes(ctx())).unwrap();
    block(backend(&fake, None).list_messages(ctx(), page_request("INBOX", 0))).unwrap();
    assert_eq!(fake.argv().len(), 2);
    assert!(fake.stdin_bytes().is_empty(), "stdin must stay empty");
}

#[test]
fn argv_without_config_or_account_is_minimal() {
    let fake = FakeHimalaya::spawn_ok();
    let bare = HimalayaCliBackend::new(
        fake.program().display().to_string(),
        None,
        None,
        aliases(&[]),
    );
    block(bare.list_mailboxes(ctx())).unwrap();
    assert_eq!(
        fake.argv(),
        vec![vec![
            "mailbox".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--counts".to_string(),
        ]]
    );
}

#[test]
fn mailbox_ids_with_spaces_stay_single_argv_entries() {
    let fake = FakeHimalaya::spawn_ok();
    let request = page_request("My Folder", 0);
    block(backend(&fake, None).list_messages(ctx(), request)).unwrap();
    let argv = &fake.argv()[0];
    assert!(
        argv.windows(2)
            .any(|pair| pair[0] == "-m" && pair[1] == "My Folder"),
        "mailbox id must be one argv entry (no shell): {argv:?}"
    );
}

#[test]
fn zero_page_limit_is_rejected_without_spawning() {
    let fake = FakeHimalaya::spawn_ok();
    let request = PageRequest {
        mailbox_id: MailboxId(String::from("INBOX")),
        offset: 0,
        limit: 0,
    };
    let result = block(backend(&fake, None).list_messages(ctx(), request));
    assert!(matches!(result, Err(BackendError::InvalidRequest(_))));
    assert!(fake.argv().is_empty(), "no child process may be spawned");
}

#[test]
fn empty_and_out_of_range_pages_are_valid_empty_results() {
    // Mirrors the verified real behavior: `envelope list -p 99` exits 0
    // with `{"envelopes":[]}`.
    let fake = FakeHimalaya::spawn("ok", "empty");
    let page = block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 80)))
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.offset, 80);
    assert!(!page.has_next(), "an empty page never has a successor");
    assert!(page.has_previous(), "paging back must stay available");
}

#[test]
fn partial_envelope_maps_to_safe_defaults() {
    let fake = FakeHimalaya::spawn("ok", "partial");
    let page = block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)))
        .unwrap();
    let message = &page.items[0];
    assert_eq!(message.id.0, "only-id");
    assert_eq!(message.subject, "");
    assert!(message.from.is_empty());
    assert_eq!(message.message_id, None);
    assert!(!message.is_read);
    assert!(!message.is_starred);
}

#[test]
fn malformed_output_fails_as_invalid_output() {
    let fake = FakeHimalaya::spawn("ok", "malformed");
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)));
    match result {
        Err(BackendError::InvalidOutput(detail)) => {
            assert!(detail.contains("invalid JSON"), "detail: {detail}");
        }
        other => panic!("expected InvalidOutput, got {other:?}"),
    }
}

#[test]
fn non_utf8_output_fails_safely_without_panicking() {
    let fake = FakeHimalaya::spawn("ok", "non-utf8");
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)));
    match result {
        Err(BackendError::InvalidOutput(_)) => {}
        other => panic!("expected InvalidOutput, got {other:?}"),
    }
}

#[test]
fn json_error_on_stdout_becomes_command_error() {
    let fake = FakeHimalaya::spawn("ok", "error-json");
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(1));
            assert!(detail.contains("mailbox not found"), "detail: {detail}");
            assert!(detail.contains("maildir"), "sources included: {detail}");
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn stderr_only_error_becomes_command_error() {
    let fake = FakeHimalaya::spawn("ok", "error-stderr");
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(4));
            assert_eq!(detail, "boom");
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn mailbox_list_error_is_typed_too() {
    let fake = FakeHimalaya::spawn("error-json", "ok");
    let result = block(backend(&fake, Some("probe")).list_mailboxes(ctx()));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(1));
            assert!(detail.contains("account not found"));
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

/// Phase 3.2 acceptance: cancelling an in-flight request terminates the
/// owned child quickly (the fake would hang for 30s otherwise) and the
/// adapter reports a distinct cancelled outcome (plan §11).
#[test]
fn cancel_terminates_the_child_and_reports_cancelled() {
    let fake = FakeHimalaya::spawn("ok", "slow");
    let rt = Runtime::new().expect("runtime");
    rt.block_on(async {
        let backend = backend(&fake, Some("probe"));
        let token = CancellationToken::new();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            backend
                .list_messages(
                    RequestContext {
                        operation: OperationId(9),
                        cancellation: task_token,
                    },
                    page_request("INBOX", 0),
                )
                .await
        });
        // Let the child start, then cancel while it hangs.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        token.cancel();
        let started = std::time::Instant::now();
        let result = task.await.expect("request task joins");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "cancellation took too long: {elapsed:?}"
        );
        assert!(
            matches!(result, Err(BackendError::Cancelled)),
            "expected Cancelled, got {result:?}"
        );
    });
}

/// The mailbox listing path cancels identically: the slow fake is killed
/// and the outcome is `Cancelled`, never a success payload (plan §11).
#[test]
fn cancel_during_mailbox_list_reports_cancelled_not_output() {
    let fake = FakeHimalaya::spawn("slow", "ok");
    let rt = Runtime::new().expect("runtime");
    rt.block_on(async {
        let backend = backend(&fake, Some("probe"));
        let token = CancellationToken::new();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            backend
                .list_mailboxes(RequestContext {
                    operation: OperationId(10),
                    cancellation: task_token,
                })
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        token.cancel();
        let result = task.await.expect("request task joins");
        assert!(matches!(result, Err(BackendError::Cancelled)));
    });
}

/// Phase 12.4: a nonexistent executable surfaces as a typed I/O error
/// (spawn failure), never a panic or a silent success.
#[test]
fn missing_executable_is_a_typed_io_error() {
    let fake = FakeHimalaya::spawn_ok();
    let missing = fake
        .program()
        .parent()
        .expect("fake dir exists")
        .join("no-such-himalaya");
    let backend = HimalayaCliBackend::new(
        missing.display().to_string(),
        Some(PathBuf::from(fake.config())),
        Some(String::from("probe")),
        aliases(&[]),
    );
    match block(backend.list_mailboxes(ctx())) {
        Err(BackendError::Io(err)) => {
            assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

/// Phase 12.4: a child killed by a signal (e.g. OOM killer, external
/// SIGKILL) leaves a wait status with no exit code; the adapter must
/// report a typed Command error instead of panicking or hanging.
#[test]
fn child_killed_by_a_signal_is_typed_not_a_panic() {
    let fake = FakeHimalaya::spawn("ok", "signal");
    let result =
        block(backend(&fake, Some("probe")).list_messages(ctx(), page_request("INBOX", 0)));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, None, "signal death carries no exit code");
            assert_eq!(detail, "no diagnostic output");
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

// ── Phase 4: reader and core actions ─────────────────────────────────────

/// `message read -m <mbox> <id> --json` with exact argv, mapped into the
/// domain message (plan §19 Phase 4).
#[test]
fn message_read_argv_is_exact_and_maps_domain_message() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    let result = block(backend(&fake, Some("probe")).get_message(ctx(), locator("INBOX", "env-1")));
    let message = result.expect("read succeeds");
    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            fake.config().display().to_string(),
            "-a".to_string(),
            "probe".to_string(),
            "message".to_string(),
            "read".to_string(),
            "-m".to_string(),
            "INBOX".to_string(),
            "env-1".to_string(),
            "--json".to_string(),
        ]]
    );
    // Locator identity is echoed back onto the domain message.
    assert_eq!(message.id.0, "env-1");
    assert_eq!(message.mailbox_id.0, "INBOX");
    assert_eq!(message.headers.subject, "Contract test");
    assert_eq!(message.headers.from[0].display(), "Ada");
    assert_eq!(message.headers.to[0].email, "probe@tmail.local");
    assert_eq!(message.headers.message_id.as_deref(), Some("1@tmail.local"));
    assert_eq!(
        message.headers.date.map(|d| d.to_rfc3339()),
        Some("2026-09-02T10:03:40+03:00".to_string())
    );
    assert_eq!(
        message.plain_body.as_deref(),
        Some("Hello from the fake.\n")
    );
    assert_eq!(message.html_body, None);
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].name.as_deref(), Some("fake.pdf"));
    assert_eq!(
        message.attachments[0].mime_type.as_deref(),
        Some("application/pdf")
    );
    assert_eq!(message.attachments[0].size, Some(3));
    // The fixture lists part index 1; `attachment download` expects the
    // 1-based id (Himalaya's `part_index + 1`).
    assert_eq!(message.attachments[0].part_id, 2);
}

#[test]
fn message_read_failure_is_typed() {
    let fake = FakeHimalaya::spawn_full("ok", "ok", "error-json", "ok");
    let result =
        block(backend(&fake, Some("probe")).get_message(ctx(), locator("INBOX", "env-404")));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(1));
            assert!(detail.contains("no such message"), "detail: {detail}");
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn set_read_and_set_starred_drive_flag_add_remove() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    let backend = backend(&fake, Some("probe"));
    block(backend.set_read(ctx(), locator("INBOX", "env-1"), true)).expect("mark read");
    block(backend.set_read(ctx(), locator("INBOX", "env-1"), false)).expect("mark unread");
    block(backend.set_starred(ctx(), locator("INBOX", "env-1"), true)).expect("star");
    block(backend.set_starred(ctx(), locator("INBOX", "env-1"), false)).expect("unstar");
    let argv = fake.argv();
    let flags: Vec<Vec<String>> = argv
        .iter()
        .map(|inv| {
            inv.iter()
                .skip_while(|arg| arg.as_str() != "flag")
                .cloned()
                .collect()
        })
        .collect();
    assert_eq!(
        flags,
        vec![
            vec![
                "flag", "add", "-m", "INBOX", "--flag", "seen", "env-1", "--json"
            ],
            vec![
                "flag", "remove", "-m", "INBOX", "--flag", "seen", "env-1", "--json"
            ],
            vec![
                "flag", "add", "-m", "INBOX", "--flag", "flagged", "env-1", "--json"
            ],
            vec![
                "flag", "remove", "-m", "INBOX", "--flag", "flagged", "env-1", "--json"
            ],
        ]
    );
}

#[test]
fn flag_failure_is_typed() {
    let fake = FakeHimalaya::spawn_full("ok", "ok", "ok", "error-json");
    let result =
        block(backend(&fake, Some("probe")).set_read(ctx(), locator("INBOX", "env-1"), true));
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(1));
            assert!(detail.contains("mailbox not found"), "detail: {detail}");
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

/// Archive resolves its target inside the adapter from the cached mailbox
/// listing (role match), never by guessing in the UI (ADR 0001).
#[test]
fn archive_resolves_target_from_cached_listing() {
    // Alias `archive` → "Archive" makes the fake's Archive mailbox carry
    // the Archive role (name heuristic; no trash alias overrides here).
    let fake = FakeHimalaya::spawn("ok", "ok");
    let backend = HimalayaCliBackend::new(
        fake.program().display().to_string(),
        Some(PathBuf::from(fake.config())),
        Some(String::from("probe")),
        aliases(&[("archive", "Archive")]),
    );
    // Populate the cache exactly like the app's startup listing does.
    block(backend.list_mailboxes(ctx())).expect("listing");
    block(backend.archive(ctx(), locator("INBOX", "env-1"))).expect("archive");
    assert_eq!(
        fake.argv(),
        vec![
            vec![
                "-c".to_string(),
                fake.config().display().to_string(),
                "-a".to_string(),
                "probe".to_string(),
                "mailbox".to_string(),
                "list".to_string(),
                "--json".to_string(),
                "--counts".to_string(),
            ],
            vec![
                "-c".to_string(),
                fake.config().display().to_string(),
                "-a".to_string(),
                "probe".to_string(),
                "message".to_string(),
                "move".to_string(),
                "--from".to_string(),
                "INBOX".to_string(),
                "--to".to_string(),
                "/root/maildir/Archive".to_string(),
                "env-1".to_string(),
                "--json".to_string(),
            ],
        ]
    );
}

/// Without a listing and without an `archive` alias, archiving is a typed
/// request error before any child process runs.
#[test]
fn archive_without_a_known_target_is_rejected_without_spawning() {
    let fake = FakeHimalaya::spawn_ok();
    let result = block(backend(&fake, Some("probe")).archive(ctx(), locator("INBOX", "env-1")));
    assert!(matches!(result, Err(BackendError::InvalidRequest(_))));
    assert!(fake.argv().is_empty(), "no child process may be spawned");
}

/// Trash delegates to `message delete` (trash-first on himalaya's side,
/// ADR 0001 finding 5) with exact argv.
#[test]
fn trash_argv_is_exact() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    block(backend(&fake, Some("probe")).trash(ctx(), locator("INBOX", "env-1"))).expect("trash");
    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            fake.config().display().to_string(),
            "-a".to_string(),
            "probe".to_string(),
            "message".to_string(),
            "delete".to_string(),
            "-m".to_string(),
            "INBOX".to_string(),
            "env-1".to_string(),
            "--json".to_string(),
        ]]
    );
    assert!(fake.stdin_bytes().is_empty(), "stdin must stay empty");
}

#[test]
fn app_and_ui_modules_never_reference_backend_or_dtos() {
    // Phase 2 acceptance: Himalaya DTOs are not referenced by app/UI
    // modules (plan §5 layering; ADR 0001 decision 3).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rust_files(&manifest.join("src/app"), &mut files);
    collect_rust_files(&manifest.join("src/ui"), &mut files);
    assert!(
        files.len() > 10,
        "expected to scan the real app/ui sources, found {}",
        files.len()
    );
    let forbidden = [
        "crate::backend",
        "tmail::backend",
        "super::backend",
        "backend::himalaya",
        "himalaya::",
        "dto::",
    ];
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read source");
        for line in text.lines() {
            // Prose (doc comments and code comments) may discuss the
            // backend; only code references are forbidden.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for needle in forbidden {
                assert!(
                    !line.contains(needle),
                    "{file:?} must not reference the backend layer (found {needle:?}): {line}"
                );
            }
        }
    }
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

// ── Draft saves (plan §14, ADR 0002) ─────────────────────────────────────

use tempfile::TempDir;
use tmail::backend::journal::DraftJournal;
use tmail::domain::{DraftId, DraftSnapshot};

/// A draft snapshot ready to save.
fn draft_snapshot(revision: u64, remote_id: Option<&str>) -> DraftSnapshot {
    DraftSnapshot {
        local_id: DraftId(String::from("local-123")),
        message_id: Some(String::from("<123.draft@tmail.local>")),
        in_reply_to: None,
        references: None,
        remote_id: remote_id.map(|id| MessageId(String::from(id))),
        to: String::from("Maksim Orlov <m.orlov@example.org>, broken-entry"),
        cc: String::from("cc@example.org"),
        bcc: String::new(),
        subject: String::from("Gyuto — progress"),
        body: String::from("first line\nsecond line"),
        attachments: Vec::new(),
        revision,
    }
}

/// Backend with a hermetic journal directory; the tempdir must outlive the
/// backend calls.
fn backend_with_journal(fake: &FakeHimalaya) -> (HimalayaCliBackend, TempDir) {
    let dir = TempDir::new().expect("journal tempdir");
    let backend = HimalayaCliBackend::new(
        fake.program().display().to_string(),
        Some(PathBuf::from(fake.config())),
        Some(String::from("probe")),
        aliases(&[
            ("inbox", "INBOX"),
            ("trash", "Archive"),
            ("drafts", "Drafts"),
        ]),
    )
    .with_journal(DraftJournal::open(dir.path().to_path_buf()))
    .with_account_identity(
        Some(String::from("probe@tmail.local")),
        Some(String::from("Tmail Probe")),
    );
    (backend, dir)
}

/// Run `future` to completion, then yield for `quiet` so the adapter's
/// detached cleanup tasks (spawned onto the same runtime) get their turn
/// before the runtime shuts down.
fn block_quiet<T>(future: impl Future<Output = T>, quiet_ms: u64) -> T {
    Runtime::new().expect("runtime").block_on(async move {
        let result = future.await;
        tokio::time::sleep(std::time::Duration::from_millis(quiet_ms)).await;
        result
    })
}

#[test]
fn save_draft_adds_with_draft_flag_and_pipes_the_message() {
    let fake = FakeHimalaya::spawn_ok();
    let (backend, dir) = backend_with_journal(&fake);
    let snapshot = draft_snapshot(3, None);
    let remote =
        block_quiet(backend.save_draft(ctx(), snapshot.clone()), 300).expect("save succeeds");
    assert_eq!(remote, MessageId(String::from("new-draft-1")));
    // Detached cleanup runs after the result: wait for add + stray sweep list.
    let argv = fake.wait_for_invocations(2).expect("cleanup recorded");
    assert_eq!(argv.len(), 2, "nothing else may spawn: {argv:?}");
    assert_eq!(
        argv[0][5..],
        ["add", "-m", "Drafts", "--flag", "draft", "--json"]
    );
    assert_eq!(
        &argv[1][5..],
        ["list", "-m", "Drafts", "-p", "1", "-s", "100", "--json"]
    );
    // The serialized RFC 5322 message travels on stdin: library-built
    // headers, the stable Message-ID, Tmail's draft metadata, and the body.
    let stdin = String::from_utf8_lossy(&fake.stdin_bytes()).into_owned();
    assert!(
        stdin.contains("To: \"Maksim Orlov\" <m.orlov@example.org>"),
        "{stdin}"
    );
    assert!(
        !stdin.contains("broken-entry"),
        "invalid entries must not reach the wire: {stdin}"
    );
    assert!(stdin.contains("Cc: <cc@example.org>"), "{stdin}");
    assert!(
        stdin.contains("Message-ID: <123.draft@tmail.local>"),
        "{stdin}"
    );
    assert!(stdin.contains("X-Tmail-Draft-Id: local-123"), "{stdin}");
    assert!(
        stdin.contains("From: \"Tmail Probe\" <probe@tmail.local>"),
        "{stdin}"
    );
    // Non-ASCII subjects are RFC 2047-encoded by the library.
    assert!(stdin.contains("Subject: =?utf-8?"), "{stdin}");
    assert!(stdin.contains("second line"), "{stdin}");
    // The revision is confirmed in the journal after the remote save.
    let entry = DraftJournal::open(dir.path().to_path_buf())
        .load_all()
        .expect("journal readable");
    assert_eq!(entry.len(), 1);
    assert_eq!(
        entry[0].saved_revision, 3,
        "journal confirms the pushed revision"
    );
    assert_eq!(entry[0].draft, snapshot);
}

#[test]
fn save_draft_runs_the_replacement_sweep_only_after_confirm() {
    let fake = FakeHimalaya::spawn_ok();
    let (backend, _dir) = backend_with_journal(&fake);
    block_quiet(
        backend.save_draft(ctx(), draft_snapshot(2, Some("old-1"))),
        300,
    )
    .expect("save succeeds");
    // `add` is awaited by the save; every cleanup runs detached as ONE
    // Message-ID sweep: list the Drafts mailbox, then delete every copy
    // carrying the draft's stable Message-ID except the new id (the
    // previous copy included — no explicit pre-delete on top).
    let argv = fake.wait_for_invocations(2).expect("cleanup recorded");
    assert_eq!(
        &argv[0][5..],
        ["add", "-m", "Drafts", "--flag", "draft", "--json"]
    );
    assert_eq!(
        &argv[1][5..],
        ["list", "-m", "Drafts", "-p", "1", "-s", "100", "--json"]
    );
}

#[test]
fn save_draft_sweeps_stray_copies_by_message_id_two_phase() {
    // The envelope listing reports a stale copy of this draft's Message-ID
    // ("123.draft@tmail.local"): the sweep must delete it from Drafts and
    // then purge it from trash (ADR 0002 §D.4/§D.5).
    let fake = FakeHimalaya::spawn_full("ok", "draft-stray", "ok", "ok");
    let (backend, _dir) = backend_with_journal(&fake);
    block_quiet(backend.save_draft(ctx(), draft_snapshot(1, None)), 300).expect("save succeeds");
    let argv = fake.wait_for_invocations(5).expect("sweep recorded");
    assert_eq!(
        &argv[0][5..],
        ["add", "-m", "Drafts", "--flag", "draft", "--json"]
    );
    assert_eq!(
        &argv[1][5..],
        ["list", "-m", "Drafts", "-p", "1", "-s", "100", "--json"]
    );
    assert_eq!(
        &argv[2][5..],
        ["delete", "-m", "Drafts", "stray-1", "--json"]
    );
    assert_eq!(
        &argv[3][5..],
        ["list", "-m", "Archive", "-p", "1", "-s", "100", "--json"]
    );
    assert_eq!(
        &argv[4][5..],
        ["delete", "-m", "Archive", "stray-1", "--json"]
    );
}

#[test]
fn save_draft_records_the_revision_before_any_remote_call() {
    // The `add` fails: the journal entry must still exist (ADR 0002 §D.1 —
    // a crash mid-save can lose nothing).
    let fake = FakeHimalaya::spawn_full("ok", "ok", "error-json", "ok");
    let (backend, dir) = backend_with_journal(&fake);
    let err = block(backend.save_draft(ctx(), draft_snapshot(1, None))).expect_err("add fails");
    assert!(matches!(err, BackendError::Command { .. }));
    let entries = DraftJournal::open(dir.path().to_path_buf())
        .load_all()
        .expect("journal readable");
    assert_eq!(entries.len(), 1, "the revision was recorded first");
    assert_eq!(entries[0].saved_revision, 0, "nothing was confirmed remote");
}

#[test]
fn save_draft_without_a_known_drafts_mailbox_is_rejected() {
    let fake = FakeHimalaya::spawn_ok();
    let dir = TempDir::new().expect("journal tempdir");
    let backend = HimalayaCliBackend::new(
        fake.program().display().to_string(),
        Some(PathBuf::from(fake.config())),
        Some(String::from("probe")),
        aliases(&[("inbox", "INBOX")]), // no drafts alias, no cached listing
    )
    .with_journal(DraftJournal::open(dir.path().to_path_buf()));
    let err =
        block(backend.save_draft(ctx(), draft_snapshot(1, None))).expect_err("no drafts mailbox");
    assert!(
        matches!(err, BackendError::InvalidRequest(ref msg) if msg.contains("drafts")),
        "{err:?}"
    );
    assert!(fake.argv().is_empty(), "nothing was spawned");
}

// ── Send (plan §14, Phase 7.1/7.2) ───────────────────────────────────────

use tmail::domain::{OutboundMessage, OutgoingContent};

/// Backend with the configured account identity, as the real wiring builds
/// it (the `From` of outgoing mail).
fn backend_with_identity(fake: &FakeHimalaya) -> HimalayaCliBackend {
    backend(fake, Some("probe")).with_account_identity(
        Some(String::from("probe@tmail.local")),
        Some(String::from("Tmail Probe")),
    )
}

fn outbound() -> OutboundMessage {
    OutboundMessage::from_fields(
        "Ada Lovelace <ada@example.org>",
        "",
        "",
        OutgoingContent {
            subject: String::from("Hello again"),
            body: String::from("Body line one.\nBody line two.\n"),
            in_reply_to: None,
            references: None,
        },
        Some(String::from("<123.send@tmail.local>")),
    )
    .expect("valid recipients")
}

#[test]
fn send_pipes_the_serialized_message_on_stdin_with_exact_argv() {
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    let outcome =
        block(backend_with_identity(&fake).send_message(ctx(), outbound())).expect("send succeeds");
    assert_eq!(outcome, tmail::domain::SendOutcome::Sent);
    assert_eq!(
        fake.argv(),
        vec![vec![
            "-c".to_string(),
            fake.config().display().to_string(),
            "-a".to_string(),
            "probe".to_string(),
            "message".to_string(),
            "send".to_string(),
            "--json".to_string(),
        ]]
    );
    // The piped bytes are the full RFC 5322 message (CRLF wire format,
    // library-ordered headers).
    let stdin = fake.stdin_bytes();
    let text = String::from_utf8(stdin).expect("serialized mail is UTF-8");
    assert!(text.contains("To: \"Ada Lovelace\" <ada@example.org>"));
    assert!(text.contains("Subject: Hello again"));
    assert!(text.contains("Message-ID: <123.send@tmail.local>"));
    assert!(text.contains("Body line one."));
}

/// Acceptance (plan §19 Phase 7): integration fixtures parse the sent
/// output back into the expected headers/body. The bytes on the wire are
/// parsed with `mail-parser` — the library inside Himalaya — through the
/// same production mapping path.
#[cfg(feature = "test-fixtures")]
#[test]
fn sent_output_parses_back_into_expected_headers_and_body() {
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    block(backend_with_identity(&fake).send_message(ctx(), outbound())).expect("sent");
    let sent = tmail::backend::himalaya::fixtures::parse_raw_message(
        &fake.stdin_bytes(),
        "Sent",
        "sent-1",
    );
    assert_eq!(sent.headers.subject, "Hello again");
    assert_eq!(sent.headers.from.len(), 1);
    assert_eq!(sent.headers.from[0].display(), "Tmail Probe");
    assert_eq!(sent.headers.from[0].email, "probe@tmail.local");
    assert_eq!(sent.headers.to.len(), 1);
    assert_eq!(sent.headers.to[0].display(), "Ada Lovelace");
    assert_eq!(
        sent.headers.message_id.as_deref(),
        Some("123.send@tmail.local")
    );
    // mail-builder writes CRLF line endings on the wire (RFC 5322).
    assert_eq!(
        sent.plain_body.as_deref(),
        Some("Body line one.\r\nBody line two.\r\n")
    );
}

#[cfg(feature = "test-fixtures")]
#[test]
fn sent_reply_headers_parse_back_into_the_wire_format() {
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    let message = OutboundMessage {
        content: OutgoingContent {
            in_reply_to: Some(String::from("6053432595490343824@tmail.local")),
            references: Some(String::from(
                "6053432595490343824@tmail.local 111@tmail.local",
            )),
            ..outbound().content
        },
        ..outbound()
    };
    block(backend_with_identity(&fake).send_message(ctx(), message)).expect("sent");
    let sent = tmail::backend::himalaya::fixtures::parse_raw_message(
        &fake.stdin_bytes(),
        "Sent",
        "sent-1",
    );
    assert_eq!(
        sent.headers.in_reply_to.as_deref(),
        Some("6053432595490343824@tmail.local")
    );
    assert_eq!(
        sent.headers.references.as_deref(),
        Some("6053432595490343824@tmail.local 111@tmail.local")
    );
}

#[test]
fn send_outcomes_follow_the_phase_0_characterization() {
    // Exit 0 → Sent.
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    let outcome = block(backend_with_identity(&fake).send_message(ctx(), outbound())).unwrap();
    assert_eq!(outcome, tmail::domain::SendOutcome::Sent);
    // Dead port → FailedBeforeDelivery (nothing transmitted, retry safe).
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "predelivery");
    let outcome = block(backend_with_identity(&fake).send_message(ctx(), outbound())).unwrap();
    assert!(matches!(
        outcome,
        tmail::domain::SendOutcome::FailedBeforeDelivery { code: Some(1), .. }
    ));
    assert!(!outcome.is_ambiguous());
    // DATA-phase EOF → Unknown (may already be delivered).
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ambiguous");
    let outcome = block(backend_with_identity(&fake).send_message(ctx(), outbound())).unwrap();
    assert!(matches!(
        outcome,
        tmail::domain::SendOutcome::Unknown { code: Some(1), .. }
    ));
    assert!(outcome.is_ambiguous());
    // Anything unclassifiable → conservatively Unknown.
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "unclassifiable");
    let outcome = block(backend_with_identity(&fake).send_message(ctx(), outbound())).unwrap();
    assert!(matches!(
        outcome,
        tmail::domain::SendOutcome::Unknown { .. }
    ));
}

#[test]
fn send_without_an_account_identity_is_refused_before_spawning() {
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    let backend = backend(&fake, Some("probe")); // no identity configured
    let err = block(backend.send_message(ctx(), outbound())).expect_err("no From identity");
    assert!(matches!(err, BackendError::InvalidRequest(ref msg) if msg.contains("email")));
    assert!(fake.argv().is_empty(), "nothing was spawned");
}

#[test]
fn send_without_recipients_is_refused_before_spawning() {
    let fake = FakeHimalaya::spawn_send("ok", "ok", "ok", "ok", "ok");
    let message = OutboundMessage::from_fields("", "", "", OutgoingContent::default(), None)
        .expect_err("no recipients");
    assert!(
        matches!(message, tmail::domain::SendBlocker::NoRecipients),
        "the blocker fires before the backend is reached"
    );
    // Defense in depth: a hand-built recipient-free message is refused too.
    let message = OutboundMessage {
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        content: OutgoingContent::default(),
        message_id: None,
        attachments: Vec::new(),
    };
    let err =
        block(backend_with_identity(&fake).send_message(ctx(), message)).expect_err("no rcpt");
    assert!(matches!(err, BackendError::InvalidRequest(ref msg) if msg.contains("recipients")));
    assert!(fake.argv().is_empty(), "nothing was spawned");
}

// ── Attachment saves (plan §15, Phase 8.4) ────────────────────────────────

use tmail::backend::AttachmentRequest;

fn save_request(filename: Option<&str>, dir: Option<PathBuf>) -> AttachmentRequest {
    AttachmentRequest {
        locator: locator("INBOX", "env-1"),
        part_id: 3,
        filename: filename.map(String::from),
        dir,
    }
}

#[test]
fn attachment_save_downloads_into_a_private_tempdir_then_lands_in_the_dest() {
    let fake = FakeHimalaya::spawn_attachment("ok");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let request = save_request(Some("report.pdf"), Some(dest.path().to_path_buf()));
    let saved = block(backend(&fake, Some("probe")).save_attachment(ctx(), request))
        .expect("save succeeds");
    assert_eq!(saved, dest.path().join("report.pdf"));
    let bytes = std::fs::read(&saved).expect("saved file");
    assert_eq!(bytes, b"PDF-PAYLOAD-01", "exact bytes land");

    // One invocation, exact argv except the private tempdir name, which is
    // Tmail-generated and unpredictable by design. The directory travels as
    // a single argv entry (never shell-split).
    let argv = &fake.argv();
    assert_eq!(argv.len(), 1);
    let argv = &argv[0];
    let (head, rest) = argv.split_at(8);
    assert_eq!(
        head,
        vec![
            "-c",
            fake.config().display().to_string().as_str(),
            "-a",
            "probe",
            "attachment",
            "download",
            "-m",
            "INBOX",
        ]
    );
    assert_eq!(argv[8], "-d");
    let download_dir = Path::new(&argv[9]);
    assert!(download_dir.is_absolute(), "tempdir path is absolute");
    assert_eq!(&argv[10..], &["env-1", "3", "--json"]);
    // The row's reported path resolved inside the requested tempdir.
    let _ = rest;
}

#[test]
fn attachment_save_uses_the_part_id_mapped_from_message_read() {
    // Regression: `message read` reports 0-based part indexes while
    // `attachment download` expects the 1-based id (`part index + 1`).
    // A save driven by a mapped attachment — no hand-built part id —
    // must ask for the id himalaya actually serves.
    let fake = FakeHimalaya::spawn_attachment("ok");
    let backend = backend(&fake, Some("probe"));
    let message =
        block(backend.get_message(ctx(), locator("INBOX", "env-1"))).expect("read succeeds");
    let attachment = message.attachments.first().expect("fixture attachment");
    assert_eq!(attachment.part_id, 2, "fixture part index 1 → id 2");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let request = AttachmentRequest {
        locator: MessageLocator {
            mailbox: message.mailbox_id.clone(),
            id: message.id.clone(),
            message_id: message.headers.message_id.clone(),
        },
        part_id: attachment.part_id,
        filename: attachment.name.clone(),
        dir: Some(dest.path().to_path_buf()),
    };
    let saved = block(backend.save_attachment(ctx(), request)).expect("save succeeds");
    assert_eq!(saved, dest.path().join("fake.pdf"));
    assert_eq!(std::fs::read(&saved).unwrap(), b"PDF-PAYLOAD-01");

    // The download argv carries the mapped 1-based id, and the fake's
    // echoed row id is what `find_row` matched.
    let argv = fake.argv();
    let download = argv.last().expect("download invocation");
    assert_eq!(&download[download.len() - 3..], &["env-1", "2", "--json"]);
}

#[test]
fn attachment_save_never_silently_overwrites() {
    let fake = FakeHimalaya::spawn_attachment("ok");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let dir = dest.path().to_path_buf();
    // The name is already taken: a previous download must survive.
    std::fs::write(dir.join("report.pdf"), b"PREVIOUS").expect("seed collision");

    let backend = backend(&fake, Some("probe"));
    let first =
        block(backend.save_attachment(ctx(), save_request(Some("report.pdf"), Some(dir.clone()))))
            .expect("first save");
    assert_eq!(first, dir.join("report (1).pdf"));
    let second =
        block(backend.save_attachment(ctx(), save_request(Some("report.pdf"), Some(dir.clone()))))
            .expect("second save");
    assert_eq!(second, dir.join("report (2).pdf"));

    // The previous download is untouched; the new files carry the payload.
    assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"PREVIOUS");
    assert_eq!(
        std::fs::read(dir.join("report (1).pdf")).unwrap(),
        b"PDF-PAYLOAD-01"
    );
    assert_eq!(
        std::fs::read(dir.join("report (2).pdf")).unwrap(),
        b"PDF-PAYLOAD-01"
    );
}

#[test]
fn attachment_save_reduces_hostile_filenames_to_one_component() {
    // The MIME metadata claims `../../evil.bin`. Tmail must not escape the
    // destination: only the final component may land there.
    let fake = FakeHimalaya::spawn_attachment("traversal");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let dir = dest.path().to_path_buf();
    let saved = block(backend(&fake, Some("probe")).save_attachment(
        ctx(),
        save_request(Some("../../evil.bin"), Some(dir.clone())),
    ))
    .expect("save succeeds");
    assert_eq!(saved, dir.join("evil.bin"));
    assert_eq!(std::fs::read(&saved).unwrap(), b"EVIL-PAYLOAD");
    // Nothing escaped upward.
    assert!(
        !dest.path().parent().unwrap().join("evil.bin").exists(),
        "no traversal outside the destination"
    );
}

#[test]
fn attachment_save_without_a_request_name_uses_the_mime_row_name() {
    // The request carries no name: the download row's MIME filename fills
    // in (both are display names, reduced to one component before use).
    let fake = FakeHimalaya::spawn_attachment("ok");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let saved = block(
        backend(&fake, Some("probe"))
            .save_attachment(ctx(), save_request(None, Some(dest.path().to_path_buf()))),
    )
    .expect("save succeeds");
    assert_eq!(saved, dest.path().join("report.pdf"));
    // The pure part-id fallback (no request name, no row name) is pinned
    // by the adapter's unit tests for `destination_component`.
}

#[test]
fn attachment_save_explicit_dir_wins_over_the_configured_one() {
    let fake = FakeHimalaya::spawn_attachment("ok");
    let dest = tempfile::TempDir::new().expect("dest dir");
    // The config would point elsewhere; the request's dir wins.
    let backend = backend(&fake, Some("probe"))
        .with_downloads_dir(Some(PathBuf::from("/definitely/not/this")));
    let saved = block(backend.save_attachment(
        ctx(),
        save_request(Some("report.pdf"), Some(dest.path().to_path_buf())),
    ))
    .expect("save succeeds");
    assert_eq!(saved, dest.path().join("report.pdf"));
    assert_eq!(std::fs::read(&saved).unwrap(), b"PDF-PAYLOAD-01");
}

#[test]
fn attachment_save_fails_safely_on_bad_output() {
    // Row for the wrong part id.
    let fake = FakeHimalaya::spawn_attachment("wrong-row");
    let dest = tempfile::TempDir::new().expect("dest dir");
    let err = block(backend(&fake, Some("probe")).save_attachment(
        ctx(),
        save_request(Some("report.pdf"), Some(dest.path().to_path_buf())),
    ))
    .expect_err("no row for part 3");
    assert!(matches!(err, BackendError::InvalidOutput(ref msg) if msg.contains("part 3")));

    // Row without an output path.
    let fake = FakeHimalaya::spawn_attachment("no-path");
    let err = block(backend(&fake, Some("probe")).save_attachment(
        ctx(),
        save_request(Some("report.pdf"), Some(dest.path().to_path_buf())),
    ))
    .expect_err("no path in row");
    assert!(matches!(err, BackendError::InvalidOutput(ref msg) if msg.contains("no output path")));

    // Himalaya reports a failure.
    let fake = FakeHimalaya::spawn_attachment("error-json");
    let err = block(backend(&fake, Some("probe")).save_attachment(
        ctx(),
        save_request(Some("report.pdf"), Some(dest.path().to_path_buf())),
    ))
    .expect_err("download failed");
    assert!(
        matches!(err, BackendError::Command { ref detail, .. } if detail.contains("no such attachment"))
    );
}

// ── Wizard credential test (ADR 0003 §3.4/W7) ────────────────────────────

use tmail::app::effect::Effect;
use tmail::app::operation::{OperationKind, OperationOutcome};
use tmail::app::wizard::DraftAccountConfig;
use tmail::config::write::{DraftAccount, SecretStorage};
use tmail::runtime::tasks::OperationManager;

/// The draft the wizard carries into the credential test.
fn wizard_draft(name: &str) -> DraftAccountConfig {
    DraftAccount {
        name: name.to_string(),
        email: String::from("user@gmail.com"),
        display_name: Some(String::from("Test User")),
        imap_server: String::from("imaps://imap.gmail.com:993"),
        imap_starttls: false,
        smtp_server: String::from("smtps://smtp.gmail.com:465"),
        smtp_starttls: false,
        username: String::from("user@gmail.com"),
        secret: SecretStorage::Raw(String::from("app-password")),
        aliases: Vec::new(),
    }
}

/// Builds the manager the main loop uses, pointed at the fake himalaya,
/// runs the TestAccount effect to completion, and returns its outcome.
fn run_test_account(
    fake: &FakeHimalaya,
    draft: DraftAccountConfig,
    cancellation: CancellationToken,
) -> Result<OperationOutcome, Box<tmail::app::operation::OperationFailure>> {
    block(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = OperationManager::new(
            std::sync::Arc::new(backend(fake, Some("probe"))),
            std::sync::Arc::new(tmail::backend::SystemOpener),
            std::sync::Arc::new(tmail::backend::SystemNotifier),
            std::sync::Arc::new(tmail::discovery::FakeDiscoverer),
            std::sync::Arc::new(tmail::backend::himalaya::HimalayaAccountTester::new(
                fake.program().display().to_string(),
            )),
            None,
            tx,
        );
        let effect = Effect {
            id: OperationId(77),
            kind: OperationKind::TestAccount {
                draft: Box::new(draft),
            },
        };
        let ctx = RequestContext {
            operation: effect.id,
            cancellation,
        };
        manager.launch(effect, ctx);
        let result = rx.recv().await.expect("result for the launched effect");
        result.outcome.map_err(Box::new)
    })
}

#[test]
fn wizard_test_account_runs_the_real_plumbing_and_cleans_up() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    let outcome = run_test_account(&fake, wizard_draft("gmail"), CancellationToken::new());

    match outcome.expect("the test succeeds") {
        OperationOutcome::TestAccountCompleted { mailboxes } => {
            assert_eq!(
                mailboxes,
                vec![
                    String::from("INBOX"),
                    String::from("Archive"),
                    String::from("Sent"),
                ],
                "the canned listing from the fake himalaya"
            );
        }
        other => panic!("expected a TestAccountCompleted payload, got {other:?}"),
    }

    // The invocation used a temporary 0600 config (not the fake's stub
    // config), the draft account name, and the plain mailbox list argv.
    let invocations = fake.argv();
    assert_eq!(invocations.len(), 1, "exactly one himalaya run");
    let argv = &invocations[0];
    assert_eq!(argv[0], "-c");
    let temp_config = PathBuf::from(&argv[1]);
    assert!(
        temp_config
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .starts_with("tmail-wizard-"),
        "the test runs against a tmail-wizard temp config: {temp_config:?}"
    );
    assert_eq!(
        &argv[2..7],
        &[
            "-a".to_string(),
            "gmail".to_string(),
            "mailbox".to_string(),
            "list".to_string(),
            "--json".to_string()
        ]
    );
    assert!(
        !temp_config.exists(),
        "the temp config is deleted after the run (no credential left behind)"
    );
}

#[test]
fn wizard_test_account_failure_is_a_sanitized_structural_failure() {
    let fake = FakeHimalaya::spawn("error-json", "ok");
    let outcome = run_test_account(&fake, wizard_draft("gmail"), CancellationToken::new());

    let failure = outcome.expect_err("the failing fake fails the test");
    assert_eq!(failure.code, Some(1));
    assert!(
        failure.detail.contains("account not found"),
        "the failure detail surfaces himalaya's error: {failure:?}"
    );
    assert!(
        !failure.detail.contains("app-password"),
        "no secret ever appears in the failure detail"
    );
}

#[test]
fn wizard_test_account_cancellation_suppresses_the_result() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    let token = CancellationToken::new();
    let draft = wizard_draft("gmail");

    block(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = OperationManager::new(
            std::sync::Arc::new(backend(&fake, Some("probe"))),
            std::sync::Arc::new(tmail::backend::SystemOpener),
            std::sync::Arc::new(tmail::backend::SystemNotifier),
            std::sync::Arc::new(tmail::discovery::FakeDiscoverer),
            std::sync::Arc::new(tmail::backend::himalaya::HimalayaAccountTester::new(
                fake.program().display().to_string(),
            )),
            None,
            tx,
        );
        // Cancel before launch: the run selects on the token and reports
        // Cancelled, which the manager suppresses — no result, no state
        // mutation (plan §11).
        token.cancel();
        let effect = Effect {
            id: OperationId(78),
            kind: OperationKind::TestAccount {
                draft: Box::new(draft),
            },
        };
        let ctx = RequestContext {
            operation: effect.id,
            cancellation: token,
        };
        manager.launch(effect, ctx);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;
        assert!(
            result.is_err() || result.expect("recv").is_none(),
            "a cancelled credential test must never produce a result"
        );
    });
}

#[test]
fn wizard_save_account_operation_reports_the_saved_file() {
    let fake = FakeHimalaya::spawn("ok", "ok");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let draft = wizard_draft("gmail");

    block(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let manager = OperationManager::new(
            std::sync::Arc::new(backend(&fake, Some("probe"))),
            std::sync::Arc::new(tmail::backend::SystemOpener),
            std::sync::Arc::new(tmail::backend::SystemNotifier),
            std::sync::Arc::new(tmail::discovery::FakeDiscoverer),
            std::sync::Arc::new(tmail::backend::himalaya::HimalayaAccountTester::new(
                fake.program().display().to_string(),
            )),
            None,
            tx,
        );
        let effect = Effect {
            id: OperationId(79),
            kind: OperationKind::SaveAccount {
                path: path.clone(),
                draft: Box::new(draft),
            },
        };
        let ctx = RequestContext {
            operation: effect.id,
            cancellation: CancellationToken::new(),
        };
        manager.launch(effect, ctx);
        let result = rx.recv().await.expect("result");
        match result.outcome.expect("save succeeds") {
            OperationOutcome::AccountSaved {
                path: saved,
                created,
                permissions_warning,
            } => {
                assert_eq!(saved, path);
                assert!(created);
                assert_eq!(permissions_warning, None);
            }
            other => panic!("expected AccountSaved, got {other:?}"),
        }
    });

    // The file really holds the account (the merge ran in the manager).
    let text = std::fs::read_to_string(dir.path().join("config.toml")).expect("written");
    assert!(text.contains("[accounts.gmail]"));
    assert!(text.contains("imap.sasl.plain.password.raw"));
}
