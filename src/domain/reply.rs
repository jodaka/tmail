//! Reply and forward draft seeding (plan §14, Phase 7.3).
//!
//! Post constructs the seeds itself, with fixture tests, rather than
//! consuming the verified Himalaya reply templates at runtime: production
//! builds never parse MIME (ADR 0001 — `mail-parser` is test-fixtures
//! only), and the templates are raw RFC 5322 output. Everything the seeds
//! need is already available as domain data — the fetched message's parsed
//! headers (including `In-Reply-To`/`References`, mapped in Phase 7.4) and
//! decoded `text/plain` body.
//!
//! Seeds are pure functions of the message: no clocks, no I/O, no config
//! beyond the account address passed in for reply-all self-exclusion
//! (Phase 7.5). Missing data degrades gracefully — an unknown date, an
//! empty body, or a message without a `Message-ID` never blocks seeding.

use crate::domain::address::{Address, to_field_list};
use crate::domain::message::Message;

/// Which reply a seeded draft represents (Phase 7.5 adds reply-all
/// recipient merging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    /// Reply to the sender only.
    Reply,
}

/// Composer-ready fields of a seeded draft (plan §14). Address fields are
/// raw composer text; `in_reply_to`/`references` are bare RFC ids that the
/// send path serializes unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
}

/// Seed a reply draft: sender as recipient, `Re:` subject, the original
/// body quoted below an attribution line (the caret lands above it in the
/// composer), and the threading headers preserved (plan §14: reply
/// preserves `Message-ID`, `In-Reply-To`, and `References` semantics).
pub fn seed_reply(message: &Message, kind: ReplyKind) -> Seed {
    let _ = kind;
    let subject = prefixed_subject(&message.headers.subject, "Re:");
    let body = format!("\n\n{}\n{}", wrote_line(message), quote(message));
    Seed {
        to: to_field_list(&message.headers.from),
        cc: String::new(),
        subject,
        body,
        in_reply_to: message.headers.message_id.clone(),
        references: extended_references(message),
    }
}

/// Seed a forward draft: empty recipients (the user picks them), `Fwd:`
/// subject, and a clear forwarded header block above the original body
/// (plan §14). A forward starts a new thread: no `In-Reply-To`/`References`.
pub fn seed_forward(message: &Message) -> Seed {
    Seed {
        to: String::new(),
        cc: String::new(),
        subject: prefixed_subject(&message.headers.subject, "Fwd:"),
        body: format!("\n\n{}", forwarded_block(message)),
        in_reply_to: None,
        references: None,
    }
}

/// `Re:`/`Fwd:` subject prefixing: an existing prefix is kept as-is, so
/// replying to "Re: Welcome" does not double up.
fn prefixed_subject(subject: &str, prefix: &str) -> String {
    let trimmed = subject.trim();
    let lower_prefix = prefix.to_ascii_lowercase();
    if trimmed.to_ascii_lowercase().starts_with(&lower_prefix) {
        trimmed.to_string()
    } else {
        format!("{prefix} {trimmed}")
    }
}

/// The attribution line above the quoted body.
fn wrote_line(message: &Message) -> String {
    let author = message
        .headers
        .from
        .first()
        .map(Address::to_field)
        .unwrap_or_else(|| String::from("(unknown sender)"));
    match message.headers.date {
        Some(date) => format!("On {}, {} wrote:", date.format("%Y-%m-%d %H:%M"), author),
        None => format!("On an unknown date, {author} wrote:"),
    }
}

/// The original `text/plain` body, each line prefixed `> ` (bare `>` on
/// empty lines). Messages without a plain body quote nothing.
fn quote(message: &Message) -> String {
    let Some(body) = &message.plain_body else {
        return String::from("(no plain text body)");
    };
    body.trim_end_matches('\n')
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::from(">")
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The forwarded header block, Gmail-style divider included.
fn forwarded_block(message: &Message) -> String {
    let date = message
        .headers
        .date
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| String::from("(unknown date)"));
    let subject = if message.headers.subject.is_empty() {
        String::from("(no subject)")
    } else {
        message.headers.subject.clone()
    };
    let mut lines = vec![
        String::from("---------- Forwarded message ---------"),
        format!(
            "From: {}",
            or_unknown(&to_field_list(&message.headers.from))
        ),
        format!("Date: {date}"),
        format!("Subject: {subject}"),
        format!("To: {}", or_unknown(&to_field_list(&message.headers.to))),
    ];
    if !message.headers.cc.is_empty() {
        lines.push(format!("Cc: {}", to_field_list(&message.headers.cc)));
    }
    lines.push(String::new());
    lines.push(message.plain_body.clone().unwrap_or_default());
    lines.join("\n")
}

fn or_unknown(field: &str) -> &str {
    if field.is_empty() { "(unknown)" } else { field }
}

/// The `References` chain for a reply: the original chain plus the
/// original `Message-ID` (plan §14: References semantics preserved).
/// `None` when the original message has neither, so fresh threads stay
/// header-free.
fn extended_references(message: &Message) -> Option<String> {
    let own = message.headers.message_id.as_deref()?;
    let mut chain: Vec<&str> = message
        .headers
        .references
        .as_deref()
        .map(|refs| refs.split_whitespace().collect())
        .unwrap_or_default();
    chain.push(own);
    Some(chain.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MailboxId, MessageHeaders, MessageId};

    fn source() -> Message {
        Message {
            id: MessageId(String::from("env-1")),
            mailbox_id: MailboxId(String::from("INBOX")),
            headers: MessageHeaders {
                subject: String::from("Plan review"),
                from: vec![Address {
                    name: Some(String::from("Bob")),
                    email: String::from("bob@example.org"),
                }],
                to: vec![Address {
                    name: None,
                    email: String::from("probe@post.local"),
                }],
                cc: vec![Address {
                    name: None,
                    email: String::from("carol@example.org"),
                }],
                date: Some(
                    DateTime::parse_from_rfc3339("2026-09-02T10:03:40+03:00").expect("valid date"),
                ),
                message_id: Some(String::from("3180034027954358661@post.local")),
                in_reply_to: None,
                references: Some(String::from("000@post.local")),
            },
            plain_body: Some(String::from("Please review.\n\nThanks\n")),
            html_body: None,
            attachments: Vec::new(),
        }
    }

    use chrono::DateTime;

    #[test]
    fn reply_addresses_the_sender_and_preserves_thread_headers() {
        let seed = seed_reply(&source(), ReplyKind::Reply);
        assert_eq!(seed.to, "Bob <bob@example.org>");
        assert_eq!(seed.cc, "");
        assert_eq!(seed.subject, "Re: Plan review");
        // The original Message-ID becomes In-Reply-To and extends References.
        assert_eq!(
            seed.in_reply_to.as_deref(),
            Some("3180034027954358661@post.local")
        );
        assert_eq!(
            seed.references.as_deref(),
            Some("000@post.local 3180034027954358661@post.local")
        );
    }

    #[test]
    fn reply_quotes_the_body_under_an_attribution_line() {
        let seed = seed_reply(&source(), ReplyKind::Reply);
        let expected = "\n\nOn 2026-09-02 10:03, Bob <bob@example.org> wrote:\n\
                        > Please review.\n>\n> Thanks";
        assert_eq!(seed.body, expected);
    }

    #[test]
    fn reply_keeps_an_existing_re_prefix() {
        let mut message = source();
        message.headers.subject = String::from("Re: Plan review");
        let seed = seed_reply(&message, ReplyKind::Reply);
        assert_eq!(seed.subject, "Re: Plan review");
    }

    #[test]
    fn reply_without_a_message_id_starts_a_fresh_thread() {
        let mut message = source();
        message.headers.message_id = None;
        let seed = seed_reply(&message, ReplyKind::Reply);
        assert_eq!(seed.in_reply_to, None);
        assert_eq!(seed.references, None);
    }

    #[test]
    fn reply_tolerates_missing_date_body_and_sender() {
        let mut message = source();
        message.headers.date = None;
        message.plain_body = None;
        let seed = seed_reply(&message, ReplyKind::Reply);
        assert!(seed.body.contains("On an unknown date,"));
        assert!(seed.body.contains("(no plain text body)"));
        message.headers.from.clear();
        let seed = seed_reply(&message, ReplyKind::Reply);
        assert_eq!(seed.to, "");
        assert!(seed.body.contains("(unknown sender)"));
    }

    #[test]
    fn forward_has_a_clear_header_block_and_no_thread_headers() {
        let seed = seed_forward(&source());
        assert_eq!(seed.to, "", "recipients are the user's choice");
        assert_eq!(seed.subject, "Fwd: Plan review");
        assert_eq!(seed.in_reply_to, None);
        assert_eq!(seed.references, None);
        let expected = "\n\n---------- Forwarded message ---------\n\
                        From: Bob <bob@example.org>\n\
                        Date: 2026-09-02 10:03\n\
                        Subject: Plan review\n\
                        To: probe@post.local\n\
                        Cc: carol@example.org\n\
                        \n\
                        Please review.\n\nThanks\n";
        assert_eq!(seed.body, expected);
    }

    #[test]
    fn forward_prefixes_and_defaults_degrade_gracefully() {
        let mut message = source();
        message.headers.subject = String::from("fwd: already forwarded");
        message.plain_body = None;
        message.headers.to.clear();
        message.headers.cc.clear();
        let seed = seed_forward(&message);
        assert_eq!(seed.subject, "fwd: already forwarded");
        assert!(seed.body.contains("To: (unknown)"));
        assert!(!seed.body.contains("Cc:"), "empty Cc is omitted");
        assert!(
            seed.body.ends_with("\n\n"),
            "missing body leaves a blank block"
        );
    }
}
