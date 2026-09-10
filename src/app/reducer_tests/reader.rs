//! Reducer tests: reader domain.

use super::*;

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
    assert!(s.session.operations.get(id).is_some());
    // Route stack: reader on top of the mailbox route.
    assert_eq!(s.session.routes.len(), 2);
    assert_eq!(s.open_summary().unwrap().id, selected.id);
    assert_eq!(s.session.focus, Focus::Reader);
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
    assert!(s.session.operations.is_empty());
}

#[test]
fn read_message_load_does_not_trigger_mark_read() {
    let mut s = state();
    s.selection = 3; // m4: read in the mock seed.
    assert!(s.selected_message().unwrap().is_read);
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    complete_message_ok(&mut s, id);
    // No further operations: a read message needs no flag change.
    assert!(s.session.operations.is_empty());
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
    let id = s
        .session
        .operations
        .start(OperationKind::LoadPage(req.clone()))
        .id;
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
    s.caches.page_cache = Some(crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    ));
    // Messages fetched in an earlier session: every Sent row's full
    // message is already on disk.
    for summary in mock::mock_page(&MailboxId(String::from("sent")), 0, 20).items {
        let message = mock::mock_message(&summary);
        s.caches.page_cache.as_ref().unwrap().store_message(
            &summary.mailbox_id,
            &summary.id.0,
            &message,
        );
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
            .all(|m| s.caches.previews.contains_key(&m.id))
    );
    assert!(
        s.messages
            .items
            .iter()
            .all(|m| s.caches.preview_requested.contains(&m.id))
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
    while !s.session.operations.is_empty() {
        let pending = s.session.operations.foreground().unwrap().id;
        if matches!(
            s.session.operations.get(pending).unwrap().kind,
            OperationKind::SetRead { .. }
        ) {
            complete_done(&mut s, pending);
        } else {
            complete_message_ok(&mut s, pending);
        }
    }
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
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
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.session.focus = Focus::Reader;

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
    assert_eq!(s.session.focus, Focus::Reader);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_attachment, None, "no chips: no cursor");
}
