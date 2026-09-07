//! Keyboard mapping unit tests (plan §10).
//!
//! Every test runs against the default keymap (`KeyMap::defaults()`), so
//! the defaults registry and the translation layer stay honest together:
//! a registry change that alters behavior fails here, and config-level
//! behavior has its own tests in `input::keymap`.

use super::*;
use crate::config::KeybindingTable;
use crate::input::keymap::KeyMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Test-local translation against the default bindings. Shadows the
/// keymap-aware `to_action` so the behavioral assertions below stay about
/// keys and actions, not about plumbing.
fn to_action(key: KeyEvent, focus: Focus) -> Option<Action> {
    to_action_with(&KeyMap::defaults(), key, focus)
}

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
    // `d` deletes in the list (trash; ticket h1m2) and in the reader
    // (ticket zg41); the reader's attachment save moved to `S`.
    assert_eq!(to_action(plain(KeyCode::Char('d')), f), Some(Action::Trash));
    assert_eq!(
        to_action(plain(KeyCode::Char('d')), Focus::Reader),
        Some(Action::Trash)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('S')), Focus::Reader),
        Some(Action::SaveAttachment)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('u')), f),
        Some(Action::MarkUnread)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('m')), f),
        Some(Action::ToggleMouseCapture)
    );
    assert_eq!(to_action(plain(KeyCode::Delete), f), Some(Action::Trash));
    // `t` opens the theme picker (ticket k5ba).
    assert_eq!(
        to_action(plain(KeyCode::Char('t')), f),
        Some(Action::OpenThemePicker)
    );
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

/// `q` mirrors Esc wherever shortcuts fire: list, sidebar, reader, and
/// over a modal — and it never fires while text entry is focused.
#[test]
fn q_mirrors_esc_outside_text_entry() {
    for f in [
        Focus::MessageList,
        Focus::Sidebar,
        Focus::Reader,
        Focus::ErrorModal,
    ] {
        assert_eq!(
            to_action(plain(KeyCode::Char('q')), f),
            to_action(plain(KeyCode::Esc), f),
            "q must equal Esc at focus {f}"
        );
        assert_eq!(
            to_action(plain(KeyCode::Char('q')), f),
            Some(Action::BackOrCancel)
        );
    }
}

#[test]
fn q_is_text_while_typing() {
    for f in [Focus::SearchField, Focus::Composer, Focus::Dialog] {
        let expected = match f {
            Focus::SearchField => Some(Action::SearchEdit(SearchEdit::Char('q'))),
            Focus::Composer => Some(Action::ComposerEdit(ComposerEdit::Char('q'))),
            _ => Some(Action::DialogEdit(DialogEdit::Char('q'))),
        };
        assert_eq!(
            to_action(plain(KeyCode::Char('q')), f),
            expected,
            "q must type at focus {f}"
        );
    }
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

// ── Attachment path dialog (plan §15, Phase 8.1) ─────────────────────────

#[test]
fn dialog_letters_are_text_never_shortcuts() {
    let f = Focus::Dialog;
    for c in "cafesu/?~".chars() {
        assert_eq!(
            to_action(plain(KeyCode::Char(c)), f),
            Some(Action::DialogEdit(DialogEdit::Char(c))),
            "{c} must be dialog text"
        );
    }
    // Ctrl/Alt chords stay unbound (global Ctrl+C/Ctrl+R matched earlier).
    assert_eq!(
        to_action(key(KeyCode::Char('c'), KeyModifiers::ALT), f),
        None
    );
}

#[test]
fn dialog_edits_and_caret_but_no_vertical_movement() {
    let f = Focus::Dialog;
    assert_eq!(
        to_action(plain(KeyCode::Backspace), f),
        Some(Action::DialogEdit(DialogEdit::Backspace))
    );
    assert_eq!(
        to_action(plain(KeyCode::Delete), f),
        Some(Action::DialogEdit(DialogEdit::Delete))
    );
    assert_eq!(
        to_action(plain(KeyCode::Left), f),
        Some(Action::DialogEdit(DialogEdit::CursorLeft))
    );
    assert_eq!(
        to_action(plain(KeyCode::Right), f),
        Some(Action::DialogEdit(DialogEdit::CursorRight))
    );
    assert_eq!(to_action(plain(KeyCode::Up), f), None);
    assert_eq!(to_action(plain(KeyCode::Down), f), None);
}

#[test]
fn dialog_enter_activates_esc_cancels() {
    let f = Focus::Dialog;
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
    assert_eq!(
        to_action(plain(KeyCode::Esc), f),
        Some(Action::BackOrCancel)
    );
}

// ── Theme picker (ticket k5ba) ───────────────────────────────────────────

/// The picker is a list dialog: its arrows bind exactly like the message
/// list's (MoveUp/MoveDown — the reducer previews the highlighted theme),
/// Enter confirms, and Esc/q cancel.
#[test]
fn theme_picker_binds_like_the_message_list() {
    let f = Focus::ThemePicker;
    assert_eq!(to_action(plain(KeyCode::Up), f), Some(Action::MoveUp));
    assert_eq!(to_action(plain(KeyCode::Down), f), Some(Action::MoveDown));
    assert_eq!(to_action(plain(KeyCode::Enter), f), Some(Action::Activate));
    assert_eq!(
        to_action(plain(KeyCode::Esc), f),
        Some(Action::BackOrCancel)
    );
    assert_eq!(
        to_action(plain(KeyCode::Char('q')), f),
        Some(Action::BackOrCancel)
    );
}

/// Reader Backspace trashes the open message (ticket zg41): the removed
/// action row advertised "Delete ⌫", and ⌫ is Backspace on Mac keyboards,
/// where the forward-Delete key never fires. Text-entry foci keep their
/// edit semantics.
#[test]
fn reader_backspace_trashes_like_delete() {
    assert_eq!(
        to_action(plain(KeyCode::Backspace), Focus::Reader),
        Some(Action::Trash)
    );
    assert_eq!(
        to_action(plain(KeyCode::Delete), Focus::Reader),
        Some(Action::Trash)
    );
    // The list keeps Backspace unbound (`d` deletes there).
    assert_eq!(
        to_action(plain(KeyCode::Backspace), Focus::MessageList),
        None
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

#[test]
fn selection_keys_map_per_focus() {
    // Space toggles the bulk mark in the list only (ticket p0s3).
    assert_eq!(
        to_action(plain(KeyCode::Char(' ')), Focus::MessageList),
        Some(Action::ToggleSelected)
    );
    assert_eq!(to_action(plain(KeyCode::Char(' ')), Focus::Sidebar), None);
    // In text-entry foci the space is typed, never a shortcut.
    assert_eq!(
        to_action(plain(KeyCode::Char(' ')), Focus::Composer),
        Some(Action::ComposerEdit(ComposerEdit::Char(' ')))
    );
    assert_eq!(
        to_action(plain(KeyCode::Char(' ')), Focus::SearchField),
        Some(Action::SearchEdit(SearchEdit::Char(' ')))
    );
}

#[test]
fn ctrl_a_is_select_all_where_shortcuts_are_accepted() {
    let ctrl_a = key(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(
        to_action(ctrl_a, Focus::MessageList),
        Some(Action::SelectAll)
    );
    assert_eq!(to_action(ctrl_a, Focus::Sidebar), Some(Action::SelectAll));
    // Text-entry foci keep their pre-existing behavior: the letter types.
    assert_eq!(
        to_action(ctrl_a, Focus::SearchField),
        Some(Action::SearchEdit(SearchEdit::Char('a')))
    );
    assert_eq!(to_action(ctrl_a, Focus::Composer), None);
}

#[test]
fn i_maps_to_mark_read() {
    assert_eq!(
        to_action(plain(KeyCode::Char('i')), Focus::MessageList),
        Some(Action::MarkRead)
    );
}

#[test]
fn ctrl_e_opens_the_external_editor_in_the_composer_only() {
    let ctrl_e = key(KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(
        to_action(ctrl_e, Focus::Composer),
        Some(Action::EditExternal)
    );
    // Outside the composer the combo does nothing: the editor edits the
    // draft body (plan §14).
    assert_eq!(to_action(ctrl_e, Focus::MessageList), None);
    assert_eq!(to_action(ctrl_e, Focus::Sidebar), None);
}

#[test]
fn legacy_ctrl_chords_match_the_keys_the_user_presses() {
    // Legacy terminal input delivers Ctrl+] as the byte 0x1D, which
    // crossterm reports as Char('5') + CONTROL (see
    // `normalize_ctrl_chords`). The default keymap binds none of the four
    // legacy chords, so rebuild one the way a user config does.
    let tables = [KeybindingTable {
        context: String::from("global"),
        entries: vec![(
            String::from("next_page"),
            vec![String::from("→"), String::from("Ctrl+]")],
        )],
    }];
    let keymap = KeyMap::build(&tables).keymap;
    let f = Focus::MessageList;
    // The event crossterm actually produces for Ctrl+] pages next.
    assert_eq!(
        to_action_with(&keymap, key(KeyCode::Char('5'), KeyModifiers::CONTROL), f),
        Some(Action::PageNext)
    );
    // The other three legacy bytes map to their conventional keys too
    // (0x1C→`\`, 0x1E→`^`, 0x1F→`_`).
    let tables = [KeybindingTable {
        context: String::from("global"),
        entries: vec![(
            String::from("compose"),
            vec![
                String::from("Ctrl+\\"),
                String::from("Ctrl+^"),
                String::from("Ctrl+_"),
            ],
        )],
    }];
    let keymap = KeyMap::build(&tables).keymap;
    for (reported, pressed) in [('4', '\\'), ('6', '^'), ('7', '_')] {
        assert_eq!(
            to_action_with(
                &keymap,
                key(KeyCode::Char(reported), KeyModifiers::CONTROL),
                f
            ),
            Some(Action::Compose),
            "Char({reported})+Ctrl must fire the {pressed:?} binding"
        );
    }
    // Chordless digits are text, never a remap — in the search field…
    assert_eq!(
        to_action(plain(KeyCode::Char('5')), Focus::SearchField),
        Some(Action::SearchEdit(SearchEdit::Char('5')))
    );
    // …and the normalized chord still only types there (']' from the
    // remapped byte), firing no binding.
    assert_eq!(
        to_action(
            key(KeyCode::Char('5'), KeyModifiers::CONTROL),
            Focus::SearchField
        ),
        Some(Action::SearchEdit(SearchEdit::Char(']')))
    );
}
