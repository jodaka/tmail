//! Reducer tests: attachments domain.

use super::*;

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

/// Enter in the reader presses the selected chip — `o`'s save-then-open
/// path (plan §15, ticket 61qx).
#[test]
fn enter_on_the_reader_opens_the_selected_attachment() {
    let mut s = reader_with_attachments();
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    let OperationKind::SaveAttachment {
        request,
        open_after,
    } = kind
    else {
        panic!("expected SaveAttachment");
    };
    assert!(open_after, "Enter arms the opener chain");
    assert_eq!(request.part_id, 3, "first chip by default");
    // The chain completes exactly like `o`'s would.
    let effects = reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::SavedPath(PathBuf::from(
                "/home/u/Downloads/report.pdf",
            ))),
        }),
    );
    let (_, open_kind) = effect_parts(&effects);
    assert!(matches!(open_kind, OperationKind::OpenPath { .. }));
}

#[test]
fn enter_reuses_a_session_saved_attachment_path() {
    let mut s = reader_with_attachments();
    let saved = PathBuf::from("/home/u/Downloads/report.pdf");
    let message_id = s.open_message.as_loaded().unwrap().id.clone();
    s.saved_attachments.insert((message_id, 3), saved.clone());
    let effects = reduce(&mut s, &Action::Activate);
    let (_, kind) = effect_parts(&effects);
    assert_eq!(
        kind,
        OperationKind::OpenPath { path: saved },
        "Enter opens a saved file without a second download"
    );
}

#[test]
fn enter_on_the_reader_without_attachments_is_inert() {
    let mut s = state();
    open_reader_with(&mut s, reply_source()); // no attachments
    no_effects(&reduce(&mut s, &Action::Activate));
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
fn keyboard_d_trashes_and_capital_s_saves_in_the_reader() {
    use crate::input::keyboard;
    use crate::input::keymap::KeyMap;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let keymap = KeyMap::defaults();
    // `d` deletes the open message now (ticket zg41); the attachment save
    // lives on `S`.
    let action = keyboard::to_action(
        &keymap,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        Focus::Reader,
    );
    assert_eq!(action, Some(Action::Trash));
    let action = keyboard::to_action(
        &keymap,
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        Focus::Reader,
    );
    assert_eq!(action, Some(Action::SaveAttachment));
}

#[test]
fn keyboard_d_and_o_map_to_save_and_open() {
    use crate::input::keyboard;
    use crate::input::keymap::KeyMap;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let keymap = KeyMap::defaults();
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    assert_eq!(
        keyboard::to_action(&keymap, key('S'), Focus::Reader),
        Some(Action::SaveAttachment)
    );
    assert_eq!(
        keyboard::to_action(&keymap, key('o'), Focus::Reader),
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
    let viewport = crate::view::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::app::reader::header_line_count(&s, width))
        .max(1);
    let total = crate::app::reader::scroll_line_count(&s, width);
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
    let deep = crate::app::reader::scroll_line_count(&s, narrow);
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
    let width = crate::view::layout::reader_width(s.size).max(10);
    let viewport = crate::view::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::app::reader::header_line_count(&s, width))
        .max(1) as i64;
    let total = crate::app::reader::scroll_line_count(&s, width) as i64;
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
    let width = crate::view::layout::reader_width(s.size).max(10);
    let viewport = crate::view::layout::reader_rows_visible(s.size)
        .saturating_sub(crate::app::reader::header_line_count(&s, width))
        .max(1) as i64;
    let total = crate::app::reader::scroll_line_count(&s, width) as i64;
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

#[test]
fn enter_on_attach_opens_the_chooser_and_esc_closes_it() {
    let mut s = state();
    open_attach_dialog(&mut s);
    // The chooser opens in its pending state: listing in flight.
    {
        let dialog = match s.overlay.as_ref().unwrap() {
            Overlay::AttachmentExplorer(dialog) => dialog,
            _ => panic!("chooser open"),
        };
        assert!(dialog.explorer.is_none());
        assert!(dialog.listing);
        assert_eq!(dialog.error, None);
    }
    // BackOrCancel restores the composer focus.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn the_landed_listing_replaces_the_pending_state() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentExplorer(dialog) => dialog,
        _ => panic!("chooser open"),
    };
    assert!(dialog.explorer.is_some(), "the explorer arrived");
    assert!(!dialog.listing);
    assert_eq!(dialog.error, None);
}

#[test]
fn arrows_move_the_selection_and_enter_submits_a_file() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);

    // The listing starts on the parent row ("../"); files come after the
    // directories, sorted by name. Two Downs: ../ → docs/ → notes.txt.
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let expected = dir.join("notes.txt");
    assert_eq!(
        match s.overlay.as_ref().unwrap() {
            Overlay::AttachmentExplorer(dialog) => dialog.selected_file(),
            _ => panic!("chooser open"),
        },
        Some(expected.clone()),
        "the selection is a file now"
    );

    // Enter submits the selected file for backend validation, raw.
    let (_, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    assert_eq!(
        kind,
        OperationKind::ReadAttachment { path: expected },
        "the selected file's path goes to the backend"
    );
    assert!(
        matches!(s.overlay.as_ref(), Some(Overlay::AttachmentExplorer(_))),
        "the chooser stays open while validating"
    );
    assert_eq!(kind.summary(), "Checking file");
}

#[test]
fn enter_on_a_directory_lists_it_and_navigation_freezes() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);

    // The first directory after the parent row is `docs/`; Enter lists it.
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id2, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    assert_eq!(
        kind,
        OperationKind::ListAttachmentFiles {
            path: Some(dir.join("docs")),
        }
    );
    {
        let dialog = match s.overlay.as_ref().unwrap() {
            Overlay::AttachmentExplorer(dialog) => dialog,
            _ => panic!("chooser open"),
        };
        assert!(dialog.listing, "navigation is frozen while listing");
    }
    // A second directory change while the listing runs is a no-op.
    no_effects(&reduce(
        &mut s,
        &Action::AttachmentBrowse(AttachmentBrowse::Parent),
    ));

    // The landing replaces the working directory.
    land_listing(&mut s, id2, &dir.join("docs"));
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentExplorer(dialog) => dialog,
        _ => panic!("chooser open"),
    };
    assert_eq!(
        dialog.explorer.as_ref().unwrap().cwd(),
        &dir.join("docs"),
        "the chooser is inside the directory now"
    );
    assert!(!dialog.listing);
}

#[test]
fn left_goes_to_the_parent_and_right_into_the_selected_dir() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);

    // Right on a directory lists it; Left lists the parent.
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id2, kind) = effect_parts(&reduce(
        &mut s,
        &Action::AttachmentBrowse(AttachmentBrowse::Open),
    ));
    assert_eq!(
        kind,
        OperationKind::ListAttachmentFiles {
            path: Some(dir.join("docs")),
        }
    );
    land_listing(&mut s, id2, &dir.join("docs"));
    let (_, kind) = effect_parts(&reduce(
        &mut s,
        &Action::AttachmentBrowse(AttachmentBrowse::Parent),
    ));
    assert_eq!(
        kind,
        OperationKind::ListAttachmentFiles { path: Some(dir) },
        "back to the chooser's opening directory"
    );
}

#[test]
fn validated_file_becomes_a_chip_and_dirties_the_draft() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);
    // Down twice: ../ → docs/ → notes.txt; then use notes.txt.
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(&mut s, id, attachment("notes.txt"));
    // The dialog closed and the chip is focused.
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    let composer = s.composer.as_ref().unwrap();
    assert_eq!(composer.field, ComposerField::Attachment(0));
    let att = &composer.draft.attachments[0];
    assert_eq!(att.name, "notes.txt");
    assert_eq!(att.path, PathBuf::from("/tmp/notes.txt"));
    assert!(composer.draft.is_dirty(), "attaching is a content edit");
    assert_eq!(
        s.status.message.as_deref(),
        Some("Attached notes.txt (1 KB)")
    );
}

#[test]
fn validation_failure_stays_in_the_chooser_retryable() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: None,
                detail: String::from("`notes.txt` does not exist"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // The chooser stays open with the detail; the selection is unchanged.
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentExplorer(dialog) => dialog,
        _ => panic!("chooser open"),
    };
    assert_eq!(dialog.error.as_deref(), Some("`notes.txt` does not exist"));
    assert!(dialog.selected_file().is_some());
    assert_eq!(s.focus, Focus::Dialog);
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn failed_listing_keeps_the_chooser_open_with_the_detail() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: None,
                detail: String::from("permission denied"),
                retry: None,
                ambiguous: false,
            }),
        }),
    );
    let dialog = match s.overlay.as_ref().unwrap() {
        Overlay::AttachmentExplorer(dialog) => dialog,
        _ => panic!("chooser open"),
    };
    assert_eq!(dialog.error.as_deref(), Some("permission denied"));
    assert!(!dialog.listing, "navigation is free again");
    assert!(dialog.explorer.is_none(), "nothing to show");
    // Esc still closes it.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
}

#[test]
fn stale_results_are_dropped() {
    let mut s = state();
    // Esc while validating: the result must not attach anything.
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::BackOrCancel);
    complete_read_ok(&mut s, id, attachment("notes.txt"));
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());

    // Moving the selection while validating: the older result is stale.
    let (_, id) = open_attach_dialog(&mut s);
    land_listing(&mut s, id, &dir);
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Up));
    complete_read_ok(&mut s, id, attachment("notes.txt"));
    assert!(s.composer.as_ref().unwrap().draft.attachments.is_empty());
}

#[test]
fn same_file_attaches_once() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    land_listing(&mut s, id, &dir);
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(&mut s, id, attachment("notes.txt"));
    assert_eq!(s.composer.as_ref().unwrap().draft.attachments.len(), 1);

    // Re-adding the same path: no duplicate chip, no dirt.
    let revision = s.composer.as_ref().unwrap().draft.revision;
    let (_, id) = open_attach_dialog(&mut s);
    land_listing(&mut s, id, &dir);
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    reduce(&mut s, &Action::AttachmentBrowse(AttachmentBrowse::Down));
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    complete_read_ok(
        &mut s,
        id,
        DraftAttachment {
            path: PathBuf::from("/tmp/notes.txt"),
            name: String::from("notes.txt"),
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
        Some("notes.txt is already attached")
    );
}

#[test]
fn listing_results_for_a_closed_chooser_are_dropped() {
    let mut s = state();
    let (_, id) = open_attach_dialog(&mut s);
    let (_guard, dir) = chooser_dir();
    reduce(&mut s, &Action::BackOrCancel);
    land_listing(&mut s, id, &dir);
    assert!(s.overlay.is_none(), "nothing resurrected");
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
