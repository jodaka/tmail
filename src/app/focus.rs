//! Focus within the mailbox screen (plan §9/§10).
//!
//! Phase 1 has a single screen, so this is the full focus model for now;
//! reader/composer screens extend it in later phases.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The `/` search field in the topbar. Text editing focus: single-letter
    /// shortcuts must not fire here (plan §10).
    SearchField,
    Sidebar,
    MessageList,
}

/// Tab order: next/previous focus cycles through this list (plan §10).
pub const FOCUS_ORDER: [Focus; 3] = [Focus::SearchField, Focus::Sidebar, Focus::MessageList];

impl Focus {
    pub fn next(self) -> Self {
        let i = FOCUS_ORDER
            .iter()
            .position(|f| *f == self)
            .expect("focus in order");
        FOCUS_ORDER[(i + 1) % FOCUS_ORDER.len()]
    }

    pub fn previous(self) -> Self {
        let i = FOCUS_ORDER
            .iter()
            .position(|f| *f == self)
            .expect("focus in order");
        FOCUS_ORDER[(i + FOCUS_ORDER.len() - 1) % FOCUS_ORDER.len()]
    }

    /// Whether single-letter shortcuts (c, r, a, f, e, s, u, /) are active.
    /// They never fire while editing a text field (plan §10).
    pub fn accepts_shortcuts(self) -> bool {
        !matches!(self, Focus::SearchField)
    }
}

impl fmt::Display for Focus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Focus::SearchField => write!(f, "search"),
            Focus::Sidebar => write!(f, "sidebar"),
            Focus::MessageList => write!(f, "list"),
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
        assert!(Focus::Sidebar.accepts_shortcuts());
        assert!(Focus::MessageList.accepts_shortcuts());
    }
}
