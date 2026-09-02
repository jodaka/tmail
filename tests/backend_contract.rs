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

use tmail::backend::himalaya::HimalayaCliBackend;
use tmail::backend::{BackendError, MailBackend};
use tmail::domain::{MailboxId, MailboxRole, PageRequest};

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
    let result = block(backend(&fake, Some("probe")).list_mailboxes());
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
    let mailboxes = block(backend(&fake, Some("probe")).list_mailboxes()).unwrap();
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
    let result = block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 40)));
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
    let page =
        block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0))).unwrap();
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
    block(backend(&fake, Some("probe")).list_mailboxes()).unwrap();
    block(backend(&fake, None).list_messages(page_request("INBOX", 0))).unwrap();
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
    block(bare.list_mailboxes()).unwrap();
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
    block(backend(&fake, None).list_messages(request)).unwrap();
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
    let result = block(backend(&fake, None).list_messages(request));
    assert!(matches!(result, Err(BackendError::InvalidRequest(_))));
    assert!(fake.argv().is_empty(), "no child process may be spawned");
}

#[test]
fn empty_and_out_of_range_pages_are_valid_empty_results() {
    // Mirrors the verified real behavior: `envelope list -p 99` exits 0
    // with `{"envelopes":[]}`.
    let fake = FakeHimalaya::spawn("ok", "empty");
    let page =
        block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 80))).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.offset, 80);
    assert!(!page.has_next(), "an empty page never has a successor");
    assert!(page.has_previous(), "paging back must stay available");
}

#[test]
fn partial_envelope_maps_to_safe_defaults() {
    let fake = FakeHimalaya::spawn("ok", "partial");
    let page =
        block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0))).unwrap();
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
    let result = block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0)));
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
    let result = block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0)));
    match result {
        Err(BackendError::InvalidOutput(_)) => {}
        other => panic!("expected InvalidOutput, got {other:?}"),
    }
}

#[test]
fn json_error_on_stdout_becomes_command_error() {
    let fake = FakeHimalaya::spawn("ok", "error-json");
    let result = block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0)));
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
    let result = block(backend(&fake, Some("probe")).list_messages(page_request("INBOX", 0)));
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
    let result = block(backend(&fake, Some("probe")).list_mailboxes());
    match result {
        Err(BackendError::Command { code, detail }) => {
            assert_eq!(code, Some(1));
            assert!(detail.contains("account not found"));
        }
        other => panic!("expected Command, got {other:?}"),
    }
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
