//! Reducer tests: cache domain.
//!
//! The on-disk cache lives in the operation manager (ticket haeb, off
//! thread I/O): the reducer emits `Cache*` effects and applies `Cached*`
//! results, so these tests simulate the manager's responses instead of
//! touching disk. The cache's own disk behavior is unit-tested in
//! `page_cache.rs`; the manager's cache arms in `runtime::tasks`.

use super::*;

// ── Summary cache (ticket haeb) ──────────────────────────────────────────

#[test]
fn cached_page_serves_instantly_and_the_fresh_load_still_runs() {
    let mut s = state();
    // A cached page for the Sent mailbox (offset 0, limit 20).
    let cached = crate::domain::Page {
        items: vec![
            mock::mock_page(&MailboxId(String::from("sent")), 0, 1)
                .items
                .remove(0),
        ],
        offset: 0,
        limit: 20,
        total: None,
    };

    // Switch to Sent: the cold context starts a cache read…
    reduce(&mut s, Action::Click(ClickTarget::Mailbox(1)));
    let effects = reduce(&mut s, Action::Click(ClickTarget::Mailbox(1)));
    let (cache_id, mailbox, query, offset, limit, fresh_background) =
        expect_cache_list_load(&effects);
    assert_eq!(mailbox.0, "sent");
    assert_eq!(query, None);
    assert_eq!((offset, limit), (0, 20));
    assert!(fresh_background, "a cold-start switch is consequence work");

    // …whose hit renders the rows immediately…
    let effects = complete_cache_page(&mut s, cache_id, cached.clone());
    assert!(
        !s.messages.items.is_empty(),
        "cached rows visible without waiting for the backend"
    );
    assert_eq!(s.messages.items[0].mailbox_id.0, "sent");
    // …and starts the fresh load in the background (Esc can never cancel
    // it into "cancelled" noise).
    let (fresh_id, req) = find_page(&effects);
    assert_eq!(req.mailbox_id.0, "sent");
    let op = s.session.operations.get(fresh_id).expect("in flight");
    assert_eq!(op.origin, OperationOrigin::Background);
    // The cached row's preview is read from the cache, too.
    let reads = expect_cache_preview_reads(&effects);
    assert_eq!(reads.len(), 1, "the cached page's single row is read");
    let first = s.messages.items[0].clone();
    no_effects(&complete_cache_message(
        &mut s,
        reads[0].0,
        mock::mock_message(&first),
    ));

    // Completing the fresh load replaces the page…
    let expected = mock::mock_page(&MailboxId(String::from("sent")), 0, 20);
    let effects = reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id: fresh_id,
            outcome: Ok(OperationOutcome::Page(expected.clone())),
        }),
    );
    // No further page work — but the rows carry no snippets, so the
    // background preview reads start (ticket wxtx)…
    assert!(
        !effects.iter().any(|e| matches!(
            e.kind,
            OperationKind::LoadPage(_) | OperationKind::Search(_)
        )),
        "no page work may follow a completed load, got {effects:?}"
    );
    let reads = expect_cache_preview_reads(&effects);
    assert_eq!(
        reads.len(),
        expected.items.len() - 1,
        "the remaining rows are read after the cached row's request: {effects:?}"
    );
    assert!(
        reads.iter().all(|(_, locator)| locator.id != first.id),
        "the already-served row is not re-read"
    );
    // …and the successful load refreshes the cache — as an effect.
    assert!(
        effects.iter().any(
            |e| matches!(&e.kind, OperationKind::CacheListStore { mailbox, query, page }
            if mailbox.0 == "sent" && query.is_none()
                && page.items.len() == expected.items.len())
        ),
        "the fresh load stores the page, got {effects:?}"
    );
}

#[test]
fn a_cache_miss_loads_the_page_in_the_foreground() {
    let mut s = state();
    reduce(&mut s, Action::Click(ClickTarget::Mailbox(1)));
    let effects = reduce(&mut s, Action::Click(ClickTarget::Mailbox(1)));
    let (cache_id, ..) = expect_cache_list_load(&effects);
    // The miss falls through to the ordinary foreground load.
    let effects = complete_cache_miss(&mut s, cache_id);
    let (load_id, req) = expect_page(&effects);
    assert_eq!(req.mailbox_id.0, "sent");
    let op = s.session.operations.get(load_id).expect("in flight");
    assert_eq!(op.origin, OperationOrigin::Foreground);
}

#[test]
fn cold_start_serves_cached_mailboxes_and_first_page_instantly() {
    let mut s = state();
    // The cached world: a mailbox listing and its first page.
    let mailboxes = mock::mock_mailboxes();
    let page = mock::mock_page(&inbox_id(), 0, mock::PAGE_SIZE);

    // Cold start: mailboxes are not loaded yet.
    s.mailboxes = crate::app::state::Loadable::Loading;
    // Startup Refresh: the cache read runs first…
    let effects = reduce(&mut s, Action::Refresh);
    let (mailboxes_id, _) = effect_parts(&effects);
    let effects = reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id: mailboxes_id,
            outcome: Ok(OperationOutcome::CachedMailboxes(mailboxes.clone())),
        }),
    );
    assert!(matches!(
        s.mailboxes,
        crate::app::state::Loadable::Loaded(_)
    ));
    // …the hit roots the route stack, starts the first page's cache read
    // and the fresh background listing.
    let (page_cache_id, ..) = expect_cache_list_load(&effects);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e.kind, OperationKind::LoadMailboxes)),
        "the fresh mailbox listing still loads"
    );
    // The cached first page is visible without waiting…
    let effects = complete_cache_page(&mut s, page_cache_id, page);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    // …and the fresh first page still loads (background: consequence work).
    let (fresh_id, req) = find_page(&effects);
    assert_eq!(req.mailbox_id, inbox_id());
    let op = s.session.operations.get(fresh_id).expect("in flight");
    assert_eq!(op.origin, OperationOrigin::Background);
}

#[test]
fn opening_a_message_serves_the_cached_copy_instantly() {
    let mut s = state();
    s.selection = 0;
    let summary = s.selected_message().unwrap().clone();

    let effects = reduce(&mut s, Action::Activate);
    // The read runs off-thread; the reader shows the spinner until it
    // lands.
    assert!(matches!(
        s.open_message,
        crate::app::state::Loadable::Loading
    ));
    let (cache_id, _) = effect_parts(&effects);
    // The hit renders the body immediately…
    let effects = complete_cache_message(&mut s, cache_id, mock::mock_message(&summary));
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
fn opening_a_message_without_a_cache_hit_loads_in_the_foreground() {
    let mut s = state();
    s.selection = 0;
    let effects = reduce(&mut s, Action::Activate);
    let (cache_id, _) = effect_parts(&effects);
    let effects = complete_cache_miss(&mut s, cache_id);
    // The spinner stays up and the fresh load is foreground work (the
    // user is waiting for this message).
    assert!(matches!(
        s.open_message,
        crate::app::state::Loadable::Loading
    ));
    let (load_id, _) = expect_kind(&effects);
    let op = s.session.operations.get(load_id).expect("in flight");
    assert_eq!(op.origin, OperationOrigin::Foreground);
}

#[test]
fn preview_result_fills_the_list_snippet_and_caches_the_message() {
    let mut s = state();
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
    // Raw completion (not `complete_preview_ok`): the store effect is
    // exactly what this test pins.
    let effects = reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(mock::mock_message(
                &summary,
            )))),
        }),
    );
    // The row now carries its one-line body preview…
    let row = s
        .messages
        .items
        .iter()
        .find(|m| m.id == locator.id)
        .unwrap();
    let snippet = row.snippet.as_deref().expect("snippet filled");
    assert!(snippet.starts_with("body line 01"), "{snippet}");
    // …and the full message is cached — as an effect (the manager owns
    // the disk).
    assert!(
        effects.iter().any(
            |e| matches!(&e.kind, OperationKind::CacheMessageStore { mailbox, id, .. }
            if mailbox == &locator.mailbox && id == &locator.id.0)
        ),
        "the preview fetch stores the message, got {effects:?}"
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
        Action::BackendCompleted(OperationResult {
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
    let sent_page = mock::mock_page(&MailboxId(String::from("sent")), 0, 20);
    let first = sent_page.items[0].clone();
    let mut message = mock::mock_message(&first);
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("a.pdf")),
        mime_type: None,
        size: None,
        part_id: 2,
    }];

    reduce(&mut s, Action::Click(ClickTarget::Mailbox(1))); // select
    let effects = reduce(&mut s, Action::Click(ClickTarget::Mailbox(1))); // activate
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let effects = complete_cache_page(&mut s, cache_id, sent_page);
    // Every row is cache-read (no snippet yet); the fresh load started.
    let reads = expect_cache_preview_reads(&effects);
    let (first_read, _) = reads
        .iter()
        .find(|(_, locator)| locator.id == first.id)
        .expect("the cached row is read too");
    // The hit serves the preview and reconciles the flag with no fetch.
    let effects = complete_cache_message(&mut s, *first_read, message);
    assert!(
        expect_previews(&effects)
            .iter()
            .all(|(_, locator)| locator.id != first.id),
        "the cached row needs no fetch"
    );
    let row = s.messages.items.iter().find(|m| m.id == first.id).unwrap();
    assert!(
        row.has_attachments,
        "the cached copy reconciles the flag with no fetch"
    );
    assert!(row.snippet.is_some());
}

/// Opening a message (the reader path) reconciles the row the same way:
/// the cache hit renders instantly and the background convergence fetch
/// applies the message (snippet, flag, read state).
#[test]
fn opening_a_message_reconciles_the_row_attachment_flag() {
    let mut s = state();
    let summary = s.messages.items[0].clone();
    assert!(!summary.has_attachments, "the envelope carried no flag");
    let (cache_id, _) = expect_kind(&reduce(&mut s, Action::Activate));
    let effects = complete_cache_message(&mut s, cache_id, mock::mock_message(&summary));
    // The hit starts the silent convergence fetch…
    let (convergence_id, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::LoadMessage(_)));
    // …whose result reconciles the row.
    let mut message = mock::mock_message(&summary);
    message.attachments = vec![crate::domain::Attachment {
        name: Some(String::from("a.pdf")),
        mime_type: None,
        size: None,
        part_id: 2,
    }];
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id: convergence_id,
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
        failure(
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
            .all(|(_, candidate)| candidate.id != locator.id),
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
    let (cache_id, _) = expect_kind(&reduce(&mut s, Action::Activate));
    let summary = s.open_summary().expect("reader open").clone();
    complete_cache_message(&mut s, cache_id, mock::mock_message(&summary));
    // Esc closes the reader — the previews are not foreground work for it
    // to absorb.
    reduce(&mut s, Action::BackOrCancel);
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
    find_page(&effects);
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
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Page(page.clone())),
        }),
    );
    // Every row gets a cache read first (ticket haeb); none is cached in
    // the fixture, so each miss may start a fetch within the window.
    let reads: Vec<OperationId> = expect_cache_preview_reads(&effects)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(reads.len(), 10, "one cache read per row");
    let mut previews = Vec::new();
    for read_id in reads {
        for effect in complete_cache_miss(&mut s, read_id) {
            if let OperationKind::Preview(locator) = &effect.kind {
                previews.push((effect.id, locator.clone()));
            }
        }
    }
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
    // The refill re-reads the still-unrequested rows; the first miss
    // within the freed window slot starts the next fetch.
    let refill_reads: Vec<OperationId> = expect_cache_preview_reads(&effects)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!refill_reads.is_empty(), "queued rows are re-read");
    let mut refill = Vec::new();
    for read_id in refill_reads {
        for effect in complete_cache_miss(&mut s, read_id) {
            if let OperationKind::Preview(locator) = &effect.kind {
                refill.push((effect.id, locator.clone()));
            }
        }
    }
    assert_eq!(refill.len(), 1, "the next queued row starts");
    assert_eq!(
        s.caches.preview_requested.len(),
        7,
        "queued rows are tracked"
    );
}

// ── Mutation invalidation (ticket kkaq) ──────────────────────────────────

/// A confirmed flag change re-stores the corrected visible page, so a
/// warm start never resurrects the stale pre-flag copy.
#[test]
fn a_confirmed_flag_change_stores_the_corrected_page() {
    let mut s = state();
    s.selection = 0; // m1: not starred.
    let (id, kind) = expect_kind(&reduce(&mut s, Action::ToggleStar));
    assert!(
        matches!(&kind, OperationKind::SetStarred { .. }),
        "kind: {kind:?}"
    );
    let effects = complete_done(&mut s, id);
    let stores: Vec<&Effect> = effects
        .iter()
        .filter(|e| matches!(e.kind, OperationKind::CacheListStore { .. }))
        .collect();
    let [effect] = &stores[..] else {
        panic!("expected exactly one page store, got {effects:?}");
    };
    let OperationKind::CacheListStore {
        mailbox,
        query,
        page,
    } = &effect.kind
    else {
        unreachable!("filtered above");
    };
    assert_eq!(mailbox.0, "inbox");
    assert_eq!(query.as_deref(), None);
    assert_eq!((page.offset, page.limit), (0, mock::PAGE_SIZE));
    let row = page
        .items
        .iter()
        .find(|m| m.id == s.messages.items[0].id)
        .expect("stored page lists the row");
    assert!(row.is_starred, "the stored copy carries the confirmed flag");
    assert!(s.messages.items[0].is_starred);
}

/// A confirmed move evicts the cached page for the visible identity: the
/// local post-move page cannot be stored truthfully (backend ids shift),
/// so a warm start must re-fetch instead of resurrecting the moved row.
#[test]
fn a_confirmed_move_evicts_the_cached_page() {
    let mut s = state();
    s.selection = 0;
    let target = s.selected_message().unwrap().id.clone();
    let (id, kind) = expect_kind(&reduce(&mut s, Action::Archive));
    assert!(matches!(&kind, OperationKind::Archive(_)), "kind: {kind:?}");
    let effects = complete_done(&mut s, id);
    // The re-sync load and the eviction both go out…
    let (_, req) = find_page(&effects);
    assert_eq!(req.mailbox_id.0, "inbox");
    assert_eq!(req.offset, 0);
    // …and the eviction targets the exact cached identity.
    let evicts: Vec<&Effect> = effects
        .iter()
        .filter(|e| matches!(e.kind, OperationKind::CacheListEvict { .. }))
        .collect();
    let [effect] = &evicts[..] else {
        panic!("expected exactly one page eviction, got {effects:?}");
    };
    let OperationKind::CacheListEvict {
        mailbox,
        query,
        offset,
        limit,
    } = &effect.kind
    else {
        unreachable!("filtered above");
    };
    assert_eq!(mailbox.0, "inbox");
    assert_eq!(query.as_deref(), None);
    assert_eq!((*offset, *limit), (0, mock::PAGE_SIZE));
    assert!(s.messages.items.iter().all(|m| m.id != target));
}
