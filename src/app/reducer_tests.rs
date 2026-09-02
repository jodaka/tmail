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
    reduce(&mut s, &Action::Tick);
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
        None => panic!("modal open"),
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
    reduce(&mut s, &Action::Tick);
    assert_eq!(s.ticks, before.ticks + 1);
    assert_eq!(s.selection, before.selection);
    assert_eq!(s.focus, before.focus);
}

#[test]
fn unimplemented_actions_are_safe_noops() {
    let mut s = state();
    let before = s.clone();
    for action in [
        Action::Compose,
        Action::Reply,
        Action::ReplyAll,
        Action::Forward,
        Action::Send,
        Action::LeaveComposer,
        Action::DiscardDraft,
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
