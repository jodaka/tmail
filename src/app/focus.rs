//! Focus within the mailbox screen (plan §9/§10).
//!
//! `ErrorModal` is a transitory focus held while the error overlay is open;
//! it is not part of the Tab cycle. Reader/composer screens extend the
//! list in later phases.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The `/` search field in the topbar. Text editing focus: single-letter
    /// shortcuts must not fire here (plan §10).
    SearchField,
    Sidebar,
    MessageList,
    /// The message reader screen is open (plan §19 Phase 4): Up/Down scroll
    /// the body; single-letter shortcuts act on the open message. Tab is
    /// handled by the reducer, which cycles the reader's own focus cursor
    /// over links and attachment chips (tickets 1fnh/hc9n), so the screen
    /// focus itself never leaves the reader.
    Reader,
    /// The composer screen is open (plan §19 Phase 6): the focused control
    /// lives in `AppState.session.composer`; single-letter shortcuts never fire
    /// while a text field is focused (plan §10).
    Composer,
    /// The attachment file chooser is open (plan §15, ticket 95x0): a
    /// modal that intercepts all input like the error modal.
    Dialog,
    /// The theme picker dialog is open (ticket k5ba): arrows preview the
    /// highlighted palette, Enter keeps it, Esc restores the original.
    ThemePicker,
    /// The shortcuts help popup is open (user request): it intercepts all
    /// input; Esc (or the help keys again) returns to the saved focus.
    Help,
    /// The Retry/Dismiss error modal is open; it intercepts all input.
    ErrorModal,
    /// The account configuration wizard (ADR 0003) owns the whole screen:
    /// it manages its own field cycle inside `WizardState`, so Tab never
    /// leaves it and single-letter shortcuts never fire.
    Wizard,
}

/// Tab order: next/previous focus cycles through this list (plan §10).
/// While the composer screen is open, the cycle is different: the
/// composer's own controls first, then the sidebar — the reducer's
/// `focus_step` bridges the two, and the off-screen mailbox-screen
/// controls (search field, message list) are skipped.
pub const FOCUS_ORDER: [Focus; 3] = [Focus::SearchField, Focus::Sidebar, Focus::MessageList];

impl Focus {
    pub fn next(self) -> Self {
        self.step(1)
    }

    pub fn previous(self) -> Self {
        self.step(-1)
    }

    /// One step around the Tab cycle (`next` is `+1`, `previous` is `-1`).
    /// The self-cycle foci — modals, the wizard, the reader, the composer —
    /// stay put: the modal/composer handles Tab itself.
    fn step(self, dir: isize) -> Self {
        match self {
            // The modal foci never cycle; the modal handles Tab itself.
            Focus::ErrorModal
            | Focus::Dialog
            | Focus::ThemePicker
            | Focus::Help
            | Focus::Wizard => self,
            // The reader screen keeps its screen focus; the reducer's
            // `focus_step` redirects Tab to the reader's internal cursor
            // (`AppState.reader_focus`: links, then chips).
            Focus::Reader => self,
            // The composer cycles its own controls (plan §10); the reducer
            // drives that cycle through `ComposerState` and steps out to
            // the sidebar at the cycle's ends (compose-mode folder list).
            Focus::Composer => self,
            _ => {
                let i = FOCUS_ORDER
                    .iter()
                    .position(|f| *f == self)
                    .expect("focus in order");
                let len = FOCUS_ORDER.len() as isize;
                let n = (i as isize + dir).rem_euclid(len) as usize;
                FOCUS_ORDER[n]
            }
        }
    }

    /// Whether single-letter shortcuts (c, r, a, f, e, s, u, /) are active.
    /// They never fire while editing a text field or while a modal is open
    /// (plan §10); the reader acts on the open message.
    pub fn accepts_shortcuts(self) -> bool {
        !matches!(
            self,
            Focus::SearchField
                | Focus::ErrorModal
                | Focus::Dialog
                | Focus::ThemePicker
                | Focus::Help
                | Focus::Composer
                | Focus::Wizard
        )
    }
}

impl fmt::Display for Focus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Focus::SearchField => write!(f, "search"),
            Focus::Sidebar => write!(f, "sidebar"),
            Focus::MessageList => write!(f, "list"),
            Focus::Reader => write!(f, "reader"),
            Focus::Composer => write!(f, "composer"),
            Focus::Dialog => write!(f, "dialog"),
            Focus::ThemePicker => write!(f, "themes"),
            Focus::Help => write!(f, "help"),
            Focus::ErrorModal => write!(f, "modal"),
            Focus::Wizard => write!(f, "wizard"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_without_repeat() {
        let mut f = Focus::SearchField;
        let seen = (0..FOCUS_ORDER.len())
            .map(|_| {
                f = f.next();
                f
            })
            .collect::<Vec<_>>();
        assert_eq!(seen.len(), 3);
        assert!(seen.contains(&Focus::Sidebar));
        assert!(seen.contains(&Focus::MessageList));
        assert_eq!(f, Focus::SearchField);
    }

    #[test]
    fn previous_is_inverse_of_next() {
        for f in FOCUS_ORDER {
            assert_eq!(f.next().previous(), f);
            assert_eq!(f.previous().next(), f);
        }
    }

    #[test]
    fn shortcuts_gated_in_search_field() {
        assert!(!Focus::SearchField.accepts_shortcuts());
        assert!(!Focus::Composer.accepts_shortcuts());
        assert!(!Focus::Dialog.accepts_shortcuts());
        assert!(!Focus::ThemePicker.accepts_shortcuts());
        assert!(Focus::Sidebar.accepts_shortcuts());
        assert!(Focus::MessageList.accepts_shortcuts());
    }

    #[test]
    fn wizard_focus_is_self_cycle_and_gates_shortcuts() {
        assert_eq!(Focus::Wizard.next(), Focus::Wizard);
        assert_eq!(Focus::Wizard.previous(), Focus::Wizard);
        assert!(!Focus::Wizard.accepts_shortcuts());
    }

    #[test]
    fn dialog_focus_is_self_cycle() {
        assert_eq!(Focus::Dialog.next(), Focus::Dialog);
        assert_eq!(Focus::Dialog.previous(), Focus::Dialog);
        assert_eq!(Focus::ThemePicker.next(), Focus::ThemePicker);
        assert_eq!(Focus::ThemePicker.previous(), Focus::ThemePicker);
    }

    #[test]
    fn reader_focus_is_self_cycle_and_accepts_shortcuts() {
        // The reducer drives the reader's internal link/chip cursor from
        // Tab; at the screen-focus level the reader never cycles away.
        assert_eq!(Focus::Reader.next(), Focus::Reader);
        assert_eq!(Focus::Reader.previous(), Focus::Reader);
        assert!(Focus::Reader.accepts_shortcuts());
    }
}
