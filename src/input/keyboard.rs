//! Keyboard → action translation (plan §10 input contract).
//!
//! Pure function of the key event and the current focus, so the mapping is
//! unit-testable without a terminal. Single-letter shortcuts never fire
//! while a text field is focused; `j`/`k` and `?` are deliberately absent
//! (plan §4 overrides).

use crate::app::action::{Action, ComposerEdit, DialogEdit, SearchEdit};
use crate::app::focus::Focus;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a key event into an action. `None` = not a bound key.
pub fn to_action(key: KeyEvent, focus: Focus) -> Option<Action> {
    use KeyCode::*;
    match key.code {
        Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Refresh),
        // Ctrl+A toggles select-all (ticket p0s3); text-entry foci never
        // intercept keys, so it only fires where shortcuts are accepted.
        Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) && focus.accepts_shortcuts() => {
            Some(Action::SelectAll)
        }
        // Ctrl+E hands the draft body to the external editor (plan §14,
        // Phase 11): composer-only, regardless of which field holds focus.
        Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) && focus == Focus::Composer => {
            Some(Action::EditExternal)
        }
        Esc => Some(Action::BackOrCancel),
        Enter => Some(enter_action(key.modifiers)),
        Tab => Some(Action::FocusNext),
        BackTab => Some(Action::FocusPrevious),
        Left if focus == Focus::Dialog => Some(Action::DialogEdit(DialogEdit::CursorLeft)),
        Right if focus == Focus::Dialog => Some(Action::DialogEdit(DialogEdit::CursorRight)),
        Up if focus == Focus::Dialog => None,
        Down if focus == Focus::Dialog => None,
        Up => Some(move_action(key.modifiers, focus, ComposerEdit::CursorUp)),
        Down => Some(move_action(key.modifiers, focus, ComposerEdit::CursorDown)),
        Left => Some(move_action(key.modifiers, focus, ComposerEdit::CursorLeft)),
        Right => Some(move_action(key.modifiers, focus, ComposerEdit::CursorRight)),
        Backspace if focus == Focus::SearchField => Some(Action::SearchEdit(SearchEdit::Backspace)),
        Backspace if focus == Focus::Dialog => Some(Action::DialogEdit(DialogEdit::Backspace)),
        Backspace if focus == Focus::Composer => {
            Some(Action::ComposerEdit(ComposerEdit::Backspace))
        }
        Delete if focus == Focus::SearchField => None,
        Delete if focus == Focus::Dialog => Some(Action::DialogEdit(DialogEdit::Delete)),
        Delete if focus == Focus::Composer => Some(Action::ComposerEdit(ComposerEdit::Delete)),
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

/// Arrows move the composer caret (Phase 6); elsewhere they move the
/// selection / scroll the focused area (plan §10). Shift+arrows are plain
/// movement, not selection: Post has no text selection in v1.
fn move_action(modifiers: KeyModifiers, focus: Focus, edit: ComposerEdit) -> Action {
    if focus == Focus::Composer && !modifiers.contains(KeyModifiers::ALT) {
        Action::ComposerEdit(edit)
    } else {
        match edit {
            ComposerEdit::CursorUp => Action::MoveUp,
            ComposerEdit::CursorDown => Action::MoveDown,
            ComposerEdit::CursorLeft => Action::PagePrevious,
            _ => Action::PageNext,
        }
    }
}

fn char_action(c: char, modifiers: KeyModifiers, focus: Focus) -> Option<Action> {
    if focus == Focus::SearchField {
        // Any printable character goes into the field; no shortcuts fire.
        return Some(Action::SearchEdit(SearchEdit::Char(c)));
    }
    if focus == Focus::Dialog {
        // Any printable character goes into the dialog entry; no shortcuts
        // fire while a modal text field is focused (plan §10).
        return match modifiers {
            m if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) => None,
            _ => Some(Action::DialogEdit(DialogEdit::Char(c))),
        };
    }
    if focus == Focus::Composer {
        // Any printable character (including '/' and letters) is composed
        // text; no single-letter shortcuts fire while editing (plan §10).
        return match modifiers {
            m if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) => None,
            _ => Some(Action::ComposerEdit(ComposerEdit::Char(c))),
        };
    }
    if modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match c {
        // `q` mirrors Esc everywhere shortcuts are accepted (list, sidebar,
        // reader, over a modal); text-entry foci never reach this match, so
        // the letter still types there.
        'q' => Some(Action::BackOrCancel),
        'c' => Some(Action::Compose),
        'r' => Some(Action::Reply),
        'a' => Some(Action::ReplyAll),
        'f' => Some(Action::Forward),
        'e' => Some(Action::Archive),
        's' => Some(Action::ToggleStar),
        'u' => Some(Action::MarkUnread),
        // `i` marks read (ticket p0s3): the focused row, or the whole
        // selection when bulk-selection mode is on. No-op in the reader
        // (an open message is already read).
        'i' => Some(Action::MarkRead),
        'm' => Some(Action::ToggleMouseCapture),
        // `d` deletes in the message list (trash; ticket h1m2) — with the
        // whole selection when bulk-selection mode is on. In the reader
        // `d` saves the selected attachment (plan §15).
        'd' if focus == Focus::MessageList => Some(Action::Trash),
        'd' => Some(Action::SaveAttachment),
        'o' => Some(Action::OpenAttachment),
        // Space toggles the focused row's bulk-selection mark, list only
        // (ticket p0s3): the sidebar has no selection semantics, and the
        // reader scrolls instead of marking.
        ' ' if focus == Focus::MessageList => Some(Action::ToggleSelected),
        _ => None, // No j/k (plan §4), no '?' help.
    }
}

#[cfg(test)]
#[path = "keyboard_tests.rs"]
mod tests;
