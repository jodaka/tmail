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
    /// Recipients, mapped into the reader's meta block (Phase 4).
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

// ── `message add --json` (ADR 0002 draft spike) ──────────────────────────

/// `message add` returns the created message's backend id; `sent` is false
/// for a plain append (drafts). It is parsed for shape fidelity but not
/// interpreted: a draft add never sends.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessageAddDto {
    pub id: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub sent: Option<bool>,
}

// ── `message read --json` (ADR 0001 finding 10) ──────────────────────────
//
// The raw serde dump of `mail_parser::Message`: a part list plus the part
// indexes that hold the plain-text body, the HTML body, and the attachments.
// Everything optional carries `#[serde(default)]` so a future himalaya (or
// an exotic message) cannot crash the reader.

/// The parsed-message dump for `message read`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessageReadDto {
    #[serde(default)]
    pub parts: Vec<PartDto>,
    /// Indexes into `parts` holding `text/plain` bodies.
    #[serde(default)]
    pub text_body: Vec<usize>,
    /// Indexes into `parts` holding `text/html` bodies.
    #[serde(default)]
    pub html_body: Vec<usize>,
    /// Indexes into `parts` holding attachments.
    #[serde(default)]
    pub attachments: Vec<usize>,
}

/// One MIME part: headers plus a tagged body.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PartDto {
    #[serde(default)]
    pub headers: Vec<HeaderDto>,
    #[serde(default)]
    pub body: Option<BodyDto>,
}

/// One header as a `(name, tagged value)` pair.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HeaderDto {
    #[serde(default)]
    pub name: HeaderNameDto,
    #[serde(default)]
    pub value: Option<HeaderValueDto>,
}

/// mail_parser's serde dumps header names in two shapes: known names as
/// plain lowercase strings (`"received"`, `"content_type"`) and every
/// name outside its enum as a tagged map (`{"other":"Delivered-To"}`) —
/// the shape real mail carries in bulk (Delivered-To, ARC-*, DKIM-,
/// List-*, X-*). The untagged wrapper plus the catch-all keep any future
/// name shape from failing the whole message (real-capture finding:
/// `invalid type: map, expected a string at line 1 column 79`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum HeaderNameDto {
    Plain(String),
    Tagged {
        other: String,
    },
    /// Catch-all for name shapes Post does not interpret (they only need
    /// to parse, not to be read).
    #[allow(dead_code)]
    Other(serde_json::Value),
}

impl Default for HeaderNameDto {
    fn default() -> Self {
        Self::Plain(String::new())
    }
}

impl HeaderNameDto {
    /// The name as written on the wire: the plain string, the `other`
    /// tag's value, or `""` for anything else (never matches a lookup).
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Plain(name) => name,
            Self::Tagged { other } => other,
            Self::Other(_) => "",
        }
    }
}

/// Header values are externally tagged on the wire (`{"Text": …}`,
/// `{"Address": …}`, `{"DateTime": …}`, `{"ContentType": …}`, …). The
/// outer untagged wrapper falls back to a raw value so unknown header
/// kinds never fail the whole message.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum HeaderValueDto {
    Known(KnownHeaderValue),
    /// Catch-all for value kinds Post does not interpret (they only need
    /// to parse, not to be read).
    #[allow(dead_code)]
    Other(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) enum KnownHeaderValue {
    Text(String),
    /// `References`-shaped headers arrive as a list of ids (mail_parser
    /// serde dump); joined into the bare space-separated form.
    TextList(Vec<String>),
    Address(AddressValueDto),
    DateTime(RawDateTimeDto),
    ContentType(ContentTypeDto),
}

/// `message read --json` address shape: mail_parser serializes the email
/// as `address` (the envelope listing uses `email`, ADR 0001 finding 10).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PartAddressDto {
    #[serde(default)]
    pub name: Option<String>,
    pub address: String,
}

/// `{"Address": {"List": [...]}}` (and group form, mapped leniently).
#[derive(Debug, Clone, Deserialize)]
pub(crate) enum AddressValueDto {
    List(Vec<PartAddressDto>),
    Group(Vec<AddressGroupDto>),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AddressGroupDto {
    #[serde(default)]
    pub addresses: Vec<PartAddressDto>,
}

/// mail_parser's `DateTime` serde shape (fixture shape, ADR 0001 finding
/// 10): wall-clock fields plus the offset carried as `tz_hour`/`tz_minute`
/// with `tz_before_gmt` as the sign.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawDateTimeDto {
    #[serde(default)]
    pub year: i32,
    #[serde(default)]
    pub month: u32,
    #[serde(default)]
    pub day: u32,
    #[serde(default)]
    pub hour: u32,
    #[serde(default)]
    pub minute: u32,
    #[serde(default)]
    pub second: u32,
    #[serde(default)]
    pub tz_before_gmt: bool,
    #[serde(default)]
    pub tz_hour: i32,
    #[serde(default)]
    pub tz_minute: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ContentTypeDto {
    #[serde(default)]
    pub c_type: String,
    #[serde(default)]
    pub c_subtype: Option<String>,
    #[serde(default)]
    pub attributes: Vec<ContentTypeAttributeDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ContentTypeAttributeDto {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
}

// ── `attachment list/download --json` (plan §15, Phase 8) ────────────────
//
// Schema: himalaya-attachment-download.json. `id` is the 1-based MIME part
// position (as a string on the wire); `size` and `inline` are required;
// `filename`, `mime`, and `path` are optional (`path` is set only by
// `attachment download`).

/// The table of attachment rows.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AttachmentsDto {
    #[serde(default)]
    pub attachments: Vec<AttachmentRowDto>,
}

/// One attachment row.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AttachmentRowDto {
    /// 1-based MIME part position, as a string.
    pub id: String,
    #[serde(default)]
    pub filename: Option<String>,
    /// Wire shape fidelity only; the saver uses the request/row filename.
    #[serde(default)]
    #[allow(dead_code)]
    pub mime: Option<String>,
    /// Decoded size in bytes (schema-required; shape fidelity only).
    #[allow(dead_code)]
    pub size: u64,
    #[serde(default)]
    #[allow(dead_code)]
    pub inline: bool,
    /// Where `attachment download` wrote the bytes.
    #[serde(default)]
    pub path: Option<String>,
}

/// Part bodies are externally tagged too: decoded text, HTML, raw binary
/// (a JSON number array), or nested part indexes for multipart. The outer
/// untagged wrapper tolerates unknown body shapes.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum BodyDto {
    Known(KnownBody),
    /// Catch-all for body kinds Post does not interpret.
    #[allow(dead_code)]
    Other(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) enum KnownBody {
    Text(String),
    Html(String),
    Binary(Vec<u8>),
    /// Nested part indexes; parsed for shape fidelity but not traversed
    /// (bodies are selected by `text_body`/`html_body`/`attachments`).
    #[allow(dead_code)]
    Multipart(Vec<usize>),
}
