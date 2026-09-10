use super::Address;
use crate::domain::mailbox::MailboxId;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

/// Opaque backend message identifier (RFC `Message-ID` is tracked separately
/// once the backend lands; see ADR 0001 finding 4). Serializable so typed
/// retry intents (plan §12) can reference it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// Where one message lives: the mailbox it was listed in plus the backend
/// id to act on. Maildir ids change when a message moves (ADR 0001 finding
/// 4), so every mutation targets the id observed in the current page and
/// carries the `Message-ID` for stale-result rejection and re-resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageLocator {
    pub mailbox: MailboxId,
    pub id: MessageId,
    /// RFC `Message-ID` header value when known; lets the reducer detect
    /// results for messages that no longer sit in the displayed page.
    pub message_id: Option<String>,
}

/// Header block of one full message (plan §7 `MessageHeaders`).
/// Serializable for the viewed-message cache (ticket haeb).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct MessageHeaders {
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Blind-carbon recipients. Outgoing mail strips `Bcc`, but draft
    /// copies stored in the Drafts mailbox keep the header (the composer
    /// needs it back when a draft is reopened), so full messages may
    /// carry it. `default` keeps cached messages from earlier versions
    /// loadable (ticket haeb).
    #[serde(default)]
    pub bcc: Vec<Address>,
    /// Parsed `Date` header in its source offset; `None` when missing or
    /// unparseable (the UI falls back to a stable placeholder).
    pub date: Option<DateTime<FixedOffset>>,
    pub message_id: Option<String>,
    /// Bare `In-Reply-To` id (`None` when absent); what reply seeding
    /// preserves (plan §14, Phase 7.4).
    pub in_reply_to: Option<String>,
    /// Bare, whitespace-separated `References` chain (`None` when absent).
    pub references: Option<String>,
}

/// Metadata for one message attachment (plan §7/§15). Bytes are fetched by
/// the attachment phase (Phase 8); the reader lists metadata only.
/// Serializable for the viewed-message cache (ticket haeb).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    /// Filename from `Content-Disposition`/`Content-Type` when provided.
    pub name: Option<String>,
    /// MIME media type as `type/subtype` when parseable.
    pub mime_type: Option<String>,
    /// Decoded size in bytes when known.
    pub size: Option<u64>,
    /// 1-based MIME part id, the id `attachment list`/`attachment
    /// download` expect (ADR 0001).
    pub part_id: usize,
}

/// One request to save an incoming attachment to disk (plan §15, Phase 8).
/// Serializable so retry intents replay it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttachmentRequest {
    /// The message the attachment belongs to.
    pub locator: MessageLocator,
    /// MIME part id, what `attachment download` expects.
    pub part_id: usize,
    /// Display filename from the message's MIME metadata. Interpreted as a
    /// name only — never as a path — so traversal is impossible.
    pub filename: Option<String>,
    /// Destination directory; `None` uses the configured downloads
    /// directory (`[tmail.attachments].downloads_dir`, else the platform
    /// default). A leading `~` is expanded by the backend, never a shell.
    pub dir: Option<std::path::PathBuf>,
}

/// One full message (plan §7 `Message`), as fetched by `message read`.
/// `raw` stays unset for now: the parsed content suffices for the reader,
/// and keeping raw bytes out of state avoids duplicating payloads (plan
/// §13.4 logs are already sanitized). Serializable for the viewed-message
/// cache (ticket haeb).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    pub headers: MessageHeaders,
    /// Decoded `text/plain` body when the message has one.
    pub plain_body: Option<String>,
    /// Raw `text/html` body when the message has one. Rendering it is the
    /// MIME/HTML subsystem's job (Phase 5); the reader falls back safely.
    pub html_body: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// One row of a message list (plan §7 `MessageSummary`). Serializable so
/// the summary cache (ticket haeb) can persist the last loaded page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    /// RFC `Message-ID` header value when the backend surfaces it. This is
    /// the only identity that survives message moves (ADR 0001 finding 4).
    pub message_id: Option<String>,
    pub from: Vec<Address>,
    /// Recipients; surfaced by the envelope listing (Phase 4 reader meta).
    pub to: Vec<Address>,
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

    /// Recipient shown in list rows of outgoing folders (the Drafts list,
    /// where every row's sender is the user's own address): first `to`
    /// address, or a placeholder. Matches the reader's `To` meta wording.
    pub fn to_display(&self) -> &str {
        self.to
            .first()
            .map(Address::display)
            .unwrap_or("(no recipients)")
    }

    /// The [`MessageLocator`] addressing this row: the shared construction
    /// behind list selection and reader/list action targets. Broad cloning
    /// is the point — a locator must outlive the row it came from.
    pub fn into_locator(&self) -> MessageLocator {
        MessageLocator {
            mailbox: self.mailbox_id.clone(),
            id: self.id.clone(),
            message_id: self.message_id.clone(),
        }
    }
}

/// The bare form of an RFC `Message-ID`: no surrounding angle brackets.
/// Envelope listings carry bare ids while draft snapshots keep the
/// bracketed form, so identity comparisons normalize both sides.
pub fn bare_message_id(id: &str) -> &str {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
}

/// The bracketed RFC form of a `Message-ID` (`<id>`), the convention draft
/// snapshots carry. A bare id (envelope listings, fetched headers) gets
/// wrapped; an already-bracketed id passes through.
pub fn bracketed_message_id(id: &str) -> String {
    let bare = id.trim();
    if bare.starts_with('<') {
        bare.to_string()
    } else {
        format!("<{bare}>")
    }
}
