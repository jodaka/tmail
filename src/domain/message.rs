use super::Address;
use crate::domain::mailbox::MailboxId;
use chrono::{DateTime, FixedOffset};

/// Opaque backend message identifier (RFC `Message-ID` is tracked separately
/// once the backend lands; see ADR 0001 finding 4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

/// One row of a message list (plan §7 `MessageSummary`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    pub from: Vec<Address>,
    pub subject: String,
    /// Not available from `envelope list` on maildir (ADR 0001 finding 2);
    /// the UI must tolerate absence.
    pub snippet: Option<String>,
    pub timestamp: DateTime<FixedOffset>,
    pub is_read: bool,
    pub is_starred: bool,
    pub has_attachments: bool,
}

impl MessageSummary {
    /// Sender shown in the list: first `from` address, or a placeholder.
    pub fn from_display(&self) -> &str {
        self.from
            .first()
            .map(Address::display)
            .unwrap_or("(unknown sender)")
    }
}
