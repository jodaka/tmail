//! Typed effects (plan §5: "Effects launch typed backend requests").
//!
//! The reducer is I/O-free; when state transitions need data from the
//! backend it returns effects, and the runtime (main.rs) spawns the typed
//! [`crate::backend::MailBackend`] requests whose results flow back as
//! `Action::MailboxesLoaded` / `Action::PageLoaded`. The Phase 3 operation
//! manager extends this with `OperationId`s and cancellation.

use crate::domain::PageRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Fetch the mailbox listing.
    LoadMailboxes,
    /// Fetch one page of message summaries.
    LoadPage(PageRequest),
}
