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

/// One open search (plan §16/§19 Phase 9): the submitted query and the
/// mailbox it runs against. Search results live in the same list state the
/// mailbox page uses (Phase 9.1); the mailbox's list context is stashed in
/// `AppState` while this route is active and restored on return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoute {
    pub query: String,
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
    /// Search results over one mailbox (Phase 9). `mailbox_id` reports the
    /// searched mailbox, so flag actions, refreshes, and pagination keep
    /// their context.
    Search(SearchRoute),
    Message(MessageRoute),
    /// The built-in composer (plan §19 Phase 6). The draft data lives in
    /// `AppState.composer`, so leaving pops the route but preserves the
    /// draft for reopening.
    Composer,
    /// The account configuration wizard (ADR 0003): a full-screen route
    /// owned by `AppState.wizard`. Background refreshes must not touch a
    /// mailbox list while it is active — there is no account yet.
    Wizard,
}

impl Route {
    /// The mailbox this route displays, if any. Search reports the mailbox
    /// its results come from; the composer has none: background refreshes
    /// must not touch a list while composing (plan §11).
    pub fn mailbox_id(&self) -> Option<&MailboxId> {
        match self {
            Route::Mailbox(r) => Some(&r.mailbox_id),
            Route::Search(r) => Some(&r.mailbox_id),
            Route::Message(r) => Some(&r.mailbox_id),
            // Like the composer: background refreshes must not touch a
            // list while the wizard runs (ADR 0003 §3.1).
            Route::Composer | Route::Wizard => None,
        }
    }
}
