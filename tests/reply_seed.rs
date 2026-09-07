//! Reply seeding against real fixture output (plan §19 Phase 7, Phase 7.3).
//!
//! `fixtures/himalaya/reply-template.eml` is actual `himalaya message
//! reply` output captured by the Phase 0 probe (ADR 0001 finding 11). The
//! seed functions run against it after the production mapping path, so the
//! reply threading headers Tmail preserves are exactly what a real exchange
//! carries. Feature-gated like the rest of the fixture pipeline: production
//! builds never parse MIME (ADR 0001).

#![cfg(feature = "test-fixtures")]

use tmail::backend::himalaya::fixtures::parse_raw_message;
use tmail::domain::{ReplyKind, seed_forward, seed_reply};

const REPLY_TEMPLATE: &[u8] = include_bytes!("../fixtures/himalaya/reply-template.eml");

#[test]
fn reply_to_a_real_himalaya_message_preserves_thread_headers() {
    let message = parse_raw_message(REPLY_TEMPLATE, "INBOX", "env-1");
    // Sanity: the fixture really carries the threading headers.
    assert_eq!(
        message.headers.message_id.as_deref(),
        Some("18d179bfb9f51e28.d8b23411a1e22891.1aaa20328b43cd8d@RFT-R993YF")
    );
    assert_eq!(
        message.headers.in_reply_to.as_deref(),
        Some("6053432595490343824@tmail.local")
    );

    let seed = seed_reply(&message, ReplyKind::Reply, Some("probe@tmail.local"));
    // In-Reply-To becomes the template's own Message-ID; References grows
    // by it while keeping the original chain.
    assert_eq!(
        seed.in_reply_to.as_deref(),
        Some("18d179bfb9f51e28.d8b23411a1e22891.1aaa20328b43cd8d@RFT-R993YF")
    );
    assert_eq!(
        seed.references.as_deref(),
        Some(
            "6053432595490343824@tmail.local \
              18d179bfb9f51e28.d8b23411a1e22891.1aaa20328b43cd8d@RFT-R993YF"
        )
    );
    assert_eq!(seed.to, "Tmail Probe <probe@tmail.local>");
    assert_eq!(
        seed.subject,
        "Re: Re: Welcome to Tmail".replace("Re: Re:", "Re:"),
    );
    // "Re: Welcome to Tmail" already carries the prefix: kept as-is.
    assert_eq!(seed.subject, "Re: Welcome to Tmail");
    assert!(
        seed.body.contains("> Hello,"),
        "the quoted body includes the template's quoted text:\n{}",
        seed.body
    );
}

#[test]
fn forward_of_a_real_himalaya_message_has_a_header_block() {
    let message = parse_raw_message(REPLY_TEMPLATE, "INBOX", "env-1");
    let seed = seed_forward(&message);
    assert_eq!(seed.in_reply_to, None);
    assert_eq!(seed.references, None);
    let body = seed.body;
    assert!(body.contains("---------- Forwarded message ---------"));
    assert!(body.contains("From: Tmail Probe <probe@tmail.local>"));
    assert!(body.contains("Subject: Re: Welcome to Tmail"));
    assert!(body.contains("To: Ada Lovelace <ada@example.org>"));
}
