//! Reducer tests: reader domain.

use super::*;

#[test]
fn activate_on_message_list_opens_the_reader() {
    let mut s = state();
    s.selection = 1;
    let selected = s.selected_message().unwrap().clone();
    let (id, kind) = open_reader(&mut s);
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
    let (id, _) = open_reader(&mut s);
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
    let (id, _) = open_reader(&mut s);
    complete_message_ok(&mut s, id);
    // No further operations: a read message needs no flag change.
    assert!(s.session.operations.is_empty());
}

#[test]
fn reader_result_fills_missing_snippet() {
    let mut s = state();
    s.messages.items[2].snippet = None;
    s.selection = 2;
    let (id, _) = open_reader(&mut s);
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
    // Messages fetched in an earlier session: the manager's cache holds
    // every Sent row's full message. The reducer only sees the reads'
    // results, so the test plays the manager and answers each
    // `CachePreviewLoad` from this set.
    let cached: std::collections::HashMap<MessageId, crate::domain::Message> =
        mock::mock_page(&MailboxId(String::from("sent")), 0, 20)
            .items
            .into_iter()
            .map(|summary| {
                let message = mock::mock_message(&summary);
                (summary.id.clone(), message)
            })
            .collect();
    // Switch to Sent: the fresh page loads (the cache read misses — no
    // cached page), and every preview is served from the cached copies —
    // no background fetches start, nothing re-requests what was fetched
    // before (ticket wxtx).
    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // activate
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let effects = complete_cache_miss(&mut s, cache_id);
    let (load, req) = expect_page(&effects);
    let effects = complete_page_ok(&mut s, load, &req, 0);
    let reads = expect_cache_preview_reads(&effects);
    assert_eq!(reads.len(), 4, "one cache read per Sent row");
    let mut followups = Vec::new();
    for (id, locator) in reads {
        let message = cached
            .get(&locator.id)
            .expect("the earlier session fetched this message")
            .clone();
        followups.extend(complete_cache_message(&mut s, id, message));
    }
    no_effects(&followups);
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
    let (id, _) = open_reader(&mut s);
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

    assert_eq!(s.reader_focus, None, "nothing focused before Tab");
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Attachment(0)));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Attachment(1)));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(
        s.reader_focus,
        Some(ReaderFocus::Attachment(0)),
        "wraps forward"
    );
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(
        s.reader_focus,
        Some(ReaderFocus::Attachment(1)),
        "wraps backward"
    );
    // Closing the reader resets the cursor.
    reduce(&mut s, &Action::BackOrCancel);
    assert_eq!(s.reader_focus, None);
}

#[test]
fn tab_is_inert_without_focusable_items() {
    let mut s = state();
    let (id, _) = open_reader(&mut s);
    complete_message_ok(&mut s, id);
    assert_eq!(s.session.focus, Focus::Reader);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, None, "no links, no chips: no cursor");
}

/// Tab walks the body's links first, then the attachment chips, wrapping
/// at both ends (tickets 1fnh/hc9n).
#[test]
fn tab_cycles_links_then_attachments() {
    let mut s = reader_with_links_and_attachment();
    assert_eq!(s.reader_focus, None);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(0)));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(1)));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Attachment(0)));
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(0)), "wraps forward");
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(
        s.reader_focus,
        Some(ReaderFocus::Attachment(0)),
        "wraps backward"
    );
    // Shift+Tab from nothing starts at the last item.
    s.reader_focus = None;
    reduce(&mut s, &Action::FocusPrevious);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Attachment(0)));
}

/// Enter on a focused link starts the platform opener for its target
/// (ticket hc9n); the completion reports success like any other open.
#[test]
fn enter_on_a_focused_link_opens_the_url() {
    let mut s = reader_with_links_and_attachment();
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(1)));
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    let OperationKind::OpenUrl { url } = kind else {
        panic!("expected OpenUrl, got {kind:?}");
    };
    assert_eq!(url, "https://two.example/b");
    assert_eq!(s.session.status.message.as_deref(), Some("Opening link…"));
    let effects = complete_done(&mut s, id);
    no_effects(&effects);
    assert_eq!(s.session.status.message.as_deref(), Some("Opened link"));
}

/// Tabbing to a link below the fold scrolls it into view (tickets
/// hc9n/1fnh): focus that cannot be seen is not focus.
#[test]
fn tab_focus_scrolls_the_focused_link_into_view() {
    let mut s = state();
    let summary = s.messages.items[1].clone();
    let mut message = mock::mock_message(&summary);
    message.plain_body = None;
    let mut html = String::new();
    for index in 0..80 {
        html.push_str(&format!("<p>filler paragraph {index}</p>"));
    }
    html.push_str("<p><a href=\"https://deep.example/x\">deep link</a></p>");
    message.html_body = Some(html);
    message.attachments = Vec::new();
    s.session
        .routes
        .push(Route::Message(crate::app::route::MessageRoute {
            mailbox_id: summary.mailbox_id.clone(),
            summary,
        }));
    s.open_message = Loadable::Loaded(message);
    s.session.focus = Focus::Reader;
    s.session.size = (80, 25);

    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(0)));
    let width = crate::view::layout::reader_width(s.session.size).max(10);
    let line = crate::app::reader::focus_line(&s, width, ReaderFocus::Link(0)).expect("link line");
    let viewport = crate::view::layout::reader_rows_visible(s.session.size)
        .saturating_sub(crate::app::reader::header_line_count(&s, width));
    assert!(s.reader_scroll > 0, "the deep link needs a scroll");
    assert!(
        line >= s.reader_scroll && line < s.reader_scroll + viewport.max(1),
        "focused line {line} outside viewport {}..{}",
        s.reader_scroll,
        s.reader_scroll + viewport.max(1)
    );
}

/// Non-web schemes never reach the opener: a status note explains the
/// refusal and no operation starts (ticket hc9n).
#[test]
fn enter_on_a_non_web_link_is_refused() {
    let mut s = reader_with_non_web_link();
    reduce(&mut s, &Action::FocusNext);
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(0)));
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(
        s.session
            .status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("Cannot open link") && m.contains("file:///etc/passwd")),
        "status: {:?}",
        s.session.status.message
    );
    assert!(s.session.operations.is_empty());
}

/// The attachment keys keep their v1 target with a link focused or with
/// nothing focused: the first chip (ticket 61qx behavior preserved).
#[test]
fn attachment_actions_fall_back_to_the_first_chip() {
    let mut s = reader_with_links_and_attachment();
    reduce(&mut s, &Action::FocusNext); // Link(0)
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(0)));
    let effects = reduce(&mut s, &Action::SaveAttachment);
    let (_, kind) = effect_parts(&effects);
    let OperationKind::SaveAttachment { request, .. } = kind else {
        panic!("expected SaveAttachment, got {kind:?}");
    };
    assert_eq!(request.part_id, 3, "the first chip is the default target");
}
