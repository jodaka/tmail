//! Mapping from Himalaya DTOs to domain types (ADR 0001 decision 3).
//!
//! Pure functions only, so every rule is unit-testable against real probe
//! fixtures. Mailbox roles are resolved *here* — from the account's
//! `mailbox.alias` table first, then by well-known names — so the UI never
//! guesses folder names (ADR 0001).

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, TimeZone};

use super::dto;
use crate::domain::time;
use crate::domain::{
    Address, Attachment, Mailbox, MailboxId, MailboxRole, Message, MessageHeaders, MessageId,
    MessageLocator, MessageSummary, Page,
};

/// Map a mailbox listing into domain mailboxes. Alias-derived roles win
/// over name heuristics.
///
/// The result is a stable partition: system folders first, user labels
/// last, each group in the backend's own order. Gmail IMAP interleaves the
/// two (a label can sit between `INBOX` and `[Gmail]/Drafts`), and the
/// sidebar renders the split as folders followed by a `LABELS` caption, so
/// the ordering must be settled here, at the one place every consumer
/// shares.
pub(crate) fn mailboxes(dto: dto::MailboxesDto, aliases: &HashMap<String, String>) -> Vec<Mailbox> {
    let mut folders = Vec::new();
    let mut labels = Vec::new();
    for dto in dto.mailboxes {
        let role = role_of(&dto.name, &dto.id, aliases);
        let mailbox = Mailbox {
            id: MailboxId(dto.id),
            name: dto.name,
            role,
            unread_count: dto.unread,
            total_count: dto.total,
        };
        if mailbox.is_label() {
            labels.push(mailbox);
        } else {
            folders.push(mailbox);
        }
    }
    folders.extend(labels);
    folders
}

/// Map one envelope page into a domain page. The total is unknown from
/// `envelope list` (ADR 0001 finding 2), and `offset` is the page-aligned
/// base the adapter actually requested.
pub(crate) fn envelopes(
    dto: dto::EnvelopesDto,
    mailbox_id: MailboxId,
    offset: usize,
    limit: usize,
) -> Page<MessageSummary> {
    Page {
        items: dto
            .envelopes
            .into_iter()
            .map(|dto| map_envelope(dto, &mailbox_id))
            .collect(),
        offset,
        limit,
        total: None,
    }
}

fn map_envelope(dto: dto::EnvelopeDto, mailbox_id: &MailboxId) -> MessageSummary {
    MessageSummary {
        id: MessageId(dto.id),
        mailbox_id: mailbox_id.clone(),
        message_id: dto.message_id,
        from: dto.from.iter().map(address).collect(),
        to: dto.to.iter().map(address).collect(),
        subject: dto.subject,
        // `envelope list` carries no snippet (ADR 0001 finding 2); Tmail
        // fills it only once full messages are fetched (Phase 4+).
        snippet: None,
        // Missing/absent Date header: the Unix epoch renders as a stable,
        // obviously-old date instead of inventing "now" (ADR 0001
        // finding 2).
        timestamp: dto.date.unwrap_or_else(time::epoch),
        is_read: has_flag(&dto.flags, "\\Seen", "seen"),
        is_starred: has_flag(&dto.flags, "\\Flagged", "flagged"),
        has_attachments: dto.has_attachment.unwrap_or(false),
    }
}

fn address(dto: &dto::AddressDto) -> Address {
    Address {
        name: dto.name.clone(),
        email: dto.email.clone(),
    }
}

fn part_address(dto: &dto::PartAddressDto) -> Address {
    Address {
        name: dto.name.clone(),
        email: dto.address.clone(),
    }
}

/// `\Seen`/`flagged`-style flag detection: IANA tag when set, otherwise the
/// raw wire spelling compared case-insensitively.
fn has_flag(flags: &[dto::FlagDto], raw: &str, iana: &str) -> bool {
    flags
        .iter()
        .any(|flag| flag.iana.as_deref() == Some(iana) || flag.raw.eq_ignore_ascii_case(raw))
}

/// Map a `message read` dump into one domain message (plan §7). Missing
/// headers, bodies, and attachments all map to safe defaults: the reader
/// must render what exists and tolerate what does not (plan §19 Phase 4).
pub(crate) fn message(dto: dto::MessageReadDto, locator: MessageLocator) -> Message {
    let headers = parts_headers(&dto.parts);
    let mut message = Message {
        id: locator.id,
        mailbox_id: locator.mailbox,
        headers: MessageHeaders {
            subject: text_header(&headers, "subject").unwrap_or_default(),
            from: address_header(&headers, "from"),
            to: address_header(&headers, "to"),
            cc: address_header(&headers, "cc"),
            // Draft copies stored in the Drafts mailbox keep their Bcc
            // header so reopening recovers the hidden recipients.
            bcc: address_header(&headers, "bcc"),
            date: date_header(&headers, "date"),
            message_id: text_header(&headers, "message-id"),
            in_reply_to: text_header(&headers, "in-reply-to").map(bare_ids),
            references: text_header(&headers, "references").map(bare_ids),
        },
        plain_body: None,
        html_body: None,
        attachments: Vec::new(),
    };
    if let Some(index) = dto.text_body.first() {
        message.plain_body = text_body(&dto, *index);
    }
    if let Some(index) = dto.html_body.first() {
        message.html_body = html_body(&dto, *index);
    }
    message.attachments = dto
        .attachments
        .iter()
        .filter_map(|index| attachment(&dto, *index))
        .collect();
    message
}

/// Headers of the first part that carries any (the top-level part for a
/// well-formed message; fallback keeps degenerate dumps renderable).
fn parts_headers(parts: &[dto::PartDto]) -> Vec<dto::HeaderDto> {
    parts
        .iter()
        .find(|part| !part.headers.is_empty())
        .map(|part| part.headers.clone())
        .unwrap_or_default()
}

fn header<'a>(headers: &'a [dto::HeaderDto], name: &str) -> Option<&'a dto::HeaderValueDto> {
    // mail_parser's serde dump spells header names with underscores
    // (`content_type`, `message_id`); the dashed RFC spelling appears in
    // synthetic shapes. Normalize both sides and match case-insensitively —
    // the Phase 5 fixture corpus caught this for attachment metadata.
    let needle = name.replace('-', "_");
    headers
        .iter()
        .find(|header| {
            header
                .name
                .as_str()
                .replace('-', "_")
                .eq_ignore_ascii_case(&needle)
        })
        .and_then(|header| header.value.as_ref())
}

fn text_header(headers: &[dto::HeaderDto], name: &str) -> Option<String> {
    match header(headers, name) {
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::Text(text))) => Some(text.clone()),
        // `References` and friends serialize as a list of ids.
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::TextList(ids))) => {
            Some(ids.join(" "))
        }
        _ => None,
    }
}

/// Normalize a `Message-ID`-shaped header to bare ids: angle brackets
/// stripped, runs of whitespace collapsed to single spaces. Bare form is
/// the identity Tmail matches on everywhere (ADR 0001 finding 4) and what
/// reply seeding preserves (plan §14, Phase 7.4).
fn bare_ids(raw: String) -> String {
    raw.split_whitespace()
        .map(crate::domain::message::bare_message_id)
        .collect::<Vec<_>>()
        .join(" ")
}

fn address_header(headers: &[dto::HeaderDto], name: &str) -> Vec<Address> {
    match header(headers, name) {
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::Address(
            dto::AddressValueDto::List(list),
        ))) => list.iter().map(part_address).collect(),
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::Address(
            dto::AddressValueDto::Group(groups),
        ))) => groups
            .iter()
            .flat_map(|group| group.addresses.iter().map(part_address))
            .collect(),
        _ => Vec::new(),
    }
}

fn date_header(headers: &[dto::HeaderDto], name: &str) -> Option<DateTime<FixedOffset>> {
    match header(headers, name) {
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::DateTime(raw))) => {
            fixed_datetime(raw)
        }
        _ => None,
    }
}

/// Convert mail_parser's split offset fields into a chrono timestamp. The
/// sign rides on `tz_before_gmt` (fixture: `tz_before_gmt: false, tz_hour:
/// 3` is `+03:00`); a malformed wall clock yields `None`, never a panic.
fn fixed_datetime(raw: &dto::RawDateTimeDto) -> Option<DateTime<FixedOffset>> {
    let minutes = raw.tz_hour.abs() * 60 + raw.tz_minute as i32;
    let offset_seconds = if raw.tz_before_gmt {
        -minutes * 60
    } else {
        minutes * 60
    };
    let offset = FixedOffset::east_opt(offset_seconds)?;
    offset
        .with_ymd_and_hms(
            raw.year,
            raw.month.clamp(1, 12),
            raw.day.clamp(1, 31),
            raw.hour.clamp(0, 23),
            raw.minute.clamp(0, 59),
            raw.second.clamp(0, 59),
        )
        .single()
}

fn body_of(dto: &dto::MessageReadDto, index: usize) -> Option<&dto::BodyDto> {
    dto.parts.get(index).and_then(|part| part.body.as_ref())
}

fn text_body(dto: &dto::MessageReadDto, index: usize) -> Option<String> {
    match body_of(dto, index) {
        Some(dto::BodyDto::Known(dto::KnownBody::Text(text))) => Some(text.clone()),
        _ => None,
    }
}

fn html_body(dto: &dto::MessageReadDto, index: usize) -> Option<String> {
    match body_of(dto, index) {
        Some(dto::BodyDto::Known(dto::KnownBody::Html(html))) => Some(html.clone()),
        // An HTML body shipped as decoded text still renders in Phase 5;
        // tolerate the loose shape instead of dropping the content.
        Some(dto::BodyDto::Known(dto::KnownBody::Text(text))) if looks_like_html(text) => {
            Some(text.clone())
        }
        _ => None,
    }
}

/// Cheap shape sniff for the loose text-as-html fallback above.
fn looks_like_html(text: &str) -> bool {
    let head = text.trim_start();
    head.starts_with('<') && head.to_ascii_lowercase().contains("html")
}

/// Attachment metadata for one part index: name from Content-Disposition
/// `filename` / Content-Type `name`, media type from Content-Type, size
/// from the decoded binary length. Parts without a usable body are skipped
/// rather than listed with fabricated sizes.
fn attachment(dto: &dto::MessageReadDto, index: usize) -> Option<Attachment> {
    let part = dto.parts.get(index)?;
    let headers = &part.headers;
    let content_type = match header(headers, "content-type") {
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::ContentType(ct))) => {
            Some(ct.clone())
        }
        _ => None,
    };
    let disposition_name = match header(headers, "content-disposition") {
        Some(dto::HeaderValueDto::Known(dto::KnownHeaderValue::ContentType(ct))) => {
            attribute(ct, "filename")
        }
        _ => None,
    };
    let type_name = content_type.as_ref().and_then(|ct| attribute(ct, "name"));
    let mime_type = content_type.as_ref().map(|ct| match &ct.c_subtype {
        Some(subtype) if !subtype.is_empty() => format!("{}/{}", ct.c_type, subtype),
        _ => ct.c_type.clone(),
    });
    let size = match part.body.as_ref()? {
        dto::BodyDto::Known(dto::KnownBody::Binary(bytes)) => Some(bytes.len() as u64),
        _ => None,
    };
    Some(Attachment {
        name: disposition_name.or(type_name),
        mime_type,
        size,
        // Himalaya's attachment commands identify parts by the 1-based
        // position in the MIME tree (`id = part_index + 1`, its own
        // `attachment download` loop over `message.attachments`), while
        // this index is 0-based. Off by one here sends `attachment
        // download` after a nonexistent part ("No attachment with id N on
        // message …"), so store the id the download expects.
        part_id: index + 1,
    })
}

fn attribute(ct: &dto::ContentTypeDto, name: &str) -> Option<String> {
    ct.attributes
        .iter()
        .find(|attr| attr.name.eq_ignore_ascii_case(name))
        .and_then(|attr| attr.value.clone())
        .filter(|value| !value.is_empty())
}

/// Resolve a mailbox's role: exact `mailbox.alias` value match (against
/// name, then id) first, then exact well-known-name heuristics.
fn role_of(name: &str, id: &str, aliases: &HashMap<String, String>) -> Option<MailboxRole> {
    aliases
        .iter()
        .filter_map(|(key, value)| alias_role(key).map(|role| (role, value)))
        .find(|(_, value)| *value == name || *value == id)
        .map(|(role, _)| role)
        .or_else(|| name_role(name).or_else(|| name_role(id)))
}

/// Himalaya's predefined `mailbox.alias` keys (config.example.toml shape).
fn alias_role(key: &str) -> Option<MailboxRole> {
    match key {
        "inbox" => Some(MailboxRole::Inbox),
        "sent" => Some(MailboxRole::Sent),
        "drafts" => Some(MailboxRole::Drafts),
        "archive" => Some(MailboxRole::Archive),
        "trash" => Some(MailboxRole::Trash),
        "junk" | "spam" => Some(MailboxRole::Spam),
        _ => None,
    }
}

/// The reverse of [`alias_role`]: the canonical `mailbox.alias` key for
/// each semantic role. One table here keeps the role↔key mapping in a
/// single place — adding a role or key only edits this file. (`junk` is
/// Himalaya's config key for `Spam`, so it is the canonical direction.)
pub(crate) fn alias_key_for_role(role: MailboxRole) -> &'static str {
    match role {
        MailboxRole::Archive => "archive",
        MailboxRole::Trash => "trash",
        MailboxRole::Inbox => "inbox",
        MailboxRole::Sent => "sent",
        MailboxRole::Drafts => "drafts",
        MailboxRole::Spam => "junk",
    }
}

/// Well-known folder names, matched exactly (case-insensitive) so lookalike
/// names never silently inherit a role.
fn name_role(name: &str) -> Option<MailboxRole> {
    match name.trim().to_ascii_lowercase().as_str() {
        "inbox" => Some(MailboxRole::Inbox),
        "sent" => Some(MailboxRole::Sent),
        "drafts" | "draft" => Some(MailboxRole::Drafts),
        "archive" | "all" => Some(MailboxRole::Archive),
        "trash" | "deleted" => Some(MailboxRole::Trash),
        "spam" | "junk" => Some(MailboxRole::Spam),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Real probe output captured in Phase 0 (ADR 0001 finding 2 shape).
    const ENVELOPE_LIST: &str = include_str!("../../../fixtures/himalaya/envelope-list.json");

    fn aliases(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (String::from(*k), String::from(*v)))
            .collect()
    }

    #[test]
    fn maps_real_envelope_fixture() {
        let dto: dto::EnvelopesDto = serde_json::from_str(ENVELOPE_LIST).expect("fixture parses");
        let mailbox_id = MailboxId(String::from("INBOX"));
        let page = envelopes(dto, mailbox_id.clone(), 0, 20);
        assert_eq!(page.offset, 0);
        assert_eq!(page.limit, 20);
        assert_eq!(page.total, None);
        assert_eq!(page.items.len(), 5);

        let first = &page.items[0];
        assert_eq!(first.id.0, "1788343420.M446833P1967Q1.RFT-R993YF");
        assert_eq!(
            first.message_id.as_deref(),
            Some("2717992022958107501@tmail.local")
        );
        assert_eq!(first.mailbox_id, mailbox_id);
        assert_eq!(first.from.len(), 1);
        assert_eq!(first.from[0].display(), "Ada Lovelace");
        assert_eq!(first.subject, "Welcome to Tmail");
        assert_eq!(first.snippet, None);
        assert!(first.is_starred, "\\Flagged maps to starred");
        assert!(!first.is_read, "no \\Seen flag");
        assert!(!first.has_attachments);

        let seen = &page.items[2];
        assert!(seen.is_read);
        assert!(!seen.is_starred);

        let unicode = &page.items[3];
        assert_eq!(unicode.subject, "Grüße mit emoji 🎉");
        assert_eq!(unicode.from[0].display(), "Dave");
    }

    #[test]
    fn missing_optional_envelope_fields_map_to_safe_defaults() {
        let dto: dto::EnvelopesDto =
            serde_json::from_str(r#"{"envelopes":[{"id":"only-id"}]}"#).expect("partial parses");
        let page = envelopes(dto, MailboxId(String::from("INBOX")), 40, 20);
        let message = &page.items[0];
        assert_eq!(message.subject, "");
        assert!(message.from.is_empty());
        assert_eq!(message.message_id, None);
        assert!(!message.is_read);
        assert!(!message.is_starred);
        assert!(!message.has_attachments);
        assert_eq!(message.timestamp.to_rfc3339(), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn role_from_alias_table() {
        let map = aliases(&[("inbox", "INBOX"), ("trash", "Archive")]);
        assert_eq!(
            role_of("INBOX", "/root/maildir/INBOX", &map),
            Some(MailboxRole::Inbox)
        );
        // The probe's trash alias points at a folder literally named
        // "Archive"; the alias must win over the name heuristic.
        assert_eq!(
            role_of("Archive", "/root/maildir/Archive", &map),
            Some(MailboxRole::Trash)
        );
    }

    #[test]
    fn role_from_wellknown_name_without_aliases() {
        let empty = HashMap::new();
        assert_eq!(role_of("INBOX", "INBOX", &empty), Some(MailboxRole::Inbox));
        assert_eq!(role_of("Sent", "Sent", &empty), Some(MailboxRole::Sent));
        assert_eq!(
            role_of("drafts", "drafts", &empty),
            Some(MailboxRole::Drafts)
        );
        assert_eq!(role_of("Junk", "Junk", &empty), Some(MailboxRole::Spam));
        assert_eq!(role_of("Projects", "Projects", &empty), None);
    }

    #[test]
    fn alias_matches_by_name_independently_of_id() {
        let map = aliases(&[("inbox", "INBOX")]);
        assert_eq!(role_of("nope", "nope", &map), None);
        assert_eq!(
            role_of("INBOX", "different-id", &map),
            Some(MailboxRole::Inbox)
        );
    }

    #[test]
    fn map_fixture_mailbox_list() {
        let dto: dto::MailboxesDto = serde_json::from_str(
            r#"{"mailboxes":[
                {"id":"/root/Archive","name":"Archive","total":null,"unread":null},
                {"id":"INBOX","name":"INBOX"}
            ]}"#,
        )
        .expect("parses");
        let mailboxes = mailboxes(dto, &aliases(&[("inbox", "INBOX"), ("trash", "Archive")]));
        assert_eq!(mailboxes.len(), 2);
        assert_eq!(mailboxes[0].role, Some(MailboxRole::Trash));
        assert_eq!(mailboxes[0].total_count, None);
        assert_eq!(mailboxes[1].role, Some(MailboxRole::Inbox));
        assert_eq!(mailboxes[1].unread_count, None);
    }

    #[test]
    fn mailbox_list_orders_folders_before_labels() {
        // Real Gmail IMAP listing shape (probed on himalaya 2.1.0): user
        // labels interleave with system folders — `Notes` sits between
        // Inbox and the [Gmail] set — and `[Gmail]/Starred` carries no
        // role on accounts without a `junk` alias, yet must stay a folder
        // (Gmail reserves the `[Gmail]/` prefix for its own folders).
        let dto: dto::MailboxesDto = serde_json::from_str(
            r#"{"mailboxes":[
                {"id":"Inbox","name":"Inbox","total":11669,"unread":6},
                {"id":"Notes","name":"Notes"},
                {"id":"[Gmail]/Drafts","name":"[Gmail]/Drafts"},
                {"id":"[Gmail]/Starred","name":"[Gmail]/Starred"},
                {"id":"social","name":"social"}
            ]}"#,
        )
        .expect("parses");
        let mailboxes = mailboxes(dto, &aliases(&[("drafts", "[Gmail]/Drafts")]));
        let ids: Vec<&str> = mailboxes.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(
            ids,
            [
                "Inbox",
                "[Gmail]/Drafts",
                "[Gmail]/Starred",
                "Notes",
                "social"
            ]
        );
        let labels = mailboxes.iter().position(Mailbox::is_label).unwrap();
        assert_eq!(labels, 3, "folders first, labels last");
    }

    /// Real probe output captured in Phase 0 (ADR 0001 finding 10).
    const MESSAGE_READ_PLAIN: &str =
        include_str!("../../../fixtures/himalaya/message-read-plain.json");
    const MESSAGE_READ_MULTIPART: &str =
        include_str!("../../../fixtures/himalaya/message-read-multipart.json");
    /// Real probe output captured from a Gmail IMAP account (himalaya
    /// 2.1.0): header names outside mail_parser's enum arrive as tagged
    /// maps and `Received` values as structured maps — the shapes real
    /// mail carries in bulk. Synthetic content, wire shapes verbatim.
    const MESSAGE_READ_GMAIL: &str =
        include_str!("../../../fixtures/himalaya/message-read-gmail.json");

    fn locator() -> MessageLocator {
        MessageLocator {
            mailbox: MailboxId(String::from("INBOX")),
            id: MessageId(String::from("env-9")),
            message_id: None,
        }
    }

    #[test]
    fn maps_real_plain_message_fixture() {
        let dto: dto::MessageReadDto =
            serde_json::from_str(MESSAGE_READ_PLAIN).expect("fixture parses");
        let message = message(dto, locator());
        assert_eq!(message.id.0, "env-9");
        assert_eq!(message.mailbox_id.0, "INBOX");
        assert_eq!(message.headers.subject, "Plain text only");
        assert_eq!(message.headers.from.len(), 1);
        assert_eq!(message.headers.from[0].display(), "Bob");
        assert_eq!(message.headers.to.len(), 1);
        assert!(message.headers.cc.is_empty());
        assert_eq!(
            message.headers.message_id.as_deref(),
            Some("3180034027954358661@tmail.local")
        );
        let date = message.headers.date.expect("date parses");
        assert_eq!(date.to_rfc3339(), "2026-09-02T10:03:40+03:00");
        assert_eq!(
            message.plain_body.as_deref(),
            Some("This is a plain text message.\nLine two.\n")
        );
        assert_eq!(message.html_body, None);
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn maps_real_multipart_message_fixture() {
        let dto: dto::MessageReadDto =
            serde_json::from_str(MESSAGE_READ_MULTIPART).expect("fixture parses");
        let message = message(dto, locator());
        assert_eq!(message.headers.subject, "HTML alternative");
        assert_eq!(
            message.plain_body.as_deref(),
            Some("This message prefers HTML.\n"),
            "plain body comes from the text_body part index"
        );
        assert!(
            message
                .html_body
                .as_deref()
                .is_some_and(|h| h.contains("html")),
            "html body comes from the html_body part index"
        );
    }

    /// Acceptance (plan §19 Phase 7): the forward representation is
    /// fixture-tested. The real `message read --json` probe output flows
    /// through the production mapping, then the forward seed.
    #[test]
    fn forward_seed_from_real_plain_fixture_carries_a_clear_block() {
        let dto: dto::MessageReadDto =
            serde_json::from_str(MESSAGE_READ_PLAIN).expect("fixture parses");
        let message = message(dto, locator());
        let seed = crate::domain::reply::seed_forward(&message);
        assert_eq!(seed.subject, "Fwd: Plain text only");
        assert_eq!(seed.in_reply_to, None, "forwards start a new thread");
        let body = seed.body;
        assert!(body.contains("---------- Forwarded message ---------"));
        assert!(body.contains("From: Bob <bob@example.org>"));
        assert!(body.contains("Date: 2026-09-02 10:03"));
        assert!(body.contains("Subject: Plain text only"));
        assert!(body.contains("To: probe@tmail.local"));
        assert!(
            body.ends_with("\nThis is a plain text message.\nLine two.\n"),
            "original body follows the block:\n{body}"
        );
    }

    #[test]
    fn maps_real_gmail_shaped_message_with_tagged_header_names() {
        let dto: dto::MessageReadDto =
            serde_json::from_str(MESSAGE_READ_GMAIL).expect("fixture parses");
        let message = message(dto, locator());
        // Known names still resolve next to the tagged ones.
        assert_eq!(message.headers.subject, "Your order has shipped");
        assert_eq!(message.headers.from.len(), 1);
        assert_eq!(message.headers.from[0].display(), "Example Shop");
        assert_eq!(message.headers.to.len(), 1);
        assert_eq!(
            message.headers.message_id.as_deref(),
            Some("1378089180.1165937.1788420358561@mailer.example.com")
        );
        let date = message.headers.date.expect("date parses");
        assert_eq!(date.to_rfc3339(), "2026-09-03T00:26:02-07:00");
        // Gmail marketing mail ships HTML-only: both body indexes point at
        // the same part, whose body is the Html variant.
        assert!(
            message
                .html_body
                .as_deref()
                .is_some_and(|html| html.contains("<html>"))
        );
        assert_eq!(message.plain_body, None);
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn empty_dump_maps_to_renderable_defaults() {
        let dto: dto::MessageReadDto = serde_json::from_str("{}").expect("parses");
        let message = message(dto, locator());
        assert_eq!(message.headers.subject, "");
        assert!(message.headers.from.is_empty());
        assert_eq!(message.headers.date, None);
        assert_eq!(message.plain_body, None);
        assert_eq!(message.html_body, None);
        assert!(message.attachments.is_empty());
    }

    #[test]
    fn draft_bcc_header_maps_to_the_hidden_recipients() {
        // Draft copies stored in the Drafts mailbox keep their Bcc header
        // (`message add` writes it); reopening the draft needs it back.
        let dto: dto::MessageReadDto = serde_json::from_str(
            r#"{
                "text_body": [0],
                "parts": [{"headers": [
                    {"name":"subject","value":{"Text":"Draft"}},
                    {"name":"bcc","value":{"Address":{"List":[
                        {"name":null,"address":"hidden@example.com"}
                    ]}}}
                ],"body":{"Text":"body"}}]
            }"#,
        )
        .expect("parses");
        let message = message(dto, locator());
        assert_eq!(message.headers.bcc.len(), 1);
        assert_eq!(message.headers.bcc[0].display(), "hidden@example.com");
    }

    #[test]
    fn reply_thread_headers_map_to_bare_ids() {
        // Angle brackets and runs of whitespace are normalized away: bare
        // ids are the identity Tmail matches on (ADR 0001 finding 4).
        let dto: dto::MessageReadDto = serde_json::from_str(
            r#"{
                "parts": [{"headers": [
                    {"name":"message-id","value":{"Text":"3180034027954358661@tmail.local"}},
                    {"name":"in-reply-to","value":{"Text":"<6053432595490343824@tmail.local>"}},
                    {"name":"references","value":{"Text":"<0@tmail.local>  <6053432595490343824@tmail.local>"}}
                ]}]
            }"#,
        )
        .expect("parses");
        let message = message(dto, locator());
        assert_eq!(
            message.headers.in_reply_to.as_deref(),
            Some("6053432595490343824@tmail.local")
        );
        assert_eq!(
            message.headers.references.as_deref(),
            Some("0@tmail.local 6053432595490343824@tmail.local")
        );
    }

    #[test]
    fn attachment_metadata_maps_from_part_headers() {
        let dto: dto::MessageReadDto = serde_json::from_str(
            r#"{
                "parts": [
                    {"headers": [{"name":"subject","value":{"Text":"att"}}],
                     "body": {"Multipart": [1, 2]}},
                    {"headers": [{"name":"content-type","value":{"ContentType":{
                        "c_type":"application","c_subtype":"pdf",
                        "attributes":[{"name":"name","value":"report.pdf"}]}}}],
                     "body": {"Binary": [1,2,3,4,5]}},
                    {"headers": [{"name":"content-type","value":{"ContentType":{
                        "c_type":"image","c_subtype":"jpeg"}}},
                        {"name":"content-disposition","value":{"ContentType":{
                        "c_type":"attachment","c_subtype":null,
                        "attributes":[{"name":"filename","value":"photo 1.jpg"}]}}}],
                     "body": {"Binary": [9,9]}}
                ],
                "attachments": [1, 2]
            }"#,
        )
        .expect("parses");
        let message = message(dto, locator());
        assert_eq!(message.attachments.len(), 2);
        let pdf = &message.attachments[0];
        assert_eq!(pdf.name.as_deref(), Some("report.pdf"));
        assert_eq!(pdf.mime_type.as_deref(), Some("application/pdf"));
        assert_eq!(pdf.size, Some(5));
        assert_eq!(pdf.part_id, 2, "part index 1 → download id 2");
        let jpg = &message.attachments[1];
        assert_eq!(
            jpg.name.as_deref(),
            Some("photo 1.jpg"),
            "disposition filename wins over type name"
        );
        assert_eq!(jpg.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(jpg.size, Some(2));
        assert_eq!(jpg.part_id, 3, "part index 2 → download id 3");
    }

    #[test]
    fn datetime_offsets_support_negative_and_broken_values() {
        let negative = fixed_datetime(&dto::RawDateTimeDto {
            year: 2026,
            month: 9,
            day: 2,
            hour: 10,
            minute: 3,
            second: 40,
            tz_before_gmt: true,
            tz_hour: 5,
            tz_minute: 30,
        });
        assert_eq!(
            negative.map(|d| d.to_rfc3339()),
            Some("2026-09-02T10:03:40-05:30".into())
        );
        // Month out of range clamps instead of panicking…
        let clamped = fixed_datetime(&dto::RawDateTimeDto {
            year: 2026,
            month: 13,
            day: 2,
            hour: 25,
            minute: 3,
            second: 40,
            tz_before_gmt: false,
            tz_hour: 0,
            tz_minute: 0,
        });
        assert!(clamped.is_some());
        // …and a hopeless wall clock is None, never a panic.
        let broken = fixed_datetime(&dto::RawDateTimeDto {
            year: 2026,
            month: 2,
            day: 30,
            hour: 10,
            minute: 3,
            second: 40,
            tz_before_gmt: false,
            tz_hour: 0,
            tz_minute: 0,
        });
        assert_eq!(broken, None);
    }
}
