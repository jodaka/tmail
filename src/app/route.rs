//! Route stack (plan §9). Returning from a deeper route restores the exact
//! previous mailbox/search page, focus, and selection.
//!
//! Phase 1 only constructs `Route::Mailbox`; `Message`/`Compose`/`Search`
//! routes are added by the phases that implement those screens.

use crate::domain::MailboxId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxRoute {
    pub mailbox_id: MailboxId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Mailbox(MailboxRoute),
}

impl Route {
    /// The mailbox this route displays, if any.
    pub fn mailbox_id(&self) -> Option<&MailboxId> {
        match self {
            Route::Mailbox(r) => Some(&r.mailbox_id),
        }
    }
}
