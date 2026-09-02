//! Reducer unit tests (plan §9: invalid/empty selections, stale inputs,
//! route behavior, resize; plan §19 Phase 2: effect-based page loads,
//! stale-result rejection, page boundaries, selection visibility).

use super::*;
use crate::app::action::SearchEdit;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::mock::{self, mock_initial_state};
use crate::app::state::AppState;
use crate::domain::{Mailbox, MailboxId, MailboxRole, MessageId, PageRequest};

fn state() -> AppState {
    mock_initial_state()
}

fn inbox_id() -> MailboxId {
    MailboxId(String::from("inbox"))
}

fn request(offset: usize) -> PageRequest {
    PageRequest {
        mailbox_id: inbox_id(),
        offset,
        limit: mock::PAGE_SIZE,
    }
}

/// Feed the reducer the result it expects for a pending `LoadPage`.
fn load_page(state: &mut AppState, offset: usize) {
    let page = mock::mock_page(&inbox_id(), offset, mock::PAGE_SIZE);
    reduce(
        state,
        &Action::PageLoaded {
            request: request(offset),
            result: Ok(page),
        },
    );
}

/// Apply an `Ok` result for `request` as the runtime would.
fn apply_ok(state: &mut AppState, req: PageRequest, offset: usize) {
    let page = mock::mock_page(&req.mailbox_id, offset, req.limit);
    reduce(
        state,
        &Action::PageLoaded {
            request: req,
            result: Ok(page),
        },
    );
}

fn page_effect(effects: &[Effect]) -> PageRequest {
    match effects {
        [Effect::LoadPage(req)] => req.clone(),
        other => panic!("expected exactly one LoadPage effect, got {other:?}"),
    }
}

fn no_effects(effects: &[Effect]) {
    assert!(effects.is_empty(), "expected no effects, got {effects:?}");
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
    let effects = reduce(&mut s, &Action::PageNext);
    let req = page_effect(&effects);
    assert_eq!(req.offset, mock::PAGE_SIZE);
    assert_eq!(s.pending_page.as_ref(), Some(&req));
    // The old page and selection stay visible until the result lands.
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.selection, 3);
    load_page(&mut s, mock::PAGE_SIZE);
    assert_eq!(s.pending_page, None);
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
    let req = page_effect(&reduce(&mut s, &Action::PageNext));
    apply_ok(&mut s, req, mock::PAGE_SIZE);
    // No page past the known total (25 items, 2 pages).
    no_effects(&reduce(&mut s, &Action::PageNext));
    let req = page_effect(&reduce(&mut s, &Action::PagePrevious));
    assert_eq!(req.offset, 0);
    apply_ok(&mut s, req, 0);
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
        page_effect(&reduce(&mut s, &Action::PageNext)).offset,
        mock::PAGE_SIZE
    );
}

#[test]
fn rapid_page_next_advances_from_the_pending_request() {
    let mut s = state();
    s.messages.total = None;
    let first = page_effect(&reduce(&mut s, &Action::PageNext));
    let second = page_effect(&reduce(&mut s, &Action::PageNext));
    assert_eq!(second.offset, first.offset + mock::PAGE_SIZE);
    // The older result is now stale and must never apply.
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: first,
            result: Ok(mock::mock_page(&inbox_id(), 0, mock::PAGE_SIZE)),
        },
    );
    assert_eq!(s.messages.offset, 0);
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: second.clone(),
            result: Ok(mock::mock_page(&inbox_id(), second.offset, mock::PAGE_SIZE)),
        },
    );
    assert_eq!(s.messages.offset, second.offset);
}

#[test]
fn stale_page_result_is_dropped() {
    let mut s = state();
    let req = page_effect(&reduce(&mut s, &Action::PageNext));
    // A result for a different offset than the pending request is stale.
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: request(40),
            result: Ok(mock::mock_page(&inbox_id(), 40, mock::PAGE_SIZE)),
        },
    );
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.pending_page.as_ref(), Some(&req));
    load_page(&mut s, mock::PAGE_SIZE);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
}

#[test]
fn stale_result_for_other_mailbox_is_dropped() {
    let mut s = state();
    let req = page_effect(&reduce(&mut s, &Action::PageNext));
    let sent = PageRequest {
        mailbox_id: MailboxId(String::from("sent")),
        offset: 0,
        limit: mock::PAGE_SIZE,
    };
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: sent,
            result: Ok(mock::mock_page(
                &MailboxId(String::from("sent")),
                0,
                mock::PAGE_SIZE,
            )),
        },
    );
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.pending_page.as_ref(), Some(&req));
}

#[test]
fn page_load_failure_keeps_last_page_and_reports() {
    let mut s = state();
    let req = page_effect(&reduce(&mut s, &Action::PageNext));
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: req,
            result: Err(String::from("himalaya exploded")),
        },
    );
    assert_eq!(s.pending_page, None);
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("himalaya exploded"))
    );
}

#[test]
fn selection_identity_survives_refresh() {
    let mut s = state();
    reduce(&mut s, &Action::MoveDown);
    reduce(&mut s, &Action::MoveDown);
    let selected: MessageId = s.selected_message().expect("selection").id.clone();
    let effects = reduce(&mut s, &Action::Refresh);
    let req = page_effect(&effects);
    assert_eq!(req.offset, 0);
    assert_eq!(s.status.message.as_deref(), Some("Refreshing…"));
    load_page(&mut s, 0);
    assert_eq!(
        s.selected_message().expect("selection after refresh").id,
        selected
    );
}

#[test]
fn refresh_on_second_page_keeps_page_and_selection() {
    let mut s = state();
    let req = page_effect(&reduce(&mut s, &Action::PageNext));
    reduce(
        &mut s,
        &Action::PageLoaded {
            request: req,
            result: Ok(mock::mock_page(
                &inbox_id(),
                mock::PAGE_SIZE,
                mock::PAGE_SIZE,
            )),
        },
    );
    s.selection = 2;
    let id = s.selected_message().unwrap().id.clone();
    let effects = reduce(&mut s, &Action::Refresh);
    assert_eq!(page_effect(&effects).offset, mock::PAGE_SIZE);
    load_page(&mut s, mock::PAGE_SIZE);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
    assert_eq!(s.selected_message().unwrap().id, id);
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
    let effects = reduce(s, &Action::Activate);
    let req = page_effect(&effects);
    assert_eq!(req.mailbox_id.0, mailbox);
    assert_eq!(req.offset, 0);
    let page = mock::mock_page(&MailboxId(String::from(mailbox)), 0, mock::PAGE_SIZE);
    reduce(
        s,
        &Action::PageLoaded {
            request: req,
            result: Ok(page),
        },
    );
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

#[test]
fn mailboxes_loaded_selects_inbox_role() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    assert_eq!(s.routes.len(), 0);
    let effects = reduce(&mut s, &Action::MailboxesLoaded(Ok(mock::mock_mailboxes())));
    let req = page_effect(&effects);
    assert_eq!(req.mailbox_id.0, "inbox");
    assert_eq!(req.offset, 0);
    assert_eq!(s.routes.len(), 1);
    assert_eq!(s.mailbox_selection, 0);
    assert_eq!(s.mailboxes, Loadable::Loaded(mock::mock_mailboxes()));
}

#[test]
fn mailboxes_loaded_falls_back_to_first_mailbox() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
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
    let effects = reduce(&mut s, &Action::MailboxesLoaded(Ok(mailboxes)));
    assert_eq!(page_effect(&effects).mailbox_id.0, "notes");
    assert_eq!(s.mailbox_selection, 0);
}

#[test]
fn mailboxes_loaded_empty_is_valid_not_an_error() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    no_effects(&reduce(&mut s, &Action::MailboxesLoaded(Ok(Vec::new()))));
    assert_eq!(s.routes.len(), 0);
    assert!(s.messages.items.is_empty());
    assert!(!s.quit_requested);
}

#[test]
fn mailboxes_loaded_failure_is_reported() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    no_effects(&reduce(
        &mut s,
        &Action::MailboxesLoaded(Err(String::from("no such account"))),
    ));
    assert_eq!(
        s.mailboxes,
        Loadable::Failed(String::from("no such account"))
    );
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("no such account"))
    );
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
        Action::Archive,
        Action::Trash,
        Action::ToggleStar,
        Action::MarkUnread,
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
    // work is only described as effects (Phase 2).
    let mut s = state();
    let effects = reduce(&mut s, &Action::Refresh);
    assert!(matches!(effects.as_slice(), [Effect::LoadPage(_)]));
    assert!(s.mailboxes.as_loaded().is_some());
}
