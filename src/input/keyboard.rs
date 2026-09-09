//! Keyboard → action translation (plan §10 input contract).
//!
//! Since configurable keybindings (`[tmail.keybindings]`), the shortcut
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
//!
//! One crossterm quirk is normalized away first: in legacy terminal input
//! the four chords Ctrl+`\` `]` `^` `_` (the bytes 0x1C–0x1F) carry no
//! glyph, so crossterm reports them as `Char('4'..'7')` + CONTROL. They
//! are mapped back to the keys that produce them (`normalize_ctrl_chords`)
//! so specs like `"Ctrl+]"` match what the user actually pressed.

use crate::app::action::{Action, AttachmentBrowse, ComposerEdit, DialogEdit, SearchEdit};
use crate::app::focus::Focus;
use crate::app::wizard;
use crate::input::keymap::KeyMap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a key event into an action. `None` = not a bound key.
pub fn to_action(keymap: &KeyMap, key: KeyEvent, focus: Focus) -> Option<Action> {
    to_action_with(keymap, key, focus)
}

/// The translation body; named separately so tests can wrap
/// [`to_action`] with a default keymap.
fn to_action_with(keymap: &KeyMap, key: KeyEvent, focus: Focus) -> Option<Action> {
    let key = normalize_ctrl_chords(key);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let text_focus = matches!(
        focus,
        Focus::SearchField | Focus::Dialog | Focus::Composer | Focus::Wizard
    );
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
            // The attachment file chooser (ticket 95x0): the explorer
            // navigates with the arrows (Shift is irrelevant to it), Left
            // and Backspace go to the parent, Right opens the selected
            // directory. Enter stays the keymap's `activate` binding so
            // the reducer decides between descending and submitting the
            // selected file; Esc/Tab fall through for cancel/focus.
            KeyCode::Up => return Some(Action::AttachmentBrowse(AttachmentBrowse::Up)),
            KeyCode::Down => return Some(Action::AttachmentBrowse(AttachmentBrowse::Down)),
            KeyCode::Left | KeyCode::Backspace => {
                return Some(Action::AttachmentBrowse(AttachmentBrowse::Parent));
            }
            KeyCode::Right => return Some(Action::AttachmentBrowse(AttachmentBrowse::Open)),
            KeyCode::PageUp => return Some(Action::AttachmentBrowse(AttachmentBrowse::PageUp)),
            KeyCode::PageDown => {
                return Some(Action::AttachmentBrowse(AttachmentBrowse::PageDown));
            }
            // Structural keys fall through; every other key is inert
            // (there is no text entry anymore).
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {}
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
                // Arrows (ticket kfmt): plain moves the caret, Ctrl moves
                // by words, Shift extends a selection, Ctrl+Shift selects
                // whole words. Alt+arrows still fall through to the
                // global bindings (move/page), as before.
                KeyCode::Left if !alt => {
                    return Some(Action::ComposerEdit(match (ctrl, shift) {
                        (false, false) => ComposerEdit::CursorLeft,
                        (true, false) => ComposerEdit::WordLeft,
                        (false, true) => ComposerEdit::SelectLeft,
                        (true, true) => ComposerEdit::SelectWordLeft,
                    }));
                }
                KeyCode::Right if !alt => {
                    return Some(Action::ComposerEdit(match (ctrl, shift) {
                        (false, false) => ComposerEdit::CursorRight,
                        (true, false) => ComposerEdit::WordRight,
                        (false, true) => ComposerEdit::SelectRight,
                        (true, true) => ComposerEdit::SelectWordRight,
                    }));
                }
                // Vertical movement has no word-wise mode; Ctrl keeps the
                // plain-line semantics it had before.
                KeyCode::Up if !alt => {
                    return Some(Action::ComposerEdit(if shift {
                        ComposerEdit::SelectUp
                    } else {
                        ComposerEdit::CursorUp
                    }));
                }
                KeyCode::Down if !alt => {
                    return Some(Action::ComposerEdit(if shift {
                        ComposerEdit::SelectDown
                    } else {
                        ComposerEdit::CursorDown
                    }));
                }
                // Enter activates the focused control (the reducer routes
                // it — body focus inserts the newline there); Ctrl+Enter
                // was handled above.
                KeyCode::Enter => return Some(Action::Activate),
                // Ctrl+E hands the body to the external editor (plan §14,
                // Phase 11): composer-only, regardless of which field holds
                // focus.
                KeyCode::Char('e') if ctrl => return Some(Action::EditExternal),
                // Undo/redo the body editor (ticket kfmt): coalesced
                // steps, so a typing run reverts at once. Body-only — the
                // single-line fields have no history.
                KeyCode::Char('z') if ctrl && !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::Undo));
                }
                KeyCode::Char('y') if ctrl && !alt => {
                    return Some(Action::ComposerEdit(ComposerEdit::Redo));
                }
                KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {}
                _ => {}
            }
            // Fall through: global chords (quit, refresh) still fire.
        }
        // The account configuration wizard (ADR 0003): printable keys type
        // into the focused field; `e`/`r` are the discovery-screen
        // shortcuts (the reducer folds them back into typed text on the
        // other steps). Structural keys fall through so Esc/Enter/Tab stay
        // rebindable through the keymap.
        Focus::Wizard => match key.code {
            // Space toggles the focused storage-mode row (checkbox
            // convention, ADR 0003 §3.2 W4); the reducer folds it back
            // into a typed character on every text field.
            KeyCode::Char(' ') if !ctrl && !alt => {
                return Some(Action::Wizard(wizard::WizardAction::ToggleStorageMode));
            }
            KeyCode::Char('e') if !ctrl && !alt => {
                return Some(Action::Wizard(wizard::WizardAction::OverrideServers));
            }
            KeyCode::Char('r') if !ctrl && !alt => {
                return Some(Action::Wizard(wizard::WizardAction::RerunDiscovery));
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                return Some(Action::Wizard(wizard::WizardAction::Edit(
                    DialogEdit::Char(c),
                )));
            }
            KeyCode::Backspace => {
                return Some(Action::Wizard(wizard::WizardAction::Edit(
                    DialogEdit::Backspace,
                )));
            }
            KeyCode::Delete => {
                return Some(Action::Wizard(wizard::WizardAction::Edit(
                    DialogEdit::Delete,
                )));
            }
            KeyCode::Left => {
                return Some(Action::Wizard(wizard::WizardAction::Edit(
                    DialogEdit::CursorLeft,
                )));
            }
            KeyCode::Right => {
                return Some(Action::Wizard(wizard::WizardAction::Edit(
                    DialogEdit::CursorRight,
                )));
            }
            KeyCode::Up => return Some(Action::Wizard(wizard::WizardAction::MoveUp)),
            KeyCode::Down => return Some(Action::Wizard(wizard::WizardAction::MoveDown)),
            // Structural keys and chords fall through to the keymap.
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {}
            KeyCode::Char(_) => {}
            _ => return None,
        },
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

/// Map crossterm's legacy reporting of Ctrl+`\` `]` `^` `_` back to the
/// keys the user presses. Legacy terminal input sends those chords as the
/// single bytes 0x1C–0x1F, which carry no glyph; crossterm therefore
/// reports them as `Char('4')`–`Char('7')` + CONTROL. The conventional
/// keys are what a config spec names (`"Ctrl+]"`), so the invented digits
/// are translated before any binding lookup. Chords only: a chordless
/// `4`–`7` is text and passes through untouched. (Ctrl+`[` is not
/// recoverable this way — its byte *is* ESC — so it stays unbindable.)
fn normalize_ctrl_chords(key: KeyEvent) -> KeyEvent {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return key;
    }
    let code = match key.code {
        KeyCode::Char('4') => KeyCode::Char('\\'),
        KeyCode::Char('5') => KeyCode::Char(']'),
        KeyCode::Char('6') => KeyCode::Char('^'),
        KeyCode::Char('7') => KeyCode::Char('_'),
        _ => return key,
    };
    KeyEvent {
        code,
        modifiers: key.modifiers,
        kind: key.kind,
        state: key.state,
    }
}

#[cfg(test)]
#[path = "keyboard_tests.rs"]
mod tests;
