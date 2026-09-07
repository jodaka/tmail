//! Integration suite against the real `himalaya` CLI over a temporary
//! Maildir (plan §20 "Integration" row, §19 Phase 12 task "temporary
//! Maildir/Himalaya integration suite where supported").
//!
//! Unlike the contract tests (which fake the executable), this suite runs
//! the actual `himalaya` binary discovered from `TMAIL_HIMALAYA` or `PATH`
//! against a disposable Maildir tree with the config shape verified during
//! Phase 0 (docs/phase-0-checklist.md). It covers listing, reading, flags,
//! drafts, and search where supported:
//!
//! - `himalaya` (or `TMAIL_HIMALAYA`) not on the machine → the test prints
//!   a skip notice and returns, so the suite stays portable (plan's
//!   "where supported" wording);
//! - maildir text search is unsupported by himalaya 2.1.0 (text filters
//!   match nothing; flag filters do) — the search test encodes that
//!   evidence instead of assuming IMAP-like full-text behavior.
//!
//! Every test builds its own hermetic environment; nothing outside the
//! temporary directory is touched.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

use tmail::app::operation::OperationId;
use tmail::backend::himalaya::HimalayaCliBackend;
use tmail::backend::journal::DraftJournal;
use tmail::backend::{MailBackend, RequestContext};
use tmail::domain::{
    DraftAttachment, DraftId, DraftSnapshot, MailboxId, MessageLocator, PageRequest, SearchRequest,
};

use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

/// The seeded message's stable identity.
const MESSAGE_ID: &str = "integration-1@tmail.local";

fn ctx() -> RequestContext {
    RequestContext {
        operation: OperationId(1),
        cancellation: CancellationToken::new(),
    }
}

fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    Runtime::new().expect("runtime").block_on(future)
}

/// Run `future`, then yield so the adapter's detached cleanup tasks (draft
/// sweeps) get their turn before the runtime shuts down.
fn block_quiet<T>(future: impl std::future::Future<Output = T>, quiet_ms: u64) -> T {
    Runtime::new().expect("runtime").block_on(async move {
        let result = future.await;
        tokio::time::sleep(Duration::from_millis(quiet_ms)).await;
        result
    })
}

/// Locate the real himalaya executable: `TMAIL_HIMALAYA` wins, else a PATH
/// scan (no shell involved).
fn discover_himalaya() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TMAIL_HIMALAYA") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("himalaya");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// One hermetic himalaya-over-maildir environment.
struct MaildirEnv {
    backend: HimalayaCliBackend,
    inbox_id: MailboxId,
    drafts_id: MailboxId,
    _dir: TempDir,
}

impl MaildirEnv {
    /// Build the environment, or `None` (with a skip notice) when no
    /// himalaya binary is available.
    fn spawn() -> Option<Self> {
        let program = match discover_himalaya() {
            Some(program) => program,
            None => {
                eprintln!("skipping maildir integration: no himalaya binary on PATH");
                return None;
            }
        };
        let dir = TempDir::new().expect("temp dir");

        // Maildir tree: every mailbox himalaya may touch must exist, or
        // `message add`/trash moves fail with "path … is not a directory".
        let root = dir.path().join("root");
        for name in ["INBOX", "Archive", "Sent", "Drafts"] {
            for part in ["cur", "new", "tmp"] {
                fs::create_dir_all(root.join(name).join(part)).expect("maildir dirs");
            }
        }

        // Config shape verified on himalaya 2.1.0 (Phase 0 probe):
        // `accounts.<name>.maildir.root`, mailbox aliases, and a smtp
        // stanza (never contacted here — send is covered by the fake).
        let config_path = dir.path().join("config.toml");
        let config = format!(
            "[accounts.probe]\n\
             email = \"probe@tmail.local\"\n\
             display-name = \"Tmail Probe\"\n\
             \n\
             [accounts.probe.maildir]\n\
             root = \"{}\"\n\
             \n\
             [accounts.probe.mailbox.alias]\n\
             inbox = \"INBOX\"\n\
             trash = \"Archive\"\n\
             drafts = \"Drafts\"\n\
             \n\
             [accounts.probe.smtp]\n\
             server = \"smtp://127.0.0.1:3425\"\n",
            root.display()
        );
        fs::write(&config_path, config).expect("write config");

        // Seed one unread, unstarred message into INBOX/cur (maildir
        // convention: files in cur carry the `:2,` flag suffix).
        let eml = format!(
            "From: Ada Lovelace <ada@example.org>\n\
             To: probe@tmail.local\n\
             Subject: Integration hello\n\
             Date: Wed, 02 Sep 2026 10:00:00 +0000\n\
             Message-ID: <{MESSAGE_ID}>\n\
             \n\
             Hello from the integration suite.\n"
        );
        let seeded = root.join("INBOX/cur/1699999999.m1.integration:2,");
        fs::write(seeded, eml).expect("seed message");

        let backend = HimalayaCliBackend::new(
            program.display().to_string(),
            Some(config_path),
            Some(String::from("probe")),
            [
                (String::from("inbox"), String::from("INBOX")),
                (String::from("trash"), String::from("Archive")),
                (String::from("drafts"), String::from("Drafts")),
                (String::from("sent"), String::from("Sent")),
            ]
            .into_iter()
            .collect(),
        )
        .with_journal(DraftJournal::open(dir.path().join("journal")))
        .with_account_identity(
            Some(String::from("probe@tmail.local")),
            Some(String::from("Tmail Probe")),
        );

        // Mailbox ids on maildir are absolute directory paths (probe
        // finding, ADR 0001), so resolve them from the listing rather
        // than guessing.
        let mailboxes = block(backend.list_mailboxes(ctx())).expect("mailbox list");
        let find = |name: &str| {
            mailboxes
                .iter()
                .find(|mailbox| mailbox.name == name)
                .map(|mailbox| mailbox.id.clone())
                .unwrap_or_else(|| panic!("mailbox {name} missing"))
        };
        Some(Self {
            inbox_id: find("INBOX"),
            drafts_id: find("Drafts"),
            backend,
            _dir: dir,
        })
    }

    fn locator(&self) -> MessageLocator {
        // The backend id is maildir-internal; resolve it through the
        // listing so the locator matches what the UI would hold.
        let page = block(self.backend.list_messages(
            ctx(),
            PageRequest {
                mailbox_id: self.inbox_id.clone(),
                offset: 0,
                limit: 20,
            },
        ))
        .expect("envelope list");
        let summary = &page.items[0];
        MessageLocator {
            mailbox: self.inbox_id.clone(),
            id: summary.id.clone(),
            message_id: Some(String::from(MESSAGE_ID)),
        }
    }
}

/// Fresh environment or skip.
fn env() -> Option<MaildirEnv> {
    MaildirEnv::spawn()
}

#[test]
fn listing_maps_the_seeded_maildir() {
    let Some(env) = env() else { return };
    let mailboxes = block(env.backend.list_mailboxes(ctx())).expect("list");
    let names: Vec<&str> = mailboxes.iter().map(|m| m.name.as_str()).collect();
    for name in ["INBOX", "Archive", "Sent", "Drafts"] {
        assert!(names.contains(&name), "missing {name} in {names:?}");
    }
    let inbox = mailboxes.iter().find(|m| m.name == "INBOX").unwrap();
    assert_eq!(
        inbox.role,
        Some(tmail::domain::MailboxRole::Inbox),
        "alias must resolve the role"
    );
    // Maildir totals are unknown (ADR 0001 finding 7): the UI degrades.
    assert_eq!(inbox.unread_count, None);
    assert_eq!(inbox.total_count, None);

    let page = block(env.backend.list_messages(
        ctx(),
        PageRequest {
            mailbox_id: env.inbox_id.clone(),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("envelope list");
    assert_eq!(page.items.len(), 1);
    let message = &page.items[0];
    assert_eq!(message.subject, "Integration hello");
    assert_eq!(message.from[0].email, "ada@example.org");
    assert_eq!(message.message_id.as_deref(), Some(MESSAGE_ID));
    assert!(!message.is_read);
    assert!(!message.is_starred);
}

#[test]
fn reading_returns_the_seeded_body_and_headers() {
    let Some(env) = env() else { return };
    let locator = env.locator();
    let message = block(env.backend.get_message(ctx(), locator)).expect("read");
    assert_eq!(message.headers.subject, "Integration hello");
    assert_eq!(message.headers.from[0].display(), "Ada Lovelace");
    assert_eq!(
        message.plain_body.as_deref(),
        Some("Hello from the integration suite.\n")
    );
}

#[test]
fn flags_round_trip_through_maildir() {
    let Some(env) = env() else { return };
    let locator = env.locator();

    block(env.backend.set_read(ctx(), locator.clone(), true)).expect("set read");
    block(env.backend.set_starred(ctx(), locator.clone(), true)).expect("set starred");

    let page = block(env.backend.list_messages(
        ctx(),
        PageRequest {
            mailbox_id: env.inbox_id.clone(),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("envelope list");
    assert!(page.items[0].is_read, "seen flag must round-trip");
    assert!(page.items[0].is_starred, "flagged flag must round-trip");

    block(env.backend.set_read(ctx(), locator.clone(), false)).expect("unset read");
    block(env.backend.set_starred(ctx(), locator.clone(), false)).expect("unset starred");

    let page = block(env.backend.list_messages(
        ctx(),
        PageRequest {
            mailbox_id: env.inbox_id.clone(),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("envelope list");
    assert!(!page.items[0].is_read);
    assert!(!page.items[0].is_starred);
}

fn draft_snapshot() -> DraftSnapshot {
    DraftSnapshot {
        local_id: DraftId(String::from("local-integration")),
        message_id: Some(String::from("<integration-draft@tmail.local>")),
        in_reply_to: None,
        references: None,
        remote_id: None,
        to: String::from("Ada <ada@example.org>"),
        cc: String::new(),
        bcc: String::new(),
        subject: String::from("Integration draft"),
        body: String::from("Draft body.\n"),
        attachments: Vec::<DraftAttachment>::new(),
        revision: 1,
    }
}

#[test]
fn drafts_save_journal_and_discard_round_trip() {
    let Some(env) = env() else { return };
    let draft = draft_snapshot();

    // Save: journal first, then `message add` into the real Drafts dir.
    let remote_id =
        block_quiet(env.backend.save_draft(ctx(), draft.clone()), 200).expect("draft save");

    let page = block(env.backend.list_messages(
        ctx(),
        PageRequest {
            mailbox_id: env.drafts_id.clone(),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("drafts listing");
    assert_eq!(page.items.len(), 1, "exactly one remote draft copy");
    assert_eq!(
        page.items[0].message_id.as_deref(),
        Some("integration-draft@tmail.local"),
        "Message-ID identifies the draft"
    );

    // The journal holds the confirmed revision for crash recovery.
    let restored = block(env.backend.load_drafts(ctx())).expect("journal load");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].draft.local_id, draft.local_id);

    // Discard: journal entry removed, remote copy swept (trash-first move
    // takes it out of Drafts; detached tasks need the quiet window).
    let mut discarded = draft;
    discarded.remote_id = Some(remote_id.clone());
    block_quiet(env.backend.delete_draft(ctx(), discarded), 500).expect("draft delete");

    let page = block(env.backend.list_messages(
        ctx(),
        PageRequest {
            mailbox_id: env.drafts_id.clone(),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("drafts listing after delete");
    assert!(page.items.is_empty(), "draft copy must leave Drafts");
    let restored = block(env.backend.load_drafts(ctx())).expect("journal load");
    assert!(restored.is_empty(), "journal entry must be removed");
}

#[test]
fn search_behavior_matches_maildir_capabilities() {
    let Some(env) = env() else { return };

    // Text search on maildir matches nothing (himalaya 2.1.0: text
    // filters are delegated to the server; maildir has none). The
    // request still completes with a valid empty page.
    let page = block(env.backend.search_messages(
        ctx(),
        SearchRequest {
            mailbox_id: env.inbox_id.clone(),
            query: String::from("Integration"),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("search must not error");
    assert!(
        page.items.is_empty(),
        "maildir text search matches nothing; got {:?}",
        page.items.iter().map(|m| &m.subject).collect::<Vec<_>>()
    );

    // Flag filters do work on maildir: after starring, a flag-filtered
    // search finds the message ("where supported"). The query must be
    // written in the bare DSL token form ("flag flagged"); the backend's
    // normalizer recognizes DSL queries by exact predicate tokens, so
    // parentheses would make it a full-text query (which maildir cannot
    // match).
    let locator = env.locator();
    block(env.backend.set_starred(ctx(), locator, true)).expect("star");

    let page = block(env.backend.search_messages(
        ctx(),
        SearchRequest {
            mailbox_id: env.inbox_id.clone(),
            query: String::from("flag flagged"),
            offset: 0,
            limit: 20,
        },
    ))
    .expect("flag search");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].message_id.as_deref(), Some(MESSAGE_ID));
}
