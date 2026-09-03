//! Reducer unit tests (plan §9: invalid/empty selections, stale inputs,
//! route behavior, resize; plan §19 Phase 3: operation registry semantics,
//! stale/superseded-result rejection, cancellation, Retry/Dismiss modal).

use super::*;
use crate::app::action::SearchEdit;
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

/// Complete `id` with an `Ok` page payload for `req` at `offset`.
fn complete_page_ok(state: &mut AppState, id: OperationId, req: &PageRequest, offset: usize) {
    let page = mock::mock_page(&req.mailbox_id, offset, req.limit);
    reduce(
        state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page)),
        }),
    );
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
    assert_eq!(snippet, Some("body line 01"));
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
    let viewport = crate::ui::layout::reader_rows_visible(s.size).max(1);
    let total = crate::ui::screens::reader::content_line_count(&s, s.size.0 as usize);
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
    let deep = crate::ui::screens::reader::content_line_count(&s, narrow);
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
    let viewport = crate::ui::layout::reader_rows_visible(s.size).max(1);
    let total = crate::ui::screens::reader::content_line_count(
        &s,
        crate::ui::layout::reader_width(s.size).max(10),
    ) as i64;
    let max = (total - viewport as i64).max(0) as usize;
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
    let viewport = crate::ui::layout::reader_rows_visible(s.size).max(1);
    let total = crate::ui::screens::reader::content_line_count(
        &s,
        crate::ui::layout::reader_width(s.size).max(10),
    ) as i64;
    let max = (total - viewport as i64).max(0) as usize;
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
