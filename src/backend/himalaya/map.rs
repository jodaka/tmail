//! Mapping from Himalaya DTOs to domain types (ADR 0001 decision 3).
//!
//! Pure functions only, so every rule is unit-testable against real probe
//! fixtures. Mailbox roles are resolved *here* — from the account's
//! `mailbox.alias` table first, then by well-known names — so the UI never
//! guesses folder names (ADR 0001).

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};

use super::dto;
use crate::domain::{Address, Mailbox, MailboxId, MailboxRole, MessageId, MessageSummary, Page};

/// Map a mailbox listing into domain mailboxes. Alias-derived roles win
/// over name heuristics.
pub(crate) fn mailboxes(dto: dto::MailboxesDto, aliases: &HashMap<String, String>) -> Vec<Mailbox> {
    dto.mailboxes
        .into_iter()
        .map(|dto| {
            let role = role_of(&dto.name, &dto.id, aliases);
            Mailbox {
                id: MailboxId(dto.id),
                name: dto.name,
                role,
                unread_count: dto.unread,
                total_count: dto.total,
            }
        })
        .collect()
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
        from: dto.from.into_iter().map(address).collect(),
        subject: dto.subject,
        // `envelope list` carries no snippet (ADR 0001 finding 2); Post
        // fills it only once full messages are fetched (Phase 4+).
        snippet: None,
        timestamp: dto.date.unwrap_or_else(epoch),
        is_read: has_flag(&dto.flags, "\\Seen", "seen"),
        is_starred: has_flag(&dto.flags, "\\Flagged", "flagged"),
        has_attachments: dto.has_attachment.unwrap_or(false),
    }
}

fn address(dto: dto::AddressDto) -> Address {
    Address {
        name: dto.name,
        email: dto.email,
    }
}

/// `\Seen`/`flagged`-style flag detection: IANA tag when set, otherwise the
/// raw wire spelling compared case-insensitively.
fn has_flag(flags: &[dto::FlagDto], raw: &str, iana: &str) -> bool {
    flags
        .iter()
        .any(|flag| flag.iana.as_deref() == Some(iana) || flag.raw.eq_ignore_ascii_case(raw))
}

fn epoch() -> DateTime<FixedOffset> {
    // Fallback for a missing/absent Date header: the Unix epoch renders as
    // a stable, obviously-old date instead of inventing "now".
    DateTime::from_timestamp(0, 0)
        .expect("epoch is valid")
        .with_timezone(&FixedOffset::east_opt(0).expect("UTC offset is valid"))
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
            Some("2717992022958107501@post.local")
        );
        assert_eq!(first.mailbox_id, mailbox_id);
        assert_eq!(first.from.len(), 1);
        assert_eq!(first.from[0].display(), "Ada Lovelace");
        assert_eq!(first.subject, "Welcome to Post");
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
}
