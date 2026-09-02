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
        message_id: Some(String::from("1@post.local")),
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
    assert_eq!(first.message_id.as_deref(), Some("1@post.local"));
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
    assert_eq!(message.headers.to[0].email, "probe@post.local");
    assert_eq!(message.headers.message_id.as_deref(), Some("1@post.local"));
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
    assert_eq!(message.attachments[0].part_id, 1);
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
        message_id: Some(String::from("<123.draft@post.local>")),
        remote_id: remote_id.map(|id| MessageId(String::from(id))),
        to: String::from("Maksim Orlov <m.orlov@example.org>, broken-entry"),
        cc: String::from("cc@example.org"),
        bcc: String::new(),
        subject: String::from("Gyuto — progress"),
        body: String::from("first line\nsecond line"),
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
        Some(String::from("probe@post.local")),
        Some(String::from("Post Probe")),
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
    // headers, the stable Message-ID, Post's draft metadata, and the body.
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
        stdin.contains("Message-ID: <123.draft@post.local>"),
        "{stdin}"
    );
    assert!(stdin.contains("X-Post-Draft-Id: local-123"), "{stdin}");
    assert!(
        stdin.contains("From: \"Post Probe\" <probe@post.local>"),
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
fn save_draft_replaces_the_old_remote_only_after_confirm() {
    let fake = FakeHimalaya::spawn_ok();
    let (backend, _dir) = backend_with_journal(&fake);
    block_quiet(
        backend.save_draft(ctx(), draft_snapshot(2, Some("old-1"))),
        300,
    )
    .expect("save succeeds");
    // `add` is awaited by the save; the old-copy delete and the stray
    // sweep run detached (two tasks, order between them unspecified).
    let argv = fake.wait_for_invocations(4).expect("cleanup recorded");
    assert_eq!(
        &argv[0][5..],
        ["add", "-m", "Drafts", "--flag", "draft", "--json"]
    );
    let mut rest = argv[1..]
        .iter()
        .map(|inv| inv[5..].to_vec())
        .collect::<Vec<_>>();
    rest.sort();
    assert_eq!(
        rest,
        vec![
            vec!["delete", "-m", "Drafts", "old-1", "--json"],
            vec!["list", "-m", "Archive", "-p", "1", "-s", "100", "--json"],
            vec!["list", "-m", "Drafts", "-p", "1", "-s", "100", "--json"],
        ]
    );
}

#[test]
fn save_draft_sweeps_stray_copies_by_message_id_two_phase() {
    // The envelope listing reports a stale copy of this draft's Message-ID
    // ("123.draft@post.local"): the sweep must delete it from Drafts and
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
