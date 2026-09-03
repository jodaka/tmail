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
    /// The list header's `[ ]`/`[X]` select-all toggle (ticket p0s3): Enter
    /// (or a click) selects/deselects all visible messages.
    SelectAllToggle,
    Sidebar,
    MessageList,
    /// The message reader screen is open (plan §19 Phase 4): Up/Down scroll
    /// the body; single-letter shortcuts act on the open message.
    Reader,
    /// The composer screen is open (plan §19 Phase 6): the focused control
    /// lives in `AppState.composer`; single-letter shortcuts never fire
    /// while a text field is focused (plan §10).
    Composer,
    /// The attachment path-entry dialog is open (plan §15): a modal text
    /// field that intercepts all input like the error modal.
    Dialog,
    /// The Retry/Dismiss error modal is open; it intercepts all input.
    ErrorModal,
}

/// Tab order: next/previous focus cycles through this list (plan §10).
pub const FOCUS_ORDER: [Focus; 4] = [
    Focus::SearchField,
    Focus::SelectAllToggle,
    Focus::Sidebar,
    Focus::MessageList,
];

impl Focus {
    pub fn next(self) -> Self {
        match self {
            // The modal foci never cycle; the modal handles Tab itself.
            Focus::ErrorModal | Focus::Dialog => self,
            // The reader screen has a single focusable area (the scrolling
            // document); Tab is inert there in v1 (plan §10).
            Focus::Reader => self,
            // The composer cycles its own controls (plan §10); the reducer
            // drives that cycle through `ComposerState`.
            Focus::Composer => self,
            _ => {
                let i = FOCUS_ORDER
                    .iter()
                    .position(|f| *f == self)
                    .expect("focus in order");
                FOCUS_ORDER[(i + 1) % FOCUS_ORDER.len()]
            }
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Focus::ErrorModal | Focus::Dialog => self,
            Focus::Reader => self,
            Focus::Composer => self,
            _ => {
                let i = FOCUS_ORDER
                    .iter()
                    .position(|f| *f == self)
                    .expect("focus in order");
                FOCUS_ORDER[(i + FOCUS_ORDER.len() - 1) % FOCUS_ORDER.len()]
            }
        }
    }

    /// Whether single-letter shortcuts (c, r, a, f, e, s, u, /) are active.
    /// They never fire while editing a text field or while a modal is open
    /// (plan §10); the reader acts on the open message.
    pub fn accepts_shortcuts(self) -> bool {
        !matches!(
            self,
            Focus::SearchField | Focus::ErrorModal | Focus::Dialog | Focus::Composer
        )
    }
}

impl fmt::Display for Focus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Focus::SearchField => write!(f, "search"),
            Focus::SelectAllToggle => write!(f, "select-all"),
            Focus::Sidebar => write!(f, "sidebar"),
            Focus::MessageList => write!(f, "list"),
            Focus::Reader => write!(f, "reader"),
            Focus::Composer => write!(f, "composer"),
            Focus::Dialog => write!(f, "dialog"),
            Focus::ErrorModal => write!(f, "modal"),
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
        assert_eq!(seen.len(), 4);
        assert!(seen.contains(&Focus::SelectAllToggle));
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
        assert!(Focus::Sidebar.accepts_shortcuts());
        assert!(Focus::MessageList.accepts_shortcuts());
    }

    #[test]
    fn dialog_focus_is_self_cycle() {
        assert_eq!(Focus::Dialog.next(), Focus::Dialog);
        assert_eq!(Focus::Dialog.previous(), Focus::Dialog);
    }

    #[test]
    fn reader_focus_is_self_cycle_and_accepts_shortcuts() {
        // Single focusable area: Tab is inert; shortcuts act on the message.
        assert_eq!(Focus::Reader.next(), Focus::Reader);
        assert_eq!(Focus::Reader.previous(), Focus::Reader);
        assert!(Focus::Reader.accepts_shortcuts());
    }
}
