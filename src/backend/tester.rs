//! The wizard credential-test adapter (ADR 0003 §3.4): injected into the
//! operation manager like the opener and notifier, so the manager never
//! names a concrete adapter (ticket 55t6) and tests swap in a fake.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::config::write::DraftAccount;
use crate::domain::TestedMailbox;

use super::BackendResult;

/// Validates one draft account by logging in for real (ADR 0003 §3.4):
/// the adapter runs its own credential path against `draft` and returns
/// the mailboxes the account exposes, each tagged with the role the
/// server itself attributed (RFC 6154 SPECIAL-USE on IMAP) when it did.
/// A cancelled token aborts the run with
/// [`BackendError::Cancelled`](super::BackendError); the adapter owns
/// its timeout policy.
#[async_trait]
pub trait AccountTester: Send + Sync {
    async fn test_account(
        &self,
        draft: &DraftAccount,
        cancellation: CancellationToken,
    ) -> BackendResult<Vec<TestedMailbox>>;

    /// Ensures the named mailboxes exist on the account (issue txps):
    /// the adapter creates the missing ones (IMAP CREATE, best effort,
    /// each bounded) and verifies against a fresh listing, returning
    /// the subset of `names` the account exposes afterwards. A cancelled
    /// token aborts with
    /// [`BackendError::Cancelled`](super::BackendError); per-name create
    /// failures are skipped, never fatal.
    async fn ensure_mailboxes(
        &self,
        draft: &DraftAccount,
        names: &[String],
        cancellation: CancellationToken,
    ) -> BackendResult<Vec<String>>;
}
