//! Keyboard mapping unit tests (plan §10).

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn navigation_keys_map_in_list_focus() {
    let f = Focus::MessageList;
    assert_eq!(to_action(plain(KeyCode::Up), f), Some(Action::MoveUp));
    assert_eq!(to_action(plain(KeyCode::Down), f), Some(Action::MoveDown));
    assert_eq!(
        to_action(plain(KeyCode::Left), f),
        Some(Action::PagePrevious)
    );
    assert_eq!(to_action(plain(KeyCode::Right), f), Some(Action::PageNext));
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
    assert_eq!(
        to_action(plain(KeyCode::Esc), f),
        Some(Action::BackOrCancel)
    );
    assert_eq!(to_action(plain(KeyCode::Tab), f), Some(Action::FocusNext));
    assert_eq!(
        to_action(plain(KeyCode::BackTab), f),
        Some(Action::FocusPrevious)
    );
}

#[test]
fn global_shortcuts() {
    let f = Focus::MessageList;
    assert_eq!(
        to_action(key(KeyCode::Char('c'), KeyModifiers::CONTROL), f),
        Some(Action::Quit)
    );
    assert_eq!(
        to_action(key(KeyCode::Char('r'), KeyModifiers::CONTROL), f),
        Some(Action::Refresh)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('/')), f),
        Some(Action::OpenSearch)
    );
}

#[test]
fn list_shortcuts_per_input_contract() {
    let f = Focus::MessageList;
    assert_eq!(to_action(plain(KeyCode::Char('r')), f), Some(Action::Reply));
    assert_eq!(
        to_action(plain(KeyCode::Char('a')), f),
        Some(Action::ReplyAll)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('f')), f),
        Some(Action::Forward)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('e')), f),
        Some(Action::Archive)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('s')), f),
        Some(Action::ToggleStar)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('u')), f),
        Some(Action::MarkUnread)
    );
    assert_eq!(to_action(plain(KeyCode::Delete), f), Some(Action::Trash));
}

#[test]
fn sidebar_focus_keeps_shortcuts_and_slash() {
    let f = Focus::Sidebar;
    assert_eq!(
        to_action(plain(KeyCode::Char('c')), f),
        Some(Action::Compose)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('/')), f),
        Some(Action::OpenSearch)
    );
    assert_eq!(to_action(plain(KeyCode::Up), f), Some(Action::MoveUp));
}

#[test]
fn search_field_swallows_letters_into_text_no_shortcuts() {
    let f = Focus::SearchField;
    for c in "cafesuxyz?/".chars() {
        assert_eq!(
            to_action(plain(KeyCode::Char(c)), f),
            Some(Action::SearchEdit(SearchEdit::Char(c))),
            "{c} must be text while editing"
        );
    }
    // Backspace edits, Delete is NOT trash while editing.
    assert_eq!(
        to_action(plain(KeyCode::Backspace), f),
        Some(Action::SearchEdit(SearchEdit::Backspace))
    );
    assert_eq!(to_action(plain(KeyCode::Delete), f), None);
    // Enter still activates (submits), Esc still leaves the field.
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
    assert_eq!(
        to_action(plain(KeyCode::Esc), f),
        Some(Action::BackOrCancel)
    );
}

#[test]
fn slash_does_not_open_search_when_already_editing() {
    let f = Focus::SearchField;
    assert_eq!(
        to_action(plain(KeyCode::Char('/')), f),
        Some(Action::SearchEdit(SearchEdit::Char('/')))
    );
}

#[test]
fn j_k_and_help_are_not_bound() {
    for f in [Focus::MessageList, Focus::Sidebar] {
        assert_eq!(to_action(plain(KeyCode::Char('j')), f), None);
        assert_eq!(to_action(plain(KeyCode::Char('k')), f), None);
        assert_eq!(to_action(plain(KeyCode::Char('?')), f), None);
    }
}

#[test]
fn ctrl_c_quits_everywhere() {
    for f in [Focus::SearchField, Focus::Sidebar, Focus::MessageList] {
        assert_eq!(
            to_action(key(KeyCode::Char('c'), KeyModifiers::CONTROL), f),
            Some(Action::Quit)
        );
    }
}

#[test]
fn ctrl_enter_sends_plain_enter_activates() {
    let f = Focus::MessageList;
    assert_eq!(
        to_action(key(KeyCode::Enter, KeyModifiers::CONTROL), f),
        Some(Action::Send)
    );
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
}

// ── Composer (plan §19 Phase 6.1) ────────────────────────────────────────

#[test]
fn composer_letters_are_text_never_shortcuts() {
    let f = Focus::Composer;
    for c in "cafrehsu/?z".chars() {
        assert_eq!(
            to_action(plain(KeyCode::Char(c)), f),
            Some(Action::ComposerEdit(ComposerEdit::Char(c))),
            "{c} must be composed text"
        );
    }
    // Ctrl/Alt chords stay unbound (global Ctrl+C/Ctrl+R matched earlier).
    assert_eq!(
        to_action(key(KeyCode::Char('c'), KeyModifiers::ALT), f),
        None
    );
    assert_eq!(
        to_action(key(KeyCode::Char('r'), KeyModifiers::ALT), f),
        None
    );
}

#[test]
fn composer_backspace_edits_and_delete_is_not_trash() {
    let f = Focus::Composer;
    assert_eq!(
        to_action(plain(KeyCode::Backspace), f),
        Some(Action::ComposerEdit(ComposerEdit::Backspace))
    );
    assert_eq!(
        to_action(plain(KeyCode::Delete), f),
        Some(Action::ComposerEdit(ComposerEdit::Delete))
    );
}

#[test]
fn composer_arrows_move_the_caret() {
    let f = Focus::Composer;
    assert_eq!(
        to_action(plain(KeyCode::Up), f),
        Some(Action::ComposerEdit(ComposerEdit::CursorUp))
    );
    assert_eq!(
        to_action(plain(KeyCode::Down), f),
        Some(Action::ComposerEdit(ComposerEdit::CursorDown))
    );
    assert_eq!(
        to_action(plain(KeyCode::Left), f),
        Some(Action::ComposerEdit(ComposerEdit::CursorLeft))
    );
    assert_eq!(
        to_action(plain(KeyCode::Right), f),
        Some(Action::ComposerEdit(ComposerEdit::CursorRight))
    );
}

#[test]
fn composer_enter_activates_ctrl_enter_sends_esc_leaves() {
    let f = Focus::Composer;
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
    assert_eq!(
        to_action(key(KeyCode::Enter, KeyModifiers::CONTROL), f),
        Some(Action::Send)
    );
    assert_eq!(
        to_action(plain(KeyCode::Esc), f),
        Some(Action::BackOrCancel)
    );
    assert_eq!(to_action(plain(KeyCode::Tab), f), Some(Action::FocusNext));
    assert_eq!(
        to_action(plain(KeyCode::BackTab), f),
        Some(Action::FocusPrevious)
    );
}

#[test]
fn global_shortcuts_still_work_from_the_composer() {
    let f = Focus::Composer;
    assert_eq!(
        to_action(key(KeyCode::Char('c'), KeyModifiers::CONTROL), f),
        Some(Action::Quit)
    );
    assert_eq!(
        to_action(key(KeyCode::Char('r'), KeyModifiers::CONTROL), f),
        Some(Action::Refresh)
    );
}

#[test]
fn unbound_keys_return_none() {
    let f = Focus::MessageList;
    assert_eq!(to_action(plain(KeyCode::Char('z')), f), None);
    assert_eq!(to_action(plain(KeyCode::F(1)), f), None);
    assert_eq!(to_action(plain(KeyCode::Home), f), None);
    // Ctrl combos other than c/r are unbound.
    assert_eq!(
        to_action(key(KeyCode::Char('x'), KeyModifiers::CONTROL), f),
        None
    );
    // Plain chars with alt are unbound (no accidental shortcuts).
    assert_eq!(
        to_action(key(KeyCode::Char('e'), KeyModifiers::ALT), f),
        None
    );
}
