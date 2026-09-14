//! Reducer tests: composer domain.

use super::*;

#[test]
fn compose_opens_the_composer_over_the_mailbox_route() {
    let mut s = state();
    compose(&mut s);
    // The list underneath is untouched (restoration by construction).
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.selection, 0);
}

#[test]
fn composer_focus_cycles_fields_and_actions() {
    let mut s = state();
    compose(&mut s);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::To
    );
    for expected in [
        ComposerField::CcToggle,
        ComposerField::BccToggle,
        ComposerField::Subject,
        ComposerField::Body,
        ComposerField::Attach,
        ComposerField::Send,
        ComposerField::Discard,
    ] {
        reduce(&mut s, Action::FocusNext);
        assert_eq!(s.session.composer.as_ref().unwrap().field, expected);
    }
    // Tab past the last control steps out to the sidebar (compose-mode
    // folder list); the next Tab re-enters the composer at its first
    // control.
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Sidebar);
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Composer);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::To
    );
    // Shift+Tab from the first control steps out to the sidebar as well,
    // and re-enters at the last control.
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(s.session.focus, Focus::Sidebar);
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(s.session.focus, Focus::Composer);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::Discard
    );
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::Send
    );
}

#[test]
fn composing_sidebar_focus_moves_the_folder_cursor_and_switches() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('d')));
    // Tab out to the sidebar (from the last control), then walk the folder
    // cursor onto Drafts and switch to it.
    s.session.composer.as_mut().unwrap().field = ComposerField::Discard;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Sidebar);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::MoveDown);
    assert_eq!(s.mailbox_selection, 2, "Drafts row");
    // '/' must not strand focus in the search field while composing.
    no_effects(&reduce(&mut s, Action::OpenSearch));
    assert_eq!(s.session.focus, Focus::Sidebar);
    let effects = reduce(&mut s, Action::Activate);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (id, req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(req.mailbox_id.0, "drafts");
    complete_page_ok(&mut s, id, &req, 0);
    // The switch closed the composer view; the draft data is kept for
    // the Drafts list (plan §14).
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    assert_eq!(s.session.composer.as_ref().unwrap().draft.to, "d");
    // Composing again starts a blank new email anyway (ticket v5x8).
    compose(&mut s);
    assert_eq!(s.session.composer.as_ref().unwrap().draft.to, "");
    assert_eq!(s.session.focus, Focus::Composer);
}

#[test]
fn enter_on_cc_toggle_reveals_and_focuses_the_cc_field() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::FocusNext); // CcToggle
    reduce(&mut s, Action::Activate);
    let composer = s.session.composer.as_ref().unwrap();
    assert!(composer.show_cc);
    assert_eq!(composer.field, ComposerField::Cc);
    // The cycle now contains Cc, not the toggle.
    reduce(&mut s, Action::FocusNext);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::BccToggle
    );
}

#[test]
fn typing_edits_the_focused_field_only() {
    let mut s = state();
    compose(&mut s);
    for c in "max@".chars() {
        reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    // Tab through to Subject and type there.
    reduce(&mut s, Action::FocusNext);
    reduce(&mut s, Action::FocusNext);
    reduce(&mut s, Action::FocusNext); // Subject
    for c in "Hi".chars() {
        reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "max@");
    assert_eq!(composer.draft.subject, "Hi");
    assert!(composer.draft.cc.is_empty());
}

#[test]
fn enter_inserts_newline_in_body_only() {
    let mut s = state();
    compose(&mut s);
    // Walk to the body: 3 Tabs (CcToggle, BccToggle, Subject) + 1 more.
    for _ in 0..4 {
        reduce(&mut s, Action::FocusNext);
    }
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::Body
    );
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('a')));
    reduce(&mut s, Action::Activate); // Enter
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('b')));
    assert_eq!(
        s.session.composer.as_ref().unwrap().body.lines(),
        ["a".to_string(), "b".to_string()]
    );
    // Enter on a single-line field does not edit it.
    reduce(&mut s, Action::FocusPrevious); // Subject
    reduce(&mut s, Action::Activate);
    assert_eq!(s.session.composer.as_ref().unwrap().draft.subject, "");
}

#[test]
fn shortcuts_cannot_fire_while_composing() {
    let mut s = state();
    compose(&mut s);
    // 'c' is compose text, not a new compose; 'e' is text, not archive;
    // '/' is text, not search. (Keyboard mapping tested in input tests;
    // here the reducer-level gating is exercised via focus.)
    let before_routes = s.session.routes.clone();
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('c')));
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('/')));
    reduce(&mut s, Action::Archive);
    reduce(&mut s, Action::OpenSearch);
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "c/");
    assert_eq!(s.session.routes, before_routes, "composer stays open");
    assert!(s.session.operations.is_empty());
    assert_eq!(s.session.focus, Focus::Composer);
}

#[test]
fn esc_leaves_the_composer_and_preserves_the_draft() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('d')));
    reduce(&mut s, Action::BackOrCancel);
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    // The draft data survives for reopening.
    let composer = s.session.composer.as_ref().expect("draft preserved");
    assert_eq!(composer.draft.to, "d");
}

#[test]
fn compose_again_starts_a_blank_new_email() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('d')));
    reduce(&mut s, Action::BackOrCancel);
    // The draft data survives the leave (it stays for the Drafts list).
    let composer = s.session.composer.as_ref().expect("draft preserved");
    assert_eq!(composer.draft.to, "d");
    // `c` never reopens it: composing always starts blank (ticket v5x8).
    compose(&mut s);
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "", "a blank new email");
    assert_eq!(composer.draft.local_id, None, "a fresh, never-saved draft");
    assert_eq!(composer.field, ComposerField::To);
}

#[test]
fn composing_without_a_list_underneath_is_safe() {
    // Startup with no mailbox route yet: compose still opens cleanly and
    // Esc returns to the empty root.
    let mut s = AppState::initial(mock::PAGE_SIZE);
    no_effects(&reduce(&mut s, Action::Compose));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    reduce(&mut s, Action::BackOrCancel);
    assert!(s.session.routes.is_empty());
    assert!(!s.session.quit_requested);
}

#[test]
fn message_actions_do_not_fire_from_composer_focus() {
    let mut s = state();
    compose(&mut s);
    no_effects(&reduce(&mut s, Action::ToggleStar));
    no_effects(&reduce(&mut s, Action::Archive));
    no_effects(&reduce(&mut s, Action::Trash));
    no_effects(&reduce(&mut s, Action::MarkUnread));
    assert!(s.session.operations.is_empty());
}

#[test]
fn composer_edits_without_composer_open_are_inert() {
    let mut s = state();
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    assert!(s.session.composer.is_none());
    assert!(!matches!(s.active_route(), Some(Route::Composer)));
}

#[test]
fn edits_arm_the_debounce_and_tick_starts_the_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0); // sets the clock
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    // Before the two-second window elapses nothing is requested.
    no_effects(&tick(&mut s, 1));
    // At 2 s the save fires with the current revision and content.
    let effects = tick(&mut s, 2);
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.to, "x");
    assert!(s.session.operations.get(id).is_some());
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
    // Confirmation of the newest revision cleans the draft.
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty());
    assert_eq!(draft.save, crate::domain::DraftSaveState::Saved);
    assert_eq!(draft.saved_revision, 1);
    assert_eq!(
        draft.remote_id,
        Some(MessageId(String::from("remote-1"))),
        "the confirmed remote id is remembered for replacement"
    );
}

#[test]
fn saving_revision_n_cannot_mark_revision_n_plus_1_clean() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, snap1) = expect_save(&tick(&mut s, 2));
    // An edit lands while revision 1 is in flight.
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('b')));
    assert_eq!(s.session.composer.as_ref().unwrap().draft.revision, 2);
    // The stale success confirms only revision 1...
    let chained = complete_save_ok(&mut s, id1, snap1.revision, "remote-1");
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.saved_revision,
        1,
        "saved_revision must not jump to the newest revision"
    );
    // ...and the state machine immediately chains another save. The
    // first push also arms the folder recount (ticket vgze): the Drafts
    // copy count changed.
    assert_eq!(chained.len(), 2);
    let (id2, snap2) = match &chained[0].kind {
        OperationKind::SaveDraft { draft } => (chained[0].id, (**draft).clone()),
        other => panic!("expected a chained SaveDraft, got {other:?}"),
    };
    assert_eq!(chained[1].kind, OperationKind::LoadMailboxes);
    assert_eq!(
        snap2.revision, 2,
        "the chained save covers the newest revision"
    );
    assert_eq!(snap2.to, "ab");
    complete_save_ok(&mut s, id2, snap2.revision, "remote-2");
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty(), "now revision 2 is clean");
    assert_eq!(draft.remote_id, Some(MessageId(String::from("remote-2"))));
}

#[test]
fn rapid_edits_coalesce_into_one_pending_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    for c in "abc".chars() {
        reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char(c)));
        tick(&mut s, 0); // within the debounce window
    }
    no_effects(&tick(&mut s, 1));
    let (_, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.revision, 3, "all edits in the window coalesce");
    assert_eq!(snapshot.to, "abc");
}

#[test]
fn draft_save_failure_opens_retry_modal_and_retains_content() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('k')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    reduce(
        &mut s,
        failure(
            id,
            &OperationKind::SaveDraft {
                draft: Box::new(snapshot.clone()),
            },
            "imap down",
        ),
    );
    // Unsaved state, content retained, modal up (plan §14 acceptance).
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.save, crate::domain::DraftSaveState::Failed);
    assert!(draft.is_dirty());
    assert_eq!(draft.to, "k");
    assert!(s.session.overlay.is_some());
    assert_eq!(
        s.session.status.message.as_deref(),
        Some("Draft save failed")
    );
    // Retry replays the *intent*: fresh snapshot of the newest revision
    // under a new operation id.
    let effects = reduce(&mut s, Action::RetryError);
    let (retry_id, retry_snapshot) = expect_save(&effects);
    assert_ne!(retry_id, id);
    assert_eq!(retry_snapshot.local_id, snapshot.local_id);
    assert_eq!(retry_snapshot.revision, snapshot.revision);
    assert_eq!(retry_snapshot.to, "k");
    assert!(s.session.overlay.is_none());
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
}

#[test]
fn dismiss_after_failure_keeps_the_draft_awaiting_retry_or_edit() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('k')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    reduce(
        &mut s,
        failure(
            id,
            &OperationKind::SaveDraft {
                draft: Box::new(snapshot),
            },
            "imap down",
        ),
    );
    reduce(&mut s, Action::DismissError);
    assert!(s.session.overlay.is_none());
    assert_eq!(s.session.focus, Focus::Composer);
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert_eq!(draft.to, "k", "dismiss never discards content");
    // No automatic re-save while Failed; the next edit re-arms autosave.
    no_effects(&tick(&mut s, 60));
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('!')));
    let (_, retry) = expect_save(&tick(&mut s, 62));
    assert_eq!(retry.to, "k!");
}

#[test]
fn caret_moves_do_not_dirty_the_draft() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    assert!(!s.session.composer.as_ref().unwrap().draft.is_dirty());
    // Cross-field caret moves are not content edits: no new revision, no
    // follow-up save.
    reduce(&mut s, Action::FocusNext);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::CursorLeft));
    no_effects(&tick(&mut s, 20));
    assert_eq!(s.session.composer.as_ref().unwrap().draft.revision, 1);
}

#[test]
fn stale_failure_does_not_cancel_a_scheduled_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, snap1) = expect_save(&tick(&mut s, 2));
    // Newer edits re-arm the debounce while revision 1 is failing...
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('b')));
    reduce(
        &mut s,
        failure(
            id1,
            &OperationKind::SaveDraft {
                draft: Box::new(snap1.clone()),
            },
            "boom",
        ),
    );
    // The failure modal intercepts ticks (plan §9); dismiss it and the
    // debounce is still armed: the scheduled save retries with the newest
    // revision without user action.
    reduce(&mut s, Action::DismissError);
    let (_, snap2) = expect_save(&tick(&mut s, 4));
    assert_eq!(snap2.revision, 2);
    assert_eq!(snap2.to, "ab");
}

#[test]
fn startup_restores_the_last_safe_draft_from_the_journal() {
    let mut s = state();
    complete_restore(&mut s, vec![restored_draft("max@x.io", 5, 4)]);
    let composer = s.session.composer.as_ref().expect("restored");
    assert_eq!(composer.draft.to, "max@x.io");
    assert_eq!(composer.draft.revision, 5);
    assert_eq!(composer.draft.saved_revision, 4);
    assert!(
        composer.draft.is_dirty(),
        "the unconfirmed revision must re-push (self-heal)"
    );
    // The body editor carries the restored text (trailing newline intact).
    assert_eq!(composer.body.lines(), ["typed before the crash", ""]);
    // Composing still starts a blank new email (ticket v5x8): the
    // restored draft continues from the Drafts list, while the in-memory
    // copy autosaves its unconfirmed revision (next test).
    compose(&mut s);
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.to,
        "",
        "a blank new email, not the restored draft"
    );
}

#[test]
fn restored_unconfirmed_drafts_autosave_after_startup() {
    let mut s = state();
    tick(&mut s, 0); // the clock is running
    complete_restore(&mut s, vec![restored_draft("max@x.io", 5, 4)]);
    // The debounce window elapses and the gap self-heals: a save of
    // revision 5 starts without any user edit.
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.revision, 5);
    assert_eq!(
        snapshot.local_id,
        crate::domain::DraftId(String::from("local-crash-1")),
        "ids stay stable across the restart"
    );
    complete_save_ok(&mut s, id, snapshot.revision, "remote-new");
    assert!(!s.session.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn restore_is_skipped_when_a_composer_draft_already_exists() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    complete_restore(&mut s, vec![restored_draft("other@x.io", 9, 9)]);
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.to,
        "x",
        "live editing must never be clobbered"
    );
}

#[test]
fn an_empty_journal_restores_nothing() {
    let mut s = state();
    complete_restore(&mut s, Vec::new());
    assert!(s.session.composer.is_none());
}

// ── Force save on leave (plan §14 Phase 6.5) ─────────────────────────────

#[test]
fn esc_mid_debounce_forces_the_save_without_waiting() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    // Esc before the debounce elapses: the save happens NOW.
    let effects = reduce(&mut s, Action::BackOrCancel);
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.to, "x");
    assert!(s.session.operations.get(id).is_some());
    // Leaving returned to the list; the draft (and its in-flight save)
    // survive in state for reopening.
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(matches!(s.active_route(), Some(Route::Mailbox(_))));
    assert!(s.session.composer.as_ref().unwrap().draft.is_dirty());
    assert_eq!(
        s.session.composer.as_ref().unwrap().draft.save,
        crate::domain::DraftSaveState::Saving
    );
}

#[test]
fn esc_with_a_clean_draft_saves_nothing() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    // Clean draft: leaving is silent.
    no_effects(&reduce(&mut s, Action::BackOrCancel));
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
}

#[test]
fn esc_while_the_current_revision_saves_does_not_duplicate_it() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let (id, snapshot) = expect_save(&tick(&mut s, 2));
    let token = s.session.operations.cancellation(id).unwrap();
    // Esc during the in-flight autosave: it is NOT cancelled and NOT
    // duplicated — leaving just lets it finish.
    no_effects(&reduce(&mut s, Action::BackOrCancel));
    assert!(!token.is_cancelled(), "in-flight save must survive leaving");
    assert!(s.session.operations.get(id).is_some());
    assert_eq!(s.session.routes.len(), 1);
    // Its result still applies to the preserved draft.
    complete_save_ok(&mut s, id, snapshot.revision, "remote-1");
    let draft = &s.session.composer.as_ref().unwrap().draft;
    assert!(!draft.is_dirty());
    assert_eq!(draft.save, crate::domain::DraftSaveState::Saved);
}

#[test]
fn esc_with_newer_edits_supersedes_an_in_flight_older_save() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('a')));
    let (id1, _snap1) = expect_save(&tick(&mut s, 2));
    let token1 = s.session.operations.cancellation(id1).unwrap();
    // Edits during the save, then Esc.
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('b')));
    let effects = reduce(&mut s, Action::BackOrCancel);
    assert!(token1.is_cancelled(), "the older save is superseded");
    let (id2, snap2) = expect_save(&effects);
    assert_eq!(
        snap2.revision, 2,
        "the forced save carries the newest revision"
    );
    assert_eq!(snap2.to, "ab");
    complete_save_ok(&mut s, id2, snap2.revision, "remote-2");
    assert!(!s.session.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn edit_without_a_clock_still_autosaves_once_the_clock_arrives() {
    // Defensive edge: an edit before the first tick arms the debounce at
    // the next tick instead of stalling forever.
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    no_effects(&tick(&mut s, 0)); // arms the window
    let (_, snapshot) = expect_save(&tick(&mut s, 2));
    assert_eq!(snapshot.to, "x");
}

#[test]
fn discard_requires_confirmation_and_defaults_to_keep() {
    let mut s = state();
    open_discard_dialog(&mut s);
    // The draft is untouched until confirmation.
    assert!(s.session.composer.is_some());
    assert_eq!(s.session.routes.len(), 2);
    // Enter activates the focused button: Keep (the safe default).
    no_effects(&reduce(&mut s, Action::Activate));
    assert!(s.session.composer.is_some(), "keep preserves the draft");
    assert!(s.session.overlay.is_none());
    assert_eq!(s.session.focus, Focus::Composer);
    assert_eq!(s.session.routes.len(), 2);
}

#[test]
fn esc_on_the_discard_dialog_keeps_the_draft() {
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, Action::BackOrCancel);
    assert!(s.session.overlay.is_none());
    assert!(s.session.composer.is_some());
    assert_eq!(
        s.session.focus,
        Focus::Composer,
        "focus returns to the composer"
    );
}

#[test]
fn confirmed_discard_deletes_local_and_remote_state() {
    let mut s = state();
    open_discard_dialog(&mut s);
    reduce(&mut s, Action::FocusNext); // Discard button
    let effects = reduce(&mut s, Action::Activate);
    let (id, kind) = effect_parts(&effects);
    let crate::app::operation::OperationKind::DeleteDraft {
        draft,
        reason: DraftRemovalReason::Discard,
    } = &kind
    else {
        panic!("expected DeleteDraft, got {kind:?}");
    };
    assert_eq!(
        draft.local_id,
        crate::domain::DraftId(String::from("local-unsaved"))
    );
    assert_eq!(draft.to, "x");
    // Local state is gone immediately; the remote sweep runs in the op.
    assert!(
        s.session.composer.is_none(),
        "local draft removed on confirmation"
    );
    assert_eq!(s.session.routes.len(), 1);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(s.session.operations.get(id).is_some());
    assert_eq!(s.session.status.message.as_deref(), Some("Draft discarded"));
    // Confirmation completes without further state change.
    complete_done(&mut s, id);
    assert!(s.session.composer.is_none());
    // Composing starts fresh.
    compose(&mut s);
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, "");
    assert_eq!(composer.draft.revision, 0);
}

#[test]
fn discard_cancels_an_in_flight_save_of_the_same_draft() {
    let mut s = state();
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let (save_id, _) = expect_save(&tick(&mut s, 2));
    let save_token = s.session.operations.cancellation(save_id).unwrap();
    // Discard while that save is in flight.
    reduce(&mut s, Action::DiscardDraft);
    reduce(&mut s, Action::FocusNext);
    let effects = reduce(&mut s, Action::Activate);
    assert!(
        save_token.is_cancelled(),
        "the in-flight save must not resurrect the discarded draft"
    );
    assert!(s.session.operations.get(save_id).is_none());
    let (delete_id, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::DeleteDraft { .. }));
    assert!(s.session.operations.get(delete_id).is_some());
    // The cancelled save's (suppressed) result can never apply.
    reduce(
        &mut s,
        Action::BackendCompleted(OperationResult {
            id: save_id,
            outcome: Ok(OperationOutcome::DraftSaved {
                remote_id: MessageId(String::from("late-remote")),
            }),
        }),
    );
    assert!(s.session.composer.is_none());
}

#[test]
fn discard_dialog_swallows_unrelated_input() {
    let mut s = state();
    open_discard_dialog(&mut s);
    let draft_before = s.session.composer.as_ref().unwrap().draft.to.clone();
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('y')));
    reduce(&mut s, Action::SearchEdit(SearchEdit::Char('z')));
    reduce(&mut s, Action::Refresh);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::Quit);
    let composer = s.session.composer.as_ref().unwrap();
    assert_eq!(composer.draft.to, draft_before);
    assert!(!s.session.quit_requested);
    assert!(s.session.overlay.is_some(), "dialog stays open");
}

#[test]
fn discard_of_a_never_saved_draft_still_needs_confirmation() {
    let mut s = state();
    compose(&mut s);
    no_effects(&reduce(&mut s, Action::DiscardDraft));
    assert!(matches!(
        s.session.overlay,
        Some(Overlay::ConfirmDiscard(_))
    ));
    reduce(&mut s, Action::FocusNext);
    let effects = reduce(&mut s, Action::Activate);
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::DeleteDraft { .. }));
    assert!(s.session.composer.is_none());
}

#[test]
fn discard_delete_failure_opens_the_error_modal() {
    let mut s = state();
    compose(&mut s);
    reduce(&mut s, Action::DiscardDraft);
    reduce(&mut s, Action::FocusNext);
    let (id, kind) = effect_parts(&reduce(&mut s, Action::Activate));
    // The draft is already gone locally; a remote sweep failure surfaces.
    reduce(&mut s, failure(id, &kind, "imap refused"));
    let Some(Overlay::Error(_)) = &s.session.overlay else {
        panic!("error modal must open");
    };
    assert!(
        s.session.composer.is_none(),
        "the discard itself is not rolled back"
    );
}

// ── Reply / forward seeding (plan §14, Phase 7.3) ────────────────────────

#[test]
fn edit_external_saves_the_draft_first_and_flags_the_composer() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    assert!(s.session.composer.as_ref().unwrap().draft.is_dirty());

    let effects = edit_external(&mut s);
    // The forced save (step 1) rides ahead of the editor effect.
    assert_eq!(effects.len(), 2);
    assert!(matches!(effects[0].kind, OperationKind::SaveDraft { .. }));
    assert!(matches!(
        effects[1].kind,
        OperationKind::EditExternally { .. }
    ));
    // No background autosave is promised while the editor owns the file.
    assert!(s.session.composer.as_ref().unwrap().external_editing);
}

#[test]
fn edit_external_with_a_clean_draft_only_starts_the_editor() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    let effects = edit_external(&mut s);
    assert_eq!(effects.len(), 1, "only the editor effect");
    assert!(matches!(
        effects[0].kind,
        OperationKind::EditExternally { .. }
    ));
}

#[test]
fn edit_external_is_inert_without_an_editor_or_composer() {
    let mut s = state();
    // Builtin editor: nothing external to run.
    compose(&mut s);
    no_effects(&edit_external(&mut s));
    // Editor configured, but the composer does not hold focus.
    s.settings.editor_command = Some(vec![String::from("vim")]);
    s.session.focus = Focus::MessageList;
    no_effects(&edit_external(&mut s));
}

#[test]
fn no_autosave_while_the_external_editor_owns_the_file() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.session.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let effects = edit_external(&mut s);
    assert!(matches!(effects[0].kind, OperationKind::SaveDraft { .. }));
    // A dirty edit during the editor session would normally re-arm the
    // autosave; ticks must not save while the editor owns the file.
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('y')));
    no_effects(&tick(&mut s, 5));
    no_effects(&tick(&mut s, 10));
}

#[test]
fn editor_import_marks_dirty_and_saves_once() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    tick(&mut s, 0);
    let effects = edit_external(&mut s);
    let editor_id = effects.last().unwrap().id;
    let before_revision = s.session.composer.as_ref().unwrap().draft.revision;

    let effects = reduce(
        &mut s,
        Action::EditorFinished {
            id: editor_id,
            result: Ok(String::from("edited body\nline two\n")),
        },
    );
    // Steps 6–8: import → dirty → exactly one save.
    let (id, snapshot) = expect_save(&effects);
    assert_eq!(snapshot.body, "edited body\nline two\n");
    assert_eq!(snapshot.revision, before_revision + 1);
    assert!(s.session.composer.as_ref().unwrap().draft.is_dirty());
    assert!(!s.session.composer.as_ref().unwrap().external_editing);
    complete_save_ok(&mut s, id, snapshot.revision, "remote-9");
    assert!(!s.session.composer.as_ref().unwrap().draft.is_dirty());
}

#[test]
fn editor_import_without_changes_saves_nothing() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.session.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    // Body is now "x"; the editor hands back exactly that.
    let editor_id = edit_external(&mut s).last().unwrap().id;
    let effects = reduce(
        &mut s,
        Action::EditorFinished {
            id: editor_id,
            result: Ok(String::from("x")),
        },
    );
    no_effects(&effects);
    assert!(!s.session.composer.as_ref().unwrap().external_editing);
}

#[test]
fn editor_failure_keeps_the_draft_and_reports() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    s.session.composer.as_mut().unwrap().field = crate::app::composer::ComposerField::Body;
    tick(&mut s, 0);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('x')));
    let editor_id = edit_external(&mut s).last().unwrap().id;
    let effects = reduce(
        &mut s,
        Action::EditorFinished {
            id: editor_id,
            result: Err(String::from("editor exited with code 1")),
        },
    );
    no_effects(&effects);
    // The draft keeps its content and the composer is usable again.
    assert_eq!(s.session.composer.as_ref().unwrap().draft.body, "x");
    assert!(!s.session.composer.as_ref().unwrap().external_editing);
    assert!(
        s.session
            .status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("editor exited with code 1"))
    );
}

#[test]
fn the_editor_operation_is_never_cancellable() {
    let mut s = state();
    s.settings.editor_command = Some(vec![String::from("vim")]);
    compose(&mut s);
    let effects = edit_external(&mut s);
    let editor_id = effects.last().unwrap().id;
    let op = s.session.operations.get(editor_id).unwrap();
    assert!(!op.kind.is_cancellable());
}

#[test]
fn space_advances_to_the_next_row_after_toggling() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
    s.selection = 0;
    let first = s.messages.items[0].id.clone();

    no_effects(&reduce(&mut s, Action::ToggleSelected));
    assert!(s.selected.contains(&first), "row 0 marked");
    assert_eq!(s.selection, 1, "cursor advanced to row 1");

    // The next Space marks row 1 (not unmarks row 0).
    let second = s.messages.items[1].id.clone();
    no_effects(&reduce(&mut s, Action::ToggleSelected));
    assert!(s.selected.contains(&second), "row 1 marked next");
    assert!(s.selected.contains(&first), "row 0 stays marked");
    assert_eq!(s.selection, 2);

    // The last row does not advance further.
    s.selection = s.messages.items.len() - 1;
    let last = s.messages.items[s.selection].id.clone();
    no_effects(&reduce(&mut s, Action::ToggleSelected));
    assert_eq!(
        s.selection,
        s.messages.items.len() - 1,
        "clamped at the end"
    );
    assert!(s.selected.contains(&last));
}
