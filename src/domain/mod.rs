//! Core domain model (plan §7).
//!
//! Ids are opaque backend strings; no meaning is parsed from them.

pub mod address;
pub mod mailbox;
pub mod message;
pub mod page;

pub use address::Address;
pub use mailbox::{Mailbox, MailboxId, MailboxRole};
pub use message::{MessageId, MessageSummary};
pub use page::Page;
