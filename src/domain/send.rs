//! Send outcomes and the outgoing message model (plan §12/§14).
//!
//! Delivery results are modeled explicitly because "failed" can mean the
//! message *may* have been delivered — retrying an ambiguous send may
//! duplicate it. Classification follows the Phase 0 characterization
//! (`fixtures/himalaya/send-outcomes.md`): exit 0 → [`SendOutcome::Sent`];
//! errors before the SMTP DATA phase → `FailedBeforeDelivery`; anything
//! else (EOF/reset/timeout during DATA, unparseable errors) is
//! conservatively [`SendOutcome::Unknown`]. `SentButCopyFailed` covers a
//! successful delivery whose Sent-copy save failed.
//!
//! [`OutboundMessage`] is the typed intent of one send: recipients are
//! parsed and valid by construction (`from_fields` refuses invalid or
//! missing recipients), so the backend can serialize through the
//! mail-builder library (plan §14: never hand-concatenate MIME) without
//! re-judging the composer's raw text.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::address::{Address, parse_address_list};

/// The outcome of one send attempt (plan §12). `code` is the Himalaya exit
/// status when a child process ran and exited (`None` when it did not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Delivered and stored.
    Sent,
    /// Delivered, but saving the Sent copy failed. Retry remains available;
    /// the UI must warn it could send a duplicate.
    SentButCopyFailed { code: Option<i32>, detail: String },
    /// Nothing was transmitted; retrying is safe.
    FailedBeforeDelivery { code: Option<i32>, detail: String },
    /// Delivery state cannot be determined. Retry remains available; the UI
    /// must warn it could send a duplicate.
    Unknown { code: Option<i32>, detail: String },
}

impl SendOutcome {
    /// Whether retrying this outcome may duplicate the message (plan §12:
    /// "the modal must state that retry could send a duplicate").
    pub fn is_ambiguous(&self) -> bool {
        matches!(
            self,
            SendOutcome::SentButCopyFailed { .. } | SendOutcome::Unknown { .. }
        )
    }

    /// Himalaya exit status for the modal's status row (`None` for a
    /// definite success).
    pub fn code(&self) -> Option<i32> {
        match self {
            SendOutcome::Sent => None,
            SendOutcome::SentButCopyFailed { code, .. }
            | SendOutcome::FailedBeforeDelivery { code, .. }
            | SendOutcome::Unknown { code, .. } => *code,
        }
    }

    /// Safe causal detail for the modal (`FailedBeforeDelivery` and the
    /// ambiguous outcomes only).
    pub fn detail(&self) -> &str {
        match self {
            SendOutcome::Sent => "",
            SendOutcome::SentButCopyFailed { detail, .. }
            | SendOutcome::FailedBeforeDelivery { detail, .. }
            | SendOutcome::Unknown { detail, .. } => detail,
        }
    }
}

/// What stops a send before it starts (plan §14: send refuses invalid
/// recipients and empty recipient lists; the composer only flags entries
/// live). Surfaced on the status line, not as an error modal: the problem
/// is the input on screen, not an operational failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendBlocker {
    /// No recipient in To, Cc, or Bcc.
    NoRecipients,
    /// Raw text of every entry that failed to parse.
    InvalidAddresses(Vec<String>),
}

impl fmt::Display for SendBlocker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendBlocker::NoRecipients => {
                write!(f, "Cannot send: add at least one recipient")
            }
            SendBlocker::InvalidAddresses(_) => {
                write!(f, "Cannot send: fix the invalid address entries")
            }
        }
    }
}

/// Subject/body plus the threading context of an outgoing message (plan
/// §14): exactly what reply/forward seeding fills in and what a draft
/// already carries. `in_reply_to`/`references` are bare RFC ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutgoingContent {
    pub subject: String,
    pub body: String,
    /// Bare `Message-ID` this message replies to (plan §14: reply headers
    /// preserved). `None` for fresh mail and forwards.
    pub in_reply_to: Option<String>,
    /// Bare, whitespace-separated `References` chain (plan §14).
    pub references: Option<String>,
}

/// One outgoing message, fully validated (plan §7/§14). Built only through
/// [`OutboundMessage::from_fields`], which refuses invalid or missing
/// recipients — the Phase 7.1 guarantee that invalid composer entries never
/// reach the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub content: OutgoingContent,
    /// RFC form (`<id@domain>`) of the outgoing message's own `Message-ID`
    /// when already known — draft-derived sends reuse the draft's stable
    /// identity (ADR 0002 §D.6). The backend mints one when `None`.
    pub message_id: Option<String>,
}

impl OutboundMessage {
    /// Validate the raw composer address fields and build the message.
    /// `Err` names what the user must fix before sending (plan §14).
    pub fn from_fields(
        to: &str,
        cc: &str,
        bcc: &str,
        content: OutgoingContent,
        message_id: Option<String>,
    ) -> Result<Self, SendBlocker> {
        let mut invalid = Vec::new();
        let collect = |field: &str, invalid: &mut Vec<String>| -> Vec<Address> {
            let mut out = Vec::new();
            for parsed in parse_address_list(field) {
                match parsed {
                    Ok(address) => out.push(address),
                    Err(raw) => invalid.push(raw),
                }
            }
            out
        };
        let to = collect(to, &mut invalid);
        let cc = collect(cc, &mut invalid);
        let bcc = collect(bcc, &mut invalid);
        if !invalid.is_empty() {
            return Err(SendBlocker::InvalidAddresses(invalid));
        }
        if to.is_empty() && cc.is_empty() && bcc.is_empty() {
            return Err(SendBlocker::NoRecipients);
        }
        Ok(Self {
            to,
            cc,
            bcc,
            content,
            message_id,
        })
    }

    /// Total recipient count across the three fields (tests, logs).
    pub fn recipient_count(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_undefined_outcomes_are_ambiguous() {
        assert!(!SendOutcome::Sent.is_ambiguous());
        assert!(
            !SendOutcome::FailedBeforeDelivery {
                code: Some(1),
                detail: String::from("connect refused")
            }
            .is_ambiguous()
        );
        assert!(
            SendOutcome::SentButCopyFailed {
                code: None,
                detail: String::from("append failed")
            }
            .is_ambiguous()
        );
        assert!(
            SendOutcome::Unknown {
                code: Some(1),
                detail: String::from("SMTP DATA failed: reached unexpected EOF")
            }
            .is_ambiguous()
        );
    }

    #[test]
    fn outcomes_expose_code_and_safe_detail() {
        let outcome = SendOutcome::Unknown {
            code: Some(1),
            detail: String::from("connection reset"),
        };
        assert_eq!(outcome.code(), Some(1));
        assert_eq!(outcome.detail(), "connection reset");
        assert_eq!(SendOutcome::Sent.code(), None);
        assert_eq!(SendOutcome::Sent.detail(), "");
    }

    #[test]
    fn from_fields_parses_all_three_recipient_fields() {
        let message = OutboundMessage::from_fields(
            "Ada <ada@example.org>, bob@example.org",
            "",
            "Carol <carol@example.org>",
            OutgoingContent {
                subject: String::from("Hi"),
                body: String::from("Body"),
                in_reply_to: None,
                references: None,
            },
            None,
        )
        .expect("valid recipients");
        assert_eq!(message.to.len(), 2);
        assert_eq!(message.to[0].display(), "Ada");
        assert_eq!(message.to[1].email, "bob@example.org");
        assert!(message.cc.is_empty());
        assert_eq!(message.bcc.len(), 1);
        assert_eq!(message.recipient_count(), 3);
        assert_eq!(message.content.subject, "Hi");
        assert_eq!(message.content.body, "Body");
    }

    #[test]
    fn from_fields_refuses_invalid_entries_with_their_raw_text() {
        let err = OutboundMessage::from_fields(
            "ok@example.com, not an address, two@@x.io",
            "",
            "",
            OutgoingContent::default(),
            None,
        )
        .expect_err("invalid entries");
        match err {
            SendBlocker::InvalidAddresses(raw) => {
                assert_eq!(
                    raw,
                    vec![String::from("not an address"), String::from("two@@x.io")]
                )
            }
            other => panic!("unexpected blocker {other:?}"),
        }
    }

    #[test]
    fn from_fields_refuses_an_empty_recipient_list() {
        let err = OutboundMessage::from_fields("", "", "", OutgoingContent::default(), None)
            .expect_err("empty");
        assert_eq!(err, SendBlocker::NoRecipients);
        assert_eq!(err.to_string(), "Cannot send: add at least one recipient");
    }

    #[test]
    fn invalid_entries_rank_above_missing_recipients() {
        // A garbled field is the more specific problem; report it first.
        let err = OutboundMessage::from_fields("broken", "", "", OutgoingContent::default(), None)
            .expect_err("invalid");
        assert!(matches!(err, SendBlocker::InvalidAddresses(_)));
    }

    #[test]
    fn message_is_serializable_for_retry_intents() {
        let message = OutboundMessage::from_fields(
            "a@b.co",
            "",
            "",
            OutgoingContent {
                subject: String::from("S"),
                body: String::from("B"),
                in_reply_to: Some(String::from("1@x")),
                references: Some(String::from("0@x 1@x")),
            },
            Some(String::from("<9@post.local>")),
        )
        .expect("valid");
        let json = serde_json::to_string(&message).expect("serialize");
        let back: OutboundMessage = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, message);
    }
}
