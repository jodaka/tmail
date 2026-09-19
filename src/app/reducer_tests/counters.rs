//! Reducer tests: optimistic sidebar counters (ticket ng42). Confirmed
//! completions adjust `state.mailboxes` immediately; the chained listing
//! recount (q0hc) stays authoritative and overwrites the local numbers.
//! The Drafts folder's counter is its content total, never an unread
//! count.

use super::*;
use crate::app::reducer::message_results::message_moved;
use crate::domain::MessageLocator;

/// A synthetic row in a given folder, to exercise move counting without
/// disturbing the inbox seed's shared page expectations.
fn drafts_row(id: &str, is_read: bool) -> MessageSummary {
    MessageSummary {
        id: MessageId(String::from(id)),
        mailbox_id: MailboxId(String::from("drafts")),
        message_id: None,
        from: vec![],
        to: vec![],
        subject: String::from("scratch"),
        snippet: None,
        timestamp: chrono::Utc::now().fixed_offset(),
        is_read,
        is_starred: false,
        has_attachments: false,
    }
}

fn drafts_total(s: &AppState) -> u64 {
    s.mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .find(|m| m.role == Some(MailboxRole::Drafts))
        .unwrap()
        .total_count
        .unwrap()
}

fn inbox_unread(s: &AppState) -> u64 {
    s.mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .find(|m| m.id == inbox_id())
        .unwrap()
        .unread_count
        .unwrap()
}

fn locator_for_row(s: &AppState, id: &MessageId) -> MessageLocator {
    let summary = s.messages.items.iter().find(|m| &m.id == id).unwrap();
    summary.into_locator()
}

#[test]
fn mark_read_drops_the_unread_count_immediately() {
    let mut s = state();
    s.selection = 1; // m2: unread in the mock seed.
    assert!(!s.selected_message().unwrap().is_read);
    let before = inbox_unread(&s);
    let (id, kind) = expect_kind(&reduce(&mut s, Action::MarkRead));
    assert!(matches!(&kind, OperationKind::SetRead { read: true, .. }));
    let _ = complete_done(&mut s, id);
    assert_eq!(inbox_unread(&s), before - 1, "mark read = one less unread");
}

#[test]
fn mark_unread_raises_the_unread_count_immediately() {
    let mut s = state();
    s.selection = 3; // m4: read in the mock seed.
    assert!(s.selected_message().unwrap().is_read);
    let before = inbox_unread(&s);
    let (id, kind) = expect_kind(&reduce(&mut s, Action::MarkUnread));
    assert!(matches!(&kind, OperationKind::SetRead { read: false, .. }));
    let _ = complete_done(&mut s, id);
    assert_eq!(inbox_unread(&s), before + 1, "mark unread = one more");
}

#[test]
fn flag_failure_never_touches_the_count() {
    let mut s = state();
    s.selection = 1; // m2: unread.
    let before = inbox_unread(&s);
    let (id, kind) = expect_kind(&reduce(&mut s, Action::MarkRead));
    reduce(&mut s, failure(id, &kind, "refused"));
    assert_eq!(inbox_unread(&s), before, "no adjustment without success");
}

#[test]
fn bulk_mark_read_counts_the_truly_flipped_rows() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
    no_effects(&reduce(&mut s, Action::SelectAll));
    let unread_rows = s.messages.items.iter().filter(|m| !m.is_read).count();
    assert!(unread_rows >= 1, "precondition: the seed has unread rows");
    let before = inbox_unread(&s);
    let (id, _) = expect_kind(&reduce(&mut s, Action::MarkRead));
    let _ = complete_done(&mut s, id);
    assert_eq!(
        inbox_unread(&s),
        before - unread_rows as u64,
        "one less unread per flipped row"
    );
}

#[test]
fn moving_an_unread_row_drops_the_source_counter() {
    let mut s = state();
    s.selection = 1; // m2: unread.
    let before = inbox_unread(&s);
    let (id, _) = expect_kind(&reduce(&mut s, Action::Archive));
    let _ = complete_done(&mut s, id);
    assert_eq!(inbox_unread(&s), before - 1);
}

#[test]
fn moving_a_read_row_leaves_the_counters_alone() {
    let mut s = state();
    s.selection = 3; // m4: read.
    let before = inbox_unread(&s);
    let (id, _) = expect_kind(&reduce(&mut s, Action::Archive));
    let _ = complete_done(&mut s, id);
    assert_eq!(inbox_unread(&s), before);
}

#[test]
fn a_moved_draft_shrinks_the_drafts_total_not_unread() {
    let mut s = state();
    let row = drafts_row("d0", true);
    let id = row.id.clone();
    s.messages.items.insert(0, row);
    let before = drafts_total(&s);
    let locator = locator_for_row(&s, &id);
    message_moved(&mut s, &[locator]);
    // The drafts folder's unread count is invisible to the sidebar: the
    // total is what dropped.
    let drafts = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .find(|m| m.role == Some(MailboxRole::Drafts))
        .unwrap();
    assert_eq!(drafts.total_count, Some(before - 1));
    assert_eq!(drafts.unread_count, None);
}

#[test]
fn sent_unknown_count_stays_unknown() {
    let mut s = state();
    // `sent` carries unread_count None: a flip must never invent a number.
    let row = MessageSummary {
        id: MessageId(String::from("s0")),
        mailbox_id: MailboxId(String::from("sent")),
        message_id: None,
        from: vec![],
        to: vec![],
        subject: String::from("in sent"),
        snippet: None,
        timestamp: chrono::Utc::now().fixed_offset(),
        is_read: false,
        is_starred: false,
        has_attachments: false,
    };
    s.messages.items = vec![row];

    let locator = MessageLocator {
        mailbox: MailboxId(String::from("sent")),
        id: MessageId(String::from("s0")),
        message_id: None,
    };
    let _ = message_moved(&mut s, &[locator]);
    let sent = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .find(|m| m.id == MailboxId(String::from("sent")))
        .unwrap();
    assert_eq!(sent.unread_count, None, "None stays None");
}
