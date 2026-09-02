//! Send outcomes (plan §12): delivery results are modeled explicitly before
//! the composer exists, because "failed" can mean the message *may* have
//! been delivered — retrying an ambiguous send may duplicate it.
//!
//! Classification follows the Phase 0 characterization
//! (`fixtures/himalaya/send-outcomes.md`): exit 0 → [`SendOutcome::Sent`];
//! errors before the SMTP DATA phase → `FailedBeforeDelivery`; anything
//! else (EOF/reset/timeout during DATA, killed child, unparseable errors)
//! is conservatively [`SendOutcome::Unknown`]. `SentButCopyFailed` covers a
//! successful delivery whose Sent-copy save failed.

/// The outcome of one send attempt (plan §12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Delivered and stored.
    Sent,
    /// Delivered, but saving the Sent copy failed. Retry remains available;
    /// the UI must warn it could send a duplicate.
    SentButCopyFailed { detail: String },
    /// Nothing was transmitted; retrying is safe.
    FailedBeforeDelivery { detail: String },
    /// Delivery state cannot be determined. Retry remains available; the UI
    /// must warn it could send a duplicate.
    Unknown { detail: String },
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_undefined_outcomes_are_ambiguous() {
        assert!(!SendOutcome::Sent.is_ambiguous());
        assert!(
            !SendOutcome::FailedBeforeDelivery {
                detail: String::from("connect refused")
            }
            .is_ambiguous()
        );
        assert!(
            SendOutcome::SentButCopyFailed {
                detail: String::from("append failed")
            }
            .is_ambiguous()
        );
        assert!(
            SendOutcome::Unknown {
                detail: String::from("SMTP DATA failed: reached unexpected EOF")
            }
            .is_ambiguous()
        );
    }

    #[test]
    fn outcomes_carry_safe_detail() {
        let outcome = SendOutcome::Unknown {
            detail: String::from("connection reset"),
        };
        match outcome {
            SendOutcome::Unknown { detail } => assert_eq!(detail, "connection reset"),
            other => panic!("unexpected outcome {other:?}"),
        }
    }
}
