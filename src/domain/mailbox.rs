/// Opaque backend mailbox identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MailboxId(pub String);

/// Well-known mailbox roles resolved by the backend adapter (ADR 0001:
/// the UI never guesses folder names). Unknown mailboxes carry `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxRole {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Spam,
    Trash,
}

/// A mailbox as shown in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: MailboxId,
    pub name: String,
    pub role: Option<MailboxRole>,
    pub unread_count: Option<u64>,
    pub total_count: Option<u64>,
}
