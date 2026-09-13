//! Reducer tests: drafts domain.

use super::*;

#[test]
fn enter_in_other_mailboxes_still_opens_the_reader() {
    let mut s = state();
    let (_, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    assert!(
        matches!(kind, OperationKind::CacheMessageLoad { .. }),
        "the reader open reads the message cache first, got {kind:?}"
    );
}

#[test]
fn enter_on_the_open_draft_in_drafts_reuses_it_without_a_fetch() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('d')));
    tick(&mut s, 0);
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    complete_save_ok(&mut s, id, snapshot.revision, "copy-1");
    // Leave the composer, switch to Drafts; the list shows the saved copy
    // with a refreshed backend id but the same stable `Message-ID`.
    reduce(&mut s, &Action::BackOrCancel);
    switch_to(&mut s, "drafts");
    let bare = snapshot
        .message_id
        .clone()
        .expect("minted at save")
        .trim_matches(|c| c == '<' || c == '>')
        .to_string();
    s.messages.items = vec![draft_row("copy-99", Some(&bare))];
    s.selection = 0;
    // Enter: the composer reopens the exact in-memory draft — no backend
    // round-trip, content and remote identity intact.
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.to, "d");
    assert_eq!(draft.remote_id, Some(MessageId(String::from("copy-1"))));
    assert!(
        !s.session.operations.has_foreground(),
        "no fetch was started"
    );
}

#[test]
fn enter_on_a_remote_draft_fetches_and_opens_the_composer() {
    let mut s = state();
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-7", Some("1778.draft@tmail.local"))];
    s.selection = 0;
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    let OperationKind::OpenDraft(locator) = kind else {
        panic!("expected OpenDraft, got {kind:?}");
    };
    assert_eq!(locator.id, MessageId(String::from("copy-7")));
    assert_eq!(locator.mailbox, drafts_id());
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "1778.draft@tmail.local",
            )))),
        }),
    );
    // The composer opened over the list with the copy's fields…
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.to, "dest@example.com");
    assert_eq!(draft.cc, "cc@example.com");
    assert_eq!(draft.bcc, "bcc@example.com");
    assert_eq!(draft.subject, "Hello");
    assert_eq!(draft.body, "draft body");
    // …and its identities, so the next save replaces the copy instead of
    // adding a second one.
    assert_eq!(
        draft.message_id.as_deref(),
        Some("<1778.draft@tmail.local>")
    );
    assert_eq!(draft.remote_id, Some(MessageId(String::from("copy-7"))));
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('!')));
    tick(&mut s, 0);
    let (save_id, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(
        snapshot.message_id.as_deref(),
        Some("<1778.draft@tmail.local>")
    );
    assert_eq!(snapshot.remote_id, Some(MessageId(String::from("copy-7"))));
    complete_save_ok(&mut s, save_id, snapshot.revision, "copy-8");
}

#[test]
fn esc_cancels_a_draft_fetch_and_the_list_stays() {
    let mut s = state();
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-7", None)];
    s.selection = 0;
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.session.composer.is_none());
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    // The cancelled fetch's result can never mutate state (plan §11).
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "1778.draft@tmail.local",
            )))),
        }),
    );
    assert!(
        s.session.composer.is_none(),
        "cancelled results are dropped"
    );
}

#[test]
fn enter_on_a_draft_row_secures_the_parked_draft_and_swaps() {
    // A parked draft with real work never blocks the Drafts list (ticket
    // sazy): its Esc-forced save is already in flight, so Enter fetches
    // the selected copy and the fetched draft takes the composer slot.
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0); // sets the clock so the leave-save can start
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    let (parked_save, parked_snapshot) = expect_save(&reduce(&mut s, &Action::BackOrCancel));
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-9", Some("other@tmail.local"))];
    s.selection = 0;
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    let OperationKind::OpenDraft(locator) = kind else {
        panic!("expected OpenDraft, got {kind:?}");
    };
    assert_eq!(locator.id, MessageId(String::from("copy-9")));
    // The parked save confirms while its draft is still in the slot.
    complete_save_ok(&mut s, parked_save, parked_snapshot.revision, "copy-8");
    // The landed copy replaces the parked draft and opens the composer.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-9",
                "other@tmail.local",
            )))),
        }),
    );
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.subject, "Hello", "the fetched draft is editing");
    assert_eq!(s.session.status.message.as_deref(), Some("Draft opened"));
}

#[test]
fn enter_on_a_draft_row_force_saves_an_unsaved_parked_draft() {
    // A parked draft with unsaved edits and no save in flight (the journal
    // restore's shape before the next autosave): Enter first secures it
    // with a forced save, then fetches the selected copy.
    let mut s = state();
    tick(&mut s, 0); // sets the clock
    switch_to(&mut s, "drafts");
    let mut parked = crate::domain::Draft {
        to: String::from("old@example.com"),
        ..crate::domain::Draft::default()
    };
    parked.note_edit(Some(mock::now()));
    s.session.composer = Some(crate::app::composer::ComposerState::from_draft(parked));
    s.messages.items = vec![draft_row("copy-9", Some("other@tmail.local"))];
    s.selection = 0;
    let effects = reduce(&mut s, &Action::Activate);
    assert_eq!(
        effects.len(),
        2,
        "the forced save of the parked draft plus the fetch"
    );
    let OperationKind::SaveDraft { draft: snapshot } = &effects[0].kind else {
        panic!("expected SaveDraft, got {:?}", effects[0].kind);
    };
    assert_eq!(snapshot.to, "old@example.com");
    assert_eq!(snapshot.revision, 1);
    let OperationKind::OpenDraft(_) = &effects[1].kind else {
        panic!("expected OpenDraft, got {:?}", effects[1].kind);
    };
    // The parked save confirms while its draft is still in the slot…
    complete_save_ok(&mut s, effects[0].id, snapshot.revision, "copy-old");
    // …then the fetch lands and the fetched copy replaces it.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: effects[1].id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-9",
                "other@tmail.local",
            )))),
        }),
    );
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.session.composer.as_ref().unwrap().draft.subject, "Hello");
}

#[test]
fn rapid_draft_opens_supersede_and_the_last_row_wins() {
    let mut s = state();
    tick(&mut s, 0);
    switch_to(&mut s, "drafts");
    s.messages.items = vec![
        draft_row("copy-7", Some("a@tmail.local")),
        draft_row("copy-8", Some("b@tmail.local")),
    ];
    s.selection = 0;
    let (first_id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    s.selection = 1;
    let (second_id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    assert_ne!(first_id, second_id);
    // The older fetch was superseded by the newer Enter: its result can
    // never open a draft (plan §11).
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: first_id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "a@tmail.local",
            )))),
        }),
    );
    assert!(s.session.composer.is_none(), "superseded result dropped");
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: second_id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-8",
                "b@tmail.local",
            )))),
        }),
    );
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(
        s.session
            .composer
            .as_ref()
            .unwrap()
            .draft
            .message_id
            .as_deref(),
        Some("<b@tmail.local>")
    );
}

#[test]
fn draft_fetch_is_dropped_when_the_selection_moved() {
    // The draft that opens is the one the cursor is on: a fetch for a row
    // the user has already moved past is dropped (Enter again refetches).
    let mut s = state();
    tick(&mut s, 0);
    switch_to(&mut s, "drafts");
    s.messages.items = vec![
        draft_row("copy-7", Some("a@tmail.local")),
        draft_row("copy-8", Some("b@tmail.local")),
    ];
    s.selection = 0;
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    reduce(&mut s, &Action::MoveDown);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "a@tmail.local",
            )))),
        }),
    );
    assert!(s.session.composer.is_none(), "stale fetch dropped");
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
}

/// A pristine blank (the `c` artifact, left open with Esc) is not work:
/// Enter on a Drafts row replaces it instead of refusing (ticket pmbz).
#[test]
fn enter_on_a_draft_row_replaces_a_blank_c_draft() {
    let mut s = state();
    compose(&mut s);
    // Esc with no edits: the preserved draft is pristine blank.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.session.composer.as_ref().unwrap().draft.is_blank());
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-9", Some("other@tmail.local"))];
    s.selection = 0;
    let (id, kind) = effect_parts(&reduce(&mut s, &Action::Activate));
    let OperationKind::OpenDraft(locator) = kind else {
        panic!("expected OpenDraft, got {kind:?}");
    };
    assert_eq!(locator.id, MessageId(String::from("copy-9")));
    // The landed copy replaces the blank and opens the composer.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-9",
                "other@tmail.local",
            )))),
        }),
    );
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.subject, "Hello", "the fetched draft is editing");
    assert_eq!(draft.body, "draft body");
}

#[test]
fn draft_fetch_result_is_dropped_after_a_mailbox_switch() {
    let mut s = state();
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-7", None)];
    s.selection = 0;
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    switch_to(&mut s, "sent");
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "1778.draft@tmail.local",
            )))),
        }),
    );
    assert!(
        s.session.composer.is_none(),
        "the stale fetch must not compose"
    );
    assert_eq!(
        s.active_route().and_then(Route::mailbox_id).unwrap().0,
        "sent"
    );
}

#[test]
fn draft_fetch_result_never_clobbers_a_newer_draft() {
    let mut s = state();
    switch_to(&mut s, "drafts");
    s.messages.items = vec![draft_row("copy-7", None)];
    s.selection = 0;
    let (id, _) = effect_parts(&reduce(&mut s, &Action::Activate));
    // While the fetch runs, the user composes a fresh draft and leaves it.
    reduce(&mut s, &Action::Compose);
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('n')));
    reduce(&mut s, &Action::BackOrCancel);
    // Back on the drafts list when the fetch lands: the newer draft wins.
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(fetched_draft(
                "copy-7",
                "1778.draft@tmail.local",
            )))),
        }),
    );
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.to, "n", "the fetch must not clobber the newer draft");
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
}

// ── Attachment file chooser (plan §15, ticket 95x0) ──────────────────────

#[test]
fn reply_seeds_a_composer_on_top_of_the_reader() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Reply));
    // Route stack: composer above the still-open reader; Esc from the
    // composer returns to reading.
    assert_eq!(s.session.routes.len(), 3);
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.session.focus, Focus::Composer);
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Reply draft ready")
    );
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "Bob <bob@example.org>");
    assert_eq!(composer.draft.subject, "Re: Plan review");
    assert_eq!(
        composer.draft.in_reply_to.as_deref(),
        Some("318@tmail.local")
    );
    assert_eq!(
        composer.draft.references.as_deref(),
        Some("000@tmail.local 318@tmail.local")
    );
    // Quoted body with the attribution; caret starts at the very top.
    assert!(composer.draft.body.contains("On "));
    assert!(
        composer
            .draft
            .body
            .contains("wrote:\n> Please review.\n> Thanks")
    );
    // The seeded draft is clean: autosave engages on the first edit.
    assert!(!composer.draft.is_dirty());
    assert_eq!(composer.draft.revision, 0);
}

#[test]
fn forward_seeds_a_header_block_and_no_recipients() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Forward));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "");
    assert_eq!(composer.draft.subject, "Fwd: Plan review");
    assert_eq!(composer.draft.in_reply_to, None);
    assert_eq!(composer.draft.references, None);
    assert!(
        composer
            .draft
            .body
            .contains("---------- Forwarded message ---------")
    );
    assert!(composer.draft.body.contains("From: Bob <bob@example.org>"));
    assert!(composer.draft.body.contains("To: probe@tmail.local"));
}

#[test]
fn reply_needs_a_target() {
    let mut s = state();
    // No reader and an empty list: nothing to reply to.
    s.messages.items.clear();
    s.open_message = Loadable::Idle;
    no_effects(&reduce(&mut s, &Action::Reply));
    no_effects(&reduce(&mut s, &Action::Forward));
    assert!(s.session.composer.is_none());
    // Reader open but the message still loading: the seed fetch starts
    // from the reader's summary, so this is now the pending-fetch case
    // (covered by `reply_from_the_list_fetches_then_seeds_the_composer`).
}

#[test]
fn reply_never_clobbers_an_existing_draft() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Compose)); // a draft exists already
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    no_effects(&reduce(&mut s, &Action::Reply));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.to, "k", "existing draft untouched");
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("A draft is already open — send or discard it first")
    );
}

/// A draft left behind (Esc saved it) lingers in state for the Drafts
/// list — it must not block a reply from the reader (ticket 61qx): the
/// seed replaces it.
#[test]
fn reply_replaces_a_draft_left_behind() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Compose));
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    reduce(&mut s, &Action::BackOrCancel); // Esc: save & leave
    assert!(matches!(s.active_route(), Some(Route::Message(_))));
    assert!(
        s.session.composer.is_some(),
        "the left draft stays in state"
    );
    no_effects(&reduce(&mut s, &Action::Reply));
    let composer = seeded_composer(&s);
    assert_eq!(
        composer.draft.to, "Bob <bob@example.org>",
        "the reply seed replaces the left-behind draft"
    );
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Reply draft ready")
    );
}

#[test]
fn forward_replaces_a_draft_left_behind() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    no_effects(&reduce(&mut s, &Action::Compose));
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('k')));
    reduce(&mut s, &Action::BackOrCancel); // Esc: save & leave
    no_effects(&reduce(&mut s, &Action::Forward));
    let composer = seeded_composer(&s);
    assert_eq!(composer.draft.subject, "Fwd: Plan review");
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Forward draft ready")
    );
}

#[test]
fn leaving_a_seeded_reply_returns_to_the_reader() {
    let mut s = state();
    open_reader_with(&mut s, reply_source());
    reduce(&mut s, &Action::Reply);
    reduce(&mut s, &Action::BackOrCancel); // Esc: save/leave
    assert!(matches!(s.active_route(), Some(Route::Message(_))));
    assert_eq!(s.session.focus, Focus::MessageList);
    // The seeded draft stays in state (for the Drafts list).
    assert!(s.session.composer.is_some());
    // `c` starts a blank new email instead of reopening it (ticket v5x8).
    reduce(&mut s, &Action::Compose);
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.in_reply_to, None, "a blank new email");
}

#[test]
fn reply_all_merges_recipients_dedups_and_excludes_self() {
    let mut s = state();
    s.settings.account_email = Some(String::from("probe@tmail.local"));
    let mut message = reply_source();
    // Carol appears in To and Cc; the account itself was a recipient.
    message.headers.to.push(Address {
        name: Some(String::from("Carol")),
        email: String::from("carol@example.org"),
    });
    message.headers.cc.push(Address {
        name: None,
        email: String::from("CAROL@example.org"),
    });
    message.headers.cc.push(Address {
        name: None,
        email: String::from("probe@tmail.local"),
    });
    open_reader_with(&mut s, message);
    no_effects(&reduce(&mut s, &Action::ReplyAll));
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Reply-all draft ready")
    );
    let composer = seeded_composer(&s);
    // Sender first; the account's own address (probe@) is excluded even
    // though it was in To; Carol keeps her first (To) form, and her Cc
    // duplicate is dropped.
    assert_eq!(
        composer.draft.to,
        "Bob <bob@example.org>, Carol <carol@example.org>"
    );
    assert_eq!(composer.draft.cc, "");
    assert_eq!(
        composer.draft.in_reply_to.as_deref(),
        Some("318@tmail.local")
    );
}

// ── Send flow (plan §14/§19 Phase 7, Phase 7.6) ──────────────────────────

#[test]
fn ctrl_enter_sends_only_from_the_composer() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::Send));
    assert!(s.session.composer.is_none());
    // A draft may exist without the composer route (left-open draft): the
    // route gate still applies.
    compose(&mut s);
    reduce(&mut s, &Action::BackOrCancel);
    no_effects(&reduce(&mut s, &Action::Send));
    assert!(s.session.composer.is_some(), "draft data preserved");
    assert!(s.session.operations.is_empty());
}

#[test]
fn send_refuses_an_empty_recipient_list() {
    let mut s = state();
    compose(&mut s);
    // A subject alone is not enough: no To/Cc/Bcc, no send.
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    reduce(&mut s, &Action::FocusNext);
    for c in "hi".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Cannot send: add at least one recipient")
    );
    assert!(
        s.session.composer.as_ref().unwrap().draft.to.is_empty(),
        "draft untouched"
    );
}

#[test]
fn send_refuses_invalid_addresses() {
    let mut s = state();
    compose(&mut s);
    for c in "not an address".chars() {
        reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Cannot send: fix the invalid address entries")
    );
    assert!(s.session.operations.is_empty(), "nothing was started");
}

#[test]
fn send_freezes_the_composer_and_starts_one_operation() {
    let mut s = state();
    sendable(&mut s);
    let effects = reduce(&mut s, &Action::Send);
    let (id, message) = expect_send(&effects);
    assert_eq!(message.to.len(), 1);
    assert_eq!(message.to[0].email, "ada@example.org");
    assert_eq!(message.content.subject, "Hello");
    assert_eq!(message.content.body, "Body");
    assert!(s.session.operations.get(id).is_some());
    let composer = s.session.composer.as_ref().unwrap();
    assert!(composer.sending);
    assert_eq!(s.session.status.message.as_deref(), Some("Sending…"));
    // Edits are frozen while the send runs; the draft keeps its content.
    reduce(&mut s, &Action::ComposerEdit(ComposerEdit::Char('x')));
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.body, "Body");
    // A second send is refused.
    no_effects(&reduce(&mut s, &Action::Send));
    assert_eq!(s.session.operations.len(), 1);
    let _ = id;
}

#[test]
fn send_failure_keeps_the_draft_intact() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("smtp refused"),
                retry: Some(mailboxes_kind().retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // Draft intact and editable again; no route change.
    let composer = s.session.composer.as_ref().unwrap();
    assert!(!composer.sending);
    assert_eq!(composer.draft.to, "ada@example.org");
    assert_eq!(composer.draft.body, "Body");
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert!(s.session.overlay.is_some(), "Retry/Dismiss modal opens");
}

#[test]
fn send_success_leaves_the_composer_and_resolves_the_draft() {
    let mut s = state();
    sendable(&mut s);
    // The draft was saved before sending: the remote copy must be swept.
    let composer = s.session.composer.as_mut().unwrap();
    composer.draft.remote_id = Some(MessageId(String::from("remote-draft")));
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    // Composer closed, back to the mailbox, status confirms success.
    assert!(s.session.composer.is_none());
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert_eq!(s.session.status.message.as_deref(), Some("Message sent"));
    // One cleanup operation: journal entry + remote copy removal.
    let (cleanup_id, kind) = effect_parts(&effects);
    match &kind {
        OperationKind::DeleteDraft {
            draft,
            reason: DraftRemovalReason::Sent,
        } => {
            assert_eq!(
                draft.remote_id,
                Some(MessageId(String::from("remote-draft")))
            );
        }
        other => panic!("expected DeleteDraft(Sent), got {other:?}"),
    }
    assert!(s.session.operations.get(cleanup_id).is_some());
}

#[test]
fn send_success_after_leaving_still_resolves_the_draft() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    // The user left mid-send (Esc save/leaves; the draft was clean, so no
    // forced save runs).
    reduce(&mut s, &Action::BackOrCancel);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    assert!(
        s.session.composer.is_none(),
        "the sent draft must not linger"
    );
    assert_eq!(s.session.routes.len(), 1, "already left: no route to pop");
    assert_eq!(effect_parts(&effects).1.summary(), "Cleaning up sent draft");
}

#[test]
fn sent_draft_cleanup_failure_never_claims_a_failed_send() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    let effects = complete_send(&mut s, id, SendOutcome::Sent);
    let (cleanup_id, kind) = effect_parts(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: cleanup_id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("drafts mailbox gone"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    // Delivery was confirmed: no modal, no status regression.
    assert!(s.session.overlay.is_none());
    assert_eq!(s.session.status.message.as_deref(), Some("Message sent"));
}

#[test]
fn cleanup_of_a_discarded_draft_still_opens_the_modal_on_failure() {
    // The Sent-reason quietness must not weaken the discard flow (6.6).
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, &Action::FocusNext); // Discard
    let effects = reduce(&mut s, &Action::Activate);
    let (id, kind) = effect_parts(&effects);
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("sweep failed"),
                retry: Some(kind.retry_spec()),
                ambiguous: false,
            }),
        }),
    );
    assert!(
        s.session.overlay.is_some(),
        "discard cleanup failures stay visible"
    );
}

// ── Ambiguous sends (plan §12, Phase 7.7) ────────────────────────────────

#[test]
fn ambiguous_send_opens_the_duplicate_warning_and_keeps_the_draft() {
    let mut s = state();
    sendable(&mut s);
    let (id, message) = expect_send(&reduce(&mut s, &Action::Send));
    // Probe-verified ambiguous outcome: payload transmitted, himalaya
    // reported a DATA-phase EOF.
    complete_send(
        &mut s,
        id,
        SendOutcome::Unknown {
            code: Some(1),
            detail: String::from("SMTP DATA failed: Reached unexpected EOF"),
        },
    );
    let Some(Overlay::Error(dialog)) = &s.session.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous, "the modal must carry the ambiguity flag");
    assert_eq!(dialog.code, Some(1));
    assert!(dialog.detail.contains("SMTP DATA failed"));
    // Retry stays available, replaying the exact frozen message.
    assert_eq!(
        dialog.retry.as_ref().map(|spec| spec.kind.clone()),
        Some(OperationKind::Send {
            message: Box::new(message),
        })
    );
    // The draft is intact and editable again; nothing claimed success.
    let composer = s.session.composer.as_ref().unwrap();
    assert!(!composer.sending);
    assert_eq!(composer.draft.body, "Body");
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Send outcome unclear")
    );
}

#[test]
fn ambiguous_send_never_shows_a_failure_title_or_status() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::SentButCopyFailed {
            code: None,
            detail: String::from("sent-copy append failed"),
        },
    );
    // Not success ("Message sent"), not definite failure ("Send failed").
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Send outcome unclear")
    );
    let Some(Overlay::Error(dialog)) = &s.session.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous);
}

#[test]
fn retrying_an_ambiguous_send_replays_the_frozen_message() {
    let mut s = state();
    sendable(&mut s);
    let (id, message) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::Unknown {
            code: Some(1),
            detail: String::from("connection reset"),
        },
    );
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, replay) = expect_send(&effects);
    assert_ne!(retry_id, id, "a retry gets a new operation id");
    assert_eq!(replay, message, "the exact same bytes are re-sent");
    assert!(s.session.overlay.is_none());
    let composer = s.session.composer.as_ref().unwrap();
    assert!(composer.sending, "the retry send is in flight again");
}

#[test]
fn failed_before_delivery_send_reports_definite_failure_safely() {
    let mut s = state();
    sendable(&mut s);
    let (id, _) = expect_send(&reduce(&mut s, &Action::Send));
    complete_send(
        &mut s,
        id,
        SendOutcome::FailedBeforeDelivery {
            code: Some(1),
            detail: String::from("connect 127.0.0.1:3425: connection refused"),
        },
    );
    assert_eq!(s.session.status.message.as_deref(), Some("Send failed"));
    let Some(Overlay::Error(dialog)) = &s.session.overlay else {
        panic!("modal open");
    };
    // Nothing was transmitted: retrying is safe, no duplicate warning.
    assert!(!dialog.ambiguous);
    assert!(dialog.retry.is_some());
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.body, "Body", "draft intact");
}

#[test]
fn reply_from_the_list_fetches_then_seeds_the_composer() {
    let mut s = state();
    assert_eq!(s.session.focus, Focus::MessageList);
    let effects = reduce(&mut s, &Action::Reply);
    let (id, locator, kind) = expect_seed(&effects);
    assert_eq!(kind, SeedKind::Reply);
    assert_eq!(locator.id, s.messages.items[0].id);
    // The fetch is in flight; the composer has not opened yet.
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    assert!(s.session.composer.is_none());
    // The fetched message seeds the composer exactly like the reader path.
    let message = mock::mock_message(&s.messages.items[0]);
    no_effects(&reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    ));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.session.focus, Focus::Composer);
    let composer = seeded_composer(&s);
    assert!(
        composer.draft.subject.starts_with("Re:"),
        "{}",
        composer.draft.subject
    );
}

#[test]
fn forward_from_the_list_quotes_the_fetched_message() {
    let mut s = state();
    let effects = reduce(&mut s, &Action::Forward);
    let (id, _locator, kind) = expect_seed(&effects);
    assert_eq!(kind, SeedKind::Forward);
    let message = mock::mock_message(&s.messages.items[0]);
    no_effects(&reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    ));
    let composer = seeded_composer(&s);
    assert!(
        composer.draft.subject.starts_with("Fwd:"),
        "{}",
        composer.draft.subject
    );
    assert_ne!(s.session.status.message.as_deref(), None);
}

#[test]
fn a_failed_list_seed_opens_the_modal_without_a_composer() {
    let mut s = state();
    let effects = reduce(&mut s, &Action::Reply);
    let (id, _locator, _kind) = expect_seed(&effects);
    let kind = OperationKind::SeedComposer {
        locator: s.messages.items[0].clone().into_locator(),
        kind: SeedKind::Reply,
    };
    let _ = &mut s;
    reduce(&mut s, &failure(id, &kind, "himalaya exited with code 1"));
    assert!(
        matches!(s.active_route(), Some(Route::Mailbox(_))),
        "no composer on failure"
    );
    assert!(s.session.composer.is_none());
    assert!(matches!(s.session.overlay, Some(Overlay::Error(_))));
}

#[test]
fn pressing_reply_twice_supersedes_the_first_fetch() {
    let mut s = state();
    let (first_id, _, _) = expect_seed(&reduce(&mut s, &Action::Reply));
    let (second_id, _, _) = expect_seed(&reduce(&mut s, &Action::Reply));
    assert_ne!(first_id, second_id);
    // The superseded first fetch is dropped by the registry: its late
    // result must not open the composer.
    let message = mock::mock_message(&s.messages.items[0]);
    no_effects(&reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: first_id,
            outcome: Ok(OperationOutcome::Message(Box::new(message.clone()))),
        }),
    ));
    assert!(s.session.composer.is_none());
    // The newest fetch wins and seeds.
    no_effects(&reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id: second_id,
            outcome: Ok(OperationOutcome::Message(Box::new(message))),
        }),
    ));
    assert!(s.session.composer.is_some());
}
