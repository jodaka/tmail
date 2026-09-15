//! Reducer tests: mouse domain.

use super::*;

#[test]
fn click_selects_then_opens_a_message_row() {
    let mut s = state();
    // First click on a new row only selects it (the arrows' job).
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::MessageRow(2))));
    assert_eq!(s.selection, 2);
    assert_eq!(s.session.focus, Focus::MessageList);
    // Clicking the selected row opens it (Enter's job).
    let effects = reduce(&mut s, Action::Click(ClickTarget::MessageRow(2)));
    // The click starts the message cache read (ticket haeb); the miss
    // starts the fresh foreground load.
    let (cache_id, _) = effect_parts(&effects);
    let effects = complete_cache_miss(&mut s, cache_id);
    let (id, kind) = effect_parts(&effects);
    let OperationKind::LoadMessage(locator) = kind else {
        panic!("expected LoadMessage, got {kind:?}");
    };
    assert_eq!(locator.id, s.messages.items[2].id);
    assert_eq!(s.session.focus, Focus::Reader);
    assert_eq!(s.open_message, Loadable::Loading);
    // The operation id is registered so the result can apply.
    assert!(s.session.operations.get(id).is_some());
}

#[test]
fn click_message_row_out_of_range_is_inert() {
    let mut s = state();
    let before = s.selection;
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::MessageRow(10_000)),
    ));
    assert_eq!(s.selection, before);
    assert_eq!(s.session.focus, Focus::MessageList);
}

#[test]
fn click_mailbox_selects_then_switches() {
    let mut s = state();
    // A different row only selects (focus follows the click).
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::Mailbox(3))));
    assert_eq!(s.mailbox_selection, 3);
    assert_eq!(s.session.focus, Focus::Sidebar);
    // Clicking the selected mailbox switches to it (Enter's job).
    let effects = reduce(&mut s, Action::Click(ClickTarget::Mailbox(3)));
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (_, req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(req.mailbox_id.0, "archive");
    assert_eq!(
        s.active_route().and_then(Route::mailbox_id).unwrap().0,
        "archive"
    );
}

#[test]
fn click_search_field_focuses_it_like_slash() {
    let mut s = state();
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::SearchField)));
    assert_eq!(s.session.focus, Focus::SearchField);
}

#[test]
fn click_compose_button_opens_the_composer() {
    let mut s = state();
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::ComposeButton)));
    assert!(matches!(s.active_route(), Some(Route::Composer)));
    assert_eq!(s.session.focus, Focus::Composer);
}

#[test]
fn click_composer_text_field_focuses_and_places_the_caret() {
    let mut s = state();
    reduce(&mut s, Action::Compose);
    if let Some(composer) = s.session.composer.as_mut() {
        composer.draft.subject = String::from("Quarterly report");
        composer.draft.to = String::from("");
    }
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ComposerField(ComposerField::Subject)),
    ));
    let composer = s.session.composer.as_ref().expect("composer");
    assert_eq!(composer.field, ComposerField::Subject);
    assert_eq!(composer.cursor, "Quarterly report".chars().count());
    assert_eq!(s.session.focus, Focus::Composer);
}

#[test]
fn click_composer_toggles_reveal_their_field() {
    let mut s = state();
    reduce(&mut s, Action::Compose);
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ComposerField(ComposerField::BccToggle)),
    ));
    let composer = s.session.composer.as_ref().expect("composer");
    assert!(composer.show_bcc);
    assert_eq!(composer.field, ComposerField::Bcc);
    // The Bcc toggle is gone from the cycle; clicking it again is inert.
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ComposerField(ComposerField::BccToggle)),
    ));
    let composer = s.session.composer.as_ref().expect("composer");
    assert_eq!(composer.field, ComposerField::Bcc);
}

#[test]
fn click_discard_button_opens_the_confirm_dialog() {
    let mut s = state();
    reduce(&mut s, Action::Compose);
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ComposerField(ComposerField::Discard)),
    ));
    assert!(matches!(
        s.session.overlay,
        Some(Overlay::ConfirmDiscard(_))
    ));
}

#[test]
fn click_error_modal_buttons_replay_or_close() {
    // Build a retryable failure like the tests above do.
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    reduce(&mut s, failure(id, &page_kind(&req), "himalaya exploded"));
    assert!(matches!(s.session.overlay, Some(Overlay::Error(_))));

    // Clicking Retry replays the intent under a new operation id (plan §12).
    let effects = reduce(
        &mut s,
        Action::Click(ClickTarget::ErrorButton(ModalButton::Retry)),
    );
    let (replayed, kind) = effect_parts(&effects);
    assert_eq!(kind, page_kind(&req));
    assert_ne!(replayed, id);
    assert!(s.session.overlay.is_none());

    // A second failure (the retried page already runs; Refresh re-requests
    // page 0 under a new id), then Dismiss closes without new work.
    let (id, req) = expect_page(&reduce(&mut s, Action::Refresh));
    reduce(&mut s, failure(id, &page_kind(&req), "himalaya exploded"));
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ErrorButton(ModalButton::Dismiss)),
    ));
    assert!(s.session.overlay.is_none());
    assert!(
        s.session.operations.get(id).is_none(),
        "cancelled with the modal"
    );
}

#[test]
fn click_error_modal_retry_without_intent_is_inert() {
    let mut s = state();
    s.session.overlay = Some(Overlay::Error(ErrorDialog {
        code: Some(1),
        detail: String::from("no retry offered"),
        retry: None,
        ambiguous: false,
        more_failures: 0,
        scroll: 0,
        button: ModalButton::Dismiss,
        previous_focus: Focus::MessageList,
    }));
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ErrorButton(ModalButton::Retry)),
    ));
    assert!(
        s.session.overlay.is_some(),
        "a dimmed Retry button does nothing"
    );
}

#[test]
fn click_confirm_discard_buttons_keep_or_delete() {
    // Keep closes the dialog and keeps the draft.
    let mut s = state();
    reduce(&mut s, Action::Compose);
    reduce(&mut s, Action::DiscardDraft);
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ConfirmButton(ConfirmButton::Keep)),
    ));
    assert!(s.session.overlay.is_none());
    assert!(s.session.composer.is_some(), "keep keeps the draft");

    // Discard confirms the deletion (the same path Enter takes).
    reduce(&mut s, Action::DiscardDraft);
    let effects = reduce(
        &mut s,
        Action::Click(ClickTarget::ConfirmButton(ConfirmButton::Discard)),
    );
    assert!(s.session.composer.is_none());
    assert!(matches!(effects[0].kind, OperationKind::DeleteDraft { .. }));
}

#[test]
fn modal_intercepts_clicks_onto_the_screen_behind_it() {
    let mut s = state();
    reduce(&mut s, Action::Compose);
    reduce(&mut s, Action::DiscardDraft);
    let selection_before = s.selection;
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::MessageRow(4))));
    assert_eq!(s.selection, selection_before);
    assert!(s.session.overlay.is_some(), "the dialog stays open");
}

#[test]
fn click_attachment_chip_selects_then_opens() {
    let mut s = reader_with_attachments();
    // Selecting the second chip.
    no_effects(&reduce(
        &mut s,
        Action::Click(ClickTarget::ReaderAttachment(1)),
    ));
    assert_eq!(s.reader_focus, Some(ReaderFocus::Attachment(1)));
    assert_eq!(s.session.focus, Focus::Reader);
    // Clicking the selected chip opens it (the `o` path: save, then open).
    let effects = reduce(&mut s, Action::Click(ClickTarget::ReaderAttachment(1)));
    let (id, kind) = effect_parts(&effects);
    let OperationKind::SaveAttachment {
        request,
        open_after,
    } = kind
    else {
        panic!("expected SaveAttachment, got {kind:?}");
    };
    assert!(open_after);
    assert_eq!(request.part_id, 5);
    assert!(s.session.operations.get(id).is_some());
}

#[test]
fn click_link_focuses_then_opens_it() {
    let mut s = reader_with_links_and_attachment();
    // The first click only focuses the link (ticket hc9n).
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::ReaderLink(1))));
    assert_eq!(s.reader_focus, Some(ReaderFocus::Link(1)));
    assert_eq!(s.session.focus, Focus::Reader);
    // Clicking the focused link opens it in the browser.
    let effects = reduce(&mut s, Action::Click(ClickTarget::ReaderLink(1)));
    let (_, kind) = effect_parts(&effects);
    let OperationKind::OpenUrl { url } = kind else {
        panic!("expected OpenUrl, got {kind:?}");
    };
    assert_eq!(url, "https://two.example/b");
    // An out-of-range link index is inert.
    let mut s = reader_with_links_and_attachment();
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::ReaderLink(9))));
    assert_eq!(s.reader_focus, None);
}

#[test]
fn click_composer_send_button_sends_like_ctrl_enter() {
    let mut s = state();
    reduce(&mut s, Action::Compose);
    if let Some(composer) = s.session.composer.as_mut() {
        composer.draft.to = String::from("probe@tmail.local");
        composer.draft.subject = String::from("hello");
    }
    // Seed the clock so the send path can mint draft ids.
    tick(&mut s, 0);
    let effects = reduce(
        &mut s,
        Action::Click(ClickTarget::ComposerField(ComposerField::Send)),
    );
    let (_, kind) = effect_parts(&effects);
    assert!(matches!(kind, OperationKind::Send { .. }));
    assert!(s.session.composer.as_ref().is_some_and(|c| c.sending));
}

// ── Phase 10 feedback: capture toggle (yemf) ─────────────────────────────

#[test]
fn m_toggles_mouse_capture_state() {
    let mut s = state();
    assert!(!s.settings.mouse_capture);
    no_effects(&reduce(&mut s, Action::ToggleMouseCapture));
    assert!(s.settings.mouse_capture);
    assert!(
        s.session
            .status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("off —") || m.contains("on —"))
    );
    no_effects(&reduce(&mut s, Action::ToggleMouseCapture));
    assert!(!s.settings.mouse_capture);
}
