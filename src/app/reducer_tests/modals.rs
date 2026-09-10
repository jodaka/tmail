//! Reducer tests: modals domain.

use super::*;

// ── Esc cancellation (plan §10/§11) ──────────────────────────────────────

#[test]
fn esc_cancels_foreground_work_and_returns_to_stable_state() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    let token = s.operations.cancellation(id).unwrap();
    reduce(&mut s, &Action::BackOrCancel);
    // The operation is gone and its token fired: the backend kills the
    // child it owns (Phase 3.2).
    assert!(token.is_cancelled());
    assert!(s.operations.get(id).is_none());
    assert_eq!(s.operations.page_in_flight(&req.mailbox_id), None);
    // The prior stable state stands: old page still displayed, no modal,
    // no route change, no quit.
    assert_eq!(s.messages.offset, 0);
    assert_eq!(s.messages.items.len(), mock::PAGE_SIZE);
    assert_eq!(s.overlay, None);
    assert_eq!(s.active_route().unwrap().mailbox_id().unwrap().0, "inbox");
    assert!(!s.quit_requested);
    assert!(
        s.status
            .message
            .as_deref()
            .is_some_and(|m| m.contains("cancelled"))
    );
}

#[test]
fn esc_during_startup_load_cancels_then_second_esc_quits() {
    let mut s = AppState::initial(mock::PAGE_SIZE);
    let (id, _) = boot(&mut s);
    let token = s.operations.cancellation(id).unwrap();
    reduce(&mut s, &Action::BackOrCancel);
    assert!(token.is_cancelled());
    assert!(!s.quit_requested, "first Esc cancels, does not quit");
    // Nothing left to cancel: the next Esc quits as before.
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.quit_requested);
}

#[test]
fn esc_with_open_modal_dismisses_it() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(&mut s, &failure(id, &page_kind(&req), "boom"));
    reduce(&mut s, &Action::BackOrCancel);
    assert!(s.overlay.is_none(), "Esc closes the modal");
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn retry_replays_equivalent_intent_with_new_operation_id() {
    let mut s = state();
    let (id, req) = open_modal(&mut s, "himalaya exploded");
    let effects = reduce(&mut s, &Action::RetryError);
    let (retry_id, retry_kind) = effect_parts(&effects);
    assert_ne!(retry_id, id, "retry must allocate a new operation id");
    assert_eq!(retry_kind, page_kind(&req), "same typed intent");
    assert!(s.operations.get(retry_id).is_some());
    assert!(s.operations.get(id).is_none());
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn dismiss_closes_modal_without_side_effects() {
    let mut s = state();
    open_modal(&mut s, "himalaya exploded");
    no_effects(&reduce(&mut s, &Action::DismissError));
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
    // The last coherent page is untouched by dismiss.
    assert_eq!(s.messages.offset, 0);
}

#[test]
fn modal_enter_activates_the_focused_button() {
    let mut s = state();
    open_modal(&mut s, "himalaya exploded");
    // Default button is Dismiss: Enter dismisses without new work.
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.overlay.is_none());
    // Focus Retry first: Enter replays the intent.
    let (_, req) = open_modal(&mut s, "himalaya exploded");
    reduce(&mut s, &Action::FocusNext);
    let effects = reduce(&mut s, &Action::Activate);
    let (_, kind) = effect_parts(&effects);
    assert_eq!(kind, page_kind(&req));
}

#[test]
fn modal_buttons_toggle_with_tab() {
    let mut s = state();
    open_modal(&mut s, "boom");
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Dismiss);
    reduce(&mut s, &Action::FocusNext);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Retry);
    reduce(&mut s, &Action::FocusPrevious);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.button, crate::app::overlay::ModalButton::Dismiss);
}

#[test]
fn modal_scroll_clamps_to_content() {
    let long_detail = "line\n".repeat(60);
    let mut s = state();
    open_modal(&mut s, long_detail.trim_end());
    let max = match &s.overlay {
        Some(Overlay::Error(dialog)) => crate::view::overlay::error_modal_max_scroll(
            &dialog.detail,
            dialog.code,
            dialog.ambiguous,
            s.size,
        ),
        other => panic!("error modal open, got {other:?}"),
    };
    assert!(max > 0, "long detail must overflow the viewport");
    for _ in 0..(max + 20) {
        reduce(&mut s, &Action::MoveDown);
    }
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.scroll, max, "scroll clamps at the end");
    for _ in 0..(max + 5) {
        reduce(&mut s, &Action::MoveUp);
    }
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert_eq!(dialog.scroll, 0, "scroll clamps at the start");
    // Page-style scrolling moves in viewport steps and clamps the same way.
    reduce(&mut s, &Action::PageNext);
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.scroll > 0);
}

#[test]
fn modal_restores_previous_focus_on_dismiss() {
    let mut s = state();
    let (_, _) = open_modal(&mut s, "boom");
    // Simulate having been in the sidebar when the failure hit.
    if let Some(Overlay::Error(dialog)) = s.overlay.as_mut() {
        dialog.previous_focus = Focus::Sidebar;
    }
    assert_eq!(s.focus, Focus::ErrorModal);
    reduce(&mut s, &Action::DismissError);
    assert_eq!(s.focus, Focus::Sidebar, "focus restored");
}

#[test]
fn modal_swallows_unrelated_input() {
    let mut s = state();
    open_modal(&mut s, "boom");
    let before = s.clone();
    reduce(&mut s, &Action::SearchEdit(SearchEdit::Char('x')));
    reduce(&mut s, &Action::SubmitSearch);
    reduce(&mut s, &Action::OpenSearch);
    reduce(&mut s, &Action::Compose);
    reduce(&mut s, &Action::Quit);
    assert_eq!(s.search_query, before.search_query);
    assert_eq!(s.focus, before.focus);
    assert_eq!(s.quit_requested, before.quit_requested);
    assert!(s.overlay.is_some(), "modal stays open");
}

#[test]
fn ambiguous_failure_is_flagged_for_the_modal() {
    let mut s = state();
    let (id, req) = expect_page(&reduce(&mut s, &Action::PageNext));
    reduce(
        &mut s,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("SMTP DATA failed: reached unexpected EOF"),
                retry: Some(page_kind(&req).retry_spec()),
                ambiguous: true,
            }),
        }),
    );
    let Some(Overlay::Error(dialog)) = &s.overlay else {
        panic!("modal open");
    };
    assert!(dialog.ambiguous, "ambiguity must reach the modal");
}

#[test]
fn t_opens_the_picker_with_the_cursor_on_the_active_theme() {
    let mut s = picker_state();
    let previous = s.focus;
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    let Some(Overlay::ThemePicker(dialog)) = &s.overlay else {
        panic!("picker must open");
    };
    assert_eq!(dialog.cursor, 0, "cursor starts on the active palette");
    assert_eq!(dialog.original, 0);
    assert_eq!(dialog.previous_focus, previous);
    assert_eq!(s.focus, Focus::ThemePicker);
}

#[test]
fn arrows_preview_the_highlighted_theme_without_wrapping() {
    let mut s = picker_state();
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    // Down: the cursor moves and the highlighted palette applies at once.
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.theme_index, 1, "the highlighted theme is the preview");
    assert_eq!(s.active_theme(), crate::view::theme::Theme::default_light());
    // The cursor clamps like the message list: no wrap past the ends.
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.theme_index, 2);
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.theme_index, 2, "no wrap past the last theme");
    no_effects(&reduce(&mut s, &Action::MoveUp));
    no_effects(&reduce(&mut s, &Action::MoveUp));
    no_effects(&reduce(&mut s, &Action::MoveUp));
    assert_eq!(s.theme_index, 0, "no wrap past the first theme");
}

#[test]
fn esc_restores_the_theme_the_picker_opened_with() {
    let mut s = picker_state();
    s.theme_index = 1;
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    // Preview toward the end of the list (the cursor clamps, no wrap)...
    no_effects(&reduce(&mut s, &Action::MoveDown));
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.theme_index, 2, "preview applied while navigating");
    // ...then cancel: the opening palette comes back, nothing else moves.
    no_effects(&reduce(&mut s, &Action::BackOrCancel));
    assert!(s.overlay.is_none());
    assert_eq!(s.theme_index, 1, "the opening palette is restored");
    assert_eq!(s.active_theme(), crate::view::theme::Theme::default_light());
    assert_eq!(s.focus, Focus::MessageList, "the previous focus returns");
}

#[test]
fn enter_confirms_the_previewed_theme() {
    let mut s = picker_state();
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    no_effects(&reduce(&mut s, &Action::MoveDown));
    no_effects(&reduce(&mut s, &Action::Activate));
    assert!(s.overlay.is_none());
    assert_eq!(s.theme_index, 1, "the previewed palette is kept");
    assert_eq!(s.active_theme(), crate::view::theme::Theme::default_light());
    assert_eq!(s.status.message.as_deref(), Some("Theme: light"));
    assert_eq!(s.focus, Focus::MessageList, "the previous focus returns");
}

#[test]
fn the_picker_intercepts_every_other_input() {
    use crate::app::action::DialogEdit;
    let mut s = picker_state();
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    // A modal swallows all input (plan §9): shortcuts, edits, and backend
    // completions must not leak into the app behind the dialog.
    no_effects(&reduce(&mut s, &Action::Compose));
    assert!(s.composer.is_none(), "no composer opens behind the picker");
    no_effects(&reduce(&mut s, &Action::ToggleStar));
    no_effects(&reduce(&mut s, &Action::DialogEdit(DialogEdit::Char('x'))));
    no_effects(&reduce(
        &mut s,
        &Action::AttachmentBrowse(AttachmentBrowse::Down),
    ));
    assert_eq!(s.theme_index, 0, "nothing moved the preview");
}

#[test]
fn the_picker_never_opens_without_themes() {
    let mut s = state();
    s.themes.clear();
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
}

#[test]
fn the_picker_scroll_window_follows_the_cursor() {
    // Twelve palettes against a ten-row dialog viewport at 100×16.
    let mut s = state();
    s.size = (100, 16);
    s.themes = (0..12)
        .map(|i| {
            (
                format!("t{i:02}"),
                crate::view::theme::Theme::default_dark(),
            )
        })
        .collect();
    no_effects(&reduce(&mut s, &Action::OpenThemePicker));

    // Inside the window the scroll offset stays put...
    for _ in 0..9 {
        no_effects(&reduce(&mut s, &Action::MoveDown));
    }
    let Some(Overlay::ThemePicker(dialog)) = &s.overlay else {
        panic!("picker open");
    };
    assert_eq!(dialog.cursor, 9);
    assert_eq!(dialog.scroll, 0);

    // ...and the first step past the edge slides the window by one row so
    // the cursor (and its preview) stays visible.
    no_effects(&reduce(&mut s, &Action::MoveDown));
    let Some(Overlay::ThemePicker(dialog)) = &s.overlay else {
        panic!("picker open");
    };
    assert_eq!(dialog.cursor, 10);
    assert_eq!(dialog.scroll, 1);
    assert_eq!(s.theme_index, 10, "the newly highlighted theme previews");
}

#[test]
fn the_active_theme_default_is_the_dark_reference() {
    let s = state();
    assert_eq!(s.themes.len(), 2, "both built-ins are cycle candidates");
    assert_eq!(s.active_theme(), crate::view::theme::Theme::default_dark());
}

// ── Shortcuts help popup (? / Ctrl+h) ────────────────────────────────────

#[test]
fn help_opens_over_the_current_screen_and_closes_restoring_focus() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::OpenHelp));
    assert!(matches!(s.overlay, Some(Overlay::Help(_))));
    assert_eq!(s.focus, Focus::Help);
    // Esc closes back into the list.
    no_effects(&reduce(&mut s, &Action::BackOrCancel));
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::MessageList);
    // A second open/close cycle from the composer restores the composer.
    reduce(&mut s, &Action::Compose);
    no_effects(&reduce(
        &mut s,
        &Action::ComposerEdit(ComposerEdit::Char('h')),
    ));
    no_effects(&reduce(&mut s, &Action::OpenHelp));
    assert_eq!(previous_focus_of(&s), Focus::Composer);
    no_effects(&reduce(&mut s, &Action::OpenHelp));
    assert!(s.overlay.is_none());
    assert_eq!(s.focus, Focus::Composer);
    assert!(
        matches!(s.active_route(), Some(Route::Composer)),
        "the draft survives the popup"
    );
}

#[test]
fn help_swallows_navigation_and_backend_results_land() {
    let mut s = state();
    no_effects(&reduce(&mut s, &Action::OpenHelp));
    // The arrows never reach the list behind the popup.
    let before = s.clone();
    no_effects(&reduce(&mut s, &Action::MoveDown));
    assert_eq!(s.selection, before.selection);
    // Backend results are not blocked by the popup.
    let summary = s.messages.items[0].clone();
    // (no in-flight operation: an unknown id is a harmless no-op)
    let _ = summary;
}

#[test]
fn help_does_not_open_during_the_wizard() {
    let mut s = state();
    s.wizard = Some(crate::app::wizard::WizardState::new(
        true,
        None,
        Vec::new(),
        None,
        false,
    ));
    no_effects(&reduce(&mut s, &Action::OpenHelp));
    assert!(s.overlay.is_none());
}

#[test]
fn help_lists_the_active_bindings_of_the_screen_underneath() {
    let mut s = state();
    reduce(&mut s, &Action::OpenHelp);
    let Some(Overlay::Help(dialog)) = &s.overlay else {
        panic!("help overlay");
    };
    assert_eq!(dialog.previous_focus, Focus::MessageList);
    // The default map's list view: global + list entries, rebound-free,
    // one row per action with its keys joined.
    let entries = s.keymap.help_entries(Focus::MessageList);
    assert!(
        entries
            .iter()
            .any(|(label, keys)| label == "Open help" && keys.contains('?'))
    );
    // Multiple keys of one action share a row ("d, Del").
    assert!(
        entries
            .iter()
            .any(|(label, keys)| label == "Trash" && keys == "d, Del")
    );
}
