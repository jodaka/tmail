//! Reducer unit tests (plan §9: invalid/empty selections, stale inputs,
//! route behavior, resize; plan §19 Phase 3: operation registry semantics,
//! stale/superseded-result rejection, cancellation, Retry/Dismiss modal).

use super::test_prelude::*;
use super::*;
use crate::app::action::AttachmentBrowse;
use crate::app::action::ComposerEdit;
use crate::app::action::SearchEdit;
use crate::app::action::{BulkOp, ClickTarget};
use crate::app::composer::ComposerField;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::mock::{self, mock_initial_state};
use crate::app::operation::DraftRemovalReason;
use crate::app::operation::{OperationId, RetrySpec, SeedKind};
use crate::app::overlay::AttachmentFileDialog;
use crate::app::overlay::Overlay;
use crate::app::overlay::{ConfirmButton, ErrorDialog, ModalButton};
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::config::ViewMode;
use crate::domain::DraftAttachment;
use crate::domain::{Address, Message, MessageHeaders, MessageSummary};
use crate::domain::{Mailbox, MailboxId, MailboxRole, MessageId, PageRequest};
use crate::domain::{OutboundMessage, SendOutcome};
use std::path::PathBuf;

mod accounts;
mod attachments;
mod bulk;
mod cache;
mod composer;
mod drafts;
mod list;
mod mailboxes;
mod modals;
mod mouse;
mod notifications;
mod reader;
mod search;
mod status;

fn state() -> AppState {
    mock_initial_state()
}

fn inbox_id() -> MailboxId {
    MailboxId(String::from("inbox"))
}

fn mailboxes_kind() -> OperationKind {
    OperationKind::LoadMailboxes
}

fn page_kind(req: &PageRequest) -> OperationKind {
    OperationKind::LoadPage(req.clone())
}

/// Destructure an effect into `(id, kind)` for assertions.
fn effect_parts(effects: &[Effect]) -> (OperationId, OperationKind) {
    match effects {
        [effect] => (effect.id, effect.kind.clone()),
        other => panic!("expected exactly one effect, got {other:?}"),
    }
}

fn expect_page(effects: &[Effect]) -> (OperationId, PageRequest) {
    let (id, kind) = effect_parts(effects);
    match kind {
        OperationKind::LoadPage(request) => (id, request),
        other => panic!("expected a LoadPage effect, got {other:?}"),
    }
}

/// The `(id, locator)` pairs of every `CachePreviewLoad` effect.
fn expect_cache_preview_reads(
    effects: &[Effect],
) -> Vec<(OperationId, crate::domain::MessageLocator)> {
    effects
        .iter()
        .filter_map(|e| match &e.kind {
            OperationKind::CachePreviewLoad { locator } => Some((e.id, locator.clone())),
            _ => None,
        })
        .collect()
}

/// The first `LoadPage` effect, wherever it sits in the batch.
fn find_page(effects: &[Effect]) -> (OperationId, PageRequest) {
    effects
        .iter()
        .find_map(|e| match &e.kind {
            OperationKind::LoadPage(request) => Some((e.id, request.clone())),
            _ => None,
        })
        .expect("a LoadPage effect")
}

/// The `(id, mailbox, query, offset, limit, fresh_background_on_hit)` of
/// the first `CacheListLoad` effect (ticket haeb: cache reads run
/// off-thread, so tests complete them explicitly).
#[allow(clippy::type_complexity)]
fn expect_cache_list_load(
    effects: &[Effect],
) -> (OperationId, MailboxId, Option<String>, usize, usize, bool) {
    effects
        .iter()
        .find_map(|e| match &e.kind {
            OperationKind::CacheListLoad {
                mailbox,
                query,
                offset,
                limit,
                fresh_background_on_hit,
            } => Some((
                e.id,
                mailbox.clone(),
                query.clone(),
                *offset,
                *limit,
                *fresh_background_on_hit,
            )),
            _ => None,
        })
        .expect("a CacheListLoad effect")
}

/// Complete an in-flight cache read with a miss: the fresh load starts.
fn complete_cache_miss(s: &mut AppState, id: OperationId) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::CacheMiss),
        }),
    )
}

/// Complete an in-flight cache read with a cached page.
fn complete_cache_page(
    s: &mut AppState,
    id: OperationId,
    page: crate::domain::Page<crate::domain::MessageSummary>,
) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::CachedPage(page)),
        }),
    )
}

/// Complete an in-flight cache message read (reader or preview path).
fn complete_cache_message(
    s: &mut AppState,
    id: OperationId,
    message: crate::domain::Message,
) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::CachedMessage(Box::new(message))),
        }),
    )
}

/// Complete every cache-store effect in `effects` (ticket haeb: writes
/// are best-effort background work whose `Done` results carry nothing to
/// apply), assert `effects` carried nothing else, and return the stores'
/// follow-up effects.
fn no_effects_except_cache_stores(s: &mut AppState, effects: &[Effect]) {
    assert!(
        effects.iter().all(|e| matches!(
            e.kind,
            OperationKind::CacheListStore { .. }
                | OperationKind::CacheListEvict { .. }
                | OperationKind::CacheMailboxesStore { .. }
                | OperationKind::CacheMessageStore { .. }
        )),
        "expected only cache stores, got {effects:?}"
    );
    no_effects(&complete_cache_stores(s, effects));
}

/// Complete every cache-store effect in `effects` (ticket haeb: writes
/// are best-effort background work whose `Done` results carry nothing to
/// apply) and return their follow-up effects.
fn complete_cache_stores(s: &mut AppState, effects: &[Effect]) -> Vec<Effect> {
    let ids: Vec<OperationId> = effects
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                OperationKind::CacheListStore { .. }
                    | OperationKind::CacheListEvict { .. }
                    | OperationKind::CacheMailboxesStore { .. }
                    | OperationKind::CacheMessageStore { .. }
            )
        })
        .map(|e| e.id)
        .collect();
    let mut followups = Vec::new();
    for id in ids {
        followups.extend(complete_done(s, id));
    }
    followups
}

fn no_effects(effects: &[Effect]) {
    assert!(effects.is_empty(), "expected no effects, got {effects:?}");
}

/// A Tick carrying `mock::now()` plus `offset_seconds` — the reducer's
/// injected clock for autosave timing.
fn tick(s: &mut AppState, offset_seconds: i64) -> Vec<Effect> {
    let now = mock::now() + chrono::Duration::seconds(offset_seconds);
    reduce(s, Action::Tick { now: Box::new(now) })
}

/// Complete `id` with an `Ok` page payload for `req` at `offset`. Returns
/// the follow-up effects (e.g. ticket wxtx preview fetches).
fn complete_page_ok(
    state: &mut AppState,
    id: OperationId,
    req: &PageRequest,
    offset: usize,
) -> Vec<Effect> {
    let page = mock::mock_page(&req.mailbox_id, offset, req.limit);
    let effects = reduce(
        state,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    settle_cache_stores(state, effects)
}

/// A failure action matching `kind`, as the operation manager builds it.
fn failure(id: OperationId, kind: &OperationKind, detail: &str) -> Action {
    Action::BackendCompleted(OperationResult {
        id,
        outcome: Err(OperationFailure {
            code: Some(1),
            detail: String::from(detail),
            retry: Some(kind.retry_spec()),
            ambiguous: false,
        }),
    })
}

fn switch_to(s: &mut AppState, mailbox: &str) {
    s.mailbox_selection = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .position(|m| m.id.0 == mailbox)
        .unwrap();
    s.session.focus = Focus::Sidebar;
    // The cold-context cache read runs first (ticket haeb); the fixture
    // cache is empty, so the miss starts the fresh foreground load.
    let effects = reduce(s, Action::Activate);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let effects = complete_cache_miss(s, cache_id);
    let (id, req) = expect_page(&effects);
    assert_eq!(req.mailbox_id.0, mailbox);
    assert_eq!(req.offset, 0);
    complete_page_ok(s, id, &req, 0);
}

// ── Startup: mailbox listing via the operation registry ──────────────────

fn boot(s: &mut AppState) -> (OperationId, OperationKind) {
    // The cold-start listing load, as the runtime reaches it: `Refresh`
    // reads the cache first (ticket haeb; a miss in the fixture) and the
    // miss starts the fresh background listing.
    let effects = reduce(s, Action::Refresh);
    let (cache_id, _) = effect_parts(&effects);
    let effects = complete_cache_miss(s, cache_id);
    effect_parts(&effects)
}

// ── Background data never resets the user's state (ticket sazy) ──────────

/// Complete the boot listing with the mock mailboxes.
fn complete_mailboxes(s: &mut AppState, id: OperationId, mailboxes: Vec<Mailbox>) {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mailboxes)),
        }),
    );
}

/// Register a mailbox-listing load on a session whose listing already
/// applied (the fresh load the cached startup always runs behind it).
fn start_listing(s: &mut AppState) -> OperationId {
    s.session.operations.start(mailboxes_kind()).id
}

// ── Modal interactions (plan §12) ────────────────────────────────────────

fn open_modal(s: &mut AppState, detail: &str) -> (OperationId, PageRequest) {
    let (id, req) = expect_page(&reduce(s, Action::PageNext));
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(4),
                detail: String::from(detail),
                retry: Some(page_kind(&req).retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    (id, req)
}

// ── Reader and message actions (plan §19 Phase 4) ────────────────────────

/// Complete an in-flight `LoadMessage` with the mock message for the open
/// summary, exactly as the operation manager would deliver it. Returns the
/// follow-up effects the reducer emitted (e.g. the mark-read operation).
fn complete_message_ok(s: &mut AppState, id: OperationId) -> Vec<Effect> {
    let summary = s.open_summary().expect("reader open").clone();
    let message = mock::mock_message(&summary);
    let effects = reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    );
    settle_cache_stores(s, effects)
}

/// Complete an in-flight mutation with a `Done` outcome. Returns the
/// follow-up effects (e.g. the tmail-move page re-sync).
fn complete_done(s: &mut AppState, id: OperationId) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Done),
        }),
    )
}

/// Settle the cache-store effects in `effects` (ticket haeb: writes are
/// best-effort background work whose `Done` results carry nothing to
/// apply): completes each store and returns the remaining effects with
/// the stores' follow-ups appended.
fn settle_cache_stores(s: &mut AppState, effects: Vec<Effect>) -> Vec<Effect> {
    let is_store = |kind: &OperationKind| {
        matches!(
            kind,
            OperationKind::CacheListStore { .. }
                | OperationKind::CacheListEvict { .. }
                | OperationKind::CacheMailboxesStore { .. }
                | OperationKind::CacheMessageStore { .. }
        )
    };
    let store_ids: Vec<OperationId> = effects
        .iter()
        .filter(|e| is_store(&e.kind))
        .map(|e| e.id)
        .collect();
    let mut rest: Vec<Effect> = effects.into_iter().filter(|e| !is_store(&e.kind)).collect();
    for id in store_ids {
        rest.extend(complete_done(s, id));
    }
    rest
}

fn expect_kind(effects: &[Effect]) -> (OperationId, OperationKind) {
    effect_parts(effects)
}

/// Open the reader as the runtime reaches the load: `Activate` reads the
/// message cache first (ticket haeb; a miss in the fixture) and the miss
/// starts the fresh foreground load. Returns its `(id, kind)`.
fn open_reader(s: &mut AppState) -> (OperationId, OperationKind) {
    let effects = reduce(s, Action::Activate);
    let (cache_id, _) = effect_parts(&effects);
    let effects = complete_cache_miss(s, cache_id);
    effect_parts(&effects)
}

// ── Attachment save (plan §15, Phase 8.4) ────────────────────────────────

/// Open the reader on a message carrying two attachments.
fn reader_with_attachments() -> AppState {
    let mut s = state();
    let summary = s.messages.items[0].clone();
    let mut message = mock::mock_message(&summary);
    message.headers.message_id = Some(String::from("att-1@tmail.local"));
    message.attachments = vec![
        crate::domain::Attachment {
            name: Some(String::from("report.pdf")),
            mime_type: Some(String::from("application/pdf")),
            size: Some(14),
            part_id: 3,
        },
        crate::domain::Attachment {
            name: None,
            mime_type: Some(String::from("application/octet-stream")),
            size: Some(4),
            part_id: 5,
        },
    ];
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.session.focus = Focus::Reader;
    s
}

fn attachment_request(s: &AppState) -> crate::domain::AttachmentRequest {
    // Rebuild the request the reducer would issue for the selected chip.
    let message = s.open_message.as_loaded().unwrap();
    let index = match s.reader_focus {
        Some(ReaderFocus::Attachment(index)) => index,
        _ => 0,
    };
    let attachment = &message.attachments[index];
    crate::domain::AttachmentRequest {
        locator: crate::domain::MessageLocator {
            mailbox: message.mailbox_id.clone(),
            id: message.id.clone(),
            message_id: message.headers.message_id.clone(),
        },
        part_id: attachment.part_id,
        filename: attachment.name.clone(),
        dir: None,
    }
}

/// Open the reader on an HTML message with two links and one attachment
/// (tickets 1fnh/hc9n): the full Tab cycle in one fixture.
fn reader_with_links_and_attachment() -> AppState {
    let mut s = state();
    let summary = s.messages.items[1].clone();
    let mut message = mock::mock_message(&summary);
    message.plain_body = None;
    message.html_body = Some(String::from(
        "<p>read <a href=\"https://one.example/a\">one</a> and \
         <a href=\"https://two.example/b\">two</a></p>",
    ));
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("report.pdf")),
        mime_type: Some(String::from("application/pdf")),
        size: Some(14),
        part_id: 3,
    }];
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.session.focus = Focus::Reader;
    s
}

/// Open the reader on an HTML message whose only link uses a non-web
/// scheme (ticket hc9n: the opener policy must refuse it).
fn reader_with_non_web_link() -> AppState {
    let mut s = state();
    let summary = s.messages.items[1].clone();
    let mut message = mock::mock_message(&summary);
    message.plain_body = None;
    message.html_body = Some(String::from(
        "<p><a href=\"file:///etc/passwd\">local file</a></p>",
    ));
    message.attachments = Vec::new();
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.session.focus = Focus::Reader;
    s
}
/// Open the composer and return the state (asserts the route/focus).
fn compose(s: &mut AppState) -> &mut crate::app::composer::ComposerState {
    no_effects(&reduce(s, Action::Compose));
    assert_eq!(s.session.routes.len(), 2);
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.session.focus, Focus::Composer);
    s.session.composer.as_mut().expect("composer open")
}

// ── Reopening drafts from the Drafts list (plan §14) ─────────────────────

fn drafts_id() -> MailboxId {
    MailboxId(String::from("drafts"))
}

/// One Drafts-mailbox row as the envelope listing carries it: bare
/// `Message-ID` (no brackets).
fn draft_row(id: &str, message_id: Option<&str>) -> MessageSummary {
    MessageSummary {
        id: MessageId(String::from(id)),
        mailbox_id: drafts_id(),
        message_id: message_id.map(String::from),
        from: Vec::new(),
        to: Vec::new(),
        subject: String::from("Saved draft"),
        snippet: None,
        timestamp: mock::now(),
        is_read: false,
        is_starred: false,
        has_attachments: false,
    }
}

/// The fetched draft copy, as `message read` maps it.
fn fetched_draft(id: &str, message_id: &str) -> Message {
    Message {
        id: MessageId(String::from(id)),
        mailbox_id: drafts_id(),
        headers: MessageHeaders {
            subject: String::from("Hello"),
            from: Vec::new(),
            to: vec![Address {
                name: None,
                email: String::from("dest@example.com"),
            }],
            cc: vec![Address {
                name: None,
                email: String::from("cc@example.com"),
            }],
            bcc: vec![Address {
                name: None,
                email: String::from("bcc@example.com"),
            }],
            date: Some(mock::now()),
            message_id: Some(String::from(message_id)),
            in_reply_to: None,
            references: None,
        },
        plain_body: Some(String::from("draft body")),
        html_body: None,
        attachments: Vec::new(),
    }
}

/// Enter on the `+ attach` control and return the open chooser together
/// with the listing operation's id, still in flight.
fn open_attach_dialog(s: &mut AppState) -> (&mut AttachmentFileDialog, OperationId) {
    compose(s);
    while s.session.composer.as_ref().unwrap().field != ComposerField::Attach {
        reduce(s, Action::FocusNext);
    }
    let (id, kind) = effect_parts(&reduce(s, Action::Activate));
    assert_eq!(
        kind,
        OperationKind::ListAttachmentFiles { path: None },
        "the chooser opens with a home-directory listing"
    );
    assert_eq!(s.session.focus, Focus::Dialog);
    match s.session.overlay.as_mut() {
        Some(Overlay::AttachmentExplorer(dialog)) => (dialog, id),
        other => panic!("expected the attachment chooser, got {other:?}"),
    }
}

/// A deterministic listing target: a temp directory with two files and
/// one subdirectory (the explorer sorts `../`, then dirs, then files).
fn chooser_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    std::fs::write(dir.path().join("report.pdf"), b"pdf").unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"txt").unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    (dir, path)
}

/// Land the listing the runtime built over `dir`.
fn land_listing(s: &mut AppState, id: OperationId, dir: &std::path::Path) {
    let explorer = ratatui_explorer::FileExplorerBuilder::build_with_working_dir(dir).unwrap();
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Explorer(Box::new(explorer))),
        }),
    );
}

/// Complete the in-flight `ReadAttachment` for the selected file with
/// `attachment`.
fn complete_read_ok(s: &mut AppState, id: OperationId, attachment: DraftAttachment) {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Attachment(attachment)),
        }),
    );
}

fn attachment(name: &str) -> DraftAttachment {
    DraftAttachment {
        path: PathBuf::from(format!("/tmp/{name}")),
        name: String::from(name),
        size: 1234,
    }
}

// ── Draft autosave state machine (plan §14 Phase 6.3) ────────────────────

fn expect_save(effects: &[Effect]) -> (OperationId, crate::domain::DraftSnapshot) {
    match effects {
        [effect] => match &effect.kind {
            OperationKind::SaveDraft { draft } => (effect.id, (**draft).clone()),
            other => panic!("expected a SaveDraft effect, got {other:?}"),
        },
        other => panic!("expected exactly one effect, got {other:?}"),
    }
}

fn complete_save_ok(
    s: &mut AppState,
    id: OperationId,
    _revision: u64,
    remote: &str,
) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::DraftSaved {
                remote_id: MessageId(String::from(remote)),
            }),
        }),
    )
}

// ── Draft restore (plan §19 Phase 6.4 crash/restart acceptance) ──────────

fn restored_draft(to: &str, revision: u64, saved_revision: u64) -> crate::domain::RestoredDraft {
    crate::domain::RestoredDraft {
        draft: crate::domain::DraftSnapshot {
            local_id: crate::domain::DraftId(String::from("local-crash-1")),
            message_id: Some(String::from("<crash-1@tmail.local>")),
            in_reply_to: None,
            references: None,
            remote_id: Some(MessageId(String::from("remote-crash"))),
            to: String::from(to),
            cc: String::new(),
            bcc: String::new(),
            subject: String::from("after the crash"),
            body: String::from("typed before the crash\n"),
            attachments: Vec::new(),
            revision,
        },
        saved_revision,
    }
}

fn complete_restore(s: &mut AppState, drafts: Vec<crate::domain::RestoredDraft>) {
    let (id, kind) = effect_parts(&reduce(s, Action::LoadDrafts));
    assert_eq!(kind, OperationKind::LoadDrafts);
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Drafts(drafts)),
        }),
    );
}

// ── Confirmed discard (plan §14 Phase 6.6) ───────────────────────────────

fn open_discard_dialog(s: &mut AppState) {
    compose(s);
    tick(s, 0);
    reduce(s, Action::ComposerEdit(ComposerEdit::Char('x')));
    no_effects(&reduce(s, Action::DiscardDraft));
    assert!(matches!(
        s.session.overlay,
        Some(Overlay::ConfirmDiscard(_))
    ));
    assert_eq!(s.session.focus, Focus::ErrorModal);
}

/// The message a reply/forward acts on, in its parsed reader form.
fn reply_source() -> Message {
    Message {
        id: MessageId(String::from("env-reply-1")),
        mailbox_id: inbox_id(),
        headers: MessageHeaders {
            subject: String::from("Plan review"),
            from: vec![Address {
                name: Some(String::from("Bob")),
                email: String::from("bob@example.org"),
            }],
            to: vec![Address {
                name: None,
                email: String::from("probe@tmail.local"),
            }],
            cc: vec![Address {
                name: None,
                email: String::from("carol@example.org"),
            }],
            bcc: Vec::new(),
            date: Some(mock::now()),
            message_id: Some(String::from("318@tmail.local")),
            in_reply_to: None,
            references: Some(String::from("000@tmail.local")),
        },
        plain_body: Some(String::from("Please review.\nThanks\n")),
        html_body: None,
        attachments: Vec::new(),
    }
}

/// Open the reader with `message` already loaded (as a completed
/// LoadMessage would leave it).
fn open_reader_with(s: &mut AppState, message: Message) {
    let summary = MessageSummary {
        id: message.id.clone(),
        mailbox_id: message.mailbox_id.clone(),
        message_id: message.headers.message_id.clone(),
        from: message.headers.from.clone(),
        to: message.headers.to.clone(),
        subject: message.headers.subject.clone(),
        snippet: None,
        timestamp: message.headers.date.unwrap_or_else(mock::now),
        is_read: true,
        is_starred: false,
        has_attachments: false,
    };
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: message.mailbox_id.clone(),
            summary,
        }));
    s.session.focus = Focus::Reader;
    s.open_message = Loadable::Loaded(message);
}

fn seeded_composer(s: &AppState) -> &crate::app::composer::ComposerState {
    s.session.composer.as_ref().expect("seeded composer")
}

/// A composed, validly addressed state ready to send.
fn sendable(s: &mut AppState) {
    compose(s);
    let composer = s.session.composer.as_mut().unwrap();
    composer.draft.to = String::from("ada@example.org");
    composer.draft.subject = String::from("Hello");
    composer.draft.body = String::from("Body");
}

fn expect_send(effects: &[Effect]) -> (OperationId, OutboundMessage) {
    match effects {
        [effect] => match &effect.kind {
            OperationKind::Send { message } => (effect.id, (**message).clone()),
            other => panic!("expected a Send effect, got {other:?}"),
        },
        other => panic!("expected exactly one effect, got {other:?}"),
    }
}

fn complete_send(s: &mut AppState, id: OperationId, outcome: SendOutcome) -> Vec<Effect> {
    reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::SendOutcome(outcome)),
        }),
    )
}

// ── Phase 9: search (plan §16/§19) ───────────────────────────────────────

/// Destructure an effect into `(id, SearchRequest)`.
fn expect_search(effects: &[Effect]) -> (OperationId, crate::domain::SearchRequest) {
    let (id, kind) = effect_parts(effects);
    match kind {
        OperationKind::Search(request) => (id, request),
        other => panic!("expected a Search effect, got {other:?}"),
    }
}

/// The first `Search` effect, wherever it sits in the batch (a timer tick
/// also chains the sidebar listing refresh, ticket txdt).
fn find_search(effects: &[Effect]) -> (OperationId, crate::domain::SearchRequest) {
    effects
        .iter()
        .find_map(|e| match &e.kind {
            OperationKind::Search(request) => Some((e.id, request.clone())),
            _ => None,
        })
        .expect("a Search effect")
}

/// Focus the search field, type a query, and submit.
fn search(s: &mut AppState, query: &str) -> Vec<Effect> {
    reduce(&mut *s, Action::OpenSearch);
    for c in query.chars() {
        reduce(s, Action::SearchEdit(SearchEdit::Char(c)));
    }
    reduce(s, Action::SubmitSearch)
}

// ── Phase 9: periodic refresh + suppression (plan §11/§19) ──────────────

/// Enable the timer at 60s. The state fixture carries no clock yet.
fn timer(s: &mut AppState) {
    s.settings.refresh_interval_seconds = 60;
}

// ── Phase 9.5: selection preservation ────────────────────────────────────

/// A summary with a stable Message-ID for identity tests.
fn identified(
    summary: &crate::domain::MessageSummary,
    message_id: &str,
) -> crate::domain::MessageSummary {
    let mut m = summary.clone();
    m.message_id = Some(String::from(message_id));
    m
}

// ── External editor (plan §14, Phase 11) ─────────────────────────────────

fn edit_external(s: &mut AppState) -> Vec<Effect> {
    reduce(s, Action::EditExternal)
}

// ── List previews (ticket wxtx) ──────────────────────────────────────────

/// The `(id, locator)` pairs of every Preview effect.
fn expect_previews(effects: &[Effect]) -> Vec<(OperationId, crate::domain::MessageLocator)> {
    effects
        .iter()
        .filter_map(|e| match &e.kind {
            OperationKind::Preview(locator) => Some((e.id, locator.clone())),
            _ => None,
        })
        .collect()
}

/// Switch the fixture to the Sent mailbox (its rows ship without
/// snippets) and complete the fresh page load. The cold-context cache
/// read runs first and misses (no cache wired in the fixture), which
/// starts the fresh foreground load; the page apply then starts one
/// cache read per snippet-less row, each missing into a background
/// fetch. Returns the effects of those fetch starts.
fn load_sent_without_snippets(s: &mut AppState) -> Vec<Effect> {
    reduce(s, Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(s, Action::Click(ClickTarget::Mailbox(1))); // activate
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let effects = complete_cache_miss(s, cache_id);
    let (load, req) = expect_page(&effects);
    let effects = complete_page_ok(s, load, &req, 0);
    let reads = expect_cache_preview_reads(&effects);
    let mut fetches = Vec::new();
    for (id, _) in reads {
        fetches.extend(complete_cache_miss(s, id));
    }
    fetches
}

/// Complete one in-flight preview with the mocked full message. Returns
/// the follow-up effects (the rolling fetch refill).
fn complete_preview_ok(
    s: &mut AppState,
    id: OperationId,
    summary: &crate::domain::MessageSummary,
) -> Vec<Effect> {
    let message = mock::mock_message(summary);
    let effects = reduce(
        s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    );
    settle_cache_stores(s, effects)
}

// ── Theme picker (ticket k5ba) ───────────────────────────────────────────

/// Three-palette state for the picker tests.
fn picker_state() -> AppState {
    let mut s = state();
    s.settings.themes = vec![
        (
            String::from("default"),
            crate::view::theme::Theme::default_dark(),
        ),
        (
            String::from("light"),
            crate::view::theme::Theme::default_light(),
        ),
        (
            String::from("nord"),
            crate::view::theme::Theme::default_dark(),
        ),
    ];
    s.settings.theme_index = 0;
    s
}

// ── Reply / forward seeding from the list (NORMAL) ───────────────────────

fn expect_seed(effects: &[Effect]) -> (OperationId, MessageLocator, SeedKind) {
    let (id, kind) = effect_parts(effects);
    match kind {
        OperationKind::SeedComposer { locator, kind } => (id, locator, kind),
        other => panic!("expected a SeedComposer effect, got {other:?}"),
    }
}

/// The focus saved with the dialog (what the close restores).
fn previous_focus_of(s: &AppState) -> Focus {
    let Some(Overlay::Help(dialog)) = &s.session.overlay else {
        panic!("help overlay expected");
    };
    dialog.previous_focus
}
