//! Route stack (plan §9). Returning from a deeper route restores the exact
//! previous mailbox/search page, focus, and selection.
//!
//! The reader (Phase 4) pushes `Route::Message` above the mailbox route;
//! the underlying page, selection, and scroll are left untouched so `Esc`
//! restores them exactly. `Compose`/`Search` routes are added by their
//! phases.

use crate::domain::{MailboxId, MessageSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxRoute {
    pub mailbox_id: MailboxId,
}

/// One open message. Carries the summary snapshot from the list so the
/// reader can render identity and metadata before the full message arrives,
/// and so actions (star/archive/…) know their target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRoute {
    pub mailbox_id: MailboxId,
    pub summary: MessageSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Mailbox(MailboxRoute),
    Message(MessageRoute),
}

impl Route {
    /// The mailbox this route displays, if any.
    pub fn mailbox_id(&self) -> Option<&MailboxId> {
        match self {
            Route::Mailbox(r) => Some(&r.mailbox_id),
            Route::Message(r) => Some(&r.mailbox_id),
        }
    }
}
