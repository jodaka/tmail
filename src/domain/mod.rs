//! Core domain model (plan §7).
//!
//! Ids are opaque backend strings; no meaning is parsed from them.

/// Domain suffix of Message-IDs tmail synthesizes itself (draft copies,
/// journal fixtures): `.local` is reserved by RFC 6762, so the address can
/// never collide with — or be confused for — a real routable host.
pub const MESSAGE_ID_SUFFIX: &str = "@tmail.local";

pub mod address;
pub mod draft;
pub mod mailbox;
pub mod message;
pub mod operation;
pub mod page;
pub mod paths;
pub mod private_fs;
pub mod reply;
pub mod sanitize;
pub mod send;
pub mod time;
pub mod url;

pub use address::Address;
pub use draft::{
    Draft, DraftAttachment, DraftId, DraftSaveState, DraftSnapshot, RestoredDraft,
    draft_from_message,
};
pub use mailbox::{Mailbox, MailboxId, MailboxRole};
pub use message::{
    Attachment, AttachmentRequest, Message, MessageHeaders, MessageId, MessageLocator,
    MessageSummary, bare_message_id, bracketed_message_id,
};
pub use page::{Page, PageRequest, SearchRequest};
pub use reply::{ReplyKind, Seed, seed_forward, seed_reply};
pub use send::{OutboundAttachment, OutboundMessage, OutgoingContent, SendBlocker, SendOutcome};
