//! Keyboard → action translation (plan §10 input contract).
//!
//! Since configurable keybindings (`[post.keybindings]`), the shortcut
//! surface lives in the [`KeyMap`] (`input::keymap`): defaults seed it,
//! the config reshapes it, and this module only consults it. What stays
//! hardcoded — deliberately — is *text editing*: the search field, modal
//! fields, and the composer consume printable keys as text and keep their
//! caret/edit keys (plan §10: typing must type; text-entry behavior is
//! not a binding and must never be remapped away).
//!
//! Lookup order per key: the focus's own context table (list or reader)
//! first, then the global table — a context can shadow a global key
//! without conflicting with it.

use crate::app::action::{Action, ComposerEdit, DialogEdit, SearchEdit};
use crate::app::focus::Focus;
use crate::input::keymap::KeyMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a key event into an action. `None` = not a bound key.
pub fn to_action(keymap: &KeyMap, key: KeyEvent, focus: Focus) -> Option<Action> {
    to_action_with(keymap, key, focus)
}

/// The translation body; named separately so tests can wrap
/// [`to_action`] with a default keymap.
fn to_action_with(keymap: &KeyMap, key: KeyEvent, focus: Focus) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let text_focus = matches!(focus, Focus::SearchField | Focus::Dialog | Focus::Composer);
    // Ctrl+Enter sends from anywhere (plan §10, as before); plain Enter is
    // the keymap's `activate` binding, so it stays rebindable. The
    // composer's body newline is the reducer's routing of Activate.
    if key.code == KeyCode::Enter && ctrl {
        return Some(Action::Send);
    }
    // Text-entry foci consume their editing keys before any binding
    // lookup; the structural keys (Esc/Enter/Tab) fall through to the
    // keymap so cancel/activate/focus stay configurable there too.
    match focus {
        Focus::SearchField => match key.code {
            // Any printable character (chords included — see below) types.
            KeyCode::Char(c) if !ctrl && !alt => {
                return Some(Action::SearchEdit(SearchEdit::Char(c)));
            }
            KeyCode::Backspace => return Some(Action::SearchEdit(SearchEdit::Backspace)),
            // Structural keys — and the arrows, which page/move the list
            // behind the field, as before — fall through to the keymap.
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {}
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {}
            // A ctrl/alt chord pierces text entry only as quit/refresh
            // (below); every other chord still types the character, as
            // before (Ctrl+A types an `a` into the query).
            KeyCode::Char(_) => {
                return match keymap.lookup(&key, focus) {
                    Some(action @ (Action::Quit | Action::Refresh)) => Some(action),
                    _ => Some(Action::SearchEdit(SearchEdit::Char(
                        key.code.as_char().expect("Char code"),
                    ))),
                };
            }
            _ => return None,
        },
        Focus::Dialog => match key.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                return Some(Action::DialogEdit(DialogEdit::Char(c)));
            }
            KeyCode::Backspace => return Some(Action::DialogEdit(DialogEdit::Backspace)),
            KeyCode::Delete => return Some(Action::DialogEdit(DialogEdit::Delete)),
            KeyCode::Left => return Some(Action::DialogEdit(DialogEdit::CursorLeft)),
            KeyCode::Right => return Some(Action::DialogEdit(DialogEdit::CursorRight)),
            // Up/Down never leave a single-line modal field (plan §15).
            KeyCode::Up | KeyCode::Down => return None,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {}
            // Chords fall through: quit/refresh pierce (below), the rest
            // are unbound in a modal field.
            KeyCode::Char(_) => {}
            _ => return None,
        },
        Focus::Composer => {
            match key.code {
                KeyCode::Char(c) if !ctrl && !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::Char(c)));
                }
                KeyCode::Backspace => {
                    return Some(Action::ComposerEdit(ComposerEdit::Backspace));
                }
                KeyCode::Delete => return Some(Action::ComposerEdit(ComposerEdit::Delete)),
                // Caret moves without Alt; Alt+arrows fall through to the
                // global bindings (move/page), as before.
                KeyCode::Left if !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::CursorLeft));
                }
                KeyCode::Right if !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::CursorRight));
                }
                KeyCode::Up if !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::CursorUp));
                }
                KeyCode::Down if !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::CursorDown));
                }
                // Enter activates the focused control (the reducer routes
                // it — body focus inserts the newline there); Ctrl+Enter
                // was handled above.
                KeyCode::Enter => return Some(Action::Activate),
                // Ctrl+E hands the body to the external editor (plan §14,
                // Phase 11): composer-only, regardless of which field holds
                // focus.
                KeyCode::Char('e') if ctrl => return Some(Action::EditExternal),
                KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {}
                _ => {}
            }
            // Fall through: global chords (quit, refresh) still fire.
        }
        _ => {}
    }
    let action = keymap.lookup(&key, focus)?;
    // Text entry is pierced only by the quit and refresh chords (plan
    // §10): every other ctrl/alt chord stays out of the fields the user
    // is typing into, however the keymap is configured. Structural keys
    // (Esc/Enter/Tab) pass unrestricted.
    let chord = matches!(key.code, KeyCode::Char(_)) && (ctrl || alt);
    if text_focus && chord && !matches!(action, Action::Quit | Action::Refresh) {
        return None;
    }
    Some(action)
}

#[cfg(test)]
#[path = "keyboard_tests.rs"]
mod tests;
