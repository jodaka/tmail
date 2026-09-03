/// Opaque backend mailbox identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MailboxId(pub String);

/// Well-known mailbox roles resolved by the backend adapter (ADR 0001:
/// the UI never guesses folder names). Unknown mailboxes carry `None`.
/// Serializable for the summary cache (ticket haeb).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MailboxRole {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Spam,
    Trash,
}

/// A mailbox as shown in the sidebar. Serializable for the summary cache
/// (ticket haeb).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mailbox {
    pub id: MailboxId,
    pub name: String,
    pub role: Option<MailboxRole>,
    pub unread_count: Option<u64>,
    pub total_count: Option<u64>,
}
