//! Reducer tests: list domain.

use super::*;

#[test]
fn selection_moves_down_up_and_clamps() {
    let mut s = state();
    assert_eq!(s.selection, 0);
    reduce(&mut s, Action::MoveDown);
    assert_eq!(s.selection, 1);
    for _ in 0..100 {
        reduce(&mut s, Action::MoveDown);
    }
    assert_eq!(s.selection, s.messages.items.len() - 1);
    reduce(&mut s, Action::MoveUp);
    assert_eq!(s.selection, s.messages.items.len() - 2);
    for _ in 0..100 {
        reduce(&mut s, Action::MoveUp);
    }
    assert_eq!(s.selection, 0);
}

#[test]
fn page_next_requests_next_page_and_applies_result() {
    let mut s = state();
    s.selection = 3;
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    assert_eq!(req.offset, mock::PAGE_SIZE);
    // The request is registered and in flight for the active mailbox.
    assert_eq!(
        s.session.operations.page_in_flight(&inbox_id()),
        Some(req.clone())
    );
    // The old page and selection stay visible until the result lands.
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.selection, 3);
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    assert_eq!(s.session.operations.page_in_flight(&inbox_id()), None);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
    assert_eq!(s.messages.items.len(), 5);
    // The previous selection's message is not on this page.
    assert_eq!(s.selection, 0);
}

#[test]
fn page_boundaries_never_issue_requests() {
    let mut s = state();
    // No page -1.
    no_effects(&reduce(&mut s, Action::PagePrevious));
    // Advance to page 2 the way the runtime does: request, then result.
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    // No page past the known total (25 items, 2 pages).
    no_effects(&reduce(&mut s, Action::PageNext));
    let (id, req) = expect_page(&reduce(&mut s, Action::PagePrevious));
    assert_eq!(req.offset, 0);
    complete_page_ok(&mut s, id, &req, 0);
    no_effects(&reduce(&mut s, Action::PagePrevious));
}

#[test]
fn unknown_total_short_page_blocks_next_request() {
    let mut s = state();
    // Maildir listings carry no total (ADR 0001 finding 2).
    s.messages.total = None;
    s.messages.items.truncate(5);
    no_effects(&reduce(&mut s, Action::PageNext));
    // A full page may have a successor, so the request is issued.
    s.messages.items = mock::mock_page(&inbox_id(), 0, mock::PAGE_SIZE).items;
    assert_eq!(
        expect_page(&reduce(&mut s, Action::PageNext)).1.offset,
        mock::PAGE_SIZE
    );
}

#[test]
fn rapid_page_next_supersedes_the_older_request() {
    let mut s = state();
    s.messages.total = None;
    let (first_id, first) = expect_page(&reduce(&mut s, Action::PageNext));
    let first_token = s.session.operations.cancellation(first_id).unwrap();
    let (second_id, second) = expect_page(&reduce(&mut s, Action::PageNext));
    assert_eq!(second.offset, first.offset + mock::PAGE_SIZE);
    // Superseding cancelled the older operation and removed it, so its
    // result — however late — can never apply (plan §11).
    assert!(first_token.is_cancelled());
    assert!(s.session.operations.get(first_id).is_none());
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
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
        Action::BackendCompleted(OperationResult {
            id: OperationId(999),
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                40,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    assert_eq!(s.messages, before.messages);
    assert_eq!(s.session.overlay, None);
}

#[test]
fn payload_kind_mismatch_is_ignored() {
    let mut s = state();
    let (id, _req) = expect_page(&reduce(&mut s, Action::PageNext));
    // A page payload for a mailbox operation (defensive: manager routes by
    // kind) must not crash or mutate state.
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mock::mock_mailboxes())),
        }),
    );
    assert_eq!(s.messages.offset, 0);
    assert!(
        s.session.operations.get(id).is_none(),
        "operation completed"
    );
}

#[test]
fn page_result_for_other_mailbox_is_dropped() {
    let mut s = state();
    // An inbox page load is in flight…
    let (inbox_op, inbox_req) = expect_page(&reduce(&mut s, Action::PageNext));
    // …then the user switches to Sent, which starts its own request.
    s.mailbox_selection = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .position(|m| m.id.0 == "sent")
        .unwrap();
    s.session.focus = Focus::Sidebar;
    let effects = reduce(&mut s, Action::Activate);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (sent_op, sent_req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(sent_req.mailbox_id.0, "sent");
    // The slower inbox result arrives after the switch: it must never
    // replace what the Sent view is loading (plan §11).
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
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
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    reduce(&mut s, failure(id, &page_kind(&req), "himalaya exploded"));
    assert_eq!(s.messages.offset, 0, "last coherent page stays");
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    let Some(Overlay::Error(dialog)) = &s.session.overlay else {
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
    assert_eq!(s.session.focus, Focus::ErrorModal);
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Operation failed")
    );
}

#[test]
fn selection_identity_survives_refresh() {
    let mut s = state();
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    let selected: MessageId = s.selected_message().expect("selection").id.clone();
    let (id, req) = expect_page(&reduce(&mut s, Action::Refresh));
    assert_eq!(req.offset, 0);
    assert_eq!(s.session.status.message.as_deref(), Some("Refreshing…"));
    complete_page_ok(&mut s, id, &req, 0);
    assert_eq!(
        s.selected_message().expect("selection after refresh").id,
        selected
    );
}

#[test]
fn refresh_on_second_page_keeps_page_and_selection() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    s.selection = 2;
    let selected = s.selected_message().unwrap().id.clone();
    let (id, req) = expect_page(&reduce(&mut s, Action::Refresh));
    assert_eq!(req.offset, mock::PAGE_SIZE);
    complete_page_ok(&mut s, id, &req, mock::PAGE_SIZE);
    assert_eq!(s.messages.offset, mock::PAGE_SIZE);
    assert_eq!(s.selected_message().unwrap().id, selected);
}

#[test]
fn empty_list_never_panics() {
    let mut s = state();
    switch_to(&mut s, "trash");
    s.messages.items.clear();
    s.messages.total = Some(0);
    s.selection = 0;
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveUp);
    no_effects(&reduce(&mut s, Action::PageNext));
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
    assert_eq!(s.session.focus, Focus::MessageList);
}

#[test]
fn activate_on_same_mailbox_is_noop() {
    let mut s = state();
    s.selection = 5;
    s.session.focus = Focus::Sidebar;
    no_effects(&reduce(&mut s, Action::Activate));
    assert_eq!(s.selection, 5);
    assert_eq!(s.messages.offset, 0);
}

#[test]
fn sidebar_active_mailbox_marks_drafts_while_composing() {
    let mut s = state();
    let active = |s: &AppState| {
        s.sidebar_active_mailbox_id()
            .map(|id| id.0.as_str())
            .map(String::from)
    };
    assert_eq!(
        active(&s).as_deref(),
        Some("inbox"),
        "the displayed mailbox"
    );
    compose(&mut s);
    assert_eq!(
        active(&s).as_deref(),
        Some("drafts"),
        "Drafts while composing"
    );
    // Leaving the composer restores the displayed mailbox.
    reduce(&mut s, Action::BackOrCancel);
    assert_eq!(active(&s).as_deref(), Some("inbox"));
}

#[test]
fn sidebar_active_mailbox_is_none_while_composing_without_drafts() {
    // A backend with no resolved Drafts role (ADR 0001: the UI never
    // guesses folder names): composing marks no folder active.
    let mut s = state();
    s.mailboxes = Loadable::Loaded(
        mock::mock_mailboxes()
            .into_iter()
            .filter(|m| m.role != Some(MailboxRole::Drafts))
            .collect(),
    );
    compose(&mut s);
    assert_eq!(s.sidebar_active_mailbox_id(), None);
}

#[test]
fn refresh_before_mailboxes_load_starts_the_listing() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, kind) = boot(&mut s);
    assert_eq!(kind, mailboxes_kind());
    assert!(s.session.operations.get(id).is_some());
    // A second refresh while loading must not stack a duplicate request.
    no_effects(&reduce(&mut s, Action::Refresh));
    assert_eq!(s.session.operations.len(), 1);
}

#[test]
fn mailboxes_loaded_selects_inbox_role() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    assert_eq!(s.session.routes.len(), 0);
    let (id, kind) = boot(&mut s);
    assert_eq!(kind, mailboxes_kind());
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mock::mock_mailboxes())),
        }),
    );
    let effects = reduce(&mut s, Action::Refresh);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (_, req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(req.mailbox_id.0, "inbox");
    assert_eq!(req.offset, 0);
    assert_eq!(s.session.routes.len(), 1);
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
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(mailboxes)),
        }),
    );
    let effects = reduce(&mut s, Action::Refresh);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (_, req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(req.mailbox_id.0, "notes");
    assert_eq!(s.mailbox_selection, 0);
}

#[test]
fn mailboxes_loaded_empty_is_valid_not_an_error() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, _) = boot(&mut s);
    let effects = reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Mailboxes(Vec::new())),
        }),
    );
    no_effects_except_cache_stores(&mut s, &effects);
    assert_eq!(s.session.routes.len(), 0);
    assert!(s.messages.items.is_empty());
    assert!(!s.session.quit_requested);
}

#[test]
fn mailboxes_failure_keeps_a_failed_sidebar_and_refresh_reloads() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, kind) = boot(&mut s);
    reduce(&mut s, failure(id, &kind, "no such account"));
    // The startup listing is background work: the failure renders the
    // dim sidebar note without a modal interrupt.
    assert!(matches!(s.mailboxes, Loadable::Failed(_)));
    assert!(s.session.overlay.is_none());
    // `Ctrl+R` replays the typed mailbox-load intent under a new id.
    let effects = reduce(&mut s, Action::Refresh);
    let (cache_id, _) = effect_parts(&effects);
    let (retry_id, retry_kind) = effect_parts(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(retry_kind, mailboxes_kind());
    assert_ne!(retry_id, id, "refresh gets a fresh operation id");
    assert!(s.session.operations.get(retry_id).is_some());
}

#[test]
fn fresh_mailbox_listing_keeps_the_composer_open() {
    // The reported bug (ticket sazy): a cached listing roots the UI at
    // startup, the user presses `c` and types, then the fresh listing
    // lands — the composer route used to be rebuilt away with the list.
    let mut s = state();
    let id = start_listing(&mut s);
    compose(&mut s);
    let body_before = s.session.composer.as_ref().unwrap().body.lines().join("\n");
    complete_mailboxes(&mut s, id, mock::mock_mailboxes());
    // Still composing, draft intact; the sidebar data refreshed in place.
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(
        s.session.composer.as_ref().unwrap().body.lines().join("\n"),
        body_before
    );
    assert!(s.mailboxes.as_loaded().is_some());
}

#[test]
fn fresh_mailbox_listing_keeps_list_page_and_selection() {
    let mut s = state();
    let id = start_listing(&mut s);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    let selected = s.selected_message().unwrap().id.clone();
    complete_mailboxes(&mut s, id, mock::mock_mailboxes());
    // Cursor, page, and scroll are exactly where the user left them.
    assert_eq!(s.selection, 2);
    assert_eq!(s.selected_message().unwrap().id, selected);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.session.routes.len(), 1);
}

#[test]
fn fresh_mailbox_listing_keeps_sidebar_cursor_by_identity() {
    let mut s = state();
    let id = start_listing(&mut s);
    // The cursor sits on Sent; a fresh enumeration lists folders in a
    // different order. The cursor follows the mailbox, not the index.
    s.mailbox_selection = 1;
    let mut reordered = mock::mock_mailboxes();
    reordered.rotate_left(2);
    complete_mailboxes(&mut s, id, reordered);
    let expected = s
        .mailboxes
        .as_loaded()
        .unwrap()
        .iter()
        .position(|m| m.id.0 == "sent")
        .unwrap();
    assert_eq!(s.mailbox_selection, expected);
}

#[test]
fn page_result_applies_behind_the_composer() {
    // A page load finishing while the user composes updates the list
    // behind the overlay — the composer route and selection survive.
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, Action::Refresh));
    // The user moves and starts composing while the load runs; the
    // displayed page is also made stale so the result really applies.
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    let selected = s.selected_message().unwrap().id.clone();
    s.messages.items.truncate(15);
    compose(&mut s);
    complete_page_ok(&mut s, id, &req, 0);
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert_eq!(s.selected_message().unwrap().id, selected);
}

#[test]
fn mailbox_page_result_never_lands_over_search_results() {
    // A search owns the visible list: a slower mailbox page load that was
    // started before the search must not replace its results.
    let mut s = state();
    let (id, _req) = expect_page(&reduce(&mut s, Action::Refresh));
    reduce(&mut s, Action::OpenSearch);
    reduce(&mut s, Action::SearchEdit(SearchEdit::Char('x')));
    let effects = reduce(&mut s, Action::SubmitSearch);
    assert!(matches!(
        effects.first().map(|e| &e.kind),
        Some(OperationKind::Search(_))
    ));
    assert!(s.messages.items.is_empty());
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(mock::mock_page(
                &inbox_id(),
                0,
                mock::PAGE_SIZE,
            ))),
        }),
    );
    assert!(
        s.messages.items.is_empty(),
        "mailbox page must not land over search results"
    );
}

#[test]
fn identical_page_result_changes_nothing() {
    // Ticket sazy: a refresh that returns the very page already on screen
    // is a no-op — the selection and scroll stay untouched.
    let mut s = state();
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    let (id, req) = expect_page(&reduce(&mut s, Action::Refresh));
    let effects = complete_page_ok(&mut s, id, &req, 0);
    no_effects_except_cache_stores(&mut s, &effects);
    assert_eq!(s.selection, 3);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
}

#[test]
fn input_and_ticks_keep_working_while_an_operation_is_in_flight() {
    let mut s = state();
    let _ = expect_page(&reduce(&mut s, Action::PageNext));
    // Foreground work never blocks rendering or input (plan §3): movement
    // and ticks apply while the request is in flight.
    tick(&mut s, 0);
    assert!(s.session.clock.is_some());
    reduce(&mut s, Action::MoveDown);
    assert_eq!(s.selection, 1);
    assert!(s.session.operations.page_in_flight(&inbox_id()).is_some());
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
    let (id, _) = expect_page(&reduce(&mut s, Action::Refresh));
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
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
    let (id2, _) = expect_page(&reduce(&mut s, Action::Refresh));
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
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

// ── Phase 10.3: resize robustness (never panics, never invalid) ──────────

#[test]
fn resize_to_degenerate_sizes_never_panics_or_invalidates() {
    let mut s = state();
    s.selection = 5;
    for size in [(0, 0), (1, 1), (89, 19)] {
        reduce(
            &mut s,
            Action::Resize {
                width: size.0,
                height: size.1,
            },
        );
        assert_eq!(s.session.size, size);
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
        Action::Resize {
            width: 60,
            height: 15,
        },
    );
    assert_eq!(s.selection, 0);
    assert_eq!(s.list_scroll, 0);
    // Moving on an empty page stays a no-op.
    no_effects(&reduce(&mut s, Action::MoveDown));
    assert_eq!(s.selection, 0);
}

#[test]
fn resize_while_a_modal_is_open_keeps_state_coherent() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    reduce(&mut s, failure(id, &page_kind(&req), "imap down"));
    assert!(matches!(s.session.overlay, Some(Overlay::Error(_))));
    if let Some(Overlay::Error(dialog)) = s.session.overlay.as_mut() {
        dialog.scroll = 50; // far past the end at any size
    }
    reduce(
        &mut s,
        Action::Resize {
            width: 60,
            height: 15,
        },
    );
    // The modal scroll only clamps on the next Move/scroll action, and the
    // clamp uses the new size — never panics, never negative.
    reduce(&mut s, Action::MoveDown);
    if let Some(Overlay::Error(dialog)) = &s.session.overlay {
        let max = crate::view::overlay::error_modal_max_scroll(
            &dialog.detail,
            dialog.code,
            dialog.ambiguous,
            s.session.size,
        );
        assert!(dialog.scroll <= max);
    }
}

#[test]
fn resize_never_pushes_the_attachment_cursor_out_of_range() {
    let mut s = reader_with_attachments();
    s.reader_focus = Some(ReaderFocus::Attachment(1));
    for size in [(0, 0), (60, 15), (90, 25), (152, 40)] {
        reduce(
            &mut s,
            Action::Resize {
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
        let focused = match s.reader_focus {
            Some(ReaderFocus::Attachment(index)) => index,
            _ => 0,
        };
        assert!(focused < count.max(1));
    }
}

// ── Auto page size (ticket kjfq) ─────────────────────────────────────────

#[test]
fn auto_page_size_tracks_the_visible_rows_on_resize() {
    let mut s = state();
    s.settings.page_size_auto = true;
    // Shrink the terminal: the limit becomes however many rows fit the
    // list, and the visible page reloads in the background (ticket kjfq).
    reduce(
        &mut s,
        Action::Resize {
            width: 152,
            height: 20,
        },
    );
    let rows_20 = crate::view::layout::messages_visible((152, 20), ViewMode::Compact).max(1);
    assert_eq!(s.messages.limit, rows_20, "limit matches the visible rows");
    // Growing further changes the limit again and re-loads (background).
    reduce(
        &mut s,
        Action::Resize {
            width: 152,
            height: 24,
        },
    );
    let rows_24 = crate::view::layout::messages_visible((152, 24), ViewMode::Compact).max(1);
    assert_ne!(rows_20, rows_24);
    let effects = reduce(
        &mut s,
        Action::Resize {
            width: 152,
            height: 28,
        },
    );
    let (id, req) = expect_page(&effects);
    assert_eq!(
        req.limit,
        crate::view::layout::messages_visible((152, 28), ViewMode::Compact).max(1)
    );
    assert_eq!(
        s.session.operations.get(id).map(|op| op.origin),
        Some(OperationOrigin::Background),
        "silent reload"
    );
}

#[test]
fn comfortable_view_mode_halves_the_auto_page_size() {
    // `[tmail].view_mode = "comfortable"` doubles the line cost of every
    // message, so an auto-sized page holds half as many (and resize keeps
    // tracking it).
    let mut s = state();
    s.settings.page_size_auto = true;
    s.settings.view_mode = ViewMode::Comfortable;
    reduce(
        &mut s,
        Action::Resize {
            width: 152,
            height: 40,
        },
    );
    let compact = crate::view::layout::messages_visible((152, 40), ViewMode::Compact);
    let comfortable = crate::view::layout::messages_visible((152, 40), ViewMode::Comfortable);
    assert_eq!(compact, 31);
    assert_eq!(comfortable, 15);
    assert_eq!(s.messages.limit, comfortable);
}

#[test]
fn manual_page_size_is_untouched_by_resize() {
    let mut s = state();
    s.settings.page_size_auto = false;
    reduce(
        &mut s,
        Action::Resize {
            width: 90,
            height: 20,
        },
    );
    assert_eq!(s.messages.limit, mock::PAGE_SIZE, "page_size rules");
    assert!(
        s.session.operations.is_empty(),
        "no reload without auto sizing"
    );
}
