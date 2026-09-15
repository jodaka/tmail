//! The wizard credential-test adapter (ADR 0003 §3.4): injected into the
//! operation manager like the opener and notifier, so the manager never
//! names a concrete adapter (ticket 55t6) and tests swap in a fake.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::config::write::DraftAccount;

use super::BackendResult;

/// Validates one draft account by logging in for real (ADR 0003 §3.4):
/// the adapter runs its own credential path against `draft` and returns
/// the mailboxes the account exposes. A cancelled token aborts the run
/// with [`BackendError::Cancelled`](super::BackendError); the adapter owns
/// its timeout policy.
#[async_trait]
pub trait AccountTester: Send + Sync {
    async fn test_account(
        &self,
        draft: &DraftAccount,
        cancellation: CancellationToken,
    ) -> BackendResult<Vec<String>>;
}
