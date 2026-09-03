//! Reducer unit tests (plan §9: invalid/empty selections, stale inputs,
//! route behavior, resize; plan §19 Phase 3: operation registry semantics,
//! stale/superseded-result rejection, cancellation, Retry/Dismiss modal).

use super::*;
use crate::app::action::SearchEdit;
use crate::app::action::{BulkOp, ClickTarget};
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::mock::{self, mock_initial_state};
use crate::app::operation::{OperationId, RetrySpec};
use crate::app::overlay::Overlay;
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::domain::{Mailbox, MailboxId, MailboxRole, MessageId, PageRequest};

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

fn no_effects(effects: &[Effect]) {
    assert!(effects.is_empty(), "expected no effects, got {effects:?}");
}

/// A Tick carrying `mock::now()` plus `offset_seconds` — the reducer's
/// injected clock for autosave timing.
fn tick(s: &mut AppState, offset_seconds: i64) -> Vec<Effect> {
    let now = mock::now() + chrono::Duration::seconds(offset_seconds);
    reduce(s, &Action::Tick { now: Box::new(now) })
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
    reduce(
        state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    )
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

#[test]
fn selection_moves_down_up_and_clamps() {
    let mut s = state();
    assert_eq!(s.selection, 0);
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.selection, 1);
    for _ in 0..100 {
        reduce(&mut s, &Action::MoveDown);
    }
    assert_eq!(s.selection, s.messages.items.len() - 1);
    reduce(&mut s, &Action::MoveUp);
    assert_eq!(s.selection, s.messages.items.len() - 2);
    for _ in 0..100 {
        reduce(&mut s, &Action::MoveUp);
    }
    assert_eq!(s.selection, 0);
}

#[test]
fn page_next_requests_next_page_and_applies_result() {
    let mut s = state();
    s.selection = 3;
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    assert_eq!(req.offset, mock::PAGE_SIZE);
    // The request is registered and in flight for the active mailbox.
    assert_eq!(s.operations.page_in_flight(&inbox_id()), Some(req.clone()));
    // The old page and selection stay visible until the result lands.
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.selection, 3);
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    assert_eq!(s.operations.page_in_flight(&inbox_id()), None);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
    assert_eq!(s.messages.items.len(), 5);
    // The previous selection's message is not on this page.
    assert_eq!(s.selection, 0);
}

#[test]
fn page_boundaries_never_issue_requests() {
    let mut s = state();
    // No page -1.
    no_effects(&reduce(&mut s, &Action::PagePrevious));
    // Advance to page 2 the way the runtime does: request, then result.
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    // No page past the known total (25 items, 2 pages).
    no_effects(&reduce(&mut s, &Action::PageNext));
    let (id, req) = expect_page(&reduce(&mut s, &Action::PagePrevious));
    assert_eq!(req.offset, 0);
    complete_page_ok(&mut s, id, &req, 0);
    no_effects(&reduce(&mut s, &Action::PagePrevious));
}

#[test]
fn unknown_total_short_page_blocks_next_request() {
    let mut s = state();
    // Maildir listings carry no total (ADR 0001 finding 2).
    s.messages.total = None;
    s.messages.items.truncate(5);
    no_effects(&reduce(&mut s, &Action::PageNext));
    // A full page may have a successor, so the request is issued.
    s.messages.items = mock::mock_page(&inbox_id(), 0, mock::PAGE_SIZE).items;
    assert_eq!(
        expect_page(&reduce(&mut s, &Action::PageNext)).1.offset,
        mock::PAGE_SIZE
    );
}

#[test]
fn rapid_page_next_supersedes_the_older_request() {
    let mut s = state();
    s.messages.total = None;
    let (first_id, first) = expect_page(&reduce(&mut s, &Action::PageNext));
    let first_token = s.operations.cancellation(first_id).unwrap();
    let (second_id, second) = expect_page(&reduce(&mut s, &Action::PageNext));
    assert_eq!(second.offset, first.offset + mock::PAGE_SIZE);
    // Superseding cancelled the older operation and removed it, so its
    // result — however late — can never apply (plan §11).
    assert!(first_token.is_cancelled());
    assert!(s.operations.get(first_id).is_none());
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: first_id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                first.offset,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    assert_eq!(s.messages.offset, 0);
    complete_page_ok(&mut s, second_id, &second, second.offset);
    assert_eq!(s.messages.offset, second.offset);
}

#[test]
fn unknown_operation_result_is_ignored() {
    let mut s = state();
    let before = s.clone();
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: OperationId(999),
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                40,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    assert_eq!(s.messages, before.messages);
    assert_eq!(s.overlay, None);
}

#[test]
fn payload_kind_mismatch_is_ignored() {
    let mut s = state();
    let (id, _req) = expect_page(&reduce(&mut s, &Action::PageNext));
    // A page payload for a mailbox operation (defensive: manager routes by
    // kind) must not crash or mutate state.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mock::mock_mailboxes())),
        }),
    );
    assert_eq!(s.messages.offset, 0);
    assert!(s.operations.get(id).is_none(), "operation completed");
}

#[test]
fn page_result_for_other_mailbox_is_dropped() {
    let mut s = state();
    // An inbox page load is in flight…
    let (inbox_op, inbox_req) = expect_page(&reduce(&mut s, &Action::PageNext));
    // …then the user switches to Sent, which starts its own request.
    s.mailbox_selection = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .position(|m| m.id.0 == "sent")
        .unwrap();
    s.focus = Focus::Sidebar;
    let (sent_op, sent_req) = expect_page(&reduce(&mut s, &Action::Activate));
    assert_eq!(sent_req.mailbox_id.0, "sent");
    // The slower inbox result arrives after the switch: it must never
    // replace what the Sent view is loading (plan §11).
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: inbox_op,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                inbox_req.offset,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    assert!(s.messages.items.is_empty(), "sent page not loaded yet");
    complete_page_ok(&mut s, sent_op, &sent_req, 0);
    assert_eq!(s.messages.items.len(), 4, "sent page applied");
}

#[test]
fn page_load_failure_opens_the_retry_modal_and_keeps_last_page() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(&mut s, &failure(id, &page_kind(&req), "himalaya exploded"));
    assert_eq!(s.messages.offset, 0, "last coherent page stays");
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("error modal must open on failure");
    };
    assert_eq!(dialog.code, Some(1));
    assert!(dialog.detail.contains("himalaya exploded"));
    assert_eq!(
        dialog.retry,
        Some(RetrySpec {
            kind: page_kind(&req)
        })
    );
    assert_eq!(s.focus, Focus::ErrorModal);
    assert_eq!(s.status.message.as_deref(), Some("Operation failed"));
}

#[test]
fn selection_identity_survives_refresh() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let selected: MessageId = s.selected_message().expect("selection").id.clone();
    let (id, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    assert_eq!(req.offset, 0);
    assert_eq!(s.status.message.as_deref(), Some("Refreshing…"));
    complete_page_ok(&mut s, id, &req, 0);
    assert_eq!(
        s.selected_message().expect("selection after refresh").id,
        selected
    );
}

#[test]
fn refresh_on_second_page_keeps_page_and_selection() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    s.selection = 2;
    let selected = s.selected_message().unwrap().id.clone();
    let (id, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    assert_eq!(req.offset, mock::PAGE_SIZE);
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
    assert_eq!(s.selected_message().unwrap().id, selected);
}

fn switch_to(s: &mut AppState, mailbox: &str) {
    s.mailbox_selection = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .position(|m| m.id.0 == mailbox)
        .unwrap();
    s.focus = Focus::Sidebar;
    let (id, req) = expect_page(&reduce(s, &Action::Activate));
    assert_eq!(req.mailbox_id.0, mailbox);
    assert_eq!(req.offset, 0);
    complete_page_ok(s, id, &req, 0);
}

#[test]
fn empty_list_never_panics() {
    let mut s = state();
    switch_to(&mut s, "trash");
    s.messages.items.clear();
    s.messages.total = Some(0);
    s.selection = 0;
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveUp);
    no_effects(&reduce(&mut s, &Action::PageNext));
    assert_eq!(s.selection, 0);
    assert_eq!(s.messages.offset, 0);
}

#[test]
fn activate_on_sidebar_switches_mailbox_and_resets_list() {
    let mut s = state();
    s.selection = 7;
    switch_to(&mut s, "sent");
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "sent");
    assert_eq!(s.selection, 0);
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.messages.items.len(), 4);
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn activate_on_same_mailbox_is_noop() {
    let mut s = state();
    s.selection = 5;
    s.focus = Focus::Sidebar;
    no_effects(&reduce(&mut s, &Action::Activate));
    assert_eq!(s.selection, 5);
    assert_eq!(s.messages.offset, 0);
}

// ── Startup: mailbox listing via the operation registry ──────────────────

fn boot(s: &mut AppState) -> (OperationId, OperationKind) {
    effect_parts(&reduce(s, &Action::Refresh))
}

#[test]
fn refresh_before_mailboxes_load_starts_the_listing() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, kind) = boot(&mut s);
    assert_eq!(kind, mailboxes_kind());
    assert!(s.operations.get(id).is_some());
    // A second refresh while loading must not stack a duplicate request.
    no_effects(&reduce(&mut s, &Action::Refresh));
    assert_eq!(s.operations.len(), 1);
}

#[test]
fn mailboxes_loaded_selects_inbox_role() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    assert_eq!(s.routes.len(), 0);
    let (id, kind) = boot(&mut s);
    assert_eq!(kind, mailboxes_kind());
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mock::mock_mailboxes())),
        }),
    );
    let (_, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    assert_eq!(req.mailbox_id.0, "inbox");
    assert_eq!(req.offset, 0);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.mailbox_selection, 0);
    assert!(s.mailboxes.as_loaded().is_some());
}

#[test]
fn mailboxes_loaded_falls_back_to_first_mailbox() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, _) = boot(&mut s);
    let mailboxes = vec![
        Mailbox {
            id: MailboxId(String::from("notes")),
            name: String::from("Notes"),
            role: None,
            unread_count: None,
            total_count: None,
        },
        Mailbox {
            id: MailboxId(String::from("sent")),
            name: String::from("Sent"),
            role: Some(MailboxRole::Sent),
            unread_count: None,
            total_count: None,
        },
    ];
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mailboxes)),
        }),
    );
    let (_, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    assert_eq!(req.mailbox_id.0, "notes");
    assert_eq!(s.mailbox_selection, 0);
}

#[test]
fn mailboxes_loaded_empty_is_valid_not_an_error() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, _) = boot(&mut s);
    no_effects(&reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(Vec::new())),
        }),
    ));
    assert_eq!(s.routes.len(), 0);
    assert!(s.messages.items.is_empty());
    assert!(!s.quit_requested);
}

#[test]
fn mailboxes_failure_opens_modal_and_retry_reloads() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, kind) = boot(&mut s);
    reduce(&mut s, &failure(id, &kind, "no such account"));
    assert!(matches!(s.mailboxes, Loadable::Failed(_)));
    assert!(s.overlay.is_some());
    // Retry replays the typed mailbox-load intent under a new id.
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, retry_kind) = effect_parts(&effects);
    assert_eq!(retry_kind, mailboxes_kind());
    assert_ne!(retry_id, id, "retry gets a fresh operation id");
    assert!(s.overlay.is_none(), "modal closed on retry");
    assert!(matches!(s.mailboxes, Loadable::Loading));
    assert!(s.operations.get(retry_id).is_some());
}

#[test]
fn input_and_ticks_keep_working_while_an_operation_is_in_flight() {
    let mut s = state();
    let _ = expect_page(&reduce(&mut s, &Action::PageNext));
    // Foreground work never blocks rendering or input (plan §3): movement
    // and ticks apply while the request is in flight.
    tick(&mut s, 0);
    assert_eq!(s.ticks, 1);
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.selection, 1);
    assert!(s.operations.page_in_flight(&inbox_id()).is_some());
}

// ── Esc cancellation (plan §10/§11) ──────────────────────────────────────

#[test]
fn esc_cancels_foreground_work_and_returns_to_stable_state() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    let token = s.operations.cancellation(id).unwrap();
    reduce(&mut s, &Action::BackOrCancel);
    // The operation is gone and its token fired: the backend kills the
    // child it owns (Phase 3.2).
    assert!(token.is_cancelled());
    assert!(s.operations.get(id).is_none());
    assert_eq!(s.operations.page_in_flight(&req.mailbox_id), None);
    // The prior stable state stands: old page still displayed, no modal,
    // no route change, no quit.
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert_eq!(s.overlay, None);
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "inbox");
    assert!(!s.quit_requested);
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("cancelled"))
    );
}

#[test]
fn esc_during_startup_load_cancels_then_second_esc_quits() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, _) = boot(&mut s);
    let token = s.operations.cancellation(id).unwrap();
    reduce(&mut s, &Action::BackOrCancel);
    assert!(token.is_cancelled());
    assert!(!s.quit_requested, "first Esc cancels, does not quit");
    // Nothing left to cancel: the next Esc quits as before.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.quit_requested);
}

#[test]
fn esc_with_open_modal_dismisses_it() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(&mut s, &failure(id, &page_kind(&req), "boom"));
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none(), "Esc closes the modal");
    assert_eq!(s.focus, Focus::MessageList);
}

// ── Modal interactions (plan §12) ────────────────────────────────────────

fn open_modal(s: &mut AppState, detail: &str) -> (OperationId, PageRequest) {
    let (id, req) = expect_page(&reduce(s, &Action::PageNext));
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
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

#[test]
fn retry_replays_equivalent_intent_with_new_operation_id() {
    let mut s = state();
    let (id, req) = open_modal(&mut s, "himalaya exploded");
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, retry_kind) = effect_parts(&effects);
    assert_ne!(retry_id, id, "retry must allocate a new operation id");
    assert_eq!(retry_kind, page_kind(&req), "same typed intent");
    assert!(s.operations.get(retry_id).is_some());
    assert!(s.operations.get(id).is_none());
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn dismiss_closes_modal_without_side_effects() {
    let mut s = state();
    open_modal(&mut s, "himalaya exploded");
    no_effects(&reduce(&mut s, &Action::DismissError));
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
    // The last coherent page is untouched by dismiss.
    assert_eq!(s.messages.offset, 0);
}

#[test]
fn modal_enter_activates_the_focused_button() {
    let mut s = state();
    open_modal(&mut s, "himalaya exploded");
    // Default button is Dismiss: Enter dismisses without new work.
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.overlay.is_none());
    // Focus Retry first: Enter replays the intent.
    let (_, req) = open_modal(&mut s, "himalaya exploded");
    reduce(&mut s, &Action::FocusNext);
    let effects = reduce(&mut s, &Action::Activate);
    let (_, kind) = effect_parts(&effects);
    assert_eq!(kind, page_kind(&req));
}

#[test]
fn modal_buttons_toggle_with_tab() {
    let mut s = state();
    open_modal(&mut s, "boom");
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Dismiss);
    reduce(&mut s, &Action::FocusNext);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Retry);
    reduce(&mut s, &Action::FocusPrevious);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Dismiss);
}

#[test]
fn modal_scroll_clamps_to_content() {
    let long_detail = "line\n".repeat(60);
    let mut s = state();
    open_modal(&mut s, long_detail.trim_end());
    let max = match &s.overlay {
        Some(Overlay::Error(dialog)) => {
            crate::ui::components::error_modal::max_scroll(dialog, s.size)
        }
        other => panic!("error modal open, got {other:?}"),
    };
    assert!(max > 0, "long detail must overflow the viewport");
    for _ in 0..(max + 20) {
        reduce(&mut s, &Action::MoveDown);
    }
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.scroll, max, "scroll clamps at the end");
    for _ in 0..(max + 5) {
        reduce(&mut s, &Action::MoveUp);
    }
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.scroll, 0, "scroll clamps at the start");
    // Page-style scrolling moves in viewport steps and clamps the same way.
    reduce(&mut s, &Action::PageNext);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.scroll > 0);
}

#[test]
fn modal_restores_previous_focus_on_dismiss() {
    let mut s = state();
    let (_, _) = open_modal(&mut s, "boom");
    // Simulate having been in the sidebar when the failure hit.
    if let Some(Overlay::Error(dialog)) = s.overlay.as_mut() {
        dialog.previous_focus = Focus::Sidebar;
    }
    assert_eq!(s.focus, Focus::ErrorModal);
    reduce(&mut s, &Action::DismissError);
    assert_eq!(s.focus, Focus::Sidebar, "focus restored");
}

#[test]
fn modal_swallows_unrelated_input() {
    let mut s = state();
    open_modal(&mut s, "boom");
    let before = s.clone();
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Char('x')));
    reduce(&mut s, &Action::SubmitSearch);
    reduce(&mut s, &Action::OpenSearch);
    reduce(&mut s, &Action::Compose);
    reduce(&mut s, &Action::Quit);
    assert_eq!(s.search_query, before.search_query);
    assert_eq!(s.focus, before.focus);
    assert_eq!(s.quit_requested, before.quit_requested);
    assert!(s.overlay.is_some(), "modal stays open");
}

#[test]
fn ambiguous_failure_is_flagged_for_the_modal() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("SMTP DATA failed: reached unexpected EOF"),
                retry: Some(page_kind(&req).retry_spec()),
                ambiguous: true,
            }),
        }),
    );
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous, "ambiguity must reach the modal");
}

// ── Reader and message actions (plan §19 Phase 4) ────────────────────────

/// Complete an in-flight `LoadMessage` with the mock message for the open
/// summary, exactly as the operation manager would deliver it. Returns the
/// follow-up effects the reducer emitted (e.g. the mark-read operation).
fn complete_message_ok(s: &mut AppState, id: OperationId) -> Vec<Effect> {
    let summary = s.open_summary().expect("reader open").clone();
    let message = mock::mock_message(&summary);
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    )
}

/// Complete an in-flight mutation with a `Done` outcome. Returns the
/// follow-up effects (e.g. the post-move page re-sync).
fn complete_done(s: &mut AppState, id: OperationId) -> Vec<Effect> {
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Done),
        }),
    )
}

fn expect_kind(effects: &[Effect]) -> (OperationId, OperationKind) {
    effect_parts(effects)
}

#[test]
fn activate_on_message_list_opens_the_reader() {
    let mut s = state();
    s.selection = 1;
    let selected = s.selected_message().unwrap().clone();
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::Activate));
    assert!(
        matches!(&kind, OperationKind::LoadMessage(locator)
                if locator.id == selected.id && locator.mailbox == selected.mailbox_id
        ),
        "kind: {kind:?}"
    );
    assert!(s.operations.get(id).is_some());
    // Route stack: reader on top of the mailbox route.
    assert_eq!(s.routes.len(), 2);
    assert_eq!(s.open_summary().unwrap().id, selected.id);
    assert_eq!(s.focus, Focus::Reader);
    assert!(matches!(s.open_message, Loadable::Loading));
    assert_eq!(s.reader_scroll, 0);
    // The list underneath is untouched (restoration is by construction).
    assert_eq!(s.messages.offset, 0);
}

#[test]
fn reader_result_applies_and_marks_unread_read() {
    let mut s = state();
    s.selection = 1; // m2: unread in the mock seed.
    assert!(!s.selected_message().unwrap().is_read);
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    // The load completion itself emits the mark-read operation.
    let (flag_id, kind) = expect_kind(&complete_message_ok(&mut s, id));
    assert!(matches!(s.open_message, Loadable::Loaded(_)));
    assert!(
        matches!(&kind, OperationKind::SetRead { read: true, .. }),
        "kind: {kind:?}"
    );
    // The list still shows the message as unread: the UI updates only
    // after confirmation (plan §19 Phase 4 acceptance).
    assert!(!s.messages.items[1].is_read);
    complete_done(&mut s, flag_id);
    // Confirmation updates both the list row and the reader's snapshot.
    assert!(s.messages.items[1].is_read);
    assert!(s.open_summary().unwrap().is_read);
    assert!(s.operations.is_empty());
}

#[test]
fn read_message_load_does_not_trigger_mark_read() {
    let mut s = state();
    s.selection = 3; // m4: read in the mock seed.
    assert!(s.selected_message().unwrap().is_read);
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    // No further operations: a read message needs no flag change.
    assert!(s.operations.is_empty());
}

#[test]
fn reader_result_fills_missing_snippet() {
    let mut s = state();
    s.messages.items[2].snippet = None;
    s.selection = 2;
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    let snippet = s.messages.items[2].snippet.as_deref();
    // The list preview is the same one-line body text the background
    // previews produce (ticket wxtx).
    assert!(
        snippet.is_some_and(|s| s.starts_with("body line 01") && s.ends_with('…')),
        "{snippet:?}"
    );
}

#[test]
fn preview_snippets_survive_a_background_refresh() {
    let mut s = state();
    let effects = load_sent_without_snippets(&mut s);
    let previews = expect_previews(&effects);
    // Two previews land; their rows show previews.
    for (id, locator) in previews.iter().take(2) {
        let summary = s
            .messages
            .items
            .iter()
            .find(|m| m.id == locator.id)
            .expect("row listed")
            .clone();
        complete_preview_ok(&mut s, *id, &summary);
    }
    assert_eq!(
        s.messages
            .items
            .iter()
            .filter(|m| m.snippet.is_some())
            .count(),
        2
    );
    // A fresh page load (the periodic refresh path) replaces every
    // summary — the rendered previews must survive it, with no re-fetches
    // for anything already previewed or in flight (ticket wxtx).
    let req = PageRequest {
        mailbox_id: MailboxId(String::from("sent")),
        offset: 0,
        limit: 20,
    };
    let id = s.operations.start(OperationKind::LoadPage(req.clone())).id;
    let effects = complete_page_ok(&mut s, id, &req, 0);
    no_effects(&effects);
    assert_eq!(
        s.messages
            .items
            .iter()
            .filter(|m| m.snippet.is_some())
            .count(),
        2,
        "previews survive the page replacement"
    );
    // The still-in-flight previews fill their (new) rows when they land.
    for (id, locator) in previews.iter().skip(2) {
        let summary = s
            .messages
            .items
            .iter()
            .find(|m| m.id == locator.id)
            .expect("row listed")
            .clone();
        complete_preview_ok(&mut s, *id, &summary);
    }
    assert_eq!(
        s.messages
            .items
            .iter()
            .filter(|m| m.snippet.is_some())
            .count(),
        4
    );
}

#[test]
fn disk_cached_messages_fill_previews_without_fetching() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    s.page_cache = Some(crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    ));
    // Messages fetched in an earlier session: every Sent row's full
    // message is already on disk.
    for summary in mock::mock_page(&MailboxId(String::from("sent")), 0, 20).items {
        let message = mock::mock_message(&summary);
        s.page_cache
            .as_ref()
            .unwrap()
            .store_message(&summary.mailbox_id, &summary.id.0, &message);
    }
    // Switch to Sent: the fresh page loads, and every preview is served
    // from the cache — no background fetches start, nothing re-requests
    // what was fetched before (ticket wxtx).
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // activate
    let (load, req) = expect_page(&effects);
    let effects = complete_page_ok(&mut s, load, &req, 0);
    no_effects(&effects);
    assert_eq!(
        s.messages
            .items
            .iter()
            .filter(|m| m.snippet.is_some())
            .count(),
        4,
        "previews fill from the cached copies"
    );
    assert!(
        s.messages
            .items
            .iter()
            .all(|m| s.previews.contains_key(&m.id))
    );
    assert!(
        s.messages
            .items
            .iter()
            .all(|m| s.preview_requested.contains(&m.id))
    );
}

#[test]
fn esc_from_reader_restores_exact_list_state() {
    let mut s = state();
    s.selection = 5;
    s.list_scroll = 3;
    let before = s.clone();
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    // The load may start a mark-read op; settle it so nothing is in flight.
    while !s.operations.is_empty() {
        let pending = s.operations.foreground().unwrap().id;
        if matches!(
            s.operations.get(pending).unwrap().kind,
            OperationKind::SetRead { .. }
        ) {
            complete_done(&mut s, pending);
        } else {
            complete_message_ok(&mut s, pending);
        }
    }
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(matches!(s.open_message, Loadable::Idle));
    assert_eq!(s.reader_scroll, 0);
    // Exact restoration: page, selection, scroll.
    assert_eq!(s.messages.items.len(), before.messages.items.len());
    assert_eq!(s.messages.offset, before.messages.offset);
    assert_eq!(s.selection, before.selection);
    assert_eq!(s.list_scroll, before.list_scroll);
}

#[test]
fn tab_cycles_the_reader_attachment_cursor_and_wraps() {
    let mut s = state();
    let summary = s.messages.items[0].clone();
    let mut message = mock::mock_message(&summary);
    message.attachments = vec![
        crate::domain::Attachment {
            name: Some(String::from("a.pdf")),
            mime_type: Some(String::from("application/pdf")),
            size: Some(1),
            part_id: 2,
        },
        crate::domain::Attachment {
            name: Some(String::from("b.png")),
            mime_type: Some(String::from("image/png")),
            size: Some(2),
            part_id: 5,
        },
    ];
    s.routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.focus = Focus::Reader;

    assert_eq!(s.reader_attachment, None, "cursor starts at the first chip");
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_attachment, Some(1));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_attachment, Some(0), "wraps forward");
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(s.reader_attachment, Some(1), "wraps backward");
    // Closing the reader resets the cursor.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.reader_attachment, None);
}

#[test]
fn tab_is_inert_without_attachments() {
    let mut s = state();
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    assert_eq!(s.focus, Focus::Reader);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_attachment, None, "no chips: no cursor");
}

// ── Attachment save (plan §15, Phase 8.4) ────────────────────────────────

/// Open the reader on a message carrying two attachments.
fn reader_with_attachments() -> AppState {
    let mut s = state();
    let summary = s.messages.items[0].clone();
    let mut message = mock::mock_message(&summary);
    message.headers.message_id = Some(String::from("att-1@post.local"));
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
    s.routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.focus = Focus::Reader;
    s
}

fn attachment_request(s: &AppState) -> crate::domain::AttachmentRequest {
    // Rebuild the request the reducer would issue for the selected chip.
    let message = s.open_message.as_loaded().unwrap();
    let index = s.reader_attachment.unwrap_or(0);
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

#[test]
fn d_saves_the_selected_attachment_with_a_frozen_request() {
    let mut s = reader_with_attachments();
    let effects = reduce(&mut s, &Action::SaveAttachment);
    let (id, kind) = effect_parts(&effects);
    assert_eq!(kind.summary(), "Saving attachment");
    let OperationKind::SaveAttachment {
        request,
        open_after: _,
    } = kind
    else {
        panic!("expected SaveAttachment");
    };
    assert_eq!(request, attachment_request(&s));
    assert_eq!(request.part_id, 3, "first chip by default");
    assert_eq!(request.filename.as_deref(), Some("report.pdf"));
    assert_eq!(request.dir, None, "the backend resolves the downloads dir");
    assert_eq!(s.status.message.as_deref(), Some("Saving attachment…"));
    // Completing records the final path (possibly collision-renamed).
    let final_path = PathBuf::from("/home/u/Downloads/report (1).pdf");
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::SavedPath(final_path.clone())),
        }),
    );
    assert_eq!(
        s.saved_attachments.values().collect::<Vec<_>>(),
        vec![&final_path],
        "the path is remembered for Open reuse"
    );
    assert_eq!(
        s.status.message.as_deref(),
        Some(format!("Saved to {}", final_path.display())).as_deref()
    );
}

#[test]
fn saving_targets_the_cursor_chip_by_part_id() {
    let mut s = reader_with_attachments();
    // Tab to the second chip (unnamed → part-id fallback naming).
    reduce(&mut s, &Action::FocusNext);
    let effects = reduce(&mut s, &Action::SaveAttachment);
    let (_, kind) = effect_parts(&effects);
    let OperationKind::SaveAttachment {
        request,
        open_after: _,
    } = kind
    else {
        panic!("expected SaveAttachment");
    };
    assert_eq!(request.part_id, 5);
    assert_eq!(request.filename, None);
}

#[test]
fn save_is_reader_only_and_attachment_gated() {
    // From the list focus the action is inert.
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::SaveAttachment));
    assert!(s.operations.is_empty());
    // In the reader without attachments too.
    let mut s = state();
    let summary = s.messages.items[0].clone();
    let message = mock::mock_message(&summary);
    s.routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.focus = Focus::Reader;
    no_effects(&reduce(&mut s, &Action::SaveAttachment));
    assert!(s.operations.is_empty());
}

#[test]
fn save_failure_opens_a_retryable_modal() {
    let mut s = reader_with_attachments();
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::SaveAttachment));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("disk full"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));
    let retry = match &s.overlay {
        Some(Overlay::Error(dialog)) => dialog.retry.clone().expect("retryable"),
        _ => unreachable!(),
    };
    assert!(matches!(retry.kind, OperationKind::SaveAttachment { .. }));
    // Retry replays the identical request under a new operation id.
    let replayed = reduce(&mut s, &Action::RetryError);
    let (new_id, new_kind) = effect_parts(&replayed);
    assert_ne!(new_id, id);
    assert_eq!(new_kind, retry.kind);
}

#[test]
fn keyboard_d_maps_to_save_attachment() {
    use crate::input::keyboard;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let action = keyboard::to_action(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        Focus::Reader,
    );
    assert_eq!(action, Some(Action::SaveAttachment));
}

#[test]
fn keyboard_d_and_o_map_to_save_and_open() {
    use crate::input::keyboard;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    assert_eq!(
        keyboard::to_action(key('d'), Focus::Reader),
        Some(Action::SaveAttachment)
    );
    assert_eq!(
        keyboard::to_action(key('o'), Focus::Reader),
        Some(Action::OpenAttachment)
    );
}

#[test]
fn o_saves_first_then_chains_the_opener_on_the_confirmed_path() {
    let mut s = reader_with_attachments();
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::OpenAttachment));
    // Nothing saved yet: the save runs with the open-after chain armed.
    let OperationKind::SaveAttachment {
        request: _,
        open_after,
    } = kind
    else {
        panic!("expected SaveAttachment");
    };
    assert!(open_after);
    let final_path = PathBuf::from("/home/u/Downloads/report (2).pdf");
    let effects = reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::SavedPath(final_path.clone())),
        }),
    );
    // The opener chains on the path that was actually written.
    let (open_id, open_kind) = effect_parts(&effects);
    assert_eq!(
        open_kind,
        OperationKind::OpenPath {
            path: final_path.clone()
        }
    );
    // Completing the open closes the loop.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: open_id,
            outcome: Ok(OperationOutcome::Done),
        }),
    );
    assert_eq!(s.status.message.as_deref(), Some("Opened"));
}

#[test]
fn o_reuses_a_path_saved_this_session_without_a_second_save() {
    let mut s = reader_with_attachments();
    let saved = PathBuf::from("/home/u/Downloads/report.pdf");
    let message_id = s.open_message.as_loaded().unwrap().id.clone();
    s.saved_attachments.insert((message_id, 3), saved.clone());
    let effects = reduce(&mut s, &Action::OpenAttachment);
    // Straight to the opener — no download, no duplicate file.
    let (_, kind) = effect_parts(&effects);
    assert_eq!(
        kind,
        OperationKind::OpenPath { path: saved },
        "no SaveAttachment was started"
    );
}

#[test]
fn open_is_reader_only_and_attachment_gated() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::OpenAttachment));
    assert!(s.operations.is_empty());
}

#[test]
fn save_failure_keeps_the_open_chain_off() {
    // A failed save-then-open opens the modal; no opener runs.
    let mut s = reader_with_attachments();
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::OpenAttachment));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("disk full"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));
    assert!(s.saved_attachments.is_empty());
    // Retrying replays the save (with the chain armed) under a new id.
    let replayed = reduce(&mut s, &Action::RetryError);
    let (_, kind) = effect_parts(&replayed);
    assert!(matches!(
        kind,
        OperationKind::SaveAttachment {
            open_after: true,
            ..
        }
    ));
}

#[test]
fn esc_cancels_message_load_then_second_esc_returns() {
    let mut s = state();
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    let token = s.operations.cancellation(id).unwrap();
    // First Esc cancels the foreground load (plan §10 order).
    reduce(&mut s, &Action::BackOrCancel);
    assert!(token.is_cancelled());
    assert!(s.operations.get(id).is_none());
    assert_eq!(s.routes.len(), 2, "reader stays open after cancel");
    assert_eq!(s.focus, Focus::Reader);
    // Second Esc goes back to the list.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn stale_message_result_after_close_is_dropped() {
    let mut s = state();
    s.selection = 1;
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    // Capture the open summary, then close the reader before the result
    // arrives.
    let summary = s.open_summary().unwrap().clone();
    reduce(&mut s, &Action::BackOrCancel);
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.open_message, Loadable::Idle));
    let message = mock::mock_message(&summary);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    );
    assert!(
        matches!(s.open_message, Loadable::Idle),
        "a result for a closed reader must never mutate state"
    );
    assert!(s.overlay.is_none());
}

#[test]
fn message_load_failure_opens_modal_and_retry_replays() {
    let mut s = state();
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &failure(id, &kind, "no such message"));
    // Coherent state: reader open, failed placeholder, modal up.
    assert!(matches!(s.open_message, Loadable::Failed(_)));
    assert!(s.overlay.is_some());
    assert_eq!(s.routes.len(), 2);
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, retry_kind) = effect_parts(&effects);
    assert_ne!(retry_id, id);
    assert_eq!(retry_kind, kind, "same typed intent");
    assert!(s.overlay.is_none());
    assert!(matches!(s.open_message, Loadable::Loading));
}

#[test]
fn toggle_star_from_list_requests_inverse_and_applies_on_confirmation() {
    let mut s = state();
    s.selection = 0; // m1: not starred.
    assert!(!s.messages.items[0].is_starred);
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::ToggleStar));
    assert!(
        matches!(&kind, OperationKind::SetStarred { starred: true, .. }),
        "kind: {kind:?}"
    );
    // Not applied before confirmation.
    assert!(!s.messages.items[0].is_starred);
    complete_done(&mut s, id);
    assert!(s.messages.items[0].is_starred);

    // Toggling a starred message requests the inverse.
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::ToggleStar));
    assert!(
        matches!(&kind, OperationKind::SetStarred { starred: false, .. }),
        "kind: {kind:?}"
    );
    complete_done(&mut s, id);
    assert!(!s.messages.items[0].is_starred);
}

#[test]
fn star_from_reader_targets_the_open_message() {
    let mut s = state();
    s.selection = 2; // m3: not starred.
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::ToggleStar));
    assert!(matches!(
        &kind,
        OperationKind::SetStarred { starred: true, .. }
    ));
    complete_done(&mut s, id);
    assert!(
        s.open_summary().unwrap().is_starred,
        "route snapshot updated"
    );
    assert!(s.messages.items[2].is_starred, "list row updated");
}

#[test]
fn mark_unread_updates_list_and_route_after_confirmation() {
    let mut s = state();
    s.selection = 3; // m4: read.
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::MarkUnread));
    assert!(matches!(&kind, OperationKind::SetRead { read: false, .. }));
    assert!(
        s.messages.items[3].is_read,
        "not applied before confirmation"
    );
    complete_done(&mut s, id);
    assert!(!s.messages.items[3].is_read);
    assert!(!s.open_summary().unwrap().is_read);
}

#[test]
fn message_actions_do_not_fire_from_sidebar_focus() {
    let mut s = state();
    s.focus = Focus::Sidebar;
    no_effects(&reduce(&mut s, &Action::ToggleStar));
    no_effects(&reduce(&mut s, &Action::Archive));
    no_effects(&reduce(&mut s, &Action::Trash));
    no_effects(&reduce(&mut s, &Action::MarkUnread));
    assert!(s.operations.is_empty());
}

#[test]
fn archive_from_list_removes_row_and_resyncs_page() {
    let mut s = state();
    s.selection = 0;
    let target = s.selected_message().unwrap().id.clone();
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::Archive));
    assert!(matches!(&kind, OperationKind::Archive(_)), "kind: {kind:?}");
    // The move confirmation itself emits the page re-sync effect.
    let (_, req) = expect_page(&complete_done(&mut s, id));
    // Confirmation removed the row, kept the selection index on what took
    // its place, and re-synced the page at the same offset.
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE - 1);
    assert!(s.messages.items.iter().all(|m| m.id != target));
    assert_eq!(s.selection, 0);
    assert_eq!(req.offset, 0, "re-sync reloads the current page");
}

#[test]
fn trash_closes_reader_and_removes_row() {
    let mut s = state();
    s.selection = 4;
    let target = s.selected_message().unwrap().id.clone();
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::Trash));
    assert!(matches!(&kind, OperationKind::Trash(_)), "kind: {kind:?}");
    let (_, req) = expect_page(&complete_done(&mut s, id));
    // The reader closed; the row vanished; the page re-syncs.
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(matches!(s.open_message, Loadable::Idle));
    assert!(s.messages.items.iter().all(|m| m.id != target));
    assert_eq!(req.offset, 0);
}

#[test]
fn archive_failure_keeps_row_and_opens_modal() {
    let mut s = state();
    s.selection = 1;
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::Archive));
    reduce(&mut s, &failure(id, &kind, "imap server refused"));
    // Coherent failure state: nothing removed, no reload, modal up.
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert!(s.overlay.is_some());
    assert!(s.operations.is_empty());
}

#[test]
fn reader_scrolls_within_content_and_clamps() {
    let mut s = state();
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    // The scroll viewport is the rows under the fixed header (ticket 6864);
    // the clamp tracks the body alone.
    let width = s.size.0 as usize;
    let viewport = crate::ui::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::ui::screens::reader::header_line_count(&s, width))
        .max(1);
    let total = crate::ui::screens::reader::scroll_line_count(&s, width);
    assert!(
        total > viewport,
        "mock body must overflow the viewport: total {total}, viewport {viewport}"
    );
    let max = total - viewport;
    // Movement in reader focus scrolls the document.
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.reader_scroll, 1);
    for _ in 0..(max + 10) {
        reduce(&mut s, &Action::MoveDown);
    }
    assert_eq!(s.reader_scroll, max, "scroll clamps at the end");
    for _ in 0..(max + 10) {
        reduce(&mut s, &Action::MoveUp);
    }
    assert_eq!(s.reader_scroll, 0, "scroll clamps at the start");
    // Left/Right page through the reader document in viewport steps,
    // clamped like single-line movement.
    reduce(&mut s, &Action::PageNext);
    assert_eq!(s.reader_scroll, viewport.min(max));
    reduce(&mut s, &Action::PagePrevious);
    assert_eq!(s.reader_scroll, 0);
}

#[test]
fn focus_cycles_tab_shift_tab() {
    let mut s = state();
    assert_eq!(s.focus, Focus::MessageList);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.focus, Focus::SearchField);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.focus, Focus::SelectAllToggle);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.focus, Focus::Sidebar);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.focus, Focus::MessageList);
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(s.focus, Focus::Sidebar);
}

#[test]
fn modal_focus_is_outside_the_tab_cycle() {
    // The modal focus is transitory and never cycles into screen focuses.
    assert_eq!(Focus::ErrorModal.next(), Focus::ErrorModal);
    assert_eq!(Focus::ErrorModal.previous(), Focus::ErrorModal);
    assert!(!Focus::ErrorModal.accepts_shortcuts());
}

#[test]
fn open_search_focuses_field_and_typing_edits_query() {
    let mut s = state();
    reduce(&mut s, &Action::OpenSearch);
    assert_eq!(s.focus, Focus::SearchField);
    for c in "hello".chars() {
        reduce(&mut s, &Action::SearchEdit(SearchEdit::Char(c)));
    }
    assert_eq!(s.search_query, "hello");
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Backspace));
    assert_eq!(s.search_query, "hell");
}

#[test]
fn search_edit_ignored_when_field_not_focused() {
    let mut s = state();
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Char('x')));
    assert_eq!(s.search_query, "");
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Backspace));
    assert_eq!(s.search_query, "");
}

#[test]
fn esc_leaves_search_field_before_quitting() {
    let mut s = state();
    reduce(&mut s, &Action::OpenSearch);
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(!s.quit_requested);
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.quit_requested);
}

#[test]
fn esc_on_root_quits() {
    let mut s = state();
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.quit_requested);
}

#[test]
fn quit_action_requests_quit() {
    let mut s = state();
    reduce(&mut s, &Action::Quit);
    assert!(s.quit_requested);
}

#[test]
fn move_keys_do_not_cross_focus_boundaries() {
    let mut s = state();
    // Moving in the sidebar must not move the message selection.
    s.focus = Focus::Sidebar;
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.mailbox_selection, 1);
    assert_eq!(s.selection, 0);
    // Typing chars in the search field must not move anything.
    s.focus = Focus::SearchField;
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Char('c')));
    assert_eq!(s.mailbox_selection, 1);
}

#[test]
fn sidebar_movement_does_not_touch_messages_until_activated() {
    let mut s = state();
    s.focus = Focus::Sidebar;
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "inbox");
    reduce(&mut s, &Action::Activate);
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "sent");
}

#[test]
fn selection_scrolls_to_stay_visible_on_movement() {
    let mut s = state();
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 20,
        },
    );
    // 20 rows − 4 topbar − 3 statusbar − 2 list head = 11 visible rows.
    for _ in 0..14 {
        reduce(&mut s, &Action::MoveDown);
    }
    assert_eq!(s.selection, 14);
    assert_eq!(s.list_scroll, 4);
    // Moving up does not move the window until the selection hits its top.
    reduce(&mut s, &Action::MoveUp);
    assert_eq!(s.list_scroll, 4);
    for _ in 0..10 {
        reduce(&mut s, &Action::MoveUp);
    }
    assert_eq!(s.selection, 3);
    assert_eq!(s.list_scroll, 3);
}

#[test]
fn resize_keeps_selection_visible() {
    let mut s = state();
    s.selection = 20;
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 40,
        },
    );
    assert_eq!(s.list_scroll, 0);
    // Shrink below the selection: the window follows it.
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 20,
        },
    );
    assert_eq!(s.list_scroll, 10);
}

#[test]
fn scroll_is_clamped_when_the_page_shrinks() {
    let mut s = state();
    s.list_scroll = 10;
    s.selection = 12;
    s.messages.items.truncate(3);
    s.messages.total = Some(3);
    // Any reducer interaction re-normalizes visibility.
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 40,
        },
    );
    assert_eq!(s.list_scroll, 2);
}

#[test]
fn resize_updates_size() {
    let mut s = state();
    reduce(
        &mut s,
        &Action::Resize {
            width: 90,
            height: 25,
        },
    );
    assert_eq!(s.size, (90, 25));
}

#[test]
fn resize_clamps_reader_scroll_after_reflow() {
    let mut s = state();
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    // Scroll deep into a narrow (tall) document: many body lines wrap out.
    let narrow: usize = 100;
    reduce(
        &mut s,
        &Action::Resize {
            width: narrow as u16,
            height: 20,
        },
    );
    let deep = crate::ui::screens::reader::scroll_line_count(&s, narrow);
    s.reader_scroll = deep;
    // Shrinking the width re-wraps and changes the document length; the
    // anchor must respect the re-flowed budget.
    reduce(
        &mut s,
        &Action::Resize {
            width: 90,
            height: 20,
        },
    );
    let width = crate::ui::layout::reader_width(s.size).max(10);
    let viewport = crate::ui::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::ui::screens::reader::header_line_count(&s, width))
        .max(1) as i64;
    let total = crate::ui::screens::reader::scroll_line_count(&s, width) as i64;
    let max = (total - viewport).max(0) as usize;
    assert!(
        s.reader_scroll <= max,
        "scroll {} must clamp to {max}",
        s.reader_scroll
    );
    // Growing the window never resurrects an out-of-range anchor either.
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 40,
        },
    );
    let width = crate::ui::layout::reader_width(s.size).max(10);
    let viewport = crate::ui::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::ui::screens::reader::header_line_count(&s, width))
        .max(1) as i64;
    let total = crate::ui::screens::reader::scroll_line_count(&s, width) as i64;
    let max = (total - viewport).max(0) as usize;
    assert!(s.reader_scroll <= max);
}

#[test]
fn resize_leaves_reader_scroll_alone_outside_the_reader() {
    let mut s = state();
    s.reader_scroll = 7;
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 40,
        },
    );
    assert_eq!(s.reader_scroll, 7, "no reader route: the anchor is inert");
}

#[test]
fn tick_increments_counter_only() {
    let mut s = state();
    let before = s.clone();
    tick(&mut s, 0);
    assert_eq!(s.ticks, before.ticks + 1);
    assert_eq!(s.selection, before.selection);
    assert_eq!(s.focus, before.focus);
}

#[test]
fn unimplemented_actions_are_safe_noops() {
    let mut s = state();
    let before = s.clone();
    for action in [
        Action::Reply,
        Action::ReplyAll,
        Action::Forward,
        Action::Send,
        Action::LeaveComposer,
        Action::RetryError,
        Action::DismissError,
        Action::SubmitSearch,
    ] {
        no_effects(&reduce(&mut s, &action));
    }
    assert_eq!(s.selection, before.selection);
    assert_eq!(s.routes, before.routes);
    assert_eq!(s.messages, before.messages);
    assert_eq!(s.focus, before.focus);
    // Message actions need an operation target under list/reader focus and
    // stay no-ops when the list is empty.
    s.messages.items.clear();
    for action in [
        Action::Archive,
        Action::Trash,
        Action::ToggleStar,
        Action::MarkUnread,
    ] {
        no_effects(&reduce(&mut s, &action));
    }
    assert!(s.operations.is_empty());
}

#[test]
fn selection_is_normalized_to_valid_index() {
    // An out-of-range selection (defensive: may only arise from future
    // stale-result bugs) must not panic on movement.
    let mut s = state();
    s.selection = 999;
    reduce(&mut s, &Action::MoveDown);
    let len = s.messages.items.len();
    assert_eq!(s.selection, len - 1);
    assert!(s.list_scroll < len);
    reduce(&mut s, &Action::MoveUp);
    assert_eq!(s.selection, len - 2);
}

#[test]
fn mailbox_switch_updates_route_only_via_activate() {
    let mut s = state();
    let before_routes = s.routes.clone();
    // MoveDown while sidebar focused does not switch the active mailbox…
    s.focus = Focus::Sidebar;
    reduce(&mut s, &Action::MoveDown);
    assert_eq!(s.routes, before_routes);
    // …Activate does, keeping the route stack a single root entry.
    reduce(&mut s, &Action::Activate);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "sent");
}

#[test]
fn reducer_is_free_of_io_by_construction() {
    // Structural guard: reduce takes &mut state and returns plain effects;
    // it cannot spawn, read files, or touch the network itself. Backend
    // work is only described as effects.
    let mut s = state();
    let effects = reduce(&mut s, &Action::Refresh);
    assert!(matches!(
        effects.as_slice(),
        [Effect {
            kind: OperationKind::LoadPage(_),
            ..
        }]
    ));
    assert!(s.mailboxes.as_loaded().is_some());
}

// ── Composer (plan §19 Phase 6.1) ────────────────────────────────────────

use crate::app::action::ComposerEdit;
use crate::app::composer::ComposerField;

/// Open the composer and return the state (asserts the route/focus).
fn compose(s: &mut AppState) -> &mut crate::app::composer::ComposerState {
    no_effects(&reduce(s, &Action::Compose));
    assert_eq!(s.routes.len(), 2);
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.focus, Focus::Composer);
    s.composer.as_mut().expect("composer open")
}

#[test]
fn compose_opens_the_composer_over_the_mailbox_route() {
    let mut s = state();
    compose(&mut s);
    // The list underneath is untouched (restoration by construction).
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.selection, 0);
}

#[test]
fn composer_focus_cycles_fields_and_actions() {
    let mut s = state();
    compose(&mut s);
    assert_eq!(s.composer.as_ref().unwrap().field, ComposerField::To);
    for expected in [
        ComposerField::CcToggle,
        ComposerField::BccToggle,
        ComposerField::Subject,
        ComposerField::Body,
        ComposerField::Attach,
        ComposerField::Send,
        ComposerField::Discard,
        ComposerField::To,
    ] {
        reduce(&mut s, &Action::FocusNext);
        assert_eq!(s.composer.as_ref().unwrap().field, expected);
    }
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(s.composer.as_ref().unwrap().field, ComposerField::Discard);
}

#[test]
fn enter_on_cc_toggle_reveals_and_focuses_the_cc_field() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::FocusNext); // CcToggle
    reduce(&mut s, &Action::Activate);
    let composer = s.composer.as_ref().unwrap();
    assert!(composer.show_cc);
    assert_eq!(composer.field, ComposerField::Cc);
    // The cycle now contains Cc, not the toggle.
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.composer.as_ref().unwrap().field, ComposerField::BccToggle);
}

#[test]
fn typing_edits_the_focused_field_only() {
    let mut s = state();
    compose(&mut s);
    for c in "max@".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    // Tab through to Subject and type there.
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext); // Subject
    for c in "Hi".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "max@");
    assert_eq!(composer.draft.subject, "Hi");
    assert!(composer.draft.cc.is_empty());
}

#[test]
fn enter_inserts_newline_in_body_only() {
    let mut s = state();
    compose(&mut s);
    // Walk to the body: 3 Tabs (CcToggle, BccToggle, Subject) + 1 more.
    for _ in 0..4 {
        reduce(&mut s, &Action::FocusNext);
    }
    assert_eq!(s.composer.as_ref().unwrap().field, ComposerField::Body);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('a')));
    reduce(&mut s, &Action::Activate); // Enter
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('b')));
    assert_eq!(
        s.composer.as_ref().unwrap().body.lines(),
        ["a".to_string(), "b".to_string()]
    );
    // Enter on a single-line field does not edit it.
    reduce(&mut s, &Action::FocusPrevious); // Subject
    reduce(&mut s, &Action::Activate);
    assert_eq!(s.composer.as_ref().unwrap().draft.subject, "");
}

#[test]
fn shortcuts_cannot_fire_while_composing() {
    let mut s = state();
    compose(&mut s);
    // 'c' is compose text, not a new compose; 'e' is text, not archive;
    // '/' is text, not search. (Keyboard mapping tested in input tests;
    // here the reducer-level gating is exercised via focus.)
    let before_routes = s.routes.clone();
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('c')));
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('/')));
    reduce(&mut s, &Action::Archive);
    reduce(&mut s, &Action::OpenSearch);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "c/");
    assert_eq!(s.routes, before_routes, "composer stays open");
    assert!(s.operations.is_empty());
    assert_eq!(s.focus, Focus::Composer);
}

#[test]
fn esc_leaves_the_composer_and_preserves_the_draft() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('d')));
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    // The draft data survives for reopening.
    let composer = s.composer.as_ref().expect("draft preserved");
    assert_eq!(composer.draft.to, "d");
}

#[test]
fn compose_again_reopens_the_preserved_draft() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('d')));
    reduce(&mut s, &Action::BackOrCancel);
    compose(&mut s);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "d", "reopening continues the draft");
    assert_eq!(composer.field, ComposerField::To);
}

#[test]
fn composing_without_a_list_underneath_is_safe() {
    // Startup with no mailbox route yet: compose still opens cleanly and
    // Esc returns to the empty root.
    let mut s = AppState::initial(mock::PAGE_SIZE);
    no_effects(&reduce(&mut s, &Action::Compose));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.routes.is_empty());
    assert!(!s.quit_requested);
}

#[test]
fn message_actions_do_not_fire_from_composer_focus() {
    let mut s = state();
    compose(&mut s);
    no_effects(&reduce(&mut s, &Action::ToggleStar));
    no_effects(&reduce(&mut s, &Action::Archive));
    no_effects(&reduce(&mut s, &Action::Trash));
    no_effects(&reduce(&mut s, &Action::MarkUnread));
    assert!(s.operations.is_empty());
}

#[test]
fn composer_edits_without_composer_open_are_inert() {
    let mut s = state();
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    assert!(s.composer.is_none());
    assert!(!matches!(s.active_route(), Some(Route::Composer)));
}

// ── Attachment path dialog (plan §15, Phase 8.1) ─────────────────────────

use crate::app::action::DialogEdit;
use crate::app::overlay::AttachmentPathDialog;
use crate::domain::DraftAttachment;
use std::path::PathBuf;

/// Enter on the `+ attach` control and return the open dialog.
fn open_attach_dialog(s: &mut AppState) -> &mut AttachmentPathDialog {
    compose(s);
    while s.composer.as_ref().unwrap().field != ComposerField::Attach {
        reduce(s, &Action::FocusNext);
    }
    no_effects(&reduce(s, &Action::Activate));
    assert_eq!(s.focus, Focus::Dialog);
    match s.overlay.as_mut() {
        Some(Overlay::AttachmentPath(dialog)) => dialog,
        other => panic!("expected the attachment dialog, got {other:?}"),
    }
}

/// Complete the in-flight `ReadAttachment` for `raw` with `attachment`.
fn complete_read_ok(s: &mut AppState, id: OperationId, attachment: DraftAttachment) {
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
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

#[test]
fn enter_on_attach_opens_the_dialog_and_esc_closes_it() {
    let mut s = state();
    open_attach_dialog(&mut s);
    // BackOrCancel restores the composer focus.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn dialog_entry_edits_with_caret_and_clears_errors() {
    let mut s = state();
    let dialog = open_attach_dialog(&mut s);
    dialog.error = Some(String::from("stale failure"));
    for c in "~/a.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    {
        let dialog = match s.overlay.as_ref().unwrap() {
            Overlay::AttachmentPath(dialog) => dialog,
            _ => panic!("dialog open"),
        };
        assert_eq!(dialog.input, "~/a.pdf");
        assert_eq!(dialog.cursor, 7);
        assert_eq!(dialog.error, None, "an edit clears the stale failure");
    }
    reduce(&mut s, &Action::DialogEdit(DialogEdit::Backspace));
    reduce(&mut s, &Action::DialogEdit(DialogEdit::CursorLeft));
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentPath(dialog) => dialog,
        _ => panic!("dialog open"),
    };
    assert_eq!(dialog.input, "~/a.pd");
    assert_eq!(dialog.cursor, 5);
}

#[test]
fn empty_entry_is_rejected_inline_without_an_operation() {
    let mut s = state();
    open_attach_dialog(&mut s);
    reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(' ')));
    no_effects(&reduce(&mut s, &Action::Activate));
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentPath(dialog) => dialog,
        _ => panic!("dialog open"),
    };
    assert_eq!(dialog.error.as_deref(), Some("Enter a file path"));
    assert!(s.operations.is_empty(), "nothing to validate");
}

#[test]
fn submitting_starts_validation_and_keeps_the_dialog_open() {
    let mut s = state();
    open_attach_dialog(&mut s);
    for c in "~/report final.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    assert_eq!(
        kind,
        OperationKind::ReadAttachment {
            path: PathBuf::from("~/report final.pdf"),
        },
        "the raw entry goes to the backend unexpanded"
    );
    assert!(
        matches!(s.overlay.as_ref(), Some(Overlay::AttachmentPath(_))),
        "the dialog stays open while validating"
    );
    assert_eq!(kind.summary(), "Checking file");
    let _ = id;
}

#[test]
fn validated_file_becomes_a_chip_and_dirties_the_draft() {
    let mut s = state();
    open_attach_dialog(&mut s);
    for c in "~/report.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(&mut s, id, attachment("report.pdf"));
    // The dialog closed and the chip is focused.
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.field, ComposerField::Attachment(0));
    let att = &composer.draft.attachments[0];
    assert_eq!(att.name, "report.pdf");
    assert_eq!(att.path, PathBuf::from("/tmp/report.pdf"));
    assert!(composer.draft.is_dirty(), "attaching is a content edit");
    assert_eq!(
        s.status.message.as_deref(),
        Some("Attached report.pdf (1 KB)")
    );
}

#[test]
fn validation_failure_stays_in_the_dialog_retryable() {
    let mut s = state();
    open_attach_dialog(&mut s);
    for c in "~/gone.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: None,
                detail: String::from("`/home/u/gone.pdf` does not exist"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // The dialog stays open with the detail; the entry is unchanged.
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentPath(dialog) => dialog,
        _ => panic!("dialog open"),
    };
    assert_eq!(
        dialog.error.as_deref(),
        Some("`/home/u/gone.pdf` does not exist")
    );
    assert_eq!(dialog.input, "~/gone.pdf");
    assert_eq!(s.focus, Focus::Dialog);
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn stale_validation_results_are_dropped() {
    let mut s = state();
    // Esc while validating: the result must not attach anything.
    open_attach_dialog(&mut s);
    for c in "~/a.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::BackOrCancel);
    complete_read_ok(&mut s, id, attachment("a.pdf"));
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());

    // Editing the entry while validating: the older result is stale.
    open_attach_dialog(&mut s);
    for c in "~/b.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::DialogEdit(DialogEdit::Backspace));
    complete_read_ok(&mut s, id, attachment("b.pdf"));
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn same_file_attaches_once() {
    let mut s = state();
    open_attach_dialog(&mut s);
    for c in "~/a.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(&mut s, id, attachment("a.pdf"));
    assert_eq!(s.composer.as_ref().unwrap().draft.attachments.len(), 1);

    // Re-adding the same path: no duplicate chip, no dirt.
    let revision = s.composer.as_ref().unwrap().draft.revision;
    open_attach_dialog(&mut s);
    for c in "~/a.pdf".chars() {
        reduce(&mut s, &Action::DialogEdit(DialogEdit::Char(c)));
    }
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(
        &mut s,
        id,
        DraftAttachment {
            path: PathBuf::from("/tmp/a.pdf"),
            name: String::from("a.pdf"),
            size: 1234,
        },
    );
    assert_eq!(s.composer.as_ref().unwrap().draft.attachments.len(), 1);
    assert_eq!(
        s.composer.as_ref().unwrap().draft.revision,
        revision,
        "no duplicate, no content edit"
    );
    assert_eq!(
        s.status.message.as_deref(),
        Some("a.pdf is already attached")
    );
}

#[test]
fn enter_on_a_chip_removes_it_and_autosave_follows() {
    let mut s = state();
    {
        let composer = compose(&mut s);
        composer.add_attachment(attachment("a.pdf"));
        composer.add_attachment(attachment("b.pdf"));
    }
    // Tab to the first chip: To → … → Body → Attachment(0).
    while s.composer.as_ref().unwrap().field != ComposerField::Attachment(0) {
        reduce(&mut s, &Action::FocusNext);
    }
    no_effects(&reduce(&mut s, &Action::Activate));
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.attachments.len(), 1);
    assert_eq!(composer.draft.attachments[0].name, "b.pdf");
    assert_eq!(composer.field, ComposerField::Attachment(0), "next chip");
    assert!(composer.draft.is_dirty());
    // Autosave journals the shorter attachment list: the first tick arms
    // the debounce (no clock at edit time), the second one saves.
    let _ = tick(&mut s, 0);
    let (_, snap) = expect_save(&tick(&mut s, 3));
    assert_eq!(snap.attachments.len(), 1);
    assert_eq!(snap.attachments[0].name, "b.pdf");
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
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::DraftSaved {
                remote_id: MessageId(String::from(remote)),
            }),
        }),
    )
}

#[test]
fn edits_arm_the_debounce_and_tick_starts_the_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0); // sets the clock
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    // Before the two-second window elapses nothing is requested.
    no_effects(&tick(&mut s, 1));
    // At 2 s the save fires with the current revision and content.
    let effects = tick(&mut s, 2);
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.to, "x");
    assert!(s.operations.get(id).is_some());
    assert_eq!(
        s.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
    // Confirmation of the newest revision cleans the draft.
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    let draft = &s.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty());
    assert_eq!(draft.save, crate::domain::DraftSaveState::Saved);
    assert_eq!(draft.saved_revision, 1);
    assert_eq!(
        draft.remote_id,
        Some(MessageId(String::from("remote-1"))),
        "the confirmed remote id is remembered for replacement"
    );
}

#[test]
fn saving_revision_n_cannot_mark_revision_n_plus_1_clean() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, snap1) = expect_save(&tick(&mut s, 2));
    // An edit lands while revision 1 is in flight.
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('b')));
    assert_eq!(s.composer.as_ref().unwrap().draft.revision, 2);
    // The stale success confirms only revision 1...
    let chained = complete_save_ok(&mut s, id1, snap1.revision, "remote-1");
    assert_eq!(
        s.composer.as_ref().unwrap().draft.saved_revision,
        1,
        "saved_revision must not jump to the newest revision"
    );
    // ...and the state machine immediately chains another save.
    let (id2, snap2) = expect_save(&chained);
    assert_eq!(
        snap2.revision, 2,
        "the chained save covers the newest revision"
    );
    assert_eq!(snap2.to, "ab");
    complete_save_ok(&mut s, id2, snap2.revision, "remote-2");
    let draft = &s.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty(), "now revision 2 is clean");
    assert_eq!(draft.remote_id, Some(MessageId(String::from("remote-2"))));
}

#[test]
fn rapid_edits_coalesce_into_one_pending_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    for c in "abc".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
        tick(&mut s, 0); // within the debounce window
    }
    no_effects(&tick(&mut s, 1));
    let (_, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.revision, 3, "all edits in the window coalesce");
    assert_eq!(snapshot.to, "abc");
}

#[test]
fn draft_save_failure_opens_retry_modal_and_retains_content() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    reduce(
        &mut s,
        &failure(
            id,
            &OperationKind::SaveDraft {
                draft: Box::new(snapshot.clone()),
            },
            "imap down",
        ),
    );
    // Unsaved state, content retained, modal up (plan §14 acceptance).
    let draft = &s.composer.as_ref().unwrap().draft;
    assert_eq!(draft.save, crate::domain::DraftSaveState::Failed);
    assert!(draft.is_dirty());
    assert_eq!(draft.to, "k");
    assert!(s.overlay.is_some());
    assert_eq!(s.status.message.as_deref(), Some("Draft save failed"));
    // Retry replays the *intent*: fresh snapshot of the newest revision
    // under a new operation id.
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, retry_snapshot) = expect_save(&effects);
    assert_ne!(retry_id, id);
    assert_eq!(retry_snapshot.local_id, snapshot.local_id);
    assert_eq!(retry_snapshot.revision, snapshot.revision);
    assert_eq!(retry_snapshot.to, "k");
    assert!(s.overlay.is_none());
    assert_eq!(
        s.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
}

#[test]
fn dismiss_after_failure_keeps_the_draft_awaiting_retry_or_edit() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    reduce(
        &mut s,
        &failure(
            id,
            &OperationKind::SaveDraft {
                draft: Box::new(snapshot),
            },
            "imap down",
        ),
    );
    reduce(&mut s, &Action::DismissError);
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    let draft = &s.composer.as_ref().unwrap().draft;
    assert_eq!(draft.to, "k", "dismiss never discards content");
    // No automatic re-save while Failed; the next edit re-arms autosave.
    no_effects(&tick(&mut s, 60));
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('!')));
    let (_, retry) = expect_save(&tick(&mut s, 62));
    assert_eq!(retry.to, "k!");
}

#[test]
fn caret_moves_do_not_dirty_the_draft() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    assert!(!s.composer.as_ref().unwrap().draft.is_dirty());
    // Cross-field caret moves are not content edits: no new revision, no
    // follow-up save.
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::CursorLeft));
    no_effects(&tick(&mut s, 20));
    assert_eq!(s.composer.as_ref().unwrap().draft.revision, 1);
}

#[test]
fn stale_failure_does_not_cancel_a_scheduled_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, snap1) = expect_save(&tick(&mut s, 2));
    // Newer edits re-arm the debounce while revision 1 is failing...
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('b')));
    reduce(
        &mut s,
        &failure(
            id1,
            &OperationKind::SaveDraft {
                draft: Box::new(snap1.clone()),
            },
            "boom",
        ),
    );
    // The failure modal intercepts ticks (plan §9); dismiss it and the
    // debounce is still armed: the scheduled save retries with the newest
    // revision without user action.
    reduce(&mut s, &Action::DismissError);
    let (_, snap2) = expect_save(&tick(&mut s, 4));
    assert_eq!(snap2.revision, 2);
    assert_eq!(snap2.to, "ab");
}

// ── Draft restore (plan §19 Phase 6.4 crash/restart acceptance) ──────────

fn restored_draft(to: &str, revision: u64, saved_revision: u64) -> crate::domain::RestoredDraft {
    crate::domain::RestoredDraft {
        draft: crate::domain::DraftSnapshot {
            local_id: crate::domain::DraftId(String::from("local-crash-1")),
            message_id: Some(String::from("<crash-1@post.local>")),
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
    let (id, kind) = effect_parts(&reduce(s, &Action::LoadDrafts));
    assert_eq!(kind, OperationKind::LoadDrafts);
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Drafts(drafts)),
        }),
    );
}

#[test]
fn startup_restores_the_last_safe_draft_from_the_journal() {
    let mut s = state();
    complete_restore(&mut s, vec![restored_draft("max@x.io", 5, 4)]);
    let composer = s.composer.as_ref().expect("restored");
    assert_eq!(composer.draft.to, "max@x.io");
    assert_eq!(composer.draft.revision, 5);
    assert_eq!(composer.draft.saved_revision, 4);
    assert!(
        composer.draft.is_dirty(),
        "the unconfirmed revision must re-push (self-heal)"
    );
    // The body editor carries the restored text (trailing newline intact).
    assert_eq!(composer.body.lines(), ["typed before the crash", ""]);
    // Composing reopens exactly this draft.
    compose(&mut s);
    assert_eq!(
        s.composer.as_ref().unwrap().draft.local_id,
        Some(crate::domain::DraftId(String::from("local-crash-1")))
    );
}

#[test]
fn restored_unconfirmed_drafts_autosave_after_startup() {
    let mut s = state();
    tick(&mut s, 0); // the clock is running
    complete_restore(&mut s, vec![restored_draft("max@x.io", 5, 4)]);
    // The debounce window elapses and the gap self-heals: a save of
    // revision 5 starts without any user edit.
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.revision, 5);
    assert_eq!(
        snapshot.local_id,
        crate::domain::DraftId(String::from("local-crash-1")),
        "ids stay stable across the restart"
    );
    complete_save_ok(&mut s, id, snapshot.revision, "remote-new");
    assert!(!s.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn restore_is_skipped_when_a_composer_draft_already_exists() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    complete_restore(&mut s, vec![restored_draft("other@x.io", 9, 9)]);
    assert_eq!(
        s.composer.as_ref().unwrap().draft.to,
        "x",
        "live editing must never be clobbered"
    );
}

#[test]
fn an_empty_journal_restores_nothing() {
    let mut s = state();
    complete_restore(&mut s, Vec::new());
    assert!(s.composer.is_none());
}

// ── Force save on leave (plan §14 Phase 6.5) ─────────────────────────────

#[test]
fn esc_mid_debounce_forces_the_save_without_waiting() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    // Esc before the debounce elapses: the save happens NOW.
    let effects = reduce(&mut s, &Action::BackOrCancel);
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.to, "x");
    assert!(s.operations.get(id).is_some());
    // Leaving returned to the list; the draft (and its in-flight save)
    // survive in state for reopening.
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    assert!(s.composer.as_ref().unwrap().draft.is_dirty());
    assert_eq!(
        s.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
}

#[test]
fn esc_with_a_clean_draft_saves_nothing() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    // Clean draft: leaving is silent.
    no_effects(&reduce(&mut s, &Action::BackOrCancel));
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn esc_while_the_current_revision_saves_does_not_duplicate_it() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    let token = s.operations.cancellation(id).unwrap();
    // Esc during the in-flight autosave: it is NOT cancelled and NOT
    // duplicated — leaving just lets it finish.
    no_effects(&reduce(&mut s, &Action::BackOrCancel));
    assert!(!token.is_cancelled(), "in-flight save must survive leaving");
    assert!(s.operations.get(id).is_some());
    assert_eq!(s.routes.len(), 1);
    // Its result still applies to the preserved draft.
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    let draft = &s.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty());
    assert_eq!(draft.save, crate::domain::DraftSaveState::Saved);
}

#[test]
fn esc_with_newer_edits_supersedes_an_in_flight_older_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, _snap1) = expect_save(&tick(&mut s, 2));
    let token1 = s.operations.cancellation(id1).unwrap();
    // Edits during the save, then Esc.
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('b')));
    let effects = reduce(&mut s, &Action::BackOrCancel);
    assert!(token1.is_cancelled(), "the older save is superseded");
    let (id2, snap2) = expect_save(&effects);
    assert_eq!(
        snap2.revision, 2,
        "the forced save carries the newest revision"
    );
    assert_eq!(snap2.to, "ab");
    complete_save_ok(&mut s, id2, snap2.revision, "remote-2");
    assert!(!s.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn edit_without_a_clock_still_autosaves_once_the_clock_arrives() {
    // Defensive edge: an edit before the first tick arms the debounce at
    // the next tick instead of stalling forever.
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    no_effects(&tick(&mut s, 0)); // arms the window
    let (_, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.to, "x");
}

// ── Confirmed discard (plan §14 Phase 6.6) ───────────────────────────────

fn open_discard_dialog(s: &mut AppState) {
    compose(s);
    tick(s, 0);
    reduce(s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    no_effects(&reduce(s, &Action::DiscardDraft));
    assert!(matches!(s.overlay, Some(Overlay::ConfirmDiscard(_))));
    assert_eq!(s.focus, Focus::ErrorModal);
}

#[test]
fn discard_requires_confirmation_and_defaults_to_keep() {
    let mut s = state();
    open_discard_dialog(&mut s);
    // The draft is untouched until confirmation.
    assert!(s.composer.is_some());
    assert_eq!(s.routes.len(), 2);
    // Enter activates the focused button: Keep (the safe default).
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.composer.is_some(), "keep preserves the draft");
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    assert_eq!(s.routes.len(), 2);
}

#[test]
fn esc_on_the_discard_dialog_keeps_the_draft() {
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none());
    assert!(s.composer.is_some());
    assert_eq!(s.focus, Focus::Composer, "focus returns to the composer");
}

#[test]
fn confirmed_discard_deletes_local_and_remote_state() {
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, &Action::FocusNext); // Discard button
    let effects = reduce(&mut s, &Action::Activate);
    let (id, kind) = effect_parts(&effects);
    let crate::app::operation::OperationKind::DeleteDraft {
        draft,
        reason: DraftRemovalReason::Discard,
    } = &kind
    else {
        panic!("expected DeleteDraft, got {kind:?}");
    };
    assert_eq!(
        draft.local_id,
        crate::domain::DraftId(String::from("local-unsaved"))
    );
    assert_eq!(draft.to, "x");
    // Local state is gone immediately; the remote sweep runs in the op.
    assert!(s.composer.is_none(), "local draft removed on confirmation");
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert!(s.operations.get(id).is_some());
    assert_eq!(s.status.message.as_deref(), Some("Draft discarded"));
    // Confirmation completes without further state change.
    complete_done(&mut s, id);
    assert!(s.composer.is_none());
    // Composing starts fresh.
    compose(&mut s);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "");
    assert_eq!(composer.draft.revision, 0);
}

#[test]
fn discard_cancels_an_in_flight_save_of_the_same_draft() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let (save_id, _) = expect_save(&tick(&mut s, 2));
    let save_token = s.operations.cancellation(save_id).unwrap();
    // Discard while that save is in flight.
    reduce(&mut s, &Action::DiscardDraft);
    reduce(&mut s, &Action::FocusNext);
    let effects = reduce(&mut s, &Action::Activate);
    assert!(
        save_token.is_cancelled(),
        "the in-flight save must not resurrect the discarded draft"
    );
    assert!(s.operations.get(save_id).is_none());
    let (delete_id, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::DeleteDraft { .. }));
    assert!(s.operations.get(delete_id).is_some());
    // The cancelled save's (suppressed) result can never apply.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: save_id,
            outcome: Ok(OperationOutcome::DraftSaved {
                remote_id: MessageId(String::from("late-remote")),
            }),
        }),
    );
    assert!(s.composer.is_none());
}

#[test]
fn discard_dialog_swallows_unrelated_input() {
    let mut s = state();
    open_discard_dialog(&mut s);
    let draft_before = s.composer.as_ref().unwrap().draft.to.clone();
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('y')));
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Char('z')));
    reduce(&mut s, &Action::Refresh);
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::Quit);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, draft_before);
    assert!(!s.quit_requested);
    assert!(s.overlay.is_some(), "dialog stays open");
}

#[test]
fn discard_of_a_never_saved_draft_still_needs_confirmation() {
    let mut s = state();
    compose(&mut s);
    no_effects(&reduce(&mut s, &Action::DiscardDraft));
    assert!(matches!(s.overlay, Some(Overlay::ConfirmDiscard(_))));
    reduce(&mut s, &Action::FocusNext);
    let effects = reduce(&mut s, &Action::Activate);
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::DeleteDraft { .. }));
    assert!(s.composer.is_none());
}

#[test]
fn discard_delete_failure_opens_the_error_modal() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::DiscardDraft);
    reduce(&mut s, &Action::FocusNext);
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    // The draft is already gone locally; a remote sweep failure surfaces.
    reduce(&mut s, &failure(id, &kind, "imap refused"));
    let Some(Overlay::Error(_)) = &s.overlay else {
        panic!("error modal must open");
    };
    assert!(
        s.composer.is_none(),
        "the discard itself is not rolled back"
    );
}

// ── Reply / forward seeding (plan §14, Phase 7.3) ────────────────────────

use crate::domain::{Address, Message, MessageHeaders, MessageSummary};

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
                email: String::from("probe@post.local"),
            }],
            cc: vec![Address {
                name: None,
                email: String::from("carol@example.org"),
            }],
            date: Some(mock::now()),
            message_id: Some(String::from("318@post.local")),
            in_reply_to: None,
            references: Some(String::from("000@post.local")),
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
    s.routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: message.mailbox_id.clone(),
            summary,
        }));
    s.focus = Focus::Reader;
    s.open_message = Loadable::Loaded(message);
}

fn seeded_composer(s: &AppState) -> &crate::app::composer::ComposerState {
    s.composer.as_ref().expect("seeded composer")
}

#[test]
fn reply_seeds_a_composer_on_top_of_the_reader() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Reply));
    // Route stack: composer above the still-open reader; Esc from the
    // composer returns to reading.
    assert_eq!(s.routes.len(), 3);
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.focus, Focus::Composer);
    assert_eq!(s.status.message.as_deref(), Some("Reply draft ready"));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "Bob <bob@example.org>");
    assert_eq!(composer.draft.subject, "Re: Plan review");
    assert_eq!(
        composer.draft.in_reply_to.as_deref(),
        Some("318@post.local")
    );
    assert_eq!(
        composer.draft.references.as_deref(),
        Some("000@post.local 318@post.local")
    );
    // Quoted body with the attribution; caret starts at the very top.
    assert!(composer.draft.body.contains("On "));
    assert!(
        composer
            .draft
            .body
            .contains("wrote:\n> Please review.\n> Thanks")
    );
    // The seeded draft is clean: autosave engages on the first edit.
    assert!(!composer.draft.is_dirty());
    assert_eq!(composer.draft.revision, 0);
}

#[test]
fn forward_seeds_a_header_block_and_no_recipients() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Forward));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "");
    assert_eq!(composer.draft.subject, "Fwd: Plan review");
    assert_eq!(composer.draft.in_reply_to, None);
    assert_eq!(composer.draft.references, None);
    assert!(
        composer
            .draft
            .body
            .contains("---------- Forwarded message ---------")
    );
    assert!(composer.draft.body.contains("From: Bob <bob@example.org>"));
    assert!(composer.draft.body.contains("To: probe@post.local"));
}

#[test]
fn reply_needs_a_loaded_message() {
    let mut s = state();
    // No reader at all.
    no_effects(&reduce(&mut s, &Action::Reply));
    no_effects(&reduce(&mut s, &Action::Forward));
    assert!(s.composer.is_none());
    // Reader open but the message still loading.
    let summary = s.selected_message().unwrap().clone();
    let (id, _kind) = expect_kind(&reduce(&mut s, &Action::Activate));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                0,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    let _ = summary;
    no_effects(&reduce(&mut s, &Action::Reply));
    assert!(s.composer.is_none());
}

#[test]
fn reply_never_clobbers_an_existing_draft() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Compose)); // a draft exists already
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    no_effects(&reduce(&mut s, &Action::Reply));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "k", "existing draft untouched");
    assert_eq!(
        s.status.message.as_deref(),
        Some("A draft is already open — send or discard it first")
    );
}

#[test]
fn leaving_a_seeded_reply_returns_to_the_reader() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    reduce(&mut s, &Action::Reply);
    reduce(&mut s, &Action::BackOrCancel); // Esc: save/leave
    assert!(matches!(s.active_route(), Some(Route::Message(_))));
    assert_eq!(s.focus, Focus::MessageList);
    // The draft stays in state and reopens with its seeded headers.
    assert!(s.composer.is_some());
    reduce(&mut s, &Action::Compose);
    let composer = seeded_composer(&s);
    assert_eq!(
        composer.draft.in_reply_to.as_deref(),
        Some("318@post.local")
    );
}

#[test]
fn reply_all_merges_recipients_dedups_and_excludes_self() {
    let mut s = state();
    s.account_email = Some(String::from("probe@post.local"));
    let mut message = reply_source();
    // Carol appears in To and Cc; the account itself was a recipient.
    message.headers.to.push(Address {
        name: Some(String::from("Carol")),
        email: String::from("carol@example.org"),
    });
    message.headers.cc.push(Address {
        name: None,
        email: String::from("CAROL@example.org"),
    });
    message.headers.cc.push(Address {
        name: None,
        email: String::from("probe@post.local"),
    });
    open_reader_with(&mut s, message);
    no_effects(&reduce(&mut s, &Action::ReplyAll));
    assert_eq!(s.status.message.as_deref(), Some("Reply-all draft ready"));
    let composer = seeded_composer(&s);
    // Sender first; the account's own address (probe@) is excluded even
    // though it was in To; Carol keeps her first (To) form, and her Cc
    // duplicate is dropped.
    assert_eq!(
        composer.draft.to,
        "Bob <bob@example.org>, Carol <carol@example.org>"
    );
    assert_eq!(composer.draft.cc, "");
    assert_eq!(
        composer.draft.in_reply_to.as_deref(),
        Some("318@post.local")
    );
}

// ── Send flow (plan §14/§19 Phase 7, Phase 7.6) ──────────────────────────

use crate::app::operation::DraftRemovalReason;
use crate::domain::{OutboundMessage, SendOutcome};

/// A composed, validly addressed state ready to send.
fn sendable(s: &mut AppState) {
    compose(s);
    let composer = s.composer.as_mut().unwrap();
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
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::SendOutcome(outcome)),
        }),
    )
}

#[test]
fn ctrl_enter_sends_only_from_the_composer() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::Send));
    assert!(s.composer.is_none());
    // A draft may exist without the composer route (left-open draft): the
    // route gate still applies.
    compose(&mut s);
    reduce(&mut s, &Action::BackOrCancel);
    no_effects(&reduce(&mut s, &Action::Send));
    assert!(s.composer.is_some(), "draft data preserved");
    assert!(s.operations.is_empty());
}

#[test]
fn send_refuses_an_empty_recipient_list() {
    let mut s = state();
    compose(&mut s);
    // A subject alone is not enough: no To/Cc/Bcc, no send.
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    for c in "hi".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(
        s.status.message.as_deref(),
        Some("Cannot send: add at least one recipient")
    );
    assert!(
        s.composer.as_ref().unwrap().draft.to.is_empty(),
        "draft untouched"
    );
}

#[test]
fn send_refuses_invalid_addresses() {
    let mut s = state();
    compose(&mut s);
    for c in "not an address".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(
        s.status.message.as_deref(),
        Some("Cannot send: fix the invalid address entries")
    );
    assert!(s.operations.is_empty(), "nothing was started");
}

#[test]
fn send_freezes_the_composer_and_starts_one_operation() {
    let mut s = state();
    sendable(&mut s);
    let effects = reduce(&mut s, &Action::Send);
    let (id, message) = expect_send(&effects);
    assert_eq!(message.to.len(), 1);
    assert_eq!(message.to[0].email, "ada@example.org");
    assert_eq!(message.content.subject, "Hello");
    assert_eq!(message.content.body, "Body");
    assert!(s.operations.get(id).is_some());
    let composer = s.composer.as_ref().unwrap();
    assert!(composer.sending);
    assert_eq!(s.status.message.as_deref(), Some("Sending…"));
    // Edits are frozen while the send runs; the draft keeps its content.
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.body, "Body");
    // A second send is refused.
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(s.operations.len(), 1);
    let _ = id;
}

#[test]
fn send_failure_keeps_the_draft_intact() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("smtp refused"),
                retry: Some(mailboxes_kind().retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // Draft intact and editable again; no route change.
    let composer = s.composer.as_ref().unwrap();
    assert!(!composer.sending);
    assert_eq!(composer.draft.to, "ada@example.org");
    assert_eq!(composer.draft.body, "Body");
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert!(s.overlay.is_some(), "Retry/Dismiss modal opens");
}

#[test]
fn send_success_leaves_the_composer_and_resolves_the_draft() {
    let mut s = state();
    sendable(&mut s);
    // The draft was saved before sending: the remote copy must be swept.
    let composer = s.composer.as_mut().unwrap();
    composer.draft.remote_id = Some(MessageId(String::from("remote-draft")));
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    // Composer closed, back to the mailbox, status confirms success.
    assert!(s.composer.is_none());
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.focus, Focus::MessageList);
    assert_eq!(s.status.message.as_deref(), Some("Message sent"));
    // One cleanup operation: journal entry + remote copy removal.
    let (cleanup_id, kind) = effect_parts(&effects);
    match &kind {
        OperationKind::DeleteDraft {
            draft,
            reason: DraftRemovalReason::Sent,
        } => {
            assert_eq!(
                draft.remote_id,
                Some(MessageId(String::from("remote-draft")))
            );
        }
        other => panic!("expected DeleteDraft(Sent), got {other:?}"),
    }
    assert!(s.operations.get(cleanup_id).is_some());
}

#[test]
fn send_success_after_leaving_still_resolves_the_draft() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    // The user left mid-send (Esc save/leaves; the draft was clean, so no
    // forced save runs).
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    assert!(s.composer.is_none(), "the sent draft must not linger");
    assert_eq!(s.routes.len(), 1, "already left: no route to pop");
    assert_eq!(effect_parts(&effects).1.summary(), "Cleaning up sent draft");
}

#[test]
fn sent_draft_cleanup_failure_never_claims_a_failed_send() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    let (cleanup_id, kind) = effect_parts(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: cleanup_id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("drafts mailbox gone"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // Delivery was confirmed: no modal, no status regression.
    assert!(s.overlay.is_none());
    assert_eq!(s.status.message.as_deref(), Some("Message sent"));
}

#[test]
fn cleanup_of_a_discarded_draft_still_opens_the_modal_on_failure() {
    // The Sent-reason quietness must not weaken the discard flow (6.6).
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, &Action::FocusNext); // Discard
    let effects = reduce(&mut s, &Action::Activate);
    let (id, kind) = effect_parts(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("sweep failed"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    assert!(s.overlay.is_some(), "discard cleanup failures stay visible");
}

// ── Ambiguous sends (plan §12, Phase 7.7) ────────────────────────────────

#[test]
fn ambiguous_send_opens_the_duplicate_warning_and_keeps_the_draft() {
    let mut s = state();
    sendable(&mut s);
    let (id, message) = expect_send(&reduce(&mut s, &Action::Send));
    // Probe-verified ambiguous outcome: payload transmitted, himalaya
    // reported a DATA-phase EOF.
    complete_send(
        &mut s,
        id,
        SendOutcome::Unknown {
            code: Some(1),
            detail: String::from("SMTP DATA failed: Reached unexpected EOF"),
        },
    );
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous, "the modal must carry the ambiguity flag");
    assert_eq!(dialog.code, Some(1));
    assert!(dialog.detail.contains("SMTP DATA failed"));
    // Retry stays available, replaying the exact frozen message.
    assert_eq!(
        dialog.retry.as_ref().map(|spec| spec.kind.clone()),
        Some(OperationKind::Send {
            message: Box::new(message),
        })
    );
    // The draft is intact and editable again; nothing claimed success.
    let composer = s.composer.as_ref().unwrap();
    assert!(!composer.sending);
    assert_eq!(composer.draft.body, "Body");
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.status.message.as_deref(), Some("Send outcome unclear"));
}

#[test]
fn ambiguous_send_never_shows_a_failure_title_or_status() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::SentButCopyFailed {
            code: None,
            detail: String::from("sent-copy append failed"),
        },
    );
    // Not success ("Message sent"), not definite failure ("Send failed").
    assert_eq!(s.status.message.as_deref(), Some("Send outcome unclear"));
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous);
}

#[test]
fn retrying_an_ambiguous_send_replays_the_frozen_message() {
    let mut s = state();
    sendable(&mut s);
    let (id, message) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::Unknown {
            code: Some(1),
            detail: String::from("connection reset"),
        },
    );
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, replay) = expect_send(&effects);
    assert_ne!(retry_id, id, "a retry gets a new operation id");
    assert_eq!(replay, message, "the exact same bytes are re-sent");
    assert!(s.overlay.is_none());
    let composer = s.composer.as_ref().unwrap();
    assert!(composer.sending, "the retry send is in flight again");
}

#[test]
fn failed_before_delivery_send_reports_definite_failure_safely() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::FailedBeforeDelivery {
            code: Some(1),
            detail: String::from("connect 127.0.0.1:3425: connection refused"),
        },
    );
    assert_eq!(s.status.message.as_deref(), Some("Send failed"));
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    // Nothing was transmitted: retrying is safe, no duplicate warning.
    assert!(!dialog.ambiguous);
    assert!(dialog.retry.is_some());
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.draft.body, "Body", "draft intact");
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

/// Focus the search field, type a query, and submit.
fn search(s: &mut AppState, query: &str) -> Vec<Effect> {
    reduce(&mut *s, &Action::OpenSearch);
    for c in query.chars() {
        reduce(s, &Action::SearchEdit(SearchEdit::Char(c)));
    }
    reduce(s, &Action::SubmitSearch)
}

#[test]
fn submit_search_stashes_mailbox_context_and_runs_search() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let (page, selection, scroll) = (s.messages.clone(), s.selection, s.list_scroll);

    let effects = search(&mut s, "quote");
    let (id, request) = expect_search(&effects);
    // The query travels unchanged (Phase 9.2), scoped to the current
    // mailbox, first page, list page size.
    assert_eq!(request.query, "quote");
    assert_eq!(request.mailbox_id, inbox_id());
    assert_eq!(request.offset, 0);
    assert_eq!(request.limit, 20);
    assert!(s.operations.get(id).is_some());
    // The search route sits above the mailbox route…
    assert_eq!(s.routes.len(), 2);
    assert_eq!(
        s.active_route(),
        Some(&Route::Search(crate::app::route::SearchRoute {
            query: String::from("quote"),
            mailbox_id: inbox_id(),
        }))
    );
    // …and the mailbox list context is stashed for an exact return.
    let stash = s.search_return.as_ref().expect("stash");
    assert_eq!(stash.page, page);
    assert_eq!(stash.selection, selection);
    assert_eq!(stash.scroll, scroll);
    // The visible list is emptied while the search runs; focus moves to it.
    assert!(s.messages.items.is_empty());
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn submit_search_with_empty_query_is_inert() {
    let mut s = state();
    let before = s.clone();
    s.focus = Focus::SearchField;
    let effects = reduce(&mut s, &Action::SubmitSearch);
    no_effects(&effects);
    assert_eq!(s.routes, before.routes);
    assert!(s.search_return.is_none());
    assert!(s.status.message.is_some(), "guidance is shown");
}

#[test]
fn submit_search_from_the_reader_is_inert() {
    let mut s = state();
    reduce(&mut s, &Action::Activate); // open reader
    let before = s.routes.clone();
    s.focus = Focus::SearchField;
    s.search_query = String::from("quote");
    no_effects(&reduce(&mut s, &Action::SubmitSearch));
    assert_eq!(s.routes, before, "no search above the reader");
    assert!(s.search_return.is_none());
}

#[test]
fn search_results_apply_to_the_shared_list() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    // Results carry the searched mailbox as their context (Phase 9.1).
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    assert_eq!(s.messages.items.len(), 20);
    assert_eq!(s.messages.items[0].mailbox_id, inbox_id());
    assert!(s.messages.items[0].subject.starts_with("quote match:"));
    assert_eq!(s.selection, 0);
}

#[test]
fn search_results_superseded_by_a_navigation_race_are_dropped() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    // The user leaves the search before the result arrives. Esc first
    // cancels the in-flight search (plan §10), then leaves the route.
    reduce(&mut s, &Action::BackOrCancel);
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    // The cancelled result must not clobber the restored mailbox context.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    assert_eq!(s.messages, mock::mock_page(&inbox_id(), 0, 20));
}

#[test]
fn reader_from_search_returns_to_the_same_results() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (search_id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: search_id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    let selected = s.selected_message().cloned().expect("a result row");
    let effects = reduce(&mut s, &Action::Activate); // open reader
    assert!(matches!(s.active_route(), Some(Route::Message(_))));
    // The fetch completes; Esc then pops the reader route itself.
    let load_id = match effects.as_slice() {
        [effect] => {
            assert!(
                matches!(effect.kind, OperationKind::LoadMessage(_)),
                "expected LoadMessage"
            );
            effect.id
        }
        other => panic!("expected one effect, got {other:?}"),
    };
    // Opening an unread message also starts a SetRead; complete it so the
    // registry is idle and the next Esc reaches the route stack.
    let read_effects = reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: load_id,
            outcome: Ok(OperationOutcome::Message(Box::new(mock::mock_message(
                &selected,
            )))),
        }),
    );
    match read_effects.as_slice() {
        [effect] => {
            assert!(
                matches!(effect.kind, OperationKind::SetRead { read: true, .. }),
                "expected the mark-read follow-up"
            );
            reduce(
                &mut s,
                &Action::BackendCompleted(OperationResult {
                    id: effect.id,
                    outcome: Ok(OperationOutcome::Done),
                }),
            );
        }
        other => no_effects(other),
    }
    // Esc returns to the search, whose results were never disturbed.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Search(_))));
    assert_eq!(s.messages.items.len(), 20);
    assert_eq!(
        s.selected_message().map(|m| m.id.clone()),
        Some(selected.id)
    );
}

#[test]
fn leaving_search_restores_the_mailbox_context_exactly() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let before = (s.messages.clone(), s.selection, s.list_scroll);
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    // Esc from the search route restores page, selection, and scroll.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1);
    assert!(s.search_return.is_none());
    assert_eq!(s.messages, before.0, "mailbox page restored");
    assert_eq!(s.selection, before.1);
    assert_eq!(s.list_scroll, before.2);
    assert_eq!(s.focus, Focus::MessageList);
    // The search field clears with the results (ticket 32b3): `/` opens
    // an empty field for the next search.
    assert_eq!(s.search_query, "");
}

#[test]
fn resubmitting_edits_the_query_without_disturbing_the_stash() {
    let mut s = state();
    let effects = search(&mut s, "quo");
    let (first, _) = expect_search(&effects);
    let stash = s.search_return.clone().expect("stash");
    // Edit and re-submit while the search route is open.
    let effects = search(&mut s, "te");
    let (second, request) = expect_search(&effects);
    assert_ne!(first, second, "a new operation");
    assert_eq!(request.query, "quote");
    assert_eq!(s.routes.len(), 2, "no second search route");
    assert_eq!(s.search_return.as_ref(), Some(&stash), "stash untouched");
}

#[test]
fn search_pagination_issues_search_requests() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    // Right: next results page as a Search request.
    let effects = reduce(&mut s, &Action::PageNext);
    let (_, request) = expect_search(&effects);
    assert_eq!(request.offset, 20);
    assert_eq!(request.query, "quote");
}

#[test]
fn search_operation_supersedes_the_previous_one() {
    let mut s = state();
    let effects = search(&mut s, "quo");
    let (first, _) = expect_search(&effects);
    let effects = search(&mut s, "te");
    let (second, _) = expect_search(&effects);
    assert!(s.operations.get(first).is_none(), "older search cancelled");
    assert!(s.operations.get(second).is_some());
}

#[test]
fn switching_mailbox_from_search_rebuilds_the_route_stack() {
    let mut s = state();
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(&inbox_id(), 0, 20))),
        }),
    );
    // Pick another mailbox in the sidebar and activate it.
    s.focus = Focus::Sidebar;
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::Activate);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(
        s.active_route().unwrap().mailbox_id().unwrap().0,
        "sent",
        "the new mailbox replaces search and mailbox alike"
    );
    assert!(s.search_return.is_none(), "stash dropped with the search");
}

// ── Phase 9: periodic refresh + suppression (plan §11/§19) ──────────────

/// Enable the timer at 60s. The state fixture carries no clock yet.
fn timer(s: &mut AppState) {
    s.refresh_interval_seconds = 60;
}

#[test]
fn auto_refresh_arms_on_first_tick_and_fires_after_the_interval() {
    let mut s = state();
    timer(&mut s);
    // First tick arms the timer; nothing fires.
    no_effects(&tick(&mut s, 0));
    // Halfway: still nothing.
    no_effects(&tick(&mut s, 30));
    // The interval has elapsed on the injected clock: a *background* page
    // refresh for the visible mailbox (Phase 9.4).
    let effects = tick(&mut s, 60);
    let (id, req) = expect_page(&effects);
    assert_eq!(req.mailbox_id, inbox_id());
    assert_eq!(req.offset, 0);
    assert_eq!(
        s.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
    // Complete the background refresh; the arm point moved, so the next
    // fire needs another full interval.
    complete_page_ok(&mut s, id, &req, 0);
    let effects = tick(&mut s, 90);
    assert!(effects.is_empty());
    let effects = tick(&mut s, 120);
    expect_page(&effects);
}

#[test]
fn auto_refresh_is_disabled_without_an_interval() {
    let mut s = state();
    // refresh_interval_seconds defaults to 0 in state; the config opts in.
    no_effects(&tick(&mut s, 0));
    no_effects(&tick(&mut s, 3600));
}

#[test]
fn auto_refresh_stands_down_while_conflicting_work_is_in_flight() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    // A foreground operation (the startup page load) is in flight when the
    // interval elapses: the timer stands down and retries later.
    let (manual, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    no_effects(&tick(&mut s, 60));
    complete_page_ok(&mut s, manual, &req, 0);
    // Conflicts cleared; the manual refresh re-armed the timer to its own
    // clock, so the next automatic fire is one full interval later.
    no_effects(&tick(&mut s, 50));
    let effects = tick(&mut s, 60);
    let (id, _) = expect_page(&effects);
    assert_eq!(
        s.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background)
    );
}

#[test]
fn auto_refresh_stands_down_while_composing() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    reduce(&mut s, &Action::Compose);
    no_effects(&tick(&mut s, 60));
    // Leaving the composer unblocks the next tick.
    reduce(&mut s, &Action::LeaveComposer);
    let effects = tick(&mut s, 120);
    expect_page(&effects);
}

#[test]
fn auto_refresh_targets_the_open_search_context() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = search(&mut s, "quote");
    let (id, _) = expect_search(&effects);
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    for summary in &mut page.items {
        summary.subject = format!("quote match: {}", summary.subject);
    }
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
    // The timer refreshes the search at the current offset (Phase 9.5:
    // refresh only the visible context).
    let effects = tick(&mut s, 60);
    let (_, request) = expect_search(&effects);
    assert_eq!(request.query, "quote");
    assert_eq!(request.offset, 0);
}

#[test]
fn background_refresh_failure_never_opens_the_modal() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = tick(&mut s, 60);
    let (id, _) = expect_page(&effects);
    reduce(
        &mut s,
        &failure(
            id,
            &OperationKind::LoadPage(PageRequest {
                mailbox_id: inbox_id(),
                offset: 0,
                limit: 20,
            }),
            "connect refused",
        ),
    );
    // No Retry/Dismiss modal for background work; the status line carries
    // the failure and the record suppresses repeats (Phase 9.6).
    assert!(s.overlay.is_none());
    assert_eq!(s.last_background_error.as_deref(), Some("connect refused"));
    assert_eq!(
        s.status.message.as_deref(),
        Some("Refresh failed — the timer will retry")
    );
}

#[test]
fn background_failure_record_clears_on_success_and_on_manual_refresh() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = tick(&mut s, 60);
    let (id, req) = expect_page(&effects);
    reduce(
        &mut s,
        &failure(id, &OperationKind::LoadPage(req.clone()), "connect refused"),
    );
    assert!(s.last_background_error.is_some());
    // A success clears the record: a later failure is reported again.
    let effects = tick(&mut s, 120);
    let (id2, req2) = expect_page(&effects);
    complete_page_ok(&mut s, id2, &req2, 0);
    assert!(s.last_background_error.is_none());
    // A manual refresh that fails is foreground work: it opens the modal.
    reduce(&mut s, &Action::Refresh);
    // (its operation is in flight; complete it with a failure)
    let last = s
        .operations
        .foreground()
        .map(|op| op.id)
        .expect("manual refresh");
    let kind = s.operations.get(last).unwrap().kind.clone();
    reduce(&mut s, &failure(last, &kind, "also refused"));
    assert!(
        matches!(s.overlay, Some(Overlay::Error(_))),
        "foreground failure modals"
    );
}

#[test]
fn manual_refresh_failure_still_opens_the_modal() {
    let mut s = state();
    reduce(&mut s, &Action::Refresh);
    let last = s.operations.foreground().map(|op| op.id).expect("refresh");
    let kind = s.operations.get(last).unwrap().kind.clone();
    reduce(&mut s, &failure(last, &kind, "connect refused"));
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));
    // Manual refresh remains available after dismissing (acceptance).
    reduce(&mut s, &Action::DismissError);
    let effects = reduce(&mut s, &Action::Refresh);
    expect_page(&effects);
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

#[test]
fn refresh_preserves_the_logical_selection_when_new_mail_arrives() {
    let mut s = state();
    // Identity rows: ids m1..m20 with stable Message-IDs.
    let mut page = mock::mock_page(&inbox_id(), 0, 20);
    page.items = page
        .items
        .iter()
        .enumerate()
        .map(|(i, m)| identified(m, &format!("mid-{}", i + 1)))
        .collect();
    let (id, _) = expect_page(&reduce(&mut s, &Action::Refresh));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page.clone())),
        }),
    );
    // The user selects the third row (mid-3).
    s.selection = 2;
    // New mail arrives above: the refreshed page has a new row first, and
    // every known Message-ID shifts down by one. The refresh is a new
    // operation (the previous one completed).
    let mut refreshed = page.clone();
    let mut shifted = vec![identified(&page.items[0], "mid-new")];
    shifted.extend(page.items.iter().cloned());
    refreshed.items = shifted;
    let (id2, _) = expect_page(&reduce(&mut s, &Action::Refresh));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: id2,
            outcome: Ok(OperationOutcome::Page(refreshed)),
        }),
    );
    // The logical selection did not move: still mid-3, now one row lower.
    assert_eq!(s.selection, 3);
    assert_eq!(
        s.messages.items[s.selection].message_id.as_deref(),
        Some("mid-3")
    );
}

// ── Mouse clicks (plan §10, Phase 10.1/10.2) ─────────────────────────────

use crate::app::overlay::{ConfirmButton, ErrorDialog, ModalButton};

#[test]
fn click_selects_then_opens_a_message_row() {
    let mut s = state();
    // First click on a new row only selects it (the arrows' job).
    no_effects(&reduce(&mut s, &Action::Click(ClickTarget::MessageRow(2))));
    assert_eq!(s.selection, 2);
    assert_eq!(s.focus, Focus::MessageList);
    // Clicking the selected row opens it (Enter's job).
    let effects = reduce(&mut s, &Action::Click(ClickTarget::MessageRow(2)));
    let (id, kind) = effect_parts(&effects);
    let OperationKind::LoadMessage(locator) = kind else {
        panic!("expected LoadMessage, got {kind:?}");
    };
    assert_eq!(locator.id, s.messages.items[2].id);
    assert_eq!(s.focus, Focus::Reader);
    assert_eq!(s.open_message, Loadable::Loading);
    // The operation id is registered so the result can apply.
    assert!(s.operations.get(id).is_some());
}

#[test]
fn click_message_row_out_of_range_is_inert() {
    let mut s = state();
    let before = s.selection;
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::MessageRow(10_000)),
    ));
    assert_eq!(s.selection, before);
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn click_mailbox_selects_then_switches() {
    let mut s = state();
    // A different row only selects (focus follows the click).
    no_effects(&reduce(&mut s, &Action::Click(ClickTarget::Mailbox(3))));
    assert_eq!(s.mailbox_selection, 3);
    assert_eq!(s.focus, Focus::Sidebar);
    // Clicking the selected mailbox switches to it (Enter's job).
    let effects = reduce(&mut s, &Action::Click(ClickTarget::Mailbox(3)));
    let (_, req) = expect_page(&effects);
    assert_eq!(req.mailbox_id.0, "archive");
    assert_eq!(
        s.active_route().and_then(Route::mailbox_id).unwrap().0,
        "archive"
    );
}

#[test]
fn click_search_field_focuses_it_like_slash() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::Click(ClickTarget::SearchField)));
    assert_eq!(s.focus, Focus::SearchField);
}

#[test]
fn click_compose_button_opens_the_composer() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::Click(ClickTarget::ComposeButton)));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.focus, Focus::Composer);
}

#[test]
fn click_composer_text_field_focuses_and_places_the_caret() {
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    if let Some(composer) = s.composer.as_mut() {
        composer.draft.subject = String::from("Quarterly report");
        composer.draft.to = String::from("");
    }
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ComposerField(ComposerField::Subject)),
    ));
    let composer = s.composer.as_ref().expect("composer");
    assert_eq!(composer.field, ComposerField::Subject);
    assert_eq!(composer.cursor, "Quarterly report".chars().count());
    assert_eq!(s.focus, Focus::Composer);
}

#[test]
fn click_composer_toggles_reveal_their_field() {
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ComposerField(ComposerField::BccToggle)),
    ));
    let composer = s.composer.as_ref().expect("composer");
    assert!(composer.show_bcc);
    assert_eq!(composer.field, ComposerField::Bcc);
    // The Bcc toggle is gone from the cycle; clicking it again is inert.
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ComposerField(ComposerField::BccToggle)),
    ));
    let composer = s.composer.as_ref().expect("composer");
    assert_eq!(composer.field, ComposerField::Bcc);
}

#[test]
fn click_discard_button_opens_the_confirm_dialog() {
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ComposerField(ComposerField::Discard)),
    ));
    assert!(matches!(s.overlay, Some(Overlay::ConfirmDiscard(_))));
}

#[test]
fn click_error_modal_buttons_replay_or_close() {
    // Build a retryable failure like the tests above do.
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(&mut s, &failure(id, &page_kind(&req), "himalaya exploded"));
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));

    // Clicking Retry replays the intent under a new operation id (plan §12).
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::ErrorButton(ModalButton::Retry)),
    );
    let (replayed, kind) = effect_parts(&effects);
    assert_eq!(kind, page_kind(&req));
    assert_ne!(replayed, id);
    assert!(s.overlay.is_none());

    // A second failure (the retried page already runs; Refresh re-requests
    // page 0 under a new id), then Dismiss closes without new work.
    let (id, req) = expect_page(&reduce(&mut s, &Action::Refresh));
    reduce(&mut s, &failure(id, &page_kind(&req), "himalaya exploded"));
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ErrorButton(ModalButton::Dismiss)),
    ));
    assert!(s.overlay.is_none());
    assert!(s.operations.get(id).is_none(), "cancelled with the modal");
}

#[test]
fn click_error_modal_retry_without_intent_is_inert() {
    let mut s = state();
    s.overlay = Some(Overlay::Error(ErrorDialog {
        code: Some(1),
        detail: String::from("no retry offered"),
        retry: None,
        ambiguous: false,
        scroll: 0,
        button: ModalButton::Dismiss,
        previous_focus: Focus::MessageList,
    }));
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ErrorButton(ModalButton::Retry)),
    ));
    assert!(s.overlay.is_some(), "a dimmed Retry button does nothing");
}

#[test]
fn click_confirm_discard_buttons_keep_or_delete() {
    // Keep closes the dialog and keeps the draft.
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    reduce(&mut s, &Action::DiscardDraft);
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ConfirmButton(ConfirmButton::Keep)),
    ));
    assert!(s.overlay.is_none());
    assert!(s.composer.is_some(), "keep keeps the draft");

    // Discard confirms the deletion (the same path Enter takes).
    reduce(&mut s, &Action::DiscardDraft);
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::ConfirmButton(ConfirmButton::Discard)),
    );
    assert!(s.composer.is_none());
    assert!(matches!(effects[0].kind, OperationKind::DeleteDraft { .. }));
}

#[test]
fn modal_intercepts_clicks_onto_the_screen_behind_it() {
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    reduce(&mut s, &Action::DiscardDraft);
    let selection_before = s.selection;
    no_effects(&reduce(&mut s, &Action::Click(ClickTarget::MessageRow(4))));
    assert_eq!(s.selection, selection_before);
    assert!(s.overlay.is_some(), "the dialog stays open");
}

#[test]
fn click_attachment_chip_selects_then_opens() {
    let mut s = reader_with_attachments();
    // Selecting the second chip.
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ReaderAttachment(1)),
    ));
    assert_eq!(s.reader_attachment, Some(1));
    assert_eq!(s.focus, Focus::Reader);
    // Clicking the selected chip opens it (the `o` path: save, then open).
    let effects = reduce(&mut s, &Action::Click(ClickTarget::ReaderAttachment(1)));
    let (id, kind) = effect_parts(&effects);
    let OperationKind::SaveAttachment {
        request,
        open_after,
    } = kind
    else {
        panic!("expected SaveAttachment, got {kind:?}");
    };
    assert!(open_after);
    assert_eq!(request.part_id, 5);
    assert!(s.operations.get(id).is_some());
}

#[test]
fn click_composer_send_button_sends_like_ctrl_enter() {
    let mut s = state();
    reduce(&mut s, &Action::Compose);
    if let Some(composer) = s.composer.as_mut() {
        composer.draft.to = String::from("probe@post.local");
        composer.draft.subject = String::from("hello");
    }
    // Seed the clock so the send path can mint draft ids.
    tick(&mut s, 0);
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::ComposerField(ComposerField::Send)),
    );
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::Send { .. }));
    assert!(s.composer.as_ref().is_some_and(|c| c.sending));
}

// ── Phase 10.3: resize robustness (never panics, never invalid) ──────────

#[test]
fn resize_to_degenerate_sizes_never_panics_or_invalidates() {
    let mut s = state();
    s.selection = 5;
    for size in [(0, 0), (1, 1), (89, 19)] {
        reduce(
            &mut s,
            &Action::Resize {
                width: size.0,
                height: size.1,
            },
        );
        assert_eq!(s.size, size);
        // The selection stays a valid index into the page.
        assert!(s.selection < s.messages.items.len().max(1));
        assert!(s.list_scroll < s.messages.items.len().max(1));
    }
}

#[test]
fn resize_with_an_empty_page_stays_valid() {
    let mut s = state();
    s.messages.items.clear();
    s.messages.total = Some(0);
    reduce(
        &mut s,
        &Action::Resize {
            width: 60,
            height: 15,
        },
    );
    assert_eq!(s.selection, 0);
    assert_eq!(s.list_scroll, 0);
    // Moving on an empty page stays a no-op.
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.selection, 0);
}

#[test]
fn resize_while_a_modal_is_open_keeps_state_coherent() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(&mut s, &failure(id, &page_kind(&req), "imap down"));
    assert!(matches!(s.overlay, Some(Overlay::Error(_))));
    if let Some(Overlay::Error(dialog)) = s.overlay.as_mut() {
        dialog.scroll = 50; // far past the end at any size
    }
    reduce(
        &mut s,
        &Action::Resize {
            width: 60,
            height: 15,
        },
    );
    // The modal scroll only clamps on the next Move/scroll action, and the
    // clamp uses the new size — never panics, never negative.
    reduce(&mut s, &Action::MoveDown);
    if let Some(Overlay::Error(dialog)) = &s.overlay {
        let max = crate::ui::components::error_modal::max_scroll(dialog, s.size);
        assert!(dialog.scroll <= max);
    }
}

#[test]
fn resize_never_pushes_the_attachment_cursor_out_of_range() {
    let mut s = reader_with_attachments();
    s.reader_attachment = Some(1);
    for size in [(0, 0), (60, 15), (90, 25), (152, 40)] {
        reduce(
            &mut s,
            &Action::Resize {
                width: size.0,
                height: size.1,
            },
        );
        let count = s
            .open_message
            .as_loaded()
            .map(|m| m.attachments.len())
            .unwrap_or(0);
        // The cursor either stays None (unset) or within the chip count.
        assert!(s.reader_attachment.unwrap_or(0) < count.max(1));
    }
}

// ── Phase 10 feedback: reader action row + capture toggle (yemf) ─────────

#[test]
fn click_reader_action_runs_the_advertised_action() {
    use crate::app::action::ReaderAction;
    let mut s = reader_with_attachments();

    // Archive starts the move operation, exactly like `e`.
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::ReaderAction(ReaderAction::Archive)),
    );
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::Archive(_)));

    // Star issues the starred-flag command, exactly like `s`.
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::ReaderAction(ReaderAction::Star)),
    );
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(
        kind,
        OperationKind::SetStarred { starred: true, .. }
    ));
}

#[test]
fn click_reader_action_reply_opens_a_seeded_reply() {
    use crate::app::action::ReaderAction;
    let mut s = reader_with_attachments();
    tick(&mut s, 0); // seed the clock for draft identities
    let expected_in_reply_to = s
        .open_message
        .as_loaded()
        .and_then(|message| message.headers.message_id.clone());
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ReaderAction(ReaderAction::Reply)),
    ));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.focus, Focus::Composer);
    assert_eq!(
        s.composer.as_ref().expect("composer").draft.in_reply_to,
        expected_in_reply_to
    );
}

#[test]
fn click_reader_action_outside_the_reader_is_inert() {
    use crate::app::action::ReaderAction;
    let mut s = state(); // mailbox route: no reader, no open message
    let before = s.routes.clone();
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::ReaderAction(ReaderAction::Archive)),
    ));
    assert_eq!(s.routes, before);
}

#[test]
fn m_toggles_mouse_capture_state() {
    let mut s = state();
    assert!(!s.mouse_capture);
    no_effects(&reduce(&mut s, &Action::ToggleMouseCapture));
    assert!(s.mouse_capture);
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("off —") || m.contains("on —"))
    );
    no_effects(&reduce(&mut s, &Action::ToggleMouseCapture));
    assert!(!s.mouse_capture);
}

// ── Bulk selection (ticket p0s3) ─────────────────────────────────────────

#[test]
fn space_toggles_the_selection_mark_on_the_focused_row() {
    let mut s = state();
    s.focus = Focus::MessageList;
    s.selection = 2;
    let id = s.selected_message().unwrap().id.clone();

    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(s.selected.contains(&id), "first Space marks the row");
    assert!(s.selection_active());

    // The cursor advanced; move back to the marked row and unmark it.
    s.selection = 2;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(!s.selected.contains(&id), "second Space unmarks");
    assert!(!s.selection_active(), "empty set = selection mode off");
}

#[test]
fn space_does_nothing_outside_the_list_focus() {
    let mut s = state();
    let id = s.messages.items[0].id.clone();
    s.selected.insert(id.clone());
    s.focus = Focus::Sidebar;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    s.focus = Focus::Reader;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(s.selected.contains(&id), "marks never change off-list");
}

#[test]
fn select_all_marks_every_visible_row_and_toggles_back() {
    let mut s = state();
    s.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    assert_eq!(
        s.visible_selected_count(),
        s.messages.items.len(),
        "all visible rows marked"
    );
    assert!(s.all_visible_selected());

    no_effects(&reduce(&mut s, &Action::SelectAll));
    assert!(s.selected.is_empty(), "second Ctrl+A deselects all");
}

#[test]
fn selection_survives_paging_but_not_mailbox_switch() {
    let mut s = state();
    s.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    let marked: Vec<_> = s.selected.iter().cloned().collect();

    // Page forward and back: the marks ride backend ids.
    reduce(&mut s, &Action::PageNext);
    reduce(&mut s, &Action::PagePrevious);
    for id in &marked {
        assert!(
            s.messages.items.iter().any(|m| &m.id == id),
            "page 1 rows returned"
        );
    }
    assert!(marked.iter().all(|id| s.selected.contains(id)));

    // A mailbox switch is a new context: the set clears. The first click
    // only selects the sidebar row; the second (already-selected) switches.
    let next_mailbox = s.mailbox_selection + 1;
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(next_mailbox)));
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(next_mailbox)));
    assert!(s.selected.is_empty(), "mailbox switch clears the selection");
}

#[test]
fn bulk_archive_starts_one_operation_per_selected_message() {
    let mut s = state();
    s.focus = Focus::MessageList;
    s.selection = 0;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    reduce(&mut s, &Action::MoveDown);
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    let count = s.selected.len();
    assert_eq!(count, 2);

    let effects = reduce(&mut s, &Action::Archive);
    assert_eq!(effects.len(), count, "one operation per marked row");
    for effect in &effects {
        assert!(matches!(effect.kind, OperationKind::Archive(_)));
    }
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("2 messages")),
        "status announces the batch: {:?}",
        s.status.message
    );
}

#[test]
fn bulk_trash_read_and_unread_follow_the_same_pattern() {
    let mut s = state();
    s.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    let count = s.messages.items.len();

    let effects = reduce(&mut s, &Action::Trash);
    assert_eq!(effects.len(), count);
    assert!(
        effects
            .iter()
            .all(|e| matches!(e.kind, OperationKind::Trash(_)))
    );

    let effects = reduce(&mut s, &Action::MarkRead);
    assert_eq!(effects.len(), count);
    assert!(
        effects
            .iter()
            .all(|e| matches!(e.kind, OperationKind::SetRead { read: true, .. }))
    );

    let effects = reduce(&mut s, &Action::MarkUnread);
    assert_eq!(effects.len(), count);
    assert!(
        effects
            .iter()
            .all(|e| matches!(e.kind, OperationKind::SetRead { read: false, .. }))
    );
}

#[test]
fn mark_read_single_row_when_no_selection() {
    let mut s = state();
    s.focus = Focus::MessageList;
    s.selection = 1;
    let target = s.selected_message().unwrap().id.clone();
    let (id, kind) = expect_kind(&reduce(&mut s, &Action::MarkRead));
    assert!(
        matches!(&kind, OperationKind::SetRead { read: true, .. }),
        "kind: {kind:?}"
    );
    complete_done(&mut s, id);
    let row = s.messages.items.iter().find(|m| m.id == target).unwrap();
    assert!(row.is_read, "confirmation marks the row read");
}

#[test]
fn reader_shortcuts_do_not_act_on_the_selection() {
    let mut s = state();
    s.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    // Open the first message; the reader takes focus.
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    assert_eq!(s.focus, Focus::Reader);

    // Selection still on, but `e` in the reader archives the open message,
    // not the batch.
    let effects = reduce(&mut s, &Action::Archive);
    let (id, _) = effect_parts(&effects);
    assert!(matches!(effects[0].kind, OperationKind::Archive(_)));
    let opened = s.open_summary().unwrap().id.clone();
    if let OperationKind::Archive(locator) = &effects[0].kind {
        assert_eq!(locator.id, opened, "the open message is the target");
    }
    complete_done(&mut s, id);
}

#[test]
fn moved_messages_leave_the_selection() {
    let mut s = state();
    s.focus = Focus::MessageList;
    s.selection = 0;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    reduce(&mut s, &Action::MoveDown);
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    let first = s.messages.items[0].id.clone();

    let effects = reduce(&mut s, &Action::Archive);
    assert_eq!(effects.len(), 2, "one operation per marked row");
    // Complete the first move: its row leaves the selection, the other
    // mark stays.
    let first_id = effects[0].id;
    let _ = complete_done(&mut s, first_id);
    assert!(
        !s.selected.contains(&first),
        "the moved row is no longer marked"
    );
    assert!(
        s.selection_active(),
        "other marks survive a single move completion"
    );
}

#[test]
fn esc_clears_the_selection_when_nothing_is_pending() {
    let mut s = state();
    s.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    assert!(s.selection_active());
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.selected.is_empty(), "Esc releases the selection first");
    assert!(!s.quit_requested, "Esc does not quit while clearing");
}

#[test]
fn enter_on_the_select_all_toggle_flips_the_set() {
    let mut s = state();
    s.focus = Focus::SelectAllToggle;
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.all_visible_selected(), "Enter selects all");
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.selected.is_empty(), "Enter again clears");
}

#[test]
fn clicking_the_select_all_toggle_marks_every_visible_row() {
    let mut s = state();
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::SelectAllToggle),
    ));
    assert!(s.all_visible_selected());
    assert_eq!(s.focus, Focus::SelectAllToggle, "focus follows the click");
}

#[test]
fn bulk_button_click_dispatches_the_advertised_action() {
    let mut s = state();
    no_effects(&reduce(
        &mut s,
        &Action::Click(ClickTarget::SelectAllToggle),
    ));
    let expected = s.messages.items.len();
    let effects = reduce(
        &mut s,
        &Action::Click(ClickTarget::BulkAction(BulkOp::Trash)),
    );
    assert_eq!(effects.len(), expected);
    assert!(
        effects
            .iter()
            .all(|e| matches!(e.kind, OperationKind::Trash(_))),
        "every selected row gets a trash operation"
    );
    // The click focused the list, so bulk semantics (not reader semantics)
    // applied.
    assert_eq!(s.focus, Focus::MessageList);
}

// ── External editor (plan §14, Phase 11) ─────────────────────────────────

fn edit_external(s: &mut AppState) -> Vec<Effect> {
    reduce(s, &Action::EditExternal)
}

#[test]
fn edit_external_saves_the_draft_first_and_flags_the_composer() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    assert!(s.composer.as_ref().unwrap().draft.is_dirty());

    let effects = edit_external(&mut s);
    // The forced save (step 1) rides ahead of the editor effect.
    assert_eq!(effects.len(), 2);
    assert!(matches!(effects[0].kind, OperationKind::SaveDraft { .. }));
    assert!(matches!(
        effects[1].kind,
        OperationKind::EditExternally { .. }
    ));
    // No background autosave is promised while the editor owns the file.
    assert!(s.composer.as_ref().unwrap().external_editing);
}

#[test]
fn edit_external_with_a_clean_draft_only_starts_the_editor() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    let effects = edit_external(&mut s);
    assert_eq!(effects.len(), 1, "only the editor effect");
    assert!(matches!(
        effects[0].kind,
        OperationKind::EditExternally { .. }
    ));
}

#[test]
fn edit_external_is_inert_without_an_editor_or_composer() {
    let mut s = state();
    // Builtin editor: nothing external to run.
    compose(&mut s);
    no_effects(&edit_external(&mut s));
    // Editor configured, but the composer does not hold focus.
    s.editor_command = Some(vec![String::from("vim")]);
    s.focus = Focus::MessageList;
    no_effects(&edit_external(&mut s));
}

#[test]
fn no_autosave_while_the_external_editor_owns_the_file() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let effects = edit_external(&mut s);
    assert!(matches!(effects[0].kind, OperationKind::SaveDraft { .. }));
    // A dirty edit during the editor session would normally re-arm the
    // autosave; ticks must not save while the editor owns the file.
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('y')));
    no_effects(&tick(&mut s, 5));
    no_effects(&tick(&mut s, 10));
}

#[test]
fn editor_import_marks_dirty_and_saves_once() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    tick(&mut s, 0);
    let effects = edit_external(&mut s);
    let editor_id = effects.last().unwrap().id;
    let before_revision = s.composer.as_ref().unwrap().draft.revision;

    let effects = reduce(
        &mut s,
        &Action::EditorFinished {
            id: editor_id,
            result: Ok(String::from("edited body\nline two\n")),
        },
    );
    // Steps 6–8: import → dirty → exactly one save.
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.body, "edited body\nline two\n");
    assert_eq!(snapshot.revision, before_revision + 1);
    assert!(s.composer.as_ref().unwrap().draft.is_dirty());
    assert!(!s.composer.as_ref().unwrap().external_editing);
    complete_save_ok(&mut s, id, snapshot.revision, "remote-9");
    assert!(!s.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn editor_import_without_changes_saves_nothing() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    // Body is now "x"; the editor hands back exactly that.
    let editor_id = edit_external(&mut s).last().unwrap().id;
    let effects = reduce(
        &mut s,
        &Action::EditorFinished {
            id: editor_id,
            result: Ok(String::from("x")),
        },
    );
    no_effects(&effects);
    assert!(!s.composer.as_ref().unwrap().external_editing);
}

#[test]
fn editor_failure_keeps_the_draft_and_reports() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let editor_id = edit_external(&mut s).last().unwrap().id;
    let effects = reduce(
        &mut s,
        &Action::EditorFinished {
            id: editor_id,
            result: Err(String::from("editor exited with code 1")),
        },
    );
    no_effects(&effects);
    // The draft keeps its content and the composer is usable again.
    assert_eq!(s.composer.as_ref().unwrap().draft.body, "x");
    assert!(!s.composer.as_ref().unwrap().external_editing);
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("editor exited with code 1"))
    );
}

#[test]
fn the_editor_operation_is_never_cancellable() {
    let mut s = state();
    s.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    let effects = edit_external(&mut s);
    let editor_id = effects.last().unwrap().id;
    let op = s.operations.get(editor_id).unwrap();
    assert!(!op.kind.is_cancellable());
}

#[test]
fn space_advances_to_the_next_row_after_toggling() {
    let mut s = state();
    s.focus = Focus::MessageList;
    s.selection = 0;
    let first = s.messages.items[0].id.clone();

    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(s.selected.contains(&first), "row 0 marked");
    assert_eq!(s.selection, 1, "cursor advanced to row 1");

    // The next Space marks row 1 (not unmarks row 0).
    let second = s.messages.items[1].id.clone();
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(s.selected.contains(&second), "row 1 marked next");
    assert!(s.selected.contains(&first), "row 0 stays marked");
    assert_eq!(s.selection, 2);

    // The last row does not advance further.
    s.selection = s.messages.items.len() - 1;
    let last = s.messages.items[s.selection].id.clone();
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert_eq!(
        s.selection,
        s.messages.items.len() - 1,
        "clamped at the end"
    );
    assert!(s.selected.contains(&last));
}

// ── Summary cache (ticket haeb) ──────────────────────────────────────────

#[test]
fn cached_page_serves_instantly_and_the_fresh_load_still_runs() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let cache = crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    );
    // A cached page for the Sent mailbox (offset 0, limit 20).
    let cached = crate::domain::Page {
        items: vec![
            crate::app::mock::mock_page(&MailboxId(String::from("sent")), 0, 1)
                .items
                .remove(0),
        ],
        offset: 0,
        limit: 20,
        total: None,
    };
    cache.store(&MailboxId(String::from("sent")), None, &cached);
    s.page_cache = Some(cache);

    // Switch to Sent: the cached rows render immediately…
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1)));
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1)));
    assert!(
        !s.messages.items.is_empty(),
        "cached rows visible without waiting for the backend"
    );
    assert_eq!(s.messages.items[0].mailbox_id.0, "sent");

    // …and the fresh load still runs: completing it replaces the page.
    let expected = crate::app::mock::mock_page(&MailboxId(String::from("sent")), 0, 20);
    let effects = {
        let req = crate::domain::PageRequest {
            mailbox_id: MailboxId(String::from("sent")),
            offset: 0,
            limit: 20,
        };
        let id = s.operations.start(OperationKind::LoadPage(req.clone())).id;
        reduce(
            &mut s,
            &Action::BackendCompleted(OperationResult {
                id,
                outcome: Ok(OperationOutcome::Page(expected.clone())),
            }),
        )
    };
    // No further page work — but the rows carry no snippets, so the
    // background preview fetches start (ticket wxtx).
    assert!(
        !effects.iter().any(|e| matches!(
            e.kind,
            OperationKind::LoadPage(_) | OperationKind::Search(_)
        )),
        "no page work may follow a completed load, got {effects:?}"
    );
    let previews: Vec<_> = effects
        .iter()
        .filter_map(|e| match &e.kind {
            OperationKind::Preview(locator) => Some(locator.clone()),
            _ => None,
        })
        .collect();
    // Every row without a snippet is requested exactly once: the cached
    // page already fetched its row, the fresh page covers the rest.
    assert_eq!(
        previews.len(),
        expected.items.len() - 1,
        "the remaining rows fetch after the cached row's request: {effects:?}"
    );
    assert_eq!(
        s.preview_requested.len(),
        expected.items.len(),
        "one preview request per row overall"
    );
    for (locator, summary) in previews.iter().zip(expected.items.iter().skip(1)) {
        assert_eq!(&locator.id, &summary.id);
        assert_eq!(&locator.mailbox, &summary.mailbox_id);
        assert!(s.preview_requested.contains(&summary.id), "deduped");
    }
    assert_eq!(s.messages.items.len(), expected.items.len());
    // The successful load overwrote the cache entry.
    let cached = s
        .page_cache
        .as_ref()
        .unwrap()
        .load(&MailboxId(String::from("sent")), None, 0, 20)
        .expect("cache refreshed");
    assert_eq!(cached.items.len(), expected.items.len());
}

#[test]
fn cold_start_serves_cached_mailboxes_and_first_page_instantly() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let cache = crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    );
    // The cached world: a mailbox listing and its first page.
    cache.store_mailboxes(crate::app::mock::mock_mailboxes().as_slice());
    let page = crate::app::mock::mock_page(&inbox_id(), 0, crate::app::mock::PAGE_SIZE);
    cache.store(&inbox_id(), None, &page);
    s.page_cache = Some(cache);

    // Cold start: mailboxes are not loaded yet.
    s.mailboxes = crate::app::state::Loadable::Loading;
    // Startup Refresh: mailboxes render from cache, but the fresh listing
    // still loads.
    let effects = reduce(&mut s, &Action::Refresh);
    assert!(matches!(
        s.mailboxes,
        crate::app::state::Loadable::Loaded(_)
    ));
    let request_effects = effects.len();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e.kind, OperationKind::LoadMailboxes)),
        "the fresh mailbox listing still loads"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e.kind, OperationKind::LoadPage(_))),
        "the fresh first page still loads"
    );
    let _ = request_effects;
    // The cached first page is visible without waiting.
    assert_eq!(s.messages.items.len(), crate::app::mock::PAGE_SIZE);
}

#[test]
fn opening_a_message_serves_the_cached_copy_instantly() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let cache = crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    );
    s.page_cache = Some(cache);
    s.selection = 0;
    let summary = s.selected_message().unwrap().clone();
    // A previously viewed copy of this exact message.
    let seen = crate::app::mock::mock_message(&summary);
    s.page_cache
        .as_ref()
        .unwrap()
        .store_message(&summary.mailbox_id, &summary.id.0, &seen);

    let effects = reduce(&mut s, &Action::Activate);
    // The cached body renders immediately…
    assert!(matches!(
        s.open_message,
        crate::app::state::Loadable::Loaded(_)
    ));
    // …and the fresh load still runs.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e.kind, OperationKind::LoadMessage(_)))
    );
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
/// snippets) and complete the fresh page load. Returns the preview
/// effects the page apply produced.
fn load_sent_without_snippets(s: &mut AppState) -> Vec<Effect> {
    reduce(s, &Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(s, &Action::Click(ClickTarget::Mailbox(1))); // activate
    let (load, req) = expect_page(&effects);
    complete_page_ok(s, load, &req, 0)
}

/// Complete one in-flight preview with the mocked full message. Returns
/// the follow-up effects (the rolling fetch refill).
fn complete_preview_ok(
    s: &mut AppState,
    id: OperationId,
    summary: &crate::domain::MessageSummary,
) -> Vec<Effect> {
    let message = mock::mock_message(summary);
    reduce(
        s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    )
}

#[test]
fn preview_result_fills_the_list_snippet_and_caches_the_message() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    s.page_cache = Some(crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    ));
    let effects = load_sent_without_snippets(&mut s);
    let previews = expect_previews(&effects);
    assert_eq!(previews.len(), 4, "one preview per Sent row");

    let (id, locator) = previews[0].clone();
    let op = s.operations.get(id).expect("preview in flight");
    // Preview fetches are silent background work: they never take the
    // `Esc`-cancel / spinner slot.
    assert_eq!(op.origin, OperationOrigin::Background);
    assert_ne!(s.operations.foreground().map(|o| o.id), Some(id));

    let summary = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .expect("row listed")
        .clone();
    complete_preview_ok(&mut s, id, &summary);
    // The row now carries its one-line body preview…
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .unwrap();
    let snippet = row.snippet.as_deref().expect("snippet filled");
    assert!(snippet.starts_with("body line 01"), "{snippet}");
    // …and the full message is cached for an instant open.
    assert!(
        s.page_cache
            .as_ref()
            .unwrap()
            .load_message(&locator.mailbox, &locator.id.0)
            .is_some(),
        "preview fetch caches the message"
    );
}

#[test]
fn preview_failure_is_silent_and_never_retried() {
    let mut s = state();
    let effects = load_sent_without_snippets(&mut s);
    let (id, locator) = expect_previews(&effects)[0].clone();
    reduce(
        &mut s,
        &failure(
            id,
            &OperationKind::Preview(locator.clone()),
            "himalaya exploded",
        ),
    );
    // Decorative work: no Retry/Dismiss modal, no status noise, and the
    // row keeps no snippet.
    assert!(s.overlay.is_none());
    assert!(s.status.message.is_none());
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .unwrap();
    assert!(row.snippet.is_none());
    // The id stays requested: a later page apply never re-fetches it.
    let req = crate::domain::PageRequest {
        mailbox_id: MailboxId(String::from("sent")),
        offset: 0,
        limit: 20,
    };
    let id = s.operations.start(OperationKind::LoadPage(req.clone())).id;
    let effects = complete_page_ok(&mut s, id, &req, 0);
    assert!(
        expect_previews(&effects)
            .iter()
            .all(|(_, l)| l.id != locator.id),
        "failed previews are not re-requested"
    );
}

#[test]
fn esc_never_cancels_in_flight_previews() {
    let mut s = state();
    let effects = load_sent_without_snippets(&mut s);
    let previews = expect_previews(&effects);
    assert!(!previews.is_empty(), "previews in flight");
    // Open the reader on a row; Sent rows are read, so no flag ops start.
    s.selection = 0;
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    // Esc closes the reader — the previews are not foreground work for it
    // to absorb.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.routes.len(), 1, "reader closed");
    assert!(
        previews
            .iter()
            .all(|(id, _)| s.operations.get(*id).is_some()),
        "every preview is still in flight"
    );
}

#[test]
fn auto_refresh_is_not_blocked_by_in_flight_previews() {
    let mut s = state();
    timer(&mut s);
    no_effects(&tick(&mut s, 0));
    let effects = load_sent_without_snippets(&mut s);
    assert!(!expect_previews(&effects).is_empty());
    // The interval elapses while the silent background fetches run: the
    // timer still fires (only foreground work stands it down).
    let effects = tick(&mut s, 60);
    expect_page(&effects);
}

#[test]
fn preview_fetches_roll_within_the_window() {
    let mut s = state();
    // A page wider than the fetch window (cap: 6), rows without snippets.
    let mut page = crate::domain::Page {
        items: Vec::new(),
        offset: 0,
        limit: 20,
        total: Some(10),
    };
    for i in 0..10 {
        let mut summary = mock::mock_page(&inbox_id(), 0, 1).items.remove(0);
        summary.id = MessageId(format!("p{i}"));
        summary.snippet = None;
        page.items.push(summary);
    }
    let req = PageRequest {
        mailbox_id: inbox_id(),
        offset: 0,
        limit: 20,
    };
    let id = s.operations.start(OperationKind::LoadPage(req)).id;
    let effects = reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page.clone())),
        }),
    );
    let previews = expect_previews(&effects);
    assert_eq!(previews.len(), 6, "the window caps concurrent fetches");
    // Completing one rolls the window: the next queued row starts.
    let (first, locator) = previews[0].clone();
    let summary = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .expect("row listed")
        .clone();
    let effects = complete_preview_ok(&mut s, first, &summary);
    let refill = expect_previews(&effects);
    assert_eq!(refill.len(), 1, "the next queued row starts");
    assert_eq!(s.preview_requested.len(), 7, "queued rows are tracked");
}

// ── Auto page size (ticket kjfq) ─────────────────────────────────────────

#[test]
fn auto_page_size_tracks_the_visible_rows_on_resize() {
    let mut s = state();
    s.page_size_auto = true;
    // Shrink the terminal: the limit becomes however many rows fit the
    // list, and the visible page reloads in the background (ticket kjfq).
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 20,
        },
    );
    let rows_20 = crate::ui::layout::message_rows_visible((152, 20)).max(1);
    assert_eq!(s.messages.limit, rows_20, "limit matches the visible rows");
    // Growing further changes the limit again and re-loads (background).
    reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 24,
        },
    );
    let rows_24 = crate::ui::layout::message_rows_visible((152, 24)).max(1);
    assert_ne!(rows_20, rows_24);
    let effects = reduce(
        &mut s,
        &Action::Resize {
            width: 152,
            height: 28,
        },
    );
    let (id, req) = expect_page(&effects);
    assert_eq!(
        req.limit,
        crate::ui::layout::message_rows_visible((152, 28)).max(1)
    );
    assert_eq!(
        s.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background),
        "silent reload"
    );
}

#[test]
fn manual_page_size_is_untouched_by_resize() {
    let mut s = state();
    s.page_size_auto = false;
    reduce(
        &mut s,
        &Action::Resize {
            width: 90,
            height: 20,
        },
    );
    assert_eq!(s.messages.limit, mock::PAGE_SIZE, "page_size rules");
    assert!(s.operations.is_empty(), "no reload without auto sizing");
}
