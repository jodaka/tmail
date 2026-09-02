//! Keyboard → action translation (plan §10 input contract).
//!
//! Pure function of the key event and the current focus, so the mapping is
//! unit-testable without a terminal. Single-letter shortcuts never fire
//! while a text field is focused; `j`/`k` and `?` are deliberately absent
//! (plan §4 overrides).

use crate::app::action::{Action, SearchEdit};
use crate::app::focus::Focus;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a key event into an action. `None` = not a bound key.
pub fn to_action(key: KeyEvent, focus: Focus) -> Option<Action> {
    use KeyCode::*;
    match key.code {
        Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Refresh),
        Esc => Some(Action::BackOrCancel),
        Enter => Some(enter_action(key.modifiers)),
        Tab => Some(Action::FocusNext),
        BackTab => Some(Action::FocusPrevious),
        Up => Some(Action::MoveUp),
        Down => Some(Action::MoveDown),
        Left => Some(Action::PagePrevious),
        Right => Some(Action::PageNext),
        Backspace => search_edit(focus, SearchEdit::Backspace),
        Delete if focus != Focus::SearchField => Some(Action::Trash),
        Char('/') if focus.accepts_shortcuts() => Some(Action::OpenSearch),
        Char(c) => char_action(c, key.modifiers, focus),
        _ => None,
    }
}

/// Enter inserts a newline in a composer body (Phase 6); `Ctrl+Enter` sends.
/// In Phase 1 Enter activates the focused control.
fn enter_action(modifiers: KeyModifiers) -> Action {
    if modifiers.contains(KeyModifiers::CONTROL) {
        Action::Send
    } else {
        Action::Activate
    }
}

fn search_edit(focus: Focus, edit: SearchEdit) -> Option<Action> {
    matches!(focus, Focus::SearchField).then_some(Action::SearchEdit(edit))
}

fn char_action(c: char, modifiers: KeyModifiers, focus: Focus) -> Option<Action> {
    if focus == Focus::SearchField {
        // Any printable character goes into the field; no shortcuts fire.
        return Some(Action::SearchEdit(SearchEdit::Char(c)));
    }
    if modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match c {
        'c' => Some(Action::Compose),
        'r' => Some(Action::Reply),
        'a' => Some(Action::ReplyAll),
        'f' => Some(Action::Forward),
        'e' => Some(Action::Archive),
        's' => Some(Action::ToggleStar),
        'u' => Some(Action::MarkUnread),
        _ => None, // No j/k (plan §4), no '?' help.
    }
}

#[cfg(test)]
#[path = "keyboard_tests.rs"]
mod tests;
