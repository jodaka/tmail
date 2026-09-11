//! Reducer tests: cache domain.

use super::*;

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
    s.caches.page_cache = Some(cache);

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
        let id = s
            .session
            .operations
            .start(OperationKind::LoadPage(req.clone()))
            .id;
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
        s.caches.preview_requested.len(),
        expected.items.len(),
        "one preview request per row overall"
    );
    for (locator, summary) in previews.iter().zip(expected.items.iter().skip(1)) {
        assert_eq!(&locator.id, &summary.id);
        assert_eq!(&locator.mailbox, &summary.mailbox_id);
        assert!(s.caches.preview_requested.contains(&summary.id), "deduped");
    }
    assert_eq!(s.messages.items.len(), expected.items.len());
    // The successful load overwrote the cache entry.
    let cached = s
        .caches
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
    s.caches.page_cache = Some(cache);

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
    s.caches.page_cache = Some(cache);
    s.selection = 0;
    let summary = s.selected_message().unwrap().clone();
    // A previously viewed copy of this exact message.
    let seen = crate::app::mock::mock_message(&summary);
    s.caches
        .page_cache
        .as_ref()
        .unwrap()
        .store_message(&summary.mailbox_id, &summary.id.0, &seen);

    let effects = reduce(&mut s, &Action::Activate);
    // The cached body renders immediately…
    assert!(matches!(
        s.open_message,
        crate::app::state::Loadable::Loaded(_)
    ));
    // …and a silent, uncancellable background convergence fetch runs (it
    // reconciles read state and remote drift; it never takes the loader
    // slot, so Esc cannot cancel it into "cancelled" noise).
    let convergence = effects
        .iter()
        .find(|e| matches!(e.kind, OperationKind::LoadMessage(_)))
        .expect("the convergence fetch still runs");
    let op = s.session.operations.get(convergence.id).expect("in flight");
    assert_eq!(op.origin, OperationOrigin::Background);
    assert_ne!(
        s.session.operations.foreground().map(|o| o.id),
        Some(convergence.id)
    );
}

#[test]
fn preview_result_fills_the_list_snippet_and_caches_the_message() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    s.caches.page_cache = Some(crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    ));
    let effects = load_sent_without_snippets(&mut s);
    let previews = expect_previews(&effects);
    assert_eq!(previews.len(), 4, "one preview per Sent row");

    let (id, locator) = previews[0].clone();
    let op = s.session.operations.get(id).expect("preview in flight");
    // Preview fetches are silent background work: they never take the
    // `Esc`-cancel / spinner slot.
    assert_eq!(op.origin, OperationOrigin::Background);
    assert_ne!(s.session.operations.foreground().map(|o| o.id), Some(id));

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
        s.caches
            .page_cache
            .as_ref()
            .unwrap()
            .load_message(&locator.mailbox, &locator.id.0)
            .is_some(),
        "preview fetch caches the message"
    );
}

/// The envelope flag lies on IMAP accounts (no body structure in the
/// listing): the fetched full message reconciles the row, so the list
/// renders the paperclip for messages with attachments (ticket r84f).
#[test]
fn preview_fetch_reconciles_the_row_attachment_flag() {
    let mut s = state();
    let effects = load_sent_without_snippets(&mut s);
    let (id, locator) = expect_previews(&effects)[0].clone();
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .expect("row listed");
    assert!(!row.has_attachments, "the envelope carried no flag");

    let summary = row.clone();
    let mut message = mock::mock_message(&summary);
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("a.pdf")),
        mime_type: None,
        size: None,
        part_id: 2,
    }];
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    );
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .unwrap();
    assert!(
        row.has_attachments,
        "the fetched message reconciles the flag"
    );
    assert!(row.snippet.is_some(), "the preview still fills");

    // A background page refresh re-applies envelope rows — whose flag is
    // absent (IMAP) — and must not wipe the reconciled flag: the
    // paperclip survives (ticket r84f).
    let req = crate::domain::PageRequest {
        mailbox_id: MailboxId(String::from("sent")),
        offset: 0,
        limit: 20,
    };
    let id = s
        .session
        .operations
        .start(OperationKind::LoadPage(req.clone()))
        .id;
    complete_page_ok(&mut s, id, &req, 0);
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .unwrap();
    assert!(row.has_attachments, "the refresh keeps the reconciled flag");
}

/// The disk-cache preview branch reconciles the flag too: a message
/// fetched in an earlier session flips the row without any new fetch.
#[test]
fn cached_copies_reconcile_the_row_attachment_flag_without_a_fetch() {
    let mut s = state();
    let dir = tempfile::TempDir::new().expect("tempdir");
    s.caches.page_cache = Some(crate::app::page_cache::PageCache::open(
        dir.path().to_path_buf(),
        crate::app::page_cache::CacheLimits::default(),
    ));
    let sent_page = mock::mock_page(&MailboxId(String::from("sent")), 0, 20);
    let first = sent_page.items[0].clone();
    let mut message = mock::mock_message(&first);
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("a.pdf")),
        mime_type: None,
        size: None,
        part_id: 2,
    }];
    s.caches
        .page_cache
        .as_ref()
        .unwrap()
        .store_message(&first.mailbox_id, &first.id.0, &message);

    reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(&mut s, &Action::Click(ClickTarget::Mailbox(1))); // activate
    let (load, req) = expect_page(&effects);
    let effects = complete_page_ok(&mut s, load, &req, 0);
    // The other rows still fetch previews; the cached row does not.
    assert!(
        expect_previews(&effects)
            .iter()
            .all(|(_, l)| l.id != first.id),
        "the cached row needs no fetch"
    );
    let row = s.messages.items.iter().find(|m| m.id == first.id).unwrap();
    assert!(
        row.has_attachments,
        "the cached copy reconciles the flag with no fetch"
    );
    assert!(row.snippet.is_some());
}

/// Opening a message (the reader path) reconciles the row the same way.
#[test]
fn opening_a_message_reconciles_the_row_attachment_flag() {
    let mut s = state();
    let summary = s.messages.items[0].clone();
    assert!(!summary.has_attachments, "the envelope carried no flag");
    let (id, _) = expect_kind(&reduce(&mut s, &Action::Activate));
    let mut message = mock::mock_message(&summary);
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("a.pdf")),
        mime_type: None,
        size: None,
        part_id: 2,
    }];
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    );
    assert!(
        s.messages.items[0].has_attachments,
        "the opened message reconciles the flag"
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
    assert!(s.session.overlay.is_none());
    assert!(s.session.status.message.is_none());
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
    let id = s
        .session
        .operations
        .start(OperationKind::LoadPage(req.clone()))
        .id;
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
    assert_eq!(s.session.routes.len(), 1, "reader closed");
    assert!(
        previews
            .iter()
            .all(|(id, _)| s.session.operations.get(*id).is_some()),
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
    let id = s.session.operations.start(OperationKind::LoadPage(req)).id;
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
    assert_eq!(
        s.caches.preview_requested.len(),
        7,
        "queued rows are tracked"
    );
}
