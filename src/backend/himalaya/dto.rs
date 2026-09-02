//! Himalaya-private DTOs (ADR 0001 decision 3: these types never escape
//! `backend/himalaya/`).
//!
//! Shapes mirror `himalaya json-schema` output (fixtures/himalaya/schemas/):
//! fields required by the schema are required here, everything optional
//! carries `#[serde(default)]` so partial output maps to safe defaults
//! instead of failing. `iana` flags are deliberately read as plain strings:
//! a future himalaya adding a new IANA flag must not break parsing.

use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

/// `mailbox list --json` (schema: himalaya-mailbox-list.json).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MailboxesDto {
    #[serde(default)]
    pub mailboxes: Vec<MailboxDto>,
}

/// One mailbox row. `id` and `name` are schema-required; on maildir `id` is
/// the absolute directory path (verified on himalaya 2.1.0) and both are
/// used verbatim for follow-up `-m` arguments.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MailboxDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub unread: Option<u64>,
}

/// `envelope list --json` (schema: himalaya-envelope-list.json).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EnvelopesDto {
    #[serde(default)]
    pub envelopes: Vec<EnvelopeDto>,
}

/// One envelope row. There is no snippet field (ADR 0001 finding 2) and no
/// total, so Post's snippet stays absent and pages have unknown totals.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EnvelopeDto {
    pub id: String,
    /// `Message-ID:` header value; `None` when missing (schema default).
    #[serde(rename = "message-id", default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub flags: Vec<FlagDto>,
    #[serde(default)]
    pub from: Vec<AddressDto>,
    #[serde(default)]
    pub subject: String,
    // Parsed to keep the DTO a faithful mirror of the wire format; the
    // reader phase (Phase 4) maps recipients into domain types.
    #[allow(dead_code)]
    #[serde(default)]
    pub to: Vec<AddressDto>,
    /// ISO-8601 with offset; `None` when the header is missing/unparseable.
    #[serde(default)]
    pub date: Option<DateTime<FixedOffset>>,
    /// Only populated when the caller opted in (ADR 0001 finding 9); Post
    /// does not opt in for list rows.
    #[serde(rename = "has-attachment", default)]
    pub has_attachment: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FlagDto {
    /// Wire spelling, e.g. `\Seen` (schema-required).
    pub raw: String,
    /// IANA tag, read leniently as a plain string.
    #[serde(default)]
    pub iana: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AddressDto {
    #[serde(default)]
    pub name: Option<String>,
    pub email: String,
}
