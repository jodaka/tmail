//! Core domain model (plan §7).
//!
//! Ids are opaque backend strings; no meaning is parsed from them.

pub mod address;
pub mod draft;
pub mod mailbox;
pub mod message;
pub mod page;
pub mod send;

pub use address::Address;
pub use draft::{Draft, DraftId, DraftSaveState, DraftSnapshot, RestoredDraft};
pub use mailbox::{Mailbox, MailboxId, MailboxRole};
pub use message::{Attachment, Message, MessageHeaders, MessageId, MessageLocator, MessageSummary};
pub use page::{Page, PageRequest};
pub use send::SendOutcome;
