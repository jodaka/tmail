//! Reducer tests: bulk domain.

use super::*;

// ── Bulk selection (ticket p0s3) ─────────────────────────────────────────

#[test]
fn space_toggles_the_selection_mark_on_the_focused_row() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::Sidebar;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    s.session.focus = Focus::Reader;
    no_effects(&reduce(&mut s, &Action::ToggleSelected));
    assert!(s.selected.contains(&id), "marks never change off-list");
}

#[test]
fn select_all_marks_every_visible_row_and_toggles_back() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::MessageList;
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
        s.session
            .status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("2 messages")),
        "status announces the batch: {:?}",
        s.session.status.message
    );
}

#[test]
fn bulk_trash_read_and_unread_follow_the_same_pattern() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    // Open the first message; the reader takes focus.
    let (load_id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, load_id);
    assert_eq!(s.session.focus, Focus::Reader);

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
    s.session.focus = Focus::MessageList;
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
    s.session.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, &Action::SelectAll));
    assert!(s.selection_active());
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.selected.is_empty(), "Esc releases the selection first");
    assert!(
        !s.session.quit_requested,
        "Esc does not quit while clearing"
    );
}

#[test]
fn bulk_button_click_dispatches_the_advertised_action() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::SelectAll));
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
    assert_eq!(s.session.focus, Focus::MessageList);
}
