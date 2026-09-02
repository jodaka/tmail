//! Test-support fixture pipeline (`test-fixtures` feature, plan §13/§19
//! Phase 5.7): parses raw `.eml` bytes with `mail-parser` — the maintained
//! library inside Himalaya — and flows the result through the exact
//! production wire path (serde dump → lenient DTO → domain mapper).
//!
//! Compiled only with the `test-fixtures` feature so production builds
//! never link raw-MIME parsing: Himalaya owns MIME at runtime (ADR 0001).
//! The integration corpus in `tests/rich_render.rs` drives this from
//! `fixtures/mail/`.

use crate::backend::himalaya::{dto, map};
use crate::domain::{MailboxId, Message, MessageId, MessageLocator};

/// Parse raw RFC 5322 bytes into one domain message, byte-for-byte the way
/// `himalaya message read --json` output is consumed (ADR 0001 finding 10).
pub fn parse_raw_message(raw: &[u8], mailbox: &str, id: &str) -> Message {
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .expect("fixture must be parseable MIME");
    let dump = serde_json::to_value(&parsed).expect("mail-parser serde dump must serialize");
    let read: dto::MessageReadDto =
        serde_json::from_value(dump).expect("mail-parser dump must fit the lenient DTO");
    map::message(
        read,
        MessageLocator {
            mailbox: MailboxId(String::from(mailbox)),
            id: MessageId(String::from(id)),
            message_id: None,
        },
    )
}
