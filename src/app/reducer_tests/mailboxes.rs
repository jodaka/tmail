//! Reducer tests: the compact mode's focus and Mailboxes popup (issue
//! brnw). Compact mode hides the sidebar, so Tab never lands on it and
//! the list head's mailbox title carries the mailbox switcher instead.

use super::*;

fn compact_state() -> AppState {
    let mut s = state();
    s.session.size = (100, 30);
    s
}

#[test]
fn compact_tab_cycles_list_and_title_never_the_sidebar() {
    let mut s = compact_state();
    s.session.focus = Focus::MessageList;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::MailboxTitle);
    // Tab wraps back into the list; the hidden sidebar is skipped.
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::MessageList);
    // Shift+Tab walks the same two controls backward.
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(s.session.focus, Focus::MailboxTitle);
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(s.session.focus, Focus::MessageList);
    // Tab inside the search field steps out onto the title too.
    s.session.focus = Focus::SearchField;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::MailboxTitle);
}

#[test]
fn full_tab_still_reaches_the_sidebar_not_the_title() {
    let mut s = state();
    s.session.focus = Focus::MessageList;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Sidebar);
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::MessageList);
    // The title is a plain header in full mode: never a tab stop.
    s.session.focus = Focus::SearchField;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Sidebar);
}

#[test]
fn enter_on_the_focused_title_opens_the_mailboxes_popup() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    no_effects(&reduce(&mut s, Action::Activate));
    assert!(matches!(
        s.session.overlay,
        Some(crate::app::overlay::Overlay::Mailboxes(_))
    ));
    assert_eq!(s.session.focus, Focus::Mailboxes);
    // The cursor starts on the displayed mailbox (Inbox, row 0).
    let dialog = match &s.session.overlay {
        Some(crate::app::overlay::Overlay::Mailboxes(dialog)) => dialog,
        _ => panic!("popup open"),
    };
    assert_eq!(dialog.cursor, 0);
    assert_eq!(dialog.scroll, 0);
}

#[test]
fn popup_enter_on_another_mailbox_switches_and_closes() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    reduce(&mut s, Action::Activate);
    // Move onto Sent, then activate.
    reduce(&mut s, Action::MoveDown);
    let effects = reduce(&mut s, Action::Activate);
    // The popup is closed and the switch's cold-context page load
    // started for Sent. The switch lands focus in the new mailbox's
    // list — exactly what the sidebar's Enter does (switch_mailbox).
    assert!(s.session.overlay.is_none());
    assert_eq!(s.session.focus, Focus::MessageList);
    let (cache_id, ..) = expect_cache_list_load(&effects);
    let (id, req) = expect_page(&complete_cache_miss(&mut s, cache_id));
    assert_eq!(req.mailbox_id.0, "sent");
    complete_page_ok(&mut s, id, &req, 0);
    assert_eq!(s.mailbox_selection, 1, "sidebar cursor follows");
    assert!(matches!(
        s.active_route(),
        Some(Route::Mailbox(route)) if route.mailbox_id.0 == "sent"
    ));
}

#[test]
fn popup_enter_on_the_displayed_mailbox_just_closes() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    reduce(&mut s, Action::Activate);
    no_effects(&reduce(&mut s, Action::Activate));
    assert!(s.session.overlay.is_none());
    assert_eq!(s.session.focus, Focus::MailboxTitle);
    // No switch ran: the route is still Inbox.
    assert!(matches!(
        s.active_route(),
        Some(Route::Mailbox(route)) if route.mailbox_id.0 == "inbox"
    ));
}

#[test]
fn popup_esc_closes_and_restores_the_title_focus() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    reduce(&mut s, Action::Activate);
    s.session.focus = Focus::MessageList; // the user tabbed away meanwhile? no: keep dialog consistent
    let _ = reduce(&mut s, Action::BackOrCancel);
    assert!(s.session.overlay.is_none());
    // Focus restored to where the popup opened (the title button).
    assert_eq!(s.session.focus, Focus::MailboxTitle);
}

#[test]
fn popup_arrows_move_the_cursor_and_clamp() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    reduce(&mut s, Action::Activate);
    // Up from row 0 stays (non-wrapping, like the account switcher).
    reduce(&mut s, Action::MoveUp);
    assert_eq!(
        match &s.session.overlay {
            Some(crate::app::overlay::Overlay::Mailboxes(dialog)) => dialog.cursor,
            _ => panic!("popup open"),
        },
        0
    );
    // Down walks to the end and stops there.
    for _ in 0..10 {
        reduce(&mut s, Action::MoveDown);
    }
    assert_eq!(
        match &s.session.overlay {
            Some(crate::app::overlay::Overlay::Mailboxes(dialog)) => dialog.cursor,
            _ => panic!("popup open"),
        },
        5,
        "six mock mailboxes"
    );
}

#[test]
fn click_on_the_compact_title_opens_the_popup() {
    let mut s = compact_state();
    no_effects(&reduce(&mut s, Action::Click(ClickTarget::MailboxTitle)));
    assert!(matches!(
        s.session.overlay,
        Some(crate::app::overlay::Overlay::Mailboxes(_))
    ));
    assert_eq!(s.session.focus, Focus::Mailboxes);
}

#[test]
fn resize_moves_focus_off_controls_the_layout_hides() {
    // Sidebar focus + shrink: the sidebar vanishes under the cursor.
    let mut s = state();
    s.session.focus = Focus::Sidebar;
    reduce(
        &mut s,
        Action::Resize {
            width: 100,
            height: 30,
        },
    );
    assert_eq!(s.session.focus, Focus::MessageList);
    // Title focus + grow: the title is a plain header again.
    s.session.focus = Focus::MailboxTitle;
    reduce(
        &mut s,
        Action::Resize {
            width: 152,
            height: 40,
        },
    );
    assert_eq!(s.session.focus, Focus::MessageList);
    // Same-size resizes never touch valid focus.
    s.session.focus = Focus::MailboxTitle;
    reduce(
        &mut s,
        Action::Resize {
            width: 100,
            height: 30,
        },
    );
    assert_eq!(s.session.focus, Focus::MailboxTitle);
}

#[test]
fn resize_while_composing_returns_parked_sidebar_focus_to_the_composer() {
    let mut s = state();
    compose(&mut s);
    // Tab out to the sidebar (full mode: the compose-mode folder list).
    s.session.composer.as_mut().unwrap().field = ComposerField::Discard;
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Sidebar);
    // Shrink: the sidebar vanished, so the focus returns into the
    // composer (its fields are still on screen).
    reduce(
        &mut s,
        Action::Resize {
            width: 100,
            height: 30,
        },
    );
    assert_eq!(s.session.focus, Focus::Composer);
}

#[test]
fn compact_composer_tab_wraps_instead_of_stepping_out_to_the_hidden_sidebar() {
    let mut s = compact_state();
    compose(&mut s);
    s.session.composer.as_mut().unwrap().field = ComposerField::Discard;
    // Tab past the last control: the sidebar is hidden in compact mode,
    // so the cycle stays inside the composer (wraps to the first field).
    reduce(&mut s, Action::FocusNext);
    assert_eq!(s.session.focus, Focus::Composer);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::To
    );
    // Shift+Tab before the first control wraps to the last.
    reduce(&mut s, Action::FocusPrevious);
    assert_eq!(
        s.session.composer.as_ref().unwrap().field,
        ComposerField::Discard
    );
}

#[test]
fn help_opens_over_the_title_focus_too() {
    let mut s = compact_state();
    s.session.focus = Focus::MailboxTitle;
    no_effects(&reduce(&mut s, Action::OpenHelp));
    assert!(matches!(
        s.session.overlay,
        Some(crate::app::overlay::Overlay::Help(_))
    ));
}
